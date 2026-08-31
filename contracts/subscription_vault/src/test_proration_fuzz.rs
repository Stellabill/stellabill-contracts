//! Property-based fuzz tests and edge-case unit tests for prorated first-charge calculation.
//!
//! Verifies that `calculate_prorated_first_charge(amount, interval, remaining_seconds)`:
//! - Always produces a result in `[0, amount]` (bounds invariant).
//! - Is monotonic with respect to `remaining_seconds` (monotonicity invariant).
//! - Handles extreme edge cases (e.g. interval=1, amount=i128::MAX/2, remaining_seconds > interval)
//!   without overflow or underflow.
//! - Runs at least 10,000 fuzz cases via `proptest`.

use crate::charge_core::calculate_prorated_first_charge;
use crate::types::Error;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10_000))]

    /// Fuzzes random (amount, interval, remaining_seconds) triples to prove bounds
    /// and monotonicity invariants hold under all inputs.
    #[test]
    fn fuzz_prorated_first_charge_triple(
        amount in 0..=i128::MAX,
        interval in 1..=u64::MAX,
        rem1 in 0..=u64::MAX,
        rem2 in 0..=u64::MAX,
    ) {
        let (rem_low, rem_high) = if rem1 <= rem2 { (rem1, rem2) } else { (rem2, rem1) };

        let res_low = calculate_prorated_first_charge(amount, interval, rem_low)
            .expect("valid amount and non-zero interval must not error");
        let res_high = calculate_prorated_first_charge(amount, interval, rem_high)
            .expect("valid amount and non-zero interval must not error");

        // 1. Bounds Invariant: 0 <= prorated_charge <= amount
        prop_assert!(res_low >= 0, "prorated charge must be non-negative");
        prop_assert!(res_low <= amount, "prorated charge must not exceed total amount");
        prop_assert!(res_high >= 0, "prorated charge must be non-negative");
        prop_assert!(res_high <= amount, "prorated charge must not exceed total amount");

        // 2. Monotonicity Invariant: rem_low <= rem_high => charge(rem_low) <= charge(rem_high)
        prop_assert!(
            res_low <= res_high,
            "prorated charge must be monotonic with respect to remaining_seconds"
        );

        // 3. Exact boundary assertions
        if rem_low == 0 {
            prop_assert_eq!(res_low, 0, "zero remaining seconds must yield 0 charge");
        }
        if rem_high >= interval {
            prop_assert_eq!(res_high, amount, "remaining_seconds >= interval must yield full amount");
        }
    }
}

// ---------------------------------------------------------------------------
// Explicit Edge-Case Unit Tests
// ---------------------------------------------------------------------------

#[test]
fn test_edge_case_interval_one_amount_half_max_remaining_greater_than_interval() {
    let amount = i128::MAX / 2;
    let interval = 1u64;
    let remaining_seconds = 10u64; // remaining_seconds > interval

    let result = calculate_prorated_first_charge(amount, interval, remaining_seconds);
    assert_eq!(result, Ok(amount));
}

#[test]
fn test_edge_case_zero_interval_returns_invalid_input() {
    let result = calculate_prorated_first_charge(100, 0, 10);
    assert_eq!(result, Err(Error::InvalidInput));
}

#[test]
fn test_edge_case_negative_amount_returns_invalid_amount() {
    let result = calculate_prorated_first_charge(-1, 30, 10);
    assert_eq!(result, Err(Error::InvalidAmount));
}

#[test]
fn test_edge_case_zero_remaining_seconds() {
    let result = calculate_prorated_first_charge(1_000_000, 30, 0);
    assert_eq!(result, Ok(0));
}

#[test]
fn test_edge_case_remaining_seconds_equals_interval() {
    let amount = 500_000i128;
    let interval = 86_400u64;
    let result = calculate_prorated_first_charge(amount, interval, interval);
    assert_eq!(result, Ok(amount));
}

#[test]
fn test_edge_case_i128_max_values() {
    let amount = i128::MAX;
    let interval = u64::MAX;
    let remaining_seconds = u64::MAX - 1;

    let result = calculate_prorated_first_charge(amount, interval, remaining_seconds);
    assert!(result.is_ok());
    let prorated = result.unwrap();
    assert!(prorated >= 0);
    assert!(prorated <= amount);
}
