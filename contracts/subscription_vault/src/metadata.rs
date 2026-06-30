use soroban_sdk::xdr::{AccountId, PublicKey, ScAddress, Uint256};
use soroban_sdk::{Address, Bytes, BytesN, Env, String, Symbol, TryFromVal, Vec};

use crate::queries::get_subscription;
use crate::types::{
    DataKey, Error, MetadataDeletedEvent, MetadataSetEvent, MetadataSetSignedEvent, SignedMetadataPayload,
    MAX_METADATA_KEYS, MAX_METADATA_KEY_LENGTH, MAX_METADATA_VALUE_LENGTH,
};

/// Length of the domain-separator prefix baked into the canonical signed message.
///
/// Bumping this constant changes the on-chain signature domain, so it must only
/// happen as part of an explicit, reviewed contract upgrade.
const SIGNED_MSG_DOMAIN_TAG_LEN: u32 = 32;

/// Domain separator for [`build_metadata_signed_message`].
///
/// Mixed into every canonical signed-metadata payload before the user-supplied
/// fields. Changing this value invalidates every previously-signed payload, so
/// it must only be updated alongside a contract version bump.
const SIGNED_MSG_DOMAIN_TAG: &[u8; SIGNED_MSG_DOMAIN_TAG_LEN as usize] =
    b"SBL_META_SIGNED_v1\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";

/// Wrapper called by the contract entrypoint.
pub fn do_set_metadata(
    env: &Env,
    authorizer: Address,
    subscription_id: u32,
    key: String,
    value: String,
) -> Result<(), Error> {
    set_metadata(env, subscription_id, &authorizer, key, value)
}

/// Wrapper called by the contract entrypoint.
pub fn do_delete_metadata(
    env: &Env,
    authorizer: Address,
    subscription_id: u32,
    key: String,
) -> Result<(), Error> {
    delete_metadata(env, subscription_id, &authorizer, key)
}

