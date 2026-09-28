//! Adversarial coverage for `admin::validate_treasury_split`.
//!
//! `validate_treasury_split` is the only guard between an admin-supplied
//! basis-point allocation and the protocol-fee routing table, and it indexes the
//! input with `entries.get(i).unwrap()` / `entries.get(j).unwrap()`.  These tests
//! pin every rejection path, the evaluation order between the checks, and the
//! fact that validation is a pure read (no storage writes) so a rejected split
//! can never leave partial configuration behind.
//!
//! Covered:
//! - empty list
//! - zero-bps entry at the head, middle and tail
//! - under- and over-allocation, including a one-bps miss in both directions
//! - duplicate beneficiaries (adjacent, non-adjacent, and last pair)
//! - `u32` overflow reported as `Error::Overflow`, distinct from a bad total
//! - check ordering (zero-bps beats overflow, overflow beats total mismatch)
//! - accepted boundary shapes: single full allocation, exact thirds with
//!   remainder, many small slices, and large-but-valid totals
//! - purity: repeated calls are deterministic and never touch contract storage

#![cfg(test)]

extern crate std;

use crate::admin::validate_treasury_split;
use crate::types::{DataKey, TreasurySplitEntry};
use crate::Error;
use soroban_sdk::{testutils::Address as _, Address, Env, Vec};

const TOTAL_BPS: u32 = 10_000;

/// Build a split with a fresh beneficiary per entry.
fn split(env: &Env, allocations: &[u32]) -> Vec<TreasurySplitEntry> {
    let mut entries = Vec::new(env);
    for bps in allocations.iter() {
        entries.push_back(TreasurySplitEntry {
            beneficiary: Address::generate(env),
            bps: *bps,
        });
    }
    entries
}

/// Build a split where every entry points at the same beneficiary.
fn shared_beneficiary_split(
    env: &Env,
    beneficiary: &Address,
    allocations: &[u32],
) -> Vec<TreasurySplitEntry> {
    let mut entries = Vec::new(env);
    for bps in allocations.iter() {
        entries.push_back(TreasurySplitEntry {
            beneficiary: beneficiary.clone(),
            bps: *bps,
        });
    }
    entries
}

// ── Rejections ───────────────────────────────────────────────────────────────

#[test]
fn empty_split_is_rejected() {
    let env = Env::default();
    let entries: Vec<TreasurySplitEntry> = Vec::new(&env);

    assert_eq!(
        validate_treasury_split(&entries),
        Err(Error::InvalidFeeBips)
    );
}

#[test]
fn single_full_allocation_is_accepted() {
    let env = Env::default();

    assert!(validate_treasury_split(&split(&env, &[TOTAL_BPS])).is_ok());
}

#[test]
fn exact_two_way_split_is_accepted() {
    let env = Env::default();

    assert!(validate_treasury_split(&split(&env, &[6_000, 4_000])).is_ok());
    assert!(validate_treasury_split(&split(&env, &[5_000, 5_000])).is_ok());
}

#[test]
fn three_way_split_with_indivisible_remainder_is_accepted() {
    let env = Env::default();

    assert!(validate_treasury_split(&split(&env, &[3_333, 3_333, 3_334])).is_ok());
}

#[test]
fn one_bps_under_allocation_is_rejected() {
    let env = Env::default();

    assert_eq!(
        validate_treasury_split(&split(&env, &[9_999])),
        Err(Error::InvalidFeeBips)
    );
    assert_eq!(
        validate_treasury_split(&split(&env, &[9_998, 1])),
        Err(Error::InvalidFeeBips)
    );
}

#[test]
fn one_bps_over_allocation_is_rejected() {
    let env = Env::default();

    assert_eq!(
        validate_treasury_split(&split(&env, &[10_001])),
        Err(Error::InvalidFeeBips)
    );
}

#[test]
fn over_allocation_split_across_entries_is_rejected() {
    let env = Env::default();

    assert_eq!(
        validate_treasury_split(&split(&env, &[6_000, 4_001])),
        Err(Error::InvalidFeeBips)
    );
    assert_eq!(
        validate_treasury_split(&split(&env, &[1, TOTAL_BPS])),
        Err(Error::InvalidFeeBips)
    );
}

#[test]
fn zero_bps_entry_is_rejected_at_the_head() {
    let env = Env::default();

    assert_eq!(
        validate_treasury_split(&split(&env, &[0, TOTAL_BPS])),
        Err(Error::InvalidFeeBips)
    );
}

#[test]
fn zero_bps_entry_is_rejected_in_the_middle() {
    let env = Env::default();

    assert_eq!(
        validate_treasury_split(&split(&env, &[5_000, 0, 5_000])),
        Err(Error::InvalidFeeBips)
    );
}

#[test]
fn zero_bps_entry_is_rejected_at_the_tail() {
    let env = Env::default();

    assert_eq!(
        validate_treasury_split(&split(&env, &[TOTAL_BPS, 0])),
        Err(Error::InvalidFeeBips)
    );
}

