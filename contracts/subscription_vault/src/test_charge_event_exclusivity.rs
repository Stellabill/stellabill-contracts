//! Property tests asserting that a single `charge_subscription` invocation never
//! emits both a success and a failure event for the same subscription.
//!
//! **Why this matters:** Off-chain indexers consume contract events to track
//! billing outcomes. If a single charge call emitted both `SubscriptionChargedEvent`
//! (success) and `SubscriptionChargeFailedEvent` / `ChargeFailureEvent` (failure),
//! the indexer would double-count the charge, corrupting revenue accounting.
//!
//! **Event taxonomy:**
//!
//! | Topic                        | Struct                    | Meaning               |
//! |------------------------------|---------------------------|-----------------------|
//! | `"charged"`                  | `SubscriptionChargedEvent`| Charge succeeded       |
//! | `"charge_failed"`            | `SubscriptionChargeFailedEvent` | Insufficient balance |
//! | `"charge_failed_v2"`         | `ChargeFailureEvent`      | Generic charge error   |
//!
//! The success topic is emitted **only** when `charge_one` returns `Ok(Charged)`.
//! All error paths return `Err(...)` which prevents the success event from being
//! published in the enclosing `charge_subscription` entry point.
//!
//! These tests exercise every reachable branch through the Soroban client and
//! inspect `env.events().all()` after each invocation to verify mutual
//! exclusivity.

use proptest::prelude::*;
use soroban_sdk::testutils::{Events, Ledger};
use soroban_sdk::{FromVal, Symbol};

use crate::test_utils::fixtures;
use crate::test_utils::setup::TestEnv;
use crate::SubscriptionStatus;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const T0: u64 = 1_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const AMOUNT: i128 = 10_000_000; // 10 USDC (6 decimals)

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return `true` if the env's event list contains the `"charged"` success topic.
fn has_charged_event(test_env: &TestEnv) -> bool {
    let env = &test_env.env;
    env.events().all().iter().any(|event| {
        Symbol::from_val(env, &event.1.get(0).unwrap()) == Symbol::new(env, "charged")
    })
}

/// Return `true` if the env's event list contains a failure topic
/// (`"charge_failed"` or `"charge_failed_v2"`).
fn has_failure_event(test_env: &TestEnv) -> bool {
    let env = &test_env.env;
    env.events().all().iter().any(|event| {
        let sym = Symbol::from_val(env, &event.1.get(0).unwrap());
        sym == Symbol::new(env, "charge_failed") || sym == Symbol::new(env, "charge_failed_v2")
    })
}

