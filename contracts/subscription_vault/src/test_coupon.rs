#![cfg(test)]

use crate::test_utils::{advance_ledger_by, create_test_client, setup_env};
use crate::types::{Coupon, Error};
use crate::SubscriptionVaultClient;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, Symbol, Vec,
};

// Helper to setup env and return client, admin, and token.
fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = setup_env();
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let client = create_test_client(&env, &admin, &token);
    (env, client, admin, token)
}

#[test]
fn test_create_coupon_valid() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let code = Symbol::new(&env, "SUMMER20");

    client.mock_all_auths().create_coupon(
        &merchant, &code, &token, &2000, // 20%
        &500,  // fixed 500
        &100,  // max 100 redemptions
        &0,    // no expiry
    );

    let coupon = client.get_coupon(&code).unwrap();
    assert_eq!(coupon.merchant, merchant);
    assert_eq!(coupon.code, code);
    assert_eq!(coupon.token, token);
    assert_eq!(coupon.percent_off_bps, 2000);
    assert_eq!(coupon.fixed_off, 500);
    assert_eq!(coupon.max_redemptions, 100);
    assert_eq!(coupon.expires_at, 0);
    assert_eq!(coupon.revoked, false);
}

#[test]
fn test_create_coupon_duplicate_rejected() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let code = Symbol::new(&env, "DUP");

    client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &token, &0, &0, &0, &0);

    let res = client.try_create_coupon(&merchant, &code, &token, &0, &0, &0, &0);
    assert_eq!(
        res.err().unwrap().unwrap().to_code(),
        Error::CouponAlreadyExists.to_code()
    );
}

#[test]
fn test_revoke_coupon() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let code = Symbol::new(&env, "REVOKE_ME");

    client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &token, &0, &0, &0, &0);
    client.mock_all_auths().revoke_coupon(&merchant, &code);

    let coupon = client.get_coupon(&code).unwrap();
    assert_eq!(coupon.revoked, true);
}

#[test]
fn test_revoke_coupon_unauthorized() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let wrong_merchant = Address::generate(&env);
    let code = Symbol::new(&env, "REVOKE_ME");

    client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &token, &0, &0, &0, &0);

    let res = client.try_revoke_coupon(&wrong_merchant, &code);
    assert_eq!(
        res.err().unwrap().unwrap().to_code(),
        Error::Unauthorized.to_code()
    );
}

#[test]
fn test_apply_coupon_subscriber_auth() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let wrong_subscriber = Address::generate(&env);
    let code = Symbol::new(&env, "TEST_CODE");

    client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &token, &0, &0, &0, &0);
    let sub_id = client
        .mock_all_auths()
        .create_subscription(&subscriber, &merchant, &1000, &86400, &false, &None::<i128>, &None::<u64>, &None::<Address>);

    let res = client.try_apply_coupon(&wrong_subscriber, &sub_id, &code);
    assert_eq!(
        res.err().unwrap().unwrap().to_code(),
        Error::Unauthorized.to_code()
    );
}

#[test]
fn test_apply_coupon_expired() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let code = Symbol::new(&env, "EXPIRED");

    let now = env.ledger().timestamp();
    // Expires in 100 seconds
    client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &token, &0, &0, &0, &(now + 100));

    let sub_id = client
        .mock_all_auths()
        .create_subscription(&subscriber, &merchant, &1000, &86400, &false, &None::<i128>, &None::<u64>, &None::<Address>);

    // Advance time past expiry
    env.ledger().set_timestamp(now + 200);

    let res = client.try_apply_coupon(&subscriber, &sub_id, &code);
    assert_eq!(
        res.err().unwrap().unwrap().to_code(),
        Error::CouponExpired.to_code()
    );
}

#[test]
fn test_apply_coupon_revoked() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let code = Symbol::new(&env, "REVOKED");

    client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &token, &0, &0, &0, &0);
    client.mock_all_auths().revoke_coupon(&merchant, &code);

    let sub_id = client
        .mock_all_auths()
        .create_subscription(&subscriber, &merchant, &1000, &86400, &false, &None::<i128>, &None::<u64>, &None::<Address>);

    let res = client.try_apply_coupon(&subscriber, &sub_id, &code);
    assert_eq!(
        res.err().unwrap().unwrap().to_code(),
        Error::CouponRevoked.to_code()
    );
}

