//! Tests for merchant KYC attestation (feat/merchant-kyc-attestation).
//!
//! ## Coverage matrix
//!
//! | # | Scenario | Expected |
//! |---|----------|----------|
//! | 1 | KYC optional — withdraw with no attestation succeeds when flag is off | `Ok` |
//! | 2 | set_kyc_required defaults to false | `false` |
//! | 3 | admin can enable KYC-required flag | `true` |
//! | 4 | admin can disable KYC-required flag | `false` |
//! | 5 | non-admin cannot set KYC-required flag | `Error::Unauthorized` |
//! | 6 | admin can attach KYC attestation | record stored, event emitted |
//! | 7 | non-admin cannot attach KYC | `Error::Unauthorized` |
//! | 8 | double-attach on active attestation rejected | `Error::KycAlreadyAttached` |
//! | 9 | can re-attach after revocation | `Ok` |
//! | 10 | missing KYC blocks withdraw when required | `Error::KycNotAttached` |
//! | 11 | revoked KYC blocks withdraw when required | `Error::KycNotAttached` |
//! | 12 | active KYC allows withdraw when required | `Ok` |
//! | 13 | revoke non-existent attestation is idempotent | `Ok` |
//! | 14 | non-admin cannot revoke KYC | `Error::Unauthorized` |
//! | 15 | get_merchant_kyc returns None when no record | `None` |
//! | 16 | get_merchant_kyc returns Some when record exists | status = true |
//! | 17 | get_merchant_kyc status = false after revoke | status = false |
//! | 18 | attach_merchant_kyc emits MerchantKycAttachedEvent | event fields correct |
//! | 19 | revoke_merchant_kyc emits MerchantKycRevokedEvent | event fields correct |
//! | 20 | set_kyc_required emits KycRequiredSetEvent | event fields correct |
//! | 21 | KYC flag off → withdraw works without any attestation | `Ok` |
//! | 22 | KYC flag on, then turned off → withdraw works without attestation | `Ok` |

use crate::types::{
    DataKey, KycRequiredSetEvent, MerchantKycAttachedEvent, MerchantKycRevokedEvent,
    EVENT_SCHEMA_VERSION,
};
use crate::{Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::{Address as _, Events as _};
use soroban_sdk::{Address, Bytes, Env, Symbol, TryFromVal};

// ── Test helpers ─────────────────────────────────────────────────────────────

/// Creates a fully initialised contract. Returns `(env, client, token, admin)`.
fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    (env, client, token, admin)
}

/// Registers a merchant config for `merchant` so they can withdraw.
fn init_merchant(env: &Env, client: &SubscriptionVaultClient, merchant: &Address) {
    let payout = Address::generate(env);
    client.initialize_merchant_config(
        merchant,
        &payout,
        &0,
        &1,
        &None,
        &soroban_sdk::String::from_str(env, ""),
    );
}

/// Seeds a merchant balance and mints matching tokens to the contract vault
/// so that `withdraw_merchant_token_funds` does not fail on the token transfer.
fn seed_merchant_balance_and_mint(
    env: &Env,
    client: &SubscriptionVaultClient,
    merchant: &Address,
    token: &Address,
    balance: i128,
) {
    env.as_contract(&client.address, || {
        env.storage().instance().set(
            &DataKey::MerchantBalance(merchant.clone(), token.clone()),
            &balance,
        );
    });
    soroban_sdk::token::StellarAssetClient::new(env, token).mint(&client.address, &balance);
}

/// Returns a short opaque attestation hash for tests.
fn fake_hash(env: &Env) -> Bytes {
    Bytes::from_slice(env, &[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE])
}

// ── Test: global KYC-required flag ───────────────────────────────────────────

/// (2) `get_kyc_required` returns `false` by default.
#[test]
fn kyc_required_defaults_to_false() {
    let (_env, client, _token, _admin) = setup();
    assert!(!client.get_kyc_required());
}

/// (3) Admin can enable the KYC-required flag.
#[test]
fn admin_can_enable_kyc_required() {
    let (_env, client, _token, admin) = setup();
    client.set_kyc_required(&admin, &true);
    assert!(client.get_kyc_required());
}