/// Create a fully-funded active subscription ready to charge.
fn setup_funded_active(test_env: &TestEnv) -> u32 {
    let (id, _, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    fixtures::seed_balance(&test_env.env, &test_env.client, id, AMOUNT * 100);
    id
}

/// Create an active subscription with a custom prepaid balance.
fn setup_funded_active_with_balance(test_env: &TestEnv, balance: i128) -> u32 {
    let (id, _, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    fixtures::seed_balance(&test_env.env, &test_env.client, id, balance);
    id
}

/// Invoke `charge_subscription` (ignoring the Result, only caring about events).
fn charge(test_env: &TestEnv, id: u32) {
    let _ = test_env
        .client
        .try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
}

// ===========================================================================
// Property tests
// ===========================================================================

proptest! {
    /// PROPERTY: A successful interval charge must not emit any failure event.
    #[test]
    fn prop_success_charge_no_failure_event(
        prepaid_extra in 0i128..=500_000_000i128,
    ) {
        let test_env = TestEnv::default();
        test_env.env.ledger().set_timestamp(T0);
        let id = setup_funded_active_with_balance(&test_env, AMOUNT + prepaid_extra);

        test_env.env.ledger().set_timestamp(T0 + INTERVAL + 1);
        charge(&test_env, id);

        let charged = has_charged_event(&test_env);
        let failed = has_failure_event(&test_env);

        prop_assert!(
            !failed || !charged,
            "success charge emitted both charged and failure events: charged={} failed={}",
            charged,
            failed,
        );
        prop_assert!(charged, "expected charged event on success path");
    }

    /// PROPERTY: An insufficient-balance charge must not emit a success event.
    #[test]
    fn prop_insufficient_balance_no_success_event(
        balance in 0i128..(AMOUNT - 1),
    ) {
        let test_env = TestEnv::default();
        test_env.env.ledger().set_timestamp(T0);
        let id = setup_funded_active_with_balance(&test_env, balance);

        test_env.env.ledger().set_timestamp(T0 + INTERVAL + 1);
        charge(&test_env, id);

        let charged = has_charged_event(&test_env);
        let failed = has_failure_event(&test_env);

        prop_assert!(
            !charged,
            "insufficient-balance path emitted a success event despite balance {} < {}",
            balance,
            AMOUNT,
        );
        prop_assert!(failed, "insufficient-balance path should emit a failure event");
    }

    /// PROPERTY: An expired subscription charge must not emit a success event.
    #[test]
    fn prop_expired_no_success_event(
        extra_intervals in 1u64..=50u64,
    ) {
        let test_env = TestEnv::default();
        test_env.env.ledger().set_timestamp(T0);
        let id = setup_funded_active(&test_env);

        let far_future = T0 + (366 * 24 * 60 * 60) + (extra_intervals * INTERVAL);
        test_env.env.ledger().set_timestamp(far_future);
        charge(&test_env, id);

        let charged = has_charged_event(&test_env);
        let failed = has_failure_event(&test_env);

        prop_assert!(!charged, "expired subscription emitted a success event");
        prop_assert!(failed, "expired subscription should emit a failure event");
    }

    /// PROPERTY: A paused subscription charge must not emit a success event.
    #[test]
    fn prop_paused_no_success_event(
        delay in 0u64..=1_000_000u64,
    ) {
        let test_env = TestEnv::default();
        test_env.env.ledger().set_timestamp(T0);
        let (id, subscriber, _) = fixtures::create_subscription_detailed(
            &test_env.env,
            &test_env.client,
            SubscriptionStatus::Active,
            AMOUNT,
            INTERVAL,
        );
        fixtures::seed_balance(&test_env.env, &test_env.client, id, AMOUNT * 100);
        test_env.client.pause_subscription(&id, &subscriber);

        test_env.env.ledger().set_timestamp(T0 + INTERVAL + 1 + delay);
        charge(&test_env, id);

        let charged = has_charged_event(&test_env);
        let failed = has_failure_event(&test_env);

        prop_assert!(!charged, "paused subscription emitted a success event");
        prop_assert!(failed, "paused subscription should emit a failure event");
    }
}

// ===========================================================================
// Targeted edge-case tests (non-proptest, explicit scenarios)
// ===========================================================================

/// Fresh subscription: first charge succeeds with no failure events.
#[test]
fn test_fresh_subscription_first_charge_exclusivity() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let id = setup_funded_active(&test_env);

    test_env.env.ledger().set_timestamp(T0 + INTERVAL + 1);
    let _ = charge(&test_env, id);

    assert!(has_charged_event(&test_env), "first charge must emit charged event");
    assert!(!has_failure_event(&test_env), "first charge must not emit failure event");
}

/// Insufficient balance with zero balance: failure event only.
#[test]
fn test_zero_balance_failure_exclusivity() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let id = setup_funded_active_with_balance(&test_env, 0);

    test_env.env.ledger().set_timestamp(T0 + INTERVAL + 1);
    let _ = charge(&test_env, id);

    assert!(!has_charged_event(&test_env), "zero balance must not emit charged event");
    assert!(has_failure_event(&test_env), "zero balance must emit failure event");
}

/// Insufficient balance triggering grace period: failure event only, no success.
#[test]
fn test_grace_period_transition_exclusivity() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let id = setup_funded_active_with_balance(&test_env, AMOUNT - 1);

    test_env.env.ledger().set_timestamp(T0 + INTERVAL + 1);
    let _ = charge(&test_env, id);

    assert!(!has_charged_event(&test_env), "grace period must not emit charged event");
    assert!(has_failure_event(&test_env), "grace period must emit failure event");
}

/// Subscription with exactly the charge amount: success, no failure.
#[test]
fn test_exact_balance_success_exclusivity() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let id = setup_funded_active_with_balance(&test_env, AMOUNT);

    test_env.env.ledger().set_timestamp(T0 + INTERVAL + 1);
    let _ = charge(&test_env, id);

    assert!(has_charged_event(&test_env), "exact balance must emit charged event");
    assert!(!has_failure_event(&test_env), "exact balance must not emit failure event");
}