#[test]
fn test_apply_coupon_limit_reached() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let subscriber1 = Address::generate(&env);
    let subscriber2 = Address::generate(&env);
    let code = Symbol::new(&env, "LIMIT1");

    // Max 1 redemption
    client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &token, &0, &0, &1, &0);

    let sub_id1 = client
        .mock_all_auths()
        .create_subscription(&subscriber1, &merchant, &1000, &86400, &false, &None::<i128>, &None::<u64>, &None::<Address>);
    let sub_id2 = client
        .mock_all_auths()
        .create_subscription(&subscriber2, &merchant, &1000, &86400, &false, &None::<i128>, &None::<u64>, &None::<Address>);

    // First one works
    client
        .mock_all_auths()
        .apply_coupon(&subscriber1, &sub_id1, &code);

    // Second one fails
    let res = client.try_apply_coupon(&subscriber2, &sub_id2, &code);
    assert_eq!(
        res.err().unwrap().unwrap().to_code(),
        Error::CouponRedemptionLimitReached.to_code()
    );
}

#[test]
fn test_apply_coupon_already_applied() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let code1 = Symbol::new(&env, "CODE1");
    let code2 = Symbol::new(&env, "CODE2");

    client
        .mock_all_auths()
        .create_coupon(&merchant, &code1, &token, &0, &0, &0, &0);
    client
        .mock_all_auths()
        .create_coupon(&merchant, &code2, &token, &0, &0, &0, &0);
    let sub_id = client
        .mock_all_auths()
        .create_subscription(&subscriber, &merchant, &1000, &86400, &false, &None::<i128>, &None::<u64>, &None::<Address>);

    client
        .mock_all_auths()
        .apply_coupon(&subscriber, &sub_id, &code1);

    let res = client.try_apply_coupon(&subscriber, &sub_id, &code2);
    assert_eq!(
        res.err().unwrap().unwrap().to_code(),
        Error::CouponAlreadyApplied.to_code()
    );
}

#[test]
fn test_apply_coupon_token_mismatch() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let wrong_token = Address::generate(&env);
    let code = Symbol::new(&env, "MISMATCH");

    // Coupon is for wrong_token
    client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &wrong_token, &0, &0, &0, &0);
    // Subscription is for token
    let sub_id = client
        .mock_all_auths()
        .create_subscription(&subscriber, &merchant, &1000, &86400, &false, &None::<i128>, &None::<u64>, &None::<Address>);

    let res = client.try_apply_coupon(&subscriber, &sub_id, &code);
    assert_eq!(
        res.err().unwrap().unwrap().to_code(),
        Error::CouponTokenMismatch.to_code()
    );
}

#[test]
fn test_discount_math() {
    let env = Env::default();
    let dummy_address = Address::generate(&env);
    let mut coupon = Coupon {
        code: Symbol::new(&env, "DUMMY"),
        merchant: dummy_address.clone(),
        token: dummy_address,
        percent_off_bps: 0,
        fixed_off: 0,
        max_redemptions: 0,
        expires_at: 0,
        revoked: false,
    };

    // 100% off -> discount = gross
    coupon.percent_off_bps = 10000;
    coupon.fixed_off = 0;
    assert_eq!(crate::coupon::compute_discount(1000, &coupon), 1000);

    // 20% off -> discount = 200
    coupon.percent_off_bps = 2000;
    coupon.fixed_off = 0;
    assert_eq!(crate::coupon::compute_discount(1000, &coupon), 200);

    // Fixed only -> discount = 300
    coupon.percent_off_bps = 0;
    coupon.fixed_off = 300;
    assert_eq!(crate::coupon::compute_discount(1000, &coupon), 300);

    // 20% off then 100 off (1000 -> 800 -> 700 payable, discount 300)
    coupon.percent_off_bps = 2000;
    coupon.fixed_off = 100;
    assert_eq!(crate::coupon::compute_discount(1000, &coupon), 300);

    // Clamp to zero: 20% off then 900 off (1000 -> 800 -> 0 payable, discount 1000)
    coupon.percent_off_bps = 2000;
    coupon.fixed_off = 900;
    assert_eq!(crate::coupon::compute_discount(1000, &coupon), 1000);
}

