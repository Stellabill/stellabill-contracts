//! Exact-boundary interval tests for back-to-back charges.
//!
//! Verifies the **inclusive-vs-exclusive** boundary contract that indexers rely on:
//! - `charge_subscription` at `last_payment_timestamp + interval_seconds` **succeeds**
//! - `charge_subscription` one second earlier **fails** with `IntervalNotElapsed`
//!
//! The boundary is defined by `next_charge_time` in `subscription.rs`:
//! ```text
//! next_charge_time(last_payment, interval) = last_payment + interval
//! charge_allowed when: now >= next_charge_time
//! ```
//!
//! # Security notes
//!
//! - An off-by-one allowing a charge one second too early would double-bill
//!   users within the same period.
//! - An off-by-one blocking the exact boundary would leave a gap where
//!   indexers observe a missed charge and may trigger spurious alerts.
//! - Both paths (success at boundary, rejection one second before) must be
//!   locked in by tests to prevent regressions.

use super::*;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{token, Address, Env};

const T0: u64 = 1_000_000;
const AMOUNT: i128 = 1_000_000;

fn setup_test_env() -> (
    Env,
    SubscriptionVaultClient<'static>,
    token::Client<'static>,
    token::StellarAssetClient<'static>,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = T0);

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token_admin_addr = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(token_admin_addr.clone());
    let token_client = token::Client::new(&env, &token_id.address());
    let token_admin_client = token::StellarAssetClient::new(&env, &token_id.address());

    client.init(
        &token_id.address(),
        &6,
        &admin,
        &1_000_000i128,
        &(7 * 24 * 60 * 60),
    );

    (env, client, token_client, token_admin_client)
}

fn create_and_fund(
    env: &Env,
    client: &SubscriptionVaultClient,
    token_admin: &token::StellarAssetClient<'static>,
    interval: u64,
) -> (u32, Address, Address) {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &interval,
        &false,
        &None::<i128>,
        &None::<u64>,
    &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    // Fund enough for many charges.
    token_admin.mint(&subscriber, &1_000_000_000i128);
    let deposit = 50_000_000i128;
    client.deposit_funds(&id, &subscriber, &deposit, &None::<soroban_sdk::BytesN<32>>);

    (id, subscriber, merchant)
}

// ── Standard interval boundary ──────────────────────────────────────────────

/// Charge at exactly `last_payment_timestamp + interval_seconds` must succeed.
///
/// The subscription is created at `T0` which sets `last_payment_timestamp = T0`.
/// After advancing the ledger to `T0 + interval`, the charge must be accepted.
#[test]
fn test_charge_at_exact_boundary_succeeds() {
    let (env, client, _token, token_admin) = setup_test_env();
    let interval: u64 = 300; // 5 minutes
    let (id, _, _) = create_and_fund(&env, &client, &token_admin, interval);

    // Advance to exactly T0 + interval (the boundary).
    env.ledger().with_mut(|l| l.timestamp = T0 + interval);

    let res = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
    assert!(res.is_ok(), "charge at exact boundary must succeed");
    assert_eq!(res.unwrap(), Ok(ChargeExecutionResult::Charged));
}

/// Charge one second before `last_payment_timestamp + interval_seconds`
/// must fail with `IntervalNotElapsed`.
#[test]
fn test_charge_one_second_before_boundary_fails() {
    let (env, client, _token, token_admin) = setup_test_env();
    let interval: u64 = 300;
    let (id, _, _) = create_and_fund(&env, &client, &token_admin, interval);

    // Advance to T0 + interval - 1 (one second before boundary).
    env.ledger().with_mut(|l| l.timestamp = T0 + interval - 1);

    let res = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(res, Err(Ok(Error::IntervalNotElapsed)));
}

/// After a successful charge at the boundary, the next charge at
/// `new_last_payment + interval` must also succeed, confirming
/// back-to-back charging at exact boundaries works.
#[test]
fn test_back_to_back_charges_at_exact_boundary() {
    let (env, client, _token, token_admin) = setup_test_env();
    let interval: u64 = 300;
    let (id, _, _) = create_and_fund(&env, &client, &token_admin, interval);

    // First charge at T0 + interval.
    env.ledger().with_mut(|l| l.timestamp = T0 + interval);
    let r1 = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(r1.unwrap(), Ok(ChargeExecutionResult::Charged));

    // Second charge at T0 + 2*interval (exactly one interval after the first).
    env.ledger().with_mut(|l| l.timestamp = T0 + 2 * interval);
    let r2 = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(r2.unwrap(), Ok(ChargeExecutionResult::Charged));

    // Verify the subscription state reflects two charges.
    let sub = client.get_subscription(&id);
    assert_eq!(sub.lifetime_charged, AMOUNT * 2);
}

// ── Minimum interval (interval_seconds = 1) ─────────────────────────────────

/// Charge at exactly `last_payment_timestamp + 1` with interval_seconds=1.
///
/// This is the tightest possible interval.  The boundary at +1 must succeed
/// while the boundary at +0 (same second) must fail.
#[test]
fn test_interval_one_second_at_boundary_succeeds() {
    let (env, client, _token, token_admin) = setup_test_env();
    let interval: u64 = 1;
    let (id, _, _) = create_and_fund(&env, &client, &token_admin, interval);

    // Charge at T0 + 1 (exactly one second after creation).
    env.ledger().with_mut(|l| l.timestamp = T0 + 1);
    let res = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
    assert!(res.is_ok(), "charge at T0+1 with interval=1 must succeed");
    assert_eq!(res.unwrap(), Ok(ChargeExecutionResult::Charged));

    // Next charge at T0 + 2.
    env.ledger().with_mut(|l| l.timestamp = T0 + 2);
    let res2 = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
    assert!(res2.is_ok(), "charge at T0+2 with interval=1 must succeed");
    assert_eq!(res2.unwrap(), Ok(ChargeExecutionResult::Charged));
}