/// Subscription with AMOUNT - 1 balance: failure, no success.
#[test]
fn test_one_unit_short_failure_exclusivity() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let id = setup_funded_active_with_balance(&test_env, AMOUNT - 1);

    test_env.env.ledger().set_timestamp(T0 + INTERVAL + 1);
    let _ = charge(&test_env, id);

    assert!(!has_charged_event(&test_env), "one unit short must not emit charged event");
    assert!(has_failure_event(&test_env), "one unit short must emit failure event");
}

/// Expired subscription: failure event only, no success.
#[test]
fn test_expired_subscription_exclusivity() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let id = setup_funded_active(&test_env);

    let far_future = T0 + (366 * 24 * 60 * 60);
    test_env.env.ledger().set_timestamp(far_future);
    let _ = charge(&test_env, id);

    assert!(!has_charged_event(&test_env), "expired subscription must not emit charged event");
    assert!(has_failure_event(&test_env), "expired subscription must emit failure event");
}

/// Consecutive successful charges: each invocation individually has exclusive events.
#[test]
fn test_consecutive_charges_exclusivity() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let id = setup_funded_active_with_balance(&test_env, AMOUNT * 100);

    for i in 1..=5u64 {
        test_env.env.ledger().set_timestamp(T0 + (i * INTERVAL) + 1);
        charge(&test_env, id);

        assert!(has_charged_event(&test_env), "charge {} must emit charged", i);
        assert!(!has_failure_event(&test_env), "charge {} must not emit failure", i);
    }
}

/// Balance drains from sufficient to insufficient: events remain exclusive at each step.
#[test]
fn test_balance_drain_exclusivity() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let id = setup_funded_active_with_balance(&test_env, AMOUNT * 3 - 1);

    for i in 1..=2u64 {
        test_env.env.ledger().set_timestamp(T0 + (i * INTERVAL) + 1);
        charge(&test_env, id);
        assert!(has_charged_event(&test_env), "charge {} must succeed", i);
        assert!(!has_failure_event(&test_env), "charge {} must not fail", i);
    }

    test_env.env.ledger().set_timestamp(T0 + (3 * INTERVAL) + 1);
    let _ = charge(&test_env, id);
    assert!(!has_charged_event(&test_env), "final charge must not succeed");
    assert!(has_failure_event(&test_env), "final charge must emit failure");
}

/// Large balance: success event, no failure.
#[test]
fn test_large_balance_exclusivity() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let id = setup_funded_active_with_balance(&test_env, i128::MAX / 2);

    test_env.env.ledger().set_timestamp(T0 + INTERVAL + 1);
    let _ = charge(&test_env, id);

    assert!(has_charged_event(&test_env), "large balance must emit charged");
    assert!(!has_failure_event(&test_env), "large balance must not emit failure");
}

/// Minimal balance (1 unit): failure, no success.
#[test]
fn test_minimal_balance_exclusivity() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let id = setup_funded_active_with_balance(&test_env, 1);

    test_env.env.ledger().set_timestamp(T0 + INTERVAL + 1);
    let _ = charge(&test_env, id);

    assert!(!has_charged_event(&test_env), "minimal balance must not emit charged");
    assert!(has_failure_event(&test_env), "minimal balance must emit failure");
}

/// Replay charge within same interval: failure event only, no success.
#[test]
fn test_replay_charge_exclusivity() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let id = setup_funded_active(&test_env);

    test_env.env.ledger().set_timestamp(T0 + INTERVAL + 1);
    let _ = charge(&test_env, id);
    assert!(has_charged_event(&test_env), "first charge must succeed");

    let _ = charge(&test_env, id);
    assert!(!has_charged_event(&test_env), "replay must not emit charged");
    assert!(has_failure_event(&test_env), "replay must emit failure");
}

/// Non-existent subscription: failure event only, no success.
#[test]
fn test_nonexistent_subscription_exclusivity() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);

    let nonexistent_id = 999_999u32;
    let _ = charge(&test_env, nonexistent_id);

    assert!(!has_charged_event(&test_env), "nonexistent subscription must not emit charged");
    assert!(has_failure_event(&test_env), "nonexistent subscription must emit failure");
}