/// Apply an off-chain signed metadata update.
///
/// # Auth flow
///
/// 1. Look up `payload.subscription_id`. Reject if it does not exist.
/// 2. Reconstruct the canonical message from `(payload, chain_id)` and
///    verify the supplied ed25519 signature against the supplied pubkey.
///    `env.crypto().ed25519_verify` **panics** when the signature is
///    forgeries or the wrong key — that is the desired security boundary.
/// 3. Derive the Soroban Address for `signer_pubkey` and confirm it is
///    either `sub.subscriber` or `sub.merchant`.
/// 4. Reject when `now >= payload.expires_at`.
/// 5. Consume `payload.nonce` from `(signer_address, DOMAIN_METADATA_SIGNED)`.
/// 6. Apply the same key/value/key-cap invariants as [`set_metadata`] and
///    emit [`MetadataSetSignedEvent`].
///
/// # Errors
///
/// * [`Error::NotFound`] — `subscription_id` does not exist.
/// * [`Error::InvalidInput`] — `expires_at <= now` (payload already stale).
/// * [`Error::MetadataKeyTooLong`] / [`Error::MetadataValueTooLong`] —
///   payload violated length limits.
/// * [`Error::MetadataKeyLimitReached`] — adding a new key would exceed the
///   per-subscription key cap.
/// * [`Error::NonceAlreadyUsed`] — replay or out-of-order nonce.
/// * [`Error::Overflow`] — nonce counter would overflow u64.
/// * [`Error::Forbidden`] — derived signer is not the subscription's
///   subscriber or merchant (only reachable when the signature itself was
///   valid for a pubkey the contract cannot link to this subscription).
///
/// Panics with a host crypto error when the ed25519 signature fails to
/// validate. That is intentional: a forged signature must abort the
/// transaction rather than be silently downgraded to a typed error.
pub fn do_set_metadata_signed(
    env: &Env,
    signer_pubkey: BytesN<32>,
    payload: SignedMetadataPayload,
    signature: BytesN<64>,
) -> Result<(), Error> {
    let network_id = env.ledger().network_id();

    // Auth-first ordering: prove the subscription actually exists before
    // touching crypto or storage. A bogus subscription_id otherwise lets an
    // attacker burn slot in the nonce counter for IDs that will never
    // resolve.
    let sub = get_subscription(env, payload.subscription_id)?;

    // Build the canonical message and verify the ed25519 signature against
    // the supplied pubkey. Verification hashes the message internally per
    // RFC 8032 — we feed it the raw canonical bytes.
    let message = build_metadata_signed_message(env, &payload, &network_id);
    env.crypto()
        .ed25519_verify(&signer_pubkey, &message, &signature);

    // Map the raw pubkey to a Soroban Address via its XDR AccountId so the
    // nonce counter is keyed under a deterministic (Address, domain) pair.
    //
    // Soundness: the on-chain `require_auth()` decodes the signer's strkey to
    // the same underlying AccountId bytes that this path reconstructs, so a
    // pubkey that authenticates as e.g. sub.subscriber on-chain matches the
    // same address here.
    let pk_array = signer_pubkey.to_array();
    let signer_address = Address::try_from_val(
        env,
        &ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk_array)))),
    )
    .map_err(|_| Error::Overflow)?;

    // Identity check: the pubkey must correspond to a party of the
    // subscription. Without this guard a key holder could move metadata on
    // subscriptions they have no relationship with.
    if signer_address != sub.subscriber && signer_address != sub.merchant {
        return Err(Error::Forbidden);
    }

    // Reject stale payloads. Strict inequality (`> =`) means an exactly-
    // expired payload is rejected deterministically without an off-by-one
    // hazard. A generously future-dated `expires_at` (e.g. now + one
    // interval) is the recommeded setting.
    let now = env.ledger().timestamp();
    if now >= payload.expires_at {
        return Err(Error::InvalidInput);
    }

    // Replay protection: consume the next-expected nonce for
    // (signer_address, DOMAIN_METADATA_SIGNED). Rejects out-of-order or
    // duplicate nonces and emits an audit event.
    crate::nonce::check_and_advance(
        env,
        &signer_address,
        crate::nonce::DOMAIN_METADATA_SIGNED,
        payload.nonce,
    )?;

    apply_metadata_value(env, payload.subscription_id, &payload.key, &payload.value)?;

    env.events().publish(
        (Symbol::new(env, "metadata_set_signed"), payload.subscription_id),
        MetadataSetSignedEvent {
            subscription_id: payload.subscription_id,
            key: payload.key.clone(),
            signer: signer_address,
            nonce: payload.nonce,
            timestamp: now,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Read the next-expected nonce for the metadata-signed domain for a signer.
///
/// Convenience helper for off-chain tooling: callers should fetch this value
/// before constructing the next [`SignedMetadataPayload`] so that
/// `payload.nonce` matches the on-chain counter and is not rejected as a
/// replay.
///
/// # Auth
///
/// Read-only; no auth required.
pub fn get_metadata_signed_nonce(env: &Env, signer: Address) -> u64 {
    crate::nonce::get_nonce(env, &signer, crate::nonce::DOMAIN_METADATA_SIGNED)
}

/// Build the canonical, deterministic byte string that must be signed off-chain.
///
/// Layout (all multi-byte integers big-endian, no padding):
///
/// ```
///   SIGNED_MSG_DOMAIN_TAG (32 bytes)
/// || subscription_id   (4 bytes, u32 BE)
/// || key_len           (4 bytes, u32 BE)
/// || key               (key_len bytes)
/// || value_len         (4 bytes, u32 BE)
/// || value             (value_len bytes)
/// || nonce             (8 bytes, u64 BE)
/// || chain_id_len      (4 bytes, u32 BE)
/// || chain_id          (chain_id_len bytes)
/// || expires_at        (8 bytes, u64 BE)
/// ```
///
/// The domain tag and per-field length prefixes make the encoding unambiguous:
/// no two distinct payloads can ever produce the same byte sequence, and no
/// struct field boundary can be confused with another.
///
/// Exposed publicly so tests can construct the same byte stream the contract
/// verifies.
pub fn build_metadata_signed_message(
    env: &Env,
    payload: &SignedMetadataPayload,
    network_id: &BytesN<32>,
) -> Bytes {
    let mut buf = Bytes::new(env);
    buf.extend_from_slice(&SIGNED_MSG_DOMAIN_TAG[..]);

    let sub_id_bytes = payload.subscription_id.to_be_bytes();
    buf.extend_from_slice(&sub_id_bytes);

    let key_len: u32 = payload.key.len();
    let key_len_bytes = key_len.to_be_bytes();
    buf.extend_from_slice(&key_len_bytes);
    if key_len > 0 {
        let mut key_buf = [0u8; MAX_METADATA_KEY_LENGTH as usize];
        payload.key.copy_into_slice(&mut key_buf[..key_len as usize]);
        buf.extend_from_slice(&key_buf[..key_len as usize]);
    }

    let value_len: u32 = payload.value.len();
    let value_len_bytes = value_len.to_be_bytes();
    buf.extend_from_slice(&value_len_bytes);
    if value_len > 0 {
        let mut value_buf = [0u8; MAX_METADATA_VALUE_LENGTH as usize];
        payload
            .value
            .copy_into_slice(&mut value_buf[..value_len as usize]);
        buf.extend_from_slice(&value_buf[..value_len as usize]);
    }

    let nonce_bytes = payload.nonce.to_be_bytes();
    buf.extend_from_slice(&nonce_bytes);

    // Network ID is always 32 bytes.
    let network_id_len: u32 = 32;
    let network_id_len_bytes = network_id_len.to_be_bytes();
    buf.extend_from_slice(&network_id_len_bytes);
    buf.extend_from_slice(&network_id.to_array());

    let expires_bytes = payload.expires_at.to_be_bytes();
    buf.extend_from_slice(&expires_bytes);

    buf
}

/// Shared inner helper for the on-chain and signed entrypoints.
///
/// Both [`set_metadata`] (after auth) and [`do_set_metadata_signed`] (after
/// signature + nonce checks) end up here: validate key/value lengths, ensure
/// the per-subscription key cap, write the storage entry, and return.
///
/// Split out so the validation/storage/error contract is identical and any
/// future invariant change touches one place.
fn apply_metadata_value(
    env: &Env,
    subscription_id: u32,
    key: &String,
    value: &String,
) -> Result<(), Error> {
    if key.len() > MAX_METADATA_KEY_LENGTH {
        return Err(Error::MetadataKeyTooLong);
    }

    if value.len() > MAX_METADATA_VALUE_LENGTH {
        return Err(Error::MetadataValueTooLong);
    }

    let mut keys: Vec<String> = env
        .storage()
        .persistent()
        .get(&DataKey::MetadataKeys(subscription_id))
        .unwrap_or(Vec::new(env));

    let key_exists = keys.iter().any(|k| k == *key);

    if !key_exists {
        if keys.len() >= MAX_METADATA_KEYS {
            return Err(Error::MetadataKeyLimitReached);
        }
        keys.push_back(key.clone());
        env.storage()
            .persistent()
            .set(&DataKey::MetadataKeys(subscription_id), &keys);
    }

    env.storage()
        .persistent()
        .set(&DataKey::Metadata(subscription_id, key.clone()), value);

    Ok(())
}

pub fn set_metadata(
    env: &Env,
    subscription_id: u32,
    caller: &Address,
    key: String,
    value: String,
) -> Result<(), Error> {
    caller.require_auth();

    let sub = get_subscription(env, subscription_id)?;
    if caller != &sub.subscriber && caller != &sub.merchant {
        return Err(Error::Forbidden);
    }

    apply_metadata_value(env, subscription_id, &key, &value)?;

    env.events().publish(
        (Symbol::new(env, "metadata_set"), subscription_id),
        MetadataSetEvent {
            subscription_id,
            key,
            authorizer: caller.clone(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

pub fn get_metadata(env: &Env, subscription_id: u32, key: String) -> Result<String, Error> {
    let _ = get_subscription(env, subscription_id)?;

    env.storage()
        .persistent()
        .get(&DataKey::Metadata(subscription_id, key))
        .ok_or(Error::NotFound)
}

pub fn delete_metadata(
    env: &Env,
    subscription_id: u32,
    caller: &Address,
    key: String,
) -> Result<(), Error> {
    caller.require_auth();

    let sub = get_subscription(env, subscription_id)?;
    if caller != &sub.subscriber && caller != &sub.merchant {
        return Err(Error::Forbidden);
    }

    let mut keys: Vec<String> = env
        .storage()
        .persistent()
        .get(&DataKey::MetadataKeys(subscription_id))
        .unwrap_or(Vec::new(env));

    if let Some(idx) = keys.iter().position(|k| k == key) {
        let idx_u32 = idx.try_into().map_err(|_| Error::Overflow)?;
        keys.remove(idx_u32);
        env.storage()
            .persistent()
            .set(&DataKey::MetadataKeys(subscription_id), &keys);

        env.storage()
            .persistent()
            .remove(&DataKey::Metadata(subscription_id, key.clone()));

        env.events().publish(
            (Symbol::new(env, "metadata_deleted"), subscription_id),
            MetadataDeletedEvent {
                subscription_id,
                key,
                authorizer: caller.clone(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
    }

    Ok(())
}

pub fn list_metadata_keys(env: &Env, subscription_id: u32) -> Result<Vec<String>, Error> {
    let _ = get_subscription(env, subscription_id)?;

    let keys: Vec<String> = env
        .storage()
        .persistent()
        .get(&DataKey::MetadataKeys(subscription_id))
        .unwrap_or(Vec::new(env));

    Ok(keys)
}

#[cfg(test)]
mod signed_message_tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    /// Two structurally identical payloads signed on different chains must
    /// produce different message bytes — chain_id is mixed into the canonical
    /// encoding.
    #[test]
    fn message_is_chain_specific() {
        let env = Env::default();
        let sub = Address::generate(&env);
        let payload = SignedMetadataPayload {
            subscription_id: 7,
            key: String::from_str(&env, "k"),
            value: String::from_str(&env, "v"),
            nonce: 0,
            expires_at: 100,
        };

        let chain_a = BytesN::from_array(&env, &[
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        ]);
        let chain_b = BytesN::from_array(&env, &[
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
        ]);

        let msg_a = build_metadata_signed_message(&env, &payload, &chain_a);
        let msg_b = build_metadata_signed_message(&env, &payload, &chain_b);

        assert_ne!(msg_a, msg_b);
        // Ignore the unused sub warning: the address must remain generated
        // so the test stays deterministic across cargo test runs.
        let _ = sub;
    }

    /// Mutating any field of the payload changes the message bytes.
    #[test]
    fn message_changes_on_any_field_mutation() {
        let env = Env::default();
        let chain = BytesN::from_array(&env, &[
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03,
        ]);

        let base = || SignedMetadataPayload {
            subscription_id: 1,
            key: String::from_str(&env, "k"),
            value: String::from_str(&env, "v"),
            nonce: 0,
            expires_at: 100,
        };

        let base_msg = build_metadata_signed_message(&env, &base(), &chain);

        let mut different = base();
        different.subscription_id = 2;
        assert_ne!(base_msg, build_metadata_signed_message(&env, &different, &chain));

        let mut different = base();
        different.key = String::from_str(&env, "k2");
        assert_ne!(base_msg, build_metadata_signed_message(&env, &different, &chain));

        let mut different = base();
        different.value = String::from_str(&env, "v2");
        assert_ne!(base_msg, build_metadata_signed_message(&env, &different, &chain));

        let mut different = base();
        different.nonce = 1;
        assert_ne!(base_msg, build_metadata_signed_message(&env, &different, &chain));

        let mut different = base();
        different.expires_at = 101;
        assert_ne!(base_msg, build_metadata_signed_message(&env, &different, &chain));
    }

    /// Verify the auxiliary [`get_metadata_signed_nonce`] helper is wired to
    /// the signed-metadata domain and starts at zero.
    #[test]
    fn signed_nonce_helper_starts_at_zero() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(crate::SubscriptionVault, ());

        let signer = Address::generate(&env);
        env.as_contract(&contract_id, || {
            assert_eq!(
                get_metadata_signed_nonce(&env, signer.clone()),
                0
            );
        });
    }

    /// The signed-message helper module's tests exercise the inner wiring
    /// without going through the full signed-update flow. The full positive
    /// and negative paths live in `test_metadata_signed.rs`.
    #[test]
    fn smoke_helper_module_compiles() {
        let env = Env::default();
        let _ = env;
    }
}