/// Charge at T0 (same second as creation) with interval_seconds=1 must fail.
#[test]
fn test_interval_one_second_too_early_fails() {
    let (env, client, _token, token_admin) = setup_test_env();
    let interval: u64 = 1;
    let (id, _, _) = create_and_fund(&env, &client, &token_admin, interval);

    // Attempt charge at T0 (same second as last_payment_timestamp).
    env.ledger().with_mut(|l| l.timestamp = T0);
    let res = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(res, Err(Ok(Error::IntervalNotElapsed)));
}

/// Rapid back-to-back at interval=1: charge once per second for 5 seconds.
#[test]
fn test_rapid_back_to_back_interval_one() {
    let (env, client, _token, token_admin) = setup_test_env();
    let interval: u64 = 1;
    let (id, _, _) = create_and_fund(&env, &client, &token_admin, interval);

    for i in 1..=5u64 {
        env.ledger().with_mut(|l| l.timestamp = T0 + i);
        let res = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
        assert!(
            res.is_ok(),
            "charge at T0+{i} with interval=1 must succeed"
        );
        assert_eq!(res.unwrap(), Ok(ChargeExecutionResult::Charged));
    }

    let sub = client.get_subscription(&id);
    assert_eq!(sub.lifetime_charged, AMOUNT * 5);
}

// ── Edge: minimum valid interval (60 seconds) ───────────────────────────────

/// Boundary test with the minimum allowed interval (60 seconds).
#[test]
fn test_minimum_valid_interval_boundary() {
    let (env, client, _token, token_admin) = setup_test_env();
    let interval: u64 = 60; // MIN_SUBSCRIPTION_INTERVAL_SECONDS
    let (id, _, _) = create_and_fund(&env, &client, &token_admin, interval);

    // One second before boundary → rejected.
    env.ledger().with_mut(|l| l.timestamp = T0 + interval - 1);
    let early = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(early, Err(Ok(Error::IntervalNotElapsed)));

    // At boundary → accepted.
    env.ledger().with_mut(|l| l.timestamp = T0 + interval);
    let on_time = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(on_time.unwrap(), Ok(ChargeExecutionResult::Charged));
}

// ── Edge: very large interval ───────────────────────────────────────────────

/// Boundary test with a very large interval (u64::MAX / 2).
///
/// Ensures the checked-add in `next_charge_time` does not overflow and the
/// boundary logic holds for extreme values.
#[test]
fn test_very_large_interval_boundary() {
    let (env, client, _token, token_admin) = setup_test_env();
    let interval: u64 = u64::MAX / 2;
    let (id, _, _) = create_and_fund(&env, &client, &token_admin, interval);

    let boundary = T0 + interval;

    // One second before boundary → rejected.
    env.ledger().with_mut(|l| l.timestamp = boundary - 1);
    let early = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(early, Err(Ok(Error::IntervalNotElapsed)));

    // At boundary → accepted.
    env.ledger().with_mut(|l| l.timestamp = boundary);
    let on_time = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(on_time.unwrap(), Ok(ChargeExecutionResult::Charged));
}

// ── Edge: last_payment_timestamp = 0 ────────────────────────────────────────

/// Subscription created at ledger timestamp 0.
///
/// `last_payment_timestamp` is set to `env.ledger().timestamp()` at creation,
/// which is 0 in this test.  The boundary must still be enforced correctly.
#[test]
fn test_last_payment_timestamp_zero() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 0);

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token_admin_addr = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(token_admin_addr.clone());
    let token_admin = token::StellarAssetClient::new(&env, &token_id.address());

    client.init(
        &token_id.address(),
        &6,
        &admin,
        &1_000_000i128,
        &(7 * 24 * 60 * 60),
    );

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let interval: u64 = 120;

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &interval,
        &false,
        &None::<i128>,
        &None::<u64>,
    &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    token_admin.mint(&subscriber, &1_000_000_000i128);
    client.deposit_funds(&id, &subscriber, &50_000_000i128, &None::<soroban_sdk::BytesN<32>>);

    // One second before boundary (interval - 1) → rejected.
    env.ledger().with_mut(|l| l.timestamp = interval - 1);
    let early = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(early, Err(Ok(Error::IntervalNotElapsed)));

    // At boundary (interval) → accepted.
    env.ledger().with_mut(|l| l.timestamp = interval);
    let on_time = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(on_time.unwrap(), Ok(ChargeExecutionResult::Charged));
}

// ── Edge: repeated rejection then success ───────────────────────────────────

/// Attempt charges at progressively closer timestamps, confirming rejection
/// until the exact boundary, then success.
#[test]
fn test_progressive_approach_to_boundary() {
    let (env, client, _token, token_admin) = setup_test_env();
    let interval: u64 = 300;
    let (id, _, _) = create_and_fund(&env, &client, &token_admin, interval);

    // Try at T0 + 1, T0 + 100, T0 + 298, T0 + 299 — all should fail.
    for offset in [1, 100, interval - 2, interval - 1] {
        env.ledger().with_mut(|l| l.timestamp = T0 + offset);
        let res = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
        assert_eq!(
            res,
            Err(Ok(Error::IntervalNotElapsed)),
            "charge at T0+{offset} must be rejected"
        );
    }

    // At T0 + interval → success.
    env.ledger().with_mut(|l| l.timestamp = T0 + interval);
    let res = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(res.unwrap(), Ok(ChargeExecutionResult::Charged));
}
