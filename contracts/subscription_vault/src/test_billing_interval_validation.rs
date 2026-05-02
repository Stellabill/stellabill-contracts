#![cfg(test)]

//! Tests for billing interval validation and canonical time-math.
//!
//! Covers:
//! - `validate_interval` via subscription and plan-template creation
//! - `next_charge_time` semantics: exact boundary, just-before, well-past
//! - Consistency between the charge path and `get_next_charge_info`
//! - Max-interval subscription lifecycle
//! - Overflow-safe query path

use crate::test_utils::setup::TestEnv;
use crate::types::Error;
use soroban_sdk::{testutils::Address as _, Address};

/// Minimum allowed billing interval (seconds).
const MIN_INTERVAL: u64 = 60;
/// Maximum allowed billing interval (seconds) — 365 days.
const MAX_INTERVAL: u64 = 31_536_000;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn funded_subscription(test_env: &TestEnv, interval: u64) -> u32 {
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let sub_id = test_env.client.create_subscription(
        &subscriber, &merchant, &100_000_000, &interval, &false, &None, &None::<u64>,
    );
    test_env.stellar_token_client().mint(&subscriber, &1_000_000_000i128);
    test_env.client.deposit_funds(&sub_id, &subscriber, &1_000_000_000i128);
    sub_id
}

// ---------------------------------------------------------------------------
// 1. Interval validation at subscription creation
// ---------------------------------------------------------------------------

#[test]
fn test_interval_zero_rejected() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let res = test_env.client.try_create_subscription(
        &subscriber, &merchant, &100_000_000, &0, &false, &None, &None::<u64>,
    );
    assert!(matches!(res, Err(Ok(Error::InvalidInput))));
}

#[test]
fn test_interval_below_min_rejected() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let res = test_env.client.try_create_subscription(
        &subscriber, &merchant, &100_000_000, &(MIN_INTERVAL - 1), &false, &None, &None::<u64>,
    );
    assert!(matches!(res, Err(Ok(Error::InvalidInput))));
}

#[test]
fn test_interval_at_min_accepted() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    test_env.client.create_subscription(
        &subscriber, &merchant, &100_000_000, &MIN_INTERVAL, &false, &None, &None::<u64>,
    );
}

#[test]
fn test_interval_above_max_rejected() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let res = test_env.client.try_create_subscription(
        &subscriber, &merchant, &100_000_000, &(MAX_INTERVAL + 1), &false, &None, &None::<u64>,
    );
    assert!(matches!(res, Err(Ok(Error::InvalidInput))));
}

#[test]
fn test_interval_at_max_accepted() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    test_env.client.create_subscription(
        &subscriber, &merchant, &100_000_000, &MAX_INTERVAL, &false, &None, &None::<u64>,
    );
}

// ---------------------------------------------------------------------------
// 2. Interval validation at plan-template creation
// ---------------------------------------------------------------------------

#[test]
fn test_plan_template_interval_below_min_rejected() {
    let test_env = TestEnv::default();
    let merchant = Address::generate(&test_env.env);
    let res = test_env.client.try_create_plan_template(
        &merchant, &100_000_000, &(MIN_INTERVAL - 1), &false, &None,
    );
    assert!(matches!(res, Err(Ok(Error::InvalidInput))));
}

#[test]
fn test_plan_template_interval_at_min_accepted() {
    let test_env = TestEnv::default();
    let merchant = Address::generate(&test_env.env);
    test_env
        .client
        .create_plan_template(&merchant, &100_000_000, &MIN_INTERVAL, &false, &None);
}

#[test]
fn test_plan_template_interval_above_max_rejected() {
    let test_env = TestEnv::default();
    let merchant = Address::generate(&test_env.env);
    let res = test_env.client.try_create_plan_template(
        &merchant, &100_000_000, &(MAX_INTERVAL + 1), &false, &None,
    );
    assert!(matches!(res, Err(Ok(Error::InvalidInput))));
}

#[test]
fn test_plan_template_interval_at_max_accepted() {
    let test_env = TestEnv::default();
    let merchant = Address::generate(&test_env.env);
    test_env
        .client
        .create_plan_template(&merchant, &100_000_000, &MAX_INTERVAL, &false, &None);
}

// ---------------------------------------------------------------------------
// 3. Charge-path boundary semantics (canonical next_charge_time)
// ---------------------------------------------------------------------------

/// Charge at exactly `last_payment + interval` must succeed.
#[test]
fn test_charge_at_exact_boundary_succeeds() {
    let test_env = TestEnv::default();
    let interval = MIN_INTERVAL;
    // Subscription created at T=0; last_payment_timestamp = 0.
    let sub_id = funded_subscription(&test_env, interval);

    // Verify get_next_charge_info reports the correct next boundary.
    let info = test_env.client.get_next_charge_info(&sub_id);
    assert_eq!(info.next_charge_timestamp, interval);

    test_env.set_timestamp(interval); // now == 0 + interval  ✓
    test_env.client.charge_subscription(&sub_id); // must not panic
}

/// Charge one second before the boundary must be rejected.
#[test]
fn test_charge_one_second_before_boundary_rejected() {
    let test_env = TestEnv::default();
    let interval = MIN_INTERVAL;
    let sub_id = funded_subscription(&test_env, interval);

    test_env.set_timestamp(interval - 1); // now < 0 + interval  ✗
    let res = test_env.client.try_charge_subscription(&sub_id);
    assert!(matches!(res, Err(Ok(Error::IntervalNotElapsed))));
}