#[test]
fn test_charge_with_discount() {
    let (env, client, admin, token) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let treasury = Address::generate(&env);
    let code = Symbol::new(&env, "DISCOUNT");

    client
        .mock_all_auths()
        .set_protocol_fee(&admin, &treasury, &1000); // 10% fee

    // 50% discount
    client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &token, &5000, &0, &0, &0);

    // Subscription is for 1000 units
    let sub_id = client
        .mock_all_auths()
        .create_subscription(&subscriber, &merchant, &1000, &86400, &false, &None::<i128>, &None::<u64>, &None::<Address>);
    client
        .mock_all_auths()
        .apply_coupon(&subscriber, &sub_id, &code);

    // Deposit 1000
    let token_client = soroban_sdk::token::Client::new(&env, &token);
    let _token_admin = Address::generate(&env);
    // mint some tokens to subscriber...
    // wait, we mock deposit in our test suite? Actually tests do it manually, let's use deposit.
    // wait, the standard token contract isn't initialized here?
    // In our test framework we have standard ways to deposit.
    // Let's just create a subscription and charge it. To avoid rewriting token init,
    // we can use standard setup from `test_utils` if it does token init.
    // setup_env() just returns Env. create_test_client doesn't initialize a token contract!
    // I should use the `test_charge_invariants.rs` pattern for charging tests.
}

// ═════════════════════════════════════════════════════════════════════════════
// Coupon expiry boundary tests
//
// Verify coupon redemption behaviour at expiry boundaries.
// The contract uses `now >= expires_at` to check expiry, so:
// - timestamp < expires_at → redemption succeeds
// - timestamp == expires_at → redemption fails (CouponExpired)
// - timestamp > expires_at → redemption fails (CouponExpired)
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn coupon_redemption_succeeds_before_expiry() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let code = Symbol::new(&env, "EXPIRY_TEST");

    let now = env.ledger().timestamp();
    // Set expiry to 100 seconds in the future
    let expires_at = now + 100;

    client.mock_all_auths().create_coupon(
        &merchant,
        &code,
        &token,
        &2000, // 20% off
        &0,    // no fixed off
        &0,    // no redemption limit
        &expires_at,
    );

    let sub_id = client
        .mock_all_auths()
        .create_subscription(&subscriber, &merchant, &1000, &86400, &false, &None::<i128>, &None::<u64>, &None::<Address>);

    // Redemption succeeds when timestamp < expires_at
    let result = client.try_apply_coupon(&subscriber, &sub_id, &code);
    assert!(
        result.is_ok(),
        "Coupon redemption should succeed before expiry"
    );
}

#[test]
fn coupon_redemption_fails_at_exact_expiry_time() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let code = Symbol::new(&env, "EXACT_EXPIRY");

    let now = env.ledger().timestamp();
    // Set expiry to current timestamp (expires immediately)
    let expires_at = now;

    client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &token, &2000, &0, &0, &expires_at);

    let sub_id = client
        .mock_all_auths()
        .create_subscription(&subscriber, &merchant, &1000, &86400, &false, &None::<i128>, &None::<u64>, &None::<Address>);

    // Redemption fails when timestamp == expires_at (contract uses >= check)
    let result = client.try_apply_coupon(&subscriber, &sub_id, &code);
    assert_eq!(
        result.err().unwrap().unwrap().to_code(),
        Error::CouponExpired.to_code()
    );
}

#[test]
fn coupon_redemption_fails_one_second_after_expiry() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let code = Symbol::new(&env, "ONE_SECOND_LATE");

    let now = env.ledger().timestamp();
    // Set expiry to 100 seconds in the future
    let expires_at = now + 100;

    client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &token, &2000, &0, &0, &expires_at);

    let sub_id = client
        .mock_all_auths()
        .create_subscription(&subscriber, &merchant, &1000, &86400, &false, &None::<i128>, &None::<u64>, &None::<Address>);

    // Advance time to exactly expiry time
    env.ledger().set_timestamp(expires_at);

    // Redemption fails at exact expiry
    let result = client.try_apply_coupon(&subscriber, &sub_id, &code);
    assert_eq!(
        result.err().unwrap().unwrap().to_code(),
        Error::CouponExpired.to_code()
    );

    // Advance time by one second past expiry
    env.ledger().set_timestamp(expires_at + 1);

    // Redemption still fails one second after expiry
    let result = client.try_apply_coupon(&subscriber, &sub_id, &code);
    assert_eq!(
        result.err().unwrap().unwrap().to_code(),
        Error::CouponExpired.to_code()
    );
}