/// (4) Admin can disable the KYC-required flag after enabling it.
#[test]
fn admin_can_disable_kyc_required() {
    let (_env, client, _token, admin) = setup();
    client.set_kyc_required(&admin, &true);
    assert!(client.get_kyc_required());
    client.set_kyc_required(&admin, &false);
    assert!(!client.get_kyc_required());
}

/// (5) Non-admin cannot change the KYC-required flag.
#[test]
fn non_admin_cannot_set_kyc_required() {
    let (env, client, _token, _admin) = setup();
    let attacker = Address::generate(&env);
    let result = client.try_set_kyc_required(&attacker, &true);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
    // Flag must remain unchanged.
    assert!(!client.get_kyc_required());
}

/// (20) `set_kyc_required` emits a `KycRequiredSetEvent`.
#[test]
fn set_kyc_required_emits_event() {
    let (env, client, _token, admin) = setup();
    client.set_kyc_required(&admin, &true);

    let events = env.events().all();
    // Find the kyc_required_set event (last emitted)
    let last = events.last().expect("at least one event");
    let topic: Symbol = Symbol::try_from_val(&env, &last.1.get(0).unwrap()).unwrap();
    assert_eq!(topic, Symbol::new(&env, "kyc_required_set"));

    let payload = KycRequiredSetEvent::try_from_val(&env, &last.2).unwrap();
    assert!(payload.required);
    assert_eq!(payload.admin, admin);
    assert_eq!(payload.schema_version, EVENT_SCHEMA_VERSION);
}

// ── Test: attach_merchant_kyc ────────────────────────────────────────────────

/// (6) Admin can attach a KYC attestation to a merchant.
#[test]
fn admin_can_attach_merchant_kyc() {
    let (env, client, _token, admin) = setup();
    let merchant = Address::generate(&env);
    let hash = fake_hash(&env);

    client.attach_merchant_kyc(&admin, &merchant, &hash, &1_000_000u64);

    let record = client
        .get_merchant_kyc(&merchant)
        .expect("record must exist");
    assert!(record.status);
    assert_eq!(record.attestation_hash, hash);
    assert_eq!(record.issued_at, 1_000_000u64);
}

/// (7) Non-admin cannot attach a KYC attestation.
#[test]
fn non_admin_cannot_attach_merchant_kyc() {
    let (env, client, _token, _admin) = setup();
    let merchant = Address::generate(&env);
    let attacker = Address::generate(&env);
    let hash = fake_hash(&env);

    let result = client.try_attach_merchant_kyc(&attacker, &merchant, &hash, &0u64);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
    assert!(client.get_merchant_kyc(&merchant).is_none());
}

/// (8) Attaching when an active attestation already exists is rejected.
#[test]
fn double_attach_on_active_attestation_is_rejected() {
    let (env, client, _token, admin) = setup();
    let merchant = Address::generate(&env);
    let hash = fake_hash(&env);

    client.attach_merchant_kyc(&admin, &merchant, &hash, &0u64);

    let second_hash = Bytes::from_slice(&env, &[0xFF; 8]);
    let result = client.try_attach_merchant_kyc(&admin, &merchant, &second_hash, &0u64);
    assert_eq!(result, Err(Ok(Error::KycAlreadyAttached)));

    // Original record must be unchanged.
    let record = client.get_merchant_kyc(&merchant).unwrap();
    assert_eq!(record.attestation_hash, hash);
}

/// (18) `attach_merchant_kyc` emits a `MerchantKycAttachedEvent`.
#[test]
fn attach_merchant_kyc_emits_event() {
    let (env, client, _token, admin) = setup();
    let merchant = Address::generate(&env);
    let hash = fake_hash(&env);
    let issued = env.ledger().timestamp();

    client.attach_merchant_kyc(&admin, &merchant, &hash, &issued);

    let events = env.events().all();
    let last = events.last().unwrap();
    let topic: Symbol = Symbol::try_from_val(&env, &last.1.get(0).unwrap()).unwrap();
    assert_eq!(topic, Symbol::new(&env, "kyc_attached"));

    let payload = MerchantKycAttachedEvent::try_from_val(&env, &last.2).unwrap();
    assert_eq!(payload.merchant, merchant);
    assert_eq!(payload.attestation_hash, hash);
    assert_eq!(payload.issued_at, issued);
    assert_eq!(payload.schema_version, EVENT_SCHEMA_VERSION);
}

// ── Test: revoke_merchant_kyc ────────────────────────────────────────────────