/// Charge well past the boundary must succeed.
#[test]
fn test_charge_past_boundary_succeeds() {
    let test_env = TestEnv::default();
    let interval = MIN_INTERVAL;
    let sub_id = funded_subscription(&test_env, interval);

    test_env.set_timestamp(interval * 5);
    test_env.client.charge_subscription(&sub_id);
}

/// After a successful charge at T=N, the next allowed time is T=N+interval.
/// A charge at T=N+interval-1 must be rejected; T=N+interval must succeed.
#[test]
fn test_window_resets_to_now_after_charge() {
    let test_env = TestEnv::default();
    let interval = MIN_INTERVAL;
    let sub_id = funded_subscription(&test_env, interval);

    // First charge at T=interval; last_payment_timestamp resets to `interval`.
    test_env.set_timestamp(interval);
    test_env.client.charge_subscription(&sub_id);

    // next allowed = 2*interval.  At 2*interval-1 the period index (floor(T/interval))
    // is still 1 — same as the just-completed charge — so the replay guard fires
    // before the interval check.  Either Replay or IntervalNotElapsed is a correct
    // "too early" rejection from the contract's perspective.
    test_env.set_timestamp(2 * interval - 1);
    let too_early = test_env.client.try_charge_subscription(&sub_id);
    assert!(
        matches!(too_early, Err(Ok(Error::IntervalNotElapsed)) | Err(Ok(Error::Replay))),
        "charge before next window should be rejected with IntervalNotElapsed or Replay"
    );

    test_env.set_timestamp(2 * interval);
    test_env.client.charge_subscription(&sub_id);
}

// ---------------------------------------------------------------------------
// 4. Max-interval boundary semantics
// ---------------------------------------------------------------------------

/// A subscription with interval = MAX_INTERVAL must be chargeable exactly
/// at `last_payment + MAX_INTERVAL` and rejected one second before.
#[test]
fn test_max_interval_boundary() {
    let test_env = TestEnv::default();
    // Created at T=0; last_payment_timestamp = 0.
    let sub_id = funded_subscription(&test_env, MAX_INTERVAL);

    // Just before boundary.
    test_env.set_timestamp(MAX_INTERVAL - 1);
    let too_early = test_env.client.try_charge_subscription(&sub_id);
    assert!(matches!(too_early, Err(Ok(Error::IntervalNotElapsed))));

    // Exact boundary.
    test_env.set_timestamp(MAX_INTERVAL);
    test_env.client.charge_subscription(&sub_id);
}

/// get_next_charge_info for MAX_INTERVAL must return MAX_INTERVAL
/// (no overflow, no clamping for realistic timestamps).
#[test]
fn test_get_next_charge_info_max_interval_no_overflow() {
    let test_env = TestEnv::default();
    let sub_id = funded_subscription(&test_env, MAX_INTERVAL);

    // last_payment_timestamp is 0 (created at T=0), interval = MAX_INTERVAL.
    // next_charge_time = 0 + MAX_INTERVAL = MAX_INTERVAL — well within u64.
    let info = test_env.client.get_next_charge_info(&sub_id);
    assert_eq!(info.next_charge_timestamp, MAX_INTERVAL);
    assert!(info.is_charge_expected);
}

// ---------------------------------------------------------------------------
// 5. Consistency: get_next_charge_info agrees with charge enforcement
// ---------------------------------------------------------------------------

/// The timestamp returned by get_next_charge_info is the exact threshold
/// used by charge_subscription: charging at that timestamp succeeds, and
/// charging one second before fails.
#[test]
fn test_next_charge_info_matches_charge_enforcement() {
    let test_env = TestEnv::default();
    let interval = 3_600u64; // 1 hour
    let sub_id = funded_subscription(&test_env, interval);

    let info = test_env.client.get_next_charge_info(&sub_id);
    let next_ts = info.next_charge_timestamp;

    // One second before the reported timestamp: must be rejected.
    test_env.set_timestamp(next_ts - 1);
    let early = test_env.client.try_charge_subscription(&sub_id);
    assert!(
        matches!(early, Err(Ok(Error::IntervalNotElapsed))),
        "charge should be rejected one second before next_charge_timestamp"
    );

    // At the exact reported timestamp: must succeed.
    test_env.set_timestamp(next_ts);
    test_env.client.charge_subscription(&sub_id);

    // After the successful charge, get_next_charge_info reflects
    // last_payment (= next_ts) + interval.
    let info_after = test_env.client.get_next_charge_info(&sub_id);
    assert_eq!(info_after.next_charge_timestamp, next_ts + interval);
}

// ---------------------------------------------------------------------------
// 6. Multiple consecutive interval charges (regression: no drift)
// ---------------------------------------------------------------------------

/// Six consecutive charges at exact boundaries must all succeed; a charge
/// one second early after the sixth must be rejected.
#[test]
fn test_consecutive_interval_charges_no_drift() {
    let test_env = TestEnv::default();
    let interval = MIN_INTERVAL;
    let sub_id = funded_subscription(&test_env, interval);

    for i in 1u64..=6 {
        test_env.set_timestamp(i * interval);
        test_env.client.charge_subscription(&sub_id);
    }

    // One second before the 7th boundary.  The period index at T=7*interval-1
    // equals 6 (same as the 6th charge), so the replay guard fires before the
    // interval check — Replay is the expected rejection here.
    test_env.set_timestamp(7 * interval - 1);
    let too_early = test_env.client.try_charge_subscription(&sub_id);
    assert!(
        matches!(too_early, Err(Ok(Error::IntervalNotElapsed)) | Err(Ok(Error::Replay))),
        "charge one second before 7th boundary should be rejected"
    );
}