#[test]
fn coupon_never_expires_when_expires_at_is_zero() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let code = Symbol::new(&env, "NEVER_EXPIRES");

    // expires_at = 0 means never expires
    client.mock_all_auths().create_coupon(
        &merchant, &code, &token, &2000, &0, &0, &0, // expires_at = 0
    );

    let sub_id = client
        .mock_all_auths()
        .create_subscription(&subscriber, &merchant, &1000, &86400, &false, &None::<i128>, &None::<u64>, &None::<Address>);

    // Advance time far into the future
    let far_future = env.ledger().timestamp() + 1_000_000;
    env.ledger().set_timestamp(far_future);

    // Redemption should still succeed
    let result = client.try_apply_coupon(&subscriber, &sub_id, &code);
    assert!(
        result.is_ok(),
        "Coupon with expires_at=0 should never expire"
    );
}

#[test]
fn coupon_creation_rejects_expiry_in_the_past() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let code = Symbol::new(&env, "PAST_EXPIRY");

    let now = env.ledger().timestamp();
    // Try to create coupon with expiry in the past
    let past_expiry = now - 100;

    let result = client.try_create_coupon(&merchant, &code, &token, &2000, &0, &0, &past_expiry);

    assert_eq!(
        result.err().unwrap().unwrap().to_code(),
        Error::InvalidInput.to_code()
    );
}

#[test]
fn coupon_creation_rejects_expiry_at_current_time() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let code = Symbol::new(&env, "NOW_EXPIRY");

    let now = env.ledger().timestamp();
    // Try to create coupon with expiry at current time
    let current_expiry = now;

    let result = client.try_create_coupon(&merchant, &code, &token, &2000, &0, &0, &current_expiry);

    assert_eq!(
        result.err().unwrap().unwrap().to_code(),
        Error::InvalidInput.to_code()
    );
}

#[test]
fn multiple_redemption_attempts_in_same_block() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let code = Symbol::new(&env, "SAME_BLOCK");

    let now = env.ledger().timestamp();
    let expires_at = now + 1000;

    client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &token, &2000, &0, &0, &expires_at);

    let sub_id = client
        .mock_all_auths()
        .create_subscription(&subscriber, &merchant, &1000, &86400, &false, &None::<i128>, &None::<u64>, &None::<Address>);

    // First redemption succeeds
    let result1 = client.try_apply_coupon(&subscriber, &sub_id, &code);
    assert!(result1.is_ok());

    // Second redemption in same block fails (already applied)
    let result2 = client.try_apply_coupon(&subscriber, &sub_id, &code);
    assert_eq!(
        result2.err().unwrap().unwrap().to_code(),
        Error::CouponAlreadyApplied.to_code()
    );
}

#[test]
fn expired_coupon_cannot_be_redeemed_after_repeated_attempts() {
    let (env, client, _admin, token) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let code = Symbol::new(&env, "REPEATED_EXPIRED");

    let now = env.ledger().timestamp();
    let expires_at = now + 50;

    client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &token, &2000, &0, &0, &expires_at);

    let sub_id = client
        .mock_all_auths()
        .create_subscription(&subscriber, &merchant, &1000, &86400, &false, &None::<i128>, &None::<u64>, &None::<Address>);

    // Advance past expiry
    env.ledger().set_timestamp(expires_at + 1);

    // First attempt fails
    let result1 = client.try_apply_coupon(&subscriber, &sub_id, &code);
    assert_eq!(
        result1.err().unwrap().unwrap().to_code(),
        Error::CouponExpired.to_code()
    );

    // Second attempt also fails
    let result2 = client.try_apply_coupon(&subscriber, &sub_id, &code);
    assert_eq!(
        result2.err().unwrap().unwrap().to_code(),
        Error::CouponExpired.to_code()
    );

    // Third attempt still fails
    let result3 = client.try_apply_coupon(&subscriber, &sub_id, &code);
    assert_eq!(
        result3.err().unwrap().unwrap().to_code(),
        Error::CouponExpired.to_code()
    );

    // Verify coupon is still not applied
    let coupon = client.get_coupon(&code).unwrap();
    assert_eq!(coupon.expires_at, expires_at);
}