/// (13) Revoking when no attestation exists is a silent no-op.
#[test]
fn revoke_non_existent_attestation_is_idempotent() {
    let (env, client, _token, admin) = setup();
    let merchant = Address::generate(&env);

    // No record exists yet — must not panic.
    client.revoke_merchant_kyc(&admin, &merchant);

    let record = client.get_merchant_kyc(&merchant).unwrap();
    assert!(!record.status);
}

/// (14) Non-admin cannot revoke a KYC attestation.
#[test]
fn non_admin_cannot_revoke_merchant_kyc() {
    let (env, client, _token, admin) = setup();
    let merchant = Address::generate(&env);
    let attacker = Address::generate(&env);
    let hash = fake_hash(&env);

    client.attach_merchant_kyc(&admin, &merchant, &hash, &0u64);

    let result = client.try_revoke_merchant_kyc(&attacker, &merchant);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));

    // Record must still be active.
    let record = client.get_merchant_kyc(&merchant).unwrap();
    assert!(record.status);
}

/// (17) After `revoke_merchant_kyc`, the stored record has `status = false`.
#[test]
fn revoke_sets_status_to_false() {
    let (env, client, _token, admin) = setup();
    let merchant = Address::generate(&env);
    let hash = fake_hash(&env);

    client.attach_merchant_kyc(&admin, &merchant, &hash, &0u64);
    assert!(client.get_merchant_kyc(&merchant).unwrap().status);

    client.revoke_merchant_kyc(&admin, &merchant);
    assert!(!client.get_merchant_kyc(&merchant).unwrap().status);
}

/// (9) After revocation, admin can attach a fresh attestation.
#[test]
fn can_reattach_after_revocation() {
    let (env, client, _token, admin) = setup();
    let merchant = Address::generate(&env);
    let hash1 = fake_hash(&env);
    let hash2 = Bytes::from_slice(&env, &[0xAB; 8]);

    client.attach_merchant_kyc(&admin, &merchant, &hash1, &0u64);
    client.revoke_merchant_kyc(&admin, &merchant);
    // Re-attach with a new hash.
    client.attach_merchant_kyc(&admin, &merchant, &hash2, &1_000u64);

    let record = client.get_merchant_kyc(&merchant).unwrap();
    assert!(record.status);
    assert_eq!(record.attestation_hash, hash2);
}

/// (19) `revoke_merchant_kyc` emits a `MerchantKycRevokedEvent`.
#[test]
fn revoke_merchant_kyc_emits_event() {
    let (env, client, _token, admin) = setup();
    let merchant = Address::generate(&env);
    let hash = fake_hash(&env);

    client.attach_merchant_kyc(&admin, &merchant, &hash, &0u64);
    client.revoke_merchant_kyc(&admin, &merchant);

    let events = env.events().all();
    let last = events.last().unwrap();
    let topic: Symbol = Symbol::try_from_val(&env, &last.1.get(0).unwrap()).unwrap();
    assert_eq!(topic, Symbol::new(&env, "kyc_revoked"));

    let payload = MerchantKycRevokedEvent::try_from_val(&env, &last.2).unwrap();
    assert_eq!(payload.merchant, merchant);
    assert_eq!(payload.schema_version, EVENT_SCHEMA_VERSION);
}

// ── Test: get_merchant_kyc ───────────────────────────────────────────────────

/// (15) `get_merchant_kyc` returns `None` when no record exists.
#[test]
fn get_merchant_kyc_returns_none_when_no_record() {
    let (env, client, _token, _admin) = setup();
    let merchant = Address::generate(&env);
    assert!(client.get_merchant_kyc(&merchant).is_none());
}

/// (16) `get_merchant_kyc` returns `Some` with `status = true` after attach.
#[test]
fn get_merchant_kyc_returns_record_after_attach() {
    let (env, client, _token, admin) = setup();
    let merchant = Address::generate(&env);
    let hash = fake_hash(&env);

    client.attach_merchant_kyc(&admin, &merchant, &hash, &42u64);

    let record = client
        .get_merchant_kyc(&merchant)
        .expect("record must exist");
    assert!(record.status);
    assert_eq!(record.attestation_hash, hash);
    assert_eq!(record.issued_at, 42u64);
}

// ── Test: withdraw gating ────────────────────────────────────────────────────