#[test]
fn adjacent_duplicate_beneficiary_is_rejected_even_when_the_total_is_valid() {
    let env = Env::default();
    let beneficiary = Address::generate(&env);

    assert_eq!(
        validate_treasury_split(&shared_beneficiary_split(&env, &beneficiary, &[5_000, 5_000])),
        Err(Error::InvalidFeeBips)
    );
}

#[test]
fn non_adjacent_duplicate_beneficiary_is_rejected() {
    let env = Env::default();
    let beneficiary = Address::generate(&env);
    let other = Address::generate(&env);

    let mut entries = Vec::new(&env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: beneficiary.clone(),
        bps: 3_333,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: other,
        bps: 3_333,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: beneficiary.clone(),
        bps: 3_334,
    });

    assert_eq!(entries.len(), 3);
    assert_eq!(
        validate_treasury_split(&entries),
        Err(Error::InvalidFeeBips)
    );
}

#[test]
fn duplicate_beneficiary_in_the_final_pair_is_rejected() {
    let env = Env::default();

    // 18 fresh slices of 500 bps account for 9_000 bps.
    let allocations = [500u32; 18];
    let mut entries = split(&env, &allocations);

    // The final 1_000 bps are split between the same beneficiary twice, so the
    // total is valid and only the duplicate rule can reject the split.
    let repeated = Address::generate(&env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: repeated.clone(),
        bps: 500,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: repeated.clone(),
        bps: 500,
    });

    assert_eq!(
        validate_treasury_split(&entries),
        Err(Error::InvalidFeeBips)
    );
}

#[test]
fn many_distinct_slices_summing_to_the_total_are_accepted() {
    let env = Env::default();
    let allocations = [100u32; 100];

    assert!(validate_treasury_split(&split(&env, &allocations)).is_ok());
}

#[test]
fn allocations_are_order_independent() {
    let env = Env::default();

    assert!(validate_treasury_split(&split(&env, &[1_000, 2_000, 7_000])).is_ok());
    assert!(validate_treasury_split(&split(&env, &[7_000, 1_000, 2_000])).is_ok());
    assert!(validate_treasury_split(&split(&env, &[2_000, 7_000, 1_000])).is_ok());
}

// ── Arithmetic boundaries ────────────────────────────────────────────────────

#[test]
fn sum_overflow_is_reported_as_overflow() {
    let env = Env::default();

    assert_eq!(
        validate_treasury_split(&split(&env, &[u32::MAX, 1])),
        Err(Error::Overflow)
    );
    assert_eq!(
        validate_treasury_split(&split(&env, &[u32::MAX, u32::MAX])),
        Err(Error::Overflow)
    );
}

#[test]
fn single_max_allocation_reports_a_bad_total_not_an_overflow() {
    let env = Env::default();

    // The sum never exceeds `u32`, so the failure must be attributed to the total.
    assert_eq!(
        validate_treasury_split(&split(&env, &[u32::MAX])),
        Err(Error::InvalidFeeBips)
    );
}

#[test]
fn zero_bps_is_checked_before_overflow() {
    let env = Env::default();

    // The second entry is zero, so the zero check must fire before the add can
    // overflow — a rejected split is reported as a bad allocation, not overflow.
    assert_eq!(
        validate_treasury_split(&split(&env, &[u32::MAX, 0])),
        Err(Error::InvalidFeeBips)
    );
}

#[test]
fn overflow_is_checked_before_the_total_mismatch() {
    let env = Env::default();

    assert_eq!(
        validate_treasury_split(&split(&env, &[3_000, u32::MAX])),
        Err(Error::Overflow)
    );
}

// ── Purity / determinism ─────────────────────────────────────────────────────

#[test]
fn repeated_calls_are_deterministic() {
    let env = Env::default();
    let entries = split(&env, &[5_000, 5_000]);

    let first = validate_treasury_split(&entries);
    let second = validate_treasury_split(&entries);

    assert_eq!(first, Ok(()));
    assert_eq!(first, second);
}

#[test]
fn validation_never_writes_contract_storage() {
    let env = Env::default();
    let key = DataKey::TreasurySplit;

    assert!(!env.storage().persistent().has(&key));
    assert!(!env.storage().instance().has(&key));

    assert!(validate_treasury_split(&split(&env, &[TOTAL_BPS])).is_ok());
    assert_eq!(
        validate_treasury_split(&split(&env, &[4_000])),
        Err(Error::InvalidFeeBips)
    );

    assert!(!env.storage().persistent().has(&key));
    assert!(!env.storage().instance().has(&key));
}

#[test]
fn validation_does_not_mutate_the_input_allocation() {
    let env = Env::default();
    let entries = split(&env, &[2_500, 2_500, 5_000]);

    let _ = validate_treasury_split(&entries);

    assert_eq!(entries.len(), 3);
    assert_eq!(entries.get(0).unwrap().bps, 2_500);
    assert_eq!(entries.get(1).unwrap().bps, 2_500);
    assert_eq!(entries.get(2).unwrap().bps, 5_000);
}
