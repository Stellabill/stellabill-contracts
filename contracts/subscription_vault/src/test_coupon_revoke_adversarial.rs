#![cfg(test)]

//! Adversarial coverage for `revoke_coupon`
//! (`contracts/subscription_vault/src/coupon.rs`).
//!
//! The happy-path suite (`test_coupon.rs`) verifies that the owner can revoke a
//! coupon. These tests pin the *negative* and boundary behaviour that the
//! revocation path actually relies on:
//!
//! * an unknown code is `CouponNotFound`, not a silent no-op,
//! * a non-owner is `Unauthorized` and cannot mutate another merchant's coupon,
//! * authorization is decided **before** any state or event is written,
//! * revocation flips only the `revoked` flag and is idempotent,
//! * exactly one `CouponRevokedEvent` is emitted, carrying the ledger
//!   timestamp and the current schema version,
//! * revoking one code cannot affect a sibling coupon.

use crate::test_utils::{create_test_client, setup_env};
use crate::types::{Coupon, CouponRevokedEvent, Error, EVENT_SCHEMA_VERSION};
use crate::SubscriptionVaultClient;
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger},
    Address, Env, IntoVal, Symbol,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = setup_env();
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let client = create_test_client(&env, &admin, &token);
    (env, client, admin, token)
}

fn create_coupon(
    client: &SubscriptionVaultClient,
    merchant: &Address,
    token: &Address,
    code: &Symbol,
) {
    client.mock_all_auths().create_coupon(
        merchant, code, token, &2_000, // 20%
        &500,  // fixed 500
        &100,  // max 100 redemptions
        &0,    // no expiry
    );
}

// ── Authorization ─────────────────────────────────────────────────────────────

#[test]
fn revoke_by_owner_marks_coupon_revoked() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let code = Symbol::new(&env, "OWNER");

    create_coupon(&client, &merchant, &token, &code);
    assert!(!client.get_coupon(&code).unwrap().revoked);

    client.mock_all_auths().revoke_coupon(&merchant, &code);

    assert!(client.get_coupon(&code).unwrap().revoked);
}

#[test]
fn revoke_by_non_owner_is_unauthorized() {
    let (env, client, _admin, token) = setup();
    let owner = Address::generate(&env);
    let attacker = Address::generate(&env);
    let code = Symbol::new(&env, "NOTMINE");

    create_coupon(&client, &owner, &token, &code);

    let res = client.try_revoke_coupon(&attacker, &code);
    assert_eq!(
        res.err().unwrap().unwrap().to_code(),
        Error::Unauthorized.to_code()
    );
    assert!(!client.get_coupon(&code).unwrap().revoked);
}

#[test]
fn unauthorized_revoke_leaves_coupon_fields_untouched() {
    let (env, client, _admin, token) = setup();
    let owner = Address::generate(&env);
    let attacker = Address::generate(&env);
    let code = Symbol::new(&env, "INTACT");

    create_coupon(&client, &owner, &token, &code);
    let before = client.get_coupon(&code).unwrap();

    let _ = client.try_revoke_coupon(&attacker, &code);

    let after = client.get_coupon(&code).unwrap();
    assert_eq!(before, after);
    assert!(!after.revoked);
    assert_eq!(after.merchant, owner);
    assert_eq!(after.percent_off_bps, 2_000);
    assert_eq!(after.fixed_off, 500);
    assert_eq!(after.max_redemptions, 100);
    assert_eq!(after.redemptions, 0);
}

#[test]
fn unauthorized_revoke_emits_no_event() {
    let (env, client, _admin, token) = setup();
    let owner = Address::generate(&env);
    let attacker = Address::generate(&env);
    let code = Symbol::new(&env, "NOEVT");

    create_coupon(&client, &owner, &token, &code);
    let before = env.events().all().len();

    let _ = client.try_revoke_coupon(&attacker, &code);

    assert_eq!(env.events().all().len(), before);
}

// ── Not found ─────────────────────────────────────────────────────────────────

#[test]
fn revoke_unknown_code_is_coupon_not_found() {
    let (env, client, _admin, _token) = setup();
    let merchant = Address::generate(&env);
    let code = Symbol::new(&env, "GHOST");

    let res = client.try_revoke_coupon(&merchant, &code);
    assert_eq!(
        res.err().unwrap().unwrap().to_code(),
        Error::CouponNotFound.to_code()
    );
    assert!(client.get_coupon(&code).is_none());
}

#[test]
fn revoke_unknown_code_emits_no_event() {
    let (env, client, _admin, _token) = setup();
    let merchant = Address::generate(&env);
    let code = Symbol::new(&env, "GHOST2");

    let before = env.events().all().len();
    let _ = client.try_revoke_coupon(&merchant, &code);

    assert_eq!(env.events().all().len(), before);
}

// ── Mutation surface ──────────────────────────────────────────────────────────

#[test]
fn revoke_flips_only_the_revoked_flag() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let code = Symbol::new(&env, "SHAPE");

    create_coupon(&client, &merchant, &token, &code);
    let before = client.get_coupon(&code).unwrap();

    client.mock_all_auths().revoke_coupon(&merchant, &code);

    let after = client.get_coupon(&code).unwrap();
    let expected = Coupon {
        revoked: true,
        ..before.clone()
    };
    assert_eq!(after, expected);
}

#[test]
fn revoke_is_idempotent() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let code = Symbol::new(&env, "TWICE");

    create_coupon(&client, &merchant, &token, &code);

    client.mock_all_auths().revoke_coupon(&merchant, &code);
    client.mock_all_auths().revoke_coupon(&merchant, &code);

    assert!(client.get_coupon(&code).unwrap().revoked);
}

#[test]
fn revoking_one_code_does_not_affect_a_sibling_code() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let code_a = Symbol::new(&env, "SIBLINGA");
    let code_b = Symbol::new(&env, "SIBLINGB");

    create_coupon(&client, &merchant, &token, &code_a);
    create_coupon(&client, &merchant, &token, &code_b);
    let b_before = client.get_coupon(&code_b).unwrap();

    client.mock_all_auths().revoke_coupon(&merchant, &code_a);

    assert!(client.get_coupon(&code_a).unwrap().revoked);
    assert!(!client.get_coupon(&code_b).unwrap().revoked);
    assert_eq!(client.get_coupon(&code_b).unwrap(), b_before);
}

// ── Event contract ────────────────────────────────────────────────────────────

#[test]
fn revoke_emits_one_event_with_merchant_code_timestamp_and_schema() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let code = Symbol::new(&env, "WITHEVT");

    create_coupon(&client, &merchant, &token, &code);

    env.ledger().with_mut(|li| li.timestamp = 1_234_567);
    let before = env.events().all().len();

    client.mock_all_auths().revoke_coupon(&merchant, &code);

    let events = env.events().all();
    assert_eq!(events.len(), before + 1);
    let payload: CouponRevokedEvent = events.last().unwrap().2.into_val(&env);
    assert_eq!(payload.merchant, merchant);
    assert_eq!(payload.code, code);
    assert_eq!(payload.timestamp, 1_234_567);
    assert_eq!(payload.schema_version, EVENT_SCHEMA_VERSION);
}
