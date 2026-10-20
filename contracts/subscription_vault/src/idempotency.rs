//! Shared ring-buffer idempotency key helpers.
//!
//! Three entrypoints use idempotency keys: `charge_subscription`,
//! `deposit_funds`, and `charge_one_off`.  Each domain is scoped with a
//! unique domain constant so that reusing the same raw 32-byte key across
//! different entrypoints does **not** produce a replay collision.
//!
//! Storage key: `DataKey::IdemKey(subscription_id)` stores `IdemRingBuffer`.

use crate::types::DataKey;
use soroban_sdk::{contracttype, BytesN, Env, Vec};

/// Maximum number of idempotency keys retained per subscription.
pub(crate) const IDEM_HISTORY: u32 = 10;

/// Ring buffer of recently seen idempotency-key hashes.
#[contracttype]
#[derive(Clone, Debug)]
pub(crate) struct IdemRingBuffer {
    pub entries: Vec<BytesN<32>>,
    pub cursor: u32,
}

/// Return the raw byte representation of a 32-byte idempotency key.
fn key_bytes(key: &BytesN<32>) -> [u8; 32] {
    let mut out = [0u8; 32];
    let arr = key.to_array();
    out.copy_from_slice(&arr);
    out
}

/// Hash (domain, subscription_id, raw_key) into a 32-byte fingerprint.
///
/// The caller **must** supply the correct `domain` constant for their
/// entrypoint so that two different operations receiving the same raw key
/// produce different fingerprints.
pub fn hash_idem_key(
    env: &Env,
    domain: u32,
    subscription_id: u32,
    raw_key: &BytesN<32>,
) -> BytesN<32> {
    let raw = key_bytes(raw_key);
    let mut buf = [0u8; 40];
    buf[..4].copy_from_slice(&domain.to_be_bytes());
    buf[4..8].copy_from_slice(&subscription_id.to_be_bytes());
    buf[8..40].copy_from_slice(&raw);
    let input = soroban_sdk::Bytes::from_slice(env, &buf);
    env.crypto().sha256(&input).into()
}

/// Load the ring buffer for `subscription_id`.
///
/// Returns an empty buffer when no idempotency key has ever been stored.
fn load_buffer(env: &Env, subscription_id: u32) -> IdemRingBuffer {
    env.storage()
        .instance()
        .get(&DataKey::IdemKey(subscription_id))
        .unwrap_or(IdemRingBuffer {
            entries: Vec::new(env),
            cursor: 0,
        })
}

/// Persist the ring buffer for `subscription_id`.
fn save_buffer(env: &Env, subscription_id: u32, buf: &IdemRingBuffer) {
    env.storage()
        .instance()
        .set(&DataKey::IdemKey(subscription_id), buf);
}

/// Check whether `hashed` already exists in the ring buffer.
///
/// Returns `true` when the key is a duplicate (replay).
pub fn check_key(env: &Env, subscription_id: u32, hashed: &BytesN<32>) -> bool {
    let buf = load_buffer(env, subscription_id);
    for entry in buf.entries.iter() {
        if entry == *hashed {
            return true;
        }
    }
    false
}

/// Insert a new idempotency key hash into the ring buffer.
///
/// When the buffer is full the oldest entry (at `cursor`) is silently
/// overwritten.
pub fn push_key(env: &Env, subscription_id: u32, hashed: &BytesN<32>) {
    let mut buf = load_buffer(env, subscription_id);
    if buf.entries.len() < IDEM_HISTORY {
        buf.entries.push_back(hashed.clone());
    } else {
        let idx = buf.cursor as usize % IDEM_HISTORY as usize;
        if idx < buf.entries.len() as usize {
            buf.entries.set(idx as u32, hashed.clone());
        } else {
            buf.entries.push_back(hashed.clone());
        }
    }
    buf.cursor = buf.cursor.wrapping_add(1) % IDEM_HISTORY;
    save_buffer(env, subscription_id, &buf);
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{BytesN, Env};

    fn make_hash(env: &Env, val: u8) -> BytesN<32> {
        let mut arr = [0u8; 32];
        arr[31] = val;
        BytesN::from_array(env, &arr)
    }

    /// Instance storage is only reachable inside `env.as_contract`, so every
    /// test body runs against a freshly registered contract instance.
    fn contract_env() -> (Env, soroban_sdk::Address) {
        let env = Env::default();
        let contract_id = env.register(crate::SubscriptionVault, ());
        (env, contract_id)
    }

    #[test]
    fn test_check_key_empty_buffer() {
        let (env, contract_id) = contract_env();
        let sub_id = 1;
        let hashed = make_hash(&env, 1);

        env.as_contract(&contract_id, || {
            assert!(!check_key(&env, sub_id, &hashed));
        });
    }

    #[test]
    fn test_check_key_existing_key() {
        let (env, contract_id) = contract_env();
        let sub_id = 2;
        let hashed = make_hash(&env, 2);

        env.as_contract(&contract_id, || {
            push_key(&env, sub_id, &hashed);

            assert!(check_key(&env, sub_id, &hashed));
        });
    }

    #[test]
    fn test_check_key_missing_key_in_populated_buffer() {
        let (env, contract_id) = contract_env();
        let sub_id = 3;
        let hashed1 = make_hash(&env, 1);
        let hashed2 = make_hash(&env, 2);

        env.as_contract(&contract_id, || {
            push_key(&env, sub_id, &hashed1);

            assert!(!check_key(&env, sub_id, &hashed2));
        });
    }

    #[test]
    fn test_check_key_state_unchanged() {
        let (env, contract_id) = contract_env();
        let sub_id = 4;
        let hashed1 = make_hash(&env, 1);
        let hashed2 = make_hash(&env, 2);

        env.as_contract(&contract_id, || {
            push_key(&env, sub_id, &hashed1);

            let buf_before = load_buffer(&env, sub_id);

            let _ = check_key(&env, sub_id, &hashed2);

            let buf_after = load_buffer(&env, sub_id);

            assert_eq!(buf_before.cursor, buf_after.cursor);
            assert_eq!(buf_before.entries.len(), buf_after.entries.len());
            for i in 0..buf_before.entries.len() {
                assert_eq!(buf_before.entries.get(i), buf_after.entries.get(i));
            }
        });
    }
}