/// (1) KYC optional — withdraw without any attestation succeeds when flag is off.
#[test]
fn withdraw_without_kyc_succeeds_when_flag_is_off() {
    let (env, client, token, _admin) = setup();
    let merchant = Address::generate(&env);

    init_merchant(&env, &client, &merchant);
    seed_merchant_balance_and_mint(&env, &client, &merchant, &token, 5_000_000i128);

    // No KYC attached, flag is off — should succeed.
    client.withdraw_merchant_token_funds(&merchant, &token, &1_000_000i128);
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        4_000_000i128
    );
}

/// (10) Missing KYC attestation blocks withdraw when `kyc_required` is true.
#[test]
fn missing_kyc_blocks_withdraw_when_required() {
    let (env, client, token, admin) = setup();
    let merchant = Address::generate(&env);

    init_merchant(&env, &client, &merchant);
    seed_merchant_balance_and_mint(&env, &client, &merchant, &token, 5_000_000i128);

    client.set_kyc_required(&admin, &true);

    let result = client.try_withdraw_merchant_token_funds(&merchant, &token, &1_000_000i128);
    assert_eq!(result, Err(Ok(Error::KycNotAttached)));
    // Balance unchanged.
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        5_000_000i128
    );
}

/// (11) Revoked KYC blocks withdraw when `kyc_required` is true.
#[test]
fn revoked_kyc_blocks_withdraw_when_required() {
    let (env, client, token, admin) = setup();
    let merchant = Address::generate(&env);
    let hash = fake_hash(&env);

    init_merchant(&env, &client, &merchant);
    seed_merchant_balance_and_mint(&env, &client, &merchant, &token, 5_000_000i128);

    client.attach_merchant_kyc(&admin, &merchant, &hash, &0u64);
    client.revoke_merchant_kyc(&admin, &merchant);
    client.set_kyc_required(&admin, &true);

    let result = client.try_withdraw_merchant_token_funds(&merchant, &token, &1_000_000i128);
    assert_eq!(result, Err(Ok(Error::KycNotAttached)));
}

/// (12) Active KYC allows withdraw when `kyc_required` is true.
#[test]
fn active_kyc_allows_withdraw_when_required() {
    let (env, client, token, admin) = setup();
    let merchant = Address::generate(&env);
    let hash = fake_hash(&env);

    init_merchant(&env, &client, &merchant);
    seed_merchant_balance_and_mint(&env, &client, &merchant, &token, 5_000_000i128);

    client.set_kyc_required(&admin, &true);
    client.attach_merchant_kyc(&admin, &merchant, &hash, &0u64);

    // With active KYC and flag enabled — should succeed.
    client.withdraw_merchant_token_funds(&merchant, &token, &1_000_000i128);
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        4_000_000i128
    );
}

/// (21) KYC flag off → withdraw works without any attestation record.
#[test]
fn kyc_flag_off_withdraw_without_attestation_works() {
    let (env, client, token, admin) = setup();
    let merchant = Address::generate(&env);

    init_merchant(&env, &client, &merchant);
    seed_merchant_balance_and_mint(&env, &client, &merchant, &token, 5_000_000i128);

    // Explicitly set flag to false (same as default, just asserting explicitly).
    client.set_kyc_required(&admin, &false);
    assert!(!client.get_kyc_required());

    client.withdraw_merchant_token_funds(&merchant, &token, &2_000_000i128);
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        3_000_000i128
    );
}

/// (22) Enabling then disabling kyc_required lets a merchant withdraw without attestation.
#[test]
fn disabling_kyc_required_restores_withdraw_access() {
    let (env, client, token, admin) = setup();
    let merchant = Address::generate(&env);

    init_merchant(&env, &client, &merchant);
    seed_merchant_balance_and_mint(&env, &client, &merchant, &token, 5_000_000i128);

    // Enable KYC — now blocked.
    client.set_kyc_required(&admin, &true);
    assert_eq!(
        client.try_withdraw_merchant_token_funds(&merchant, &token, &1_000_000i128),
        Err(Ok(Error::KycNotAttached))
    );

    // Disable KYC — now allowed.
    client.set_kyc_required(&admin, &false);
    client.withdraw_merchant_token_funds(&merchant, &token, &1_000_000i128);
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        4_000_000i128
    );
}
