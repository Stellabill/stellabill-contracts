//! Adversarial coverage for `admin::clear_treasury_split` (issue #1013).
//!
//! The issue cites `admin.rs:764` / `admin.rs:774` (`entries.get(i).unwrap()`,
//! `entries.get(j).unwrap()` inside `validate_treasury_split`) as evidence that
//! the treasury-split surface needs adversarial coverage. Those indexed reads
//! are only reachable through `set_treasury_split`, while the paired
//! `clear_treasury_split` is the mutation that removes the configuration, so
//! this file covers both ends of the lifecycle:
//!
//! * a configured split is removed from client view and storage;
//! * a non-admin caller (stranger *or* the configured operator) is rejected
//!   with `Error::Forbidden`, and the split survives untouched;
//! * clearing with nothing configured succeeds without creating state;
//! * the `TreasurySplit` config cooldown is enforced on clear, and a successful
//!   clear consumes it (so an immediate re-set is rejected);
//! * clearing leaves the single-treasury / fee configuration intact;
//! * clearing on an uninitialised contract reports `Error::NotInitialized`;
//! * the whole-list validation boundaries around the cited `unwrap()` calls:
//!   long distinct lists, non-adjacent duplicates, a zero-bps entry with an
//!   otherwise valid total, and a bps sum that overflows `u32`.

#![cfg(test)]

use crate::admin::CONFIG_COOLDOWN_SECS;
use crate::test_utils::setup::TestEnv;
use crate::types::{DataKey, Error, TreasurySplitConfig, TreasurySplitEntry};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    Address, Env, Vec,
};

/// Build a `TreasurySplitEntry` vector from `(beneficiary, bps)` pairs.
fn entries_of(env: &Env, pairs: &[(Address, u32)]) -> Vec<TreasurySplitEntry> {
    let mut entries = Vec::new(env);
    for (beneficiary, bps) in pairs.iter() {
        entries.push_back(TreasurySplitEntry {
            beneficiary: beneficiary.clone(),
            bps: *bps,
        });
    }
    entries
}

/// Single-beneficiary split holding 100% of the fee.
fn full_split(env: &Env, beneficiary: &Address) -> Vec<TreasurySplitEntry> {
    entries_of(env, &[(beneficiary.clone(), 10_000)])
}

/// Read `DataKey::TreasurySplit` straight from contract storage (either tier).
fn stored_split(env: &Env, contract: &Address) -> Option<TreasurySplitConfig> {
    env.as_contract(contract, || {
        crate::admin::read_config::<TreasurySplitConfig>(env, &DataKey::TreasurySplit)
    })
}

/// Read an `Address`-valued config key straight from contract storage.
fn stored_address(env: &Env, contract: &Address, key: &DataKey) -> Option<Address> {
    env.as_contract(contract, || crate::admin::read_config::<Address>(env, key))
}

// ── Happy path ──────────────────────────────────────────────────────────────

#[test]
fn clear_removes_a_configured_split_everywhere() {
    let te = TestEnv::default();
    let b1 = Address::generate(&te.env);
    let entries = full_split(&te.env, &b1);
    te.client.set_treasury_split(&te.admin, &entries);
    assert_eq!(
        te.client.get_treasury_split(),
        Some(TreasurySplitConfig {
            entries: entries.clone()
        })
    );
    assert!(stored_split(&te.env, &te.client.address).is_some());

    te.client.clear_treasury_split(&te.admin);

    assert_eq!(te.client.get_treasury_split(), None);
    assert_eq!(stored_split(&te.env, &te.client.address), None);
}

// ── Unauthorized callers ────────────────────────────────────────────────────

#[test]
fn clear_by_stranger_is_rejected_and_the_split_survives() {
    let te = TestEnv::default();
    let b1 = Address::generate(&te.env);
    te.client.set_treasury_split(&te.admin, &full_split(&te.env, &b1));
    let before = te.client.get_treasury_split();
    let events_before = te.env.events().all().len();

    let stranger = Address::generate(&te.env);
    assert_eq!(
        te.client.try_clear_treasury_split(&stranger),
        Err(Ok(Error::Forbidden))
    );

    assert_eq!(te.client.get_treasury_split(), before);
    assert_eq!(te.env.events().all().len(), events_before);
}

#[test]
fn clear_by_the_configured_operator_is_rejected_and_the_split_survives() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);

    let b1 = Address::generate(&te.env);
    te.client.set_treasury_split(&te.admin, &full_split(&te.env, &b1));
    let before = te.client.get_treasury_split();

    // The operator role does not extend to treasury-split administration; only
    // the stored admin may clear it.
    assert_eq!(
        te.client.try_clear_treasury_split(&operator),
        Err(Ok(Error::Forbidden))
    );
    assert_eq!(te.client.get_treasury_split(), before);
}

#[test]
fn clear_on_an_uninitialised_contract_reports_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let admin = Address::generate(&env);

    let res =
        env.as_contract(&contract_id, || crate::admin::clear_treasury_split(&env, admin.clone()));
    assert_eq!(res, Err(Error::NotInitialized));
    // Nothing was created by the rejected call.
    assert_eq!(stored_split(&env, &contract_id), None);
}

// ── Boundary: nothing configured ────────────────────────────────────────────

#[test]
fn clear_with_no_configured_split_succeeds_and_leaves_state_none() {
    let te = TestEnv::default();
    assert_eq!(te.client.get_treasury_split(), None);
    assert_eq!(stored_split(&te.env, &te.client.address), None);

    te.client.clear_treasury_split(&te.admin);

    assert_eq!(te.client.get_treasury_split(), None);
    assert_eq!(stored_split(&te.env, &te.client.address), None);
}

// ── Boundary: config cooldown ──────────────────────────────────────────────

#[test]
fn clear_within_the_config_cooldown_is_rejected_and_preserves_the_split() {
    let te = TestEnv::default();
    te.env.ledger().with_mut(|l| l.timestamp = 10_000);
    let b1 = Address::generate(&te.env);
    te.client.set_treasury_split(&te.admin, &full_split(&te.env, &b1));
    let before = te.client.get_treasury_split();

    te.env
        .ledger()
        .with_mut(|l| l.timestamp = 10_000 + CONFIG_COOLDOWN_SECS - 1);
    assert_eq!(
        te.client.try_clear_treasury_split(&te.admin),
        Err(Ok(Error::CooldownActive))
    );
    assert_eq!(te.client.get_treasury_split(), before);
}

#[test]
fn clear_after_the_config_cooldown_elapses_succeeds() {
    let te = TestEnv::default();
    te.env.ledger().with_mut(|l| l.timestamp = 10_000);
    let b1 = Address::generate(&te.env);
    te.client.set_treasury_split(&te.admin, &full_split(&te.env, &b1));

    te.env
        .ledger()
        .with_mut(|l| l.timestamp = 10_000 + CONFIG_COOLDOWN_SECS);
    te.client.clear_treasury_split(&te.admin);

    assert_eq!(te.client.get_treasury_split(), None);
}

#[test]
fn clear_consumes_the_cooldown_so_an_immediate_reset_is_rejected() {
    let te = TestEnv::default();
    te.env.ledger().with_mut(|l| l.timestamp = 10_000);
    let b1 = Address::generate(&te.env);
    te.client.set_treasury_split(&te.admin, &full_split(&te.env, &b1));

    te.env
        .ledger()
        .with_mut(|l| l.timestamp = 10_000 + CONFIG_COOLDOWN_SECS);
    te.client.clear_treasury_split(&te.admin);
    assert_eq!(te.client.get_treasury_split(), None);

    // The clear armed the `TreasurySplit` cooldown, so reconfiguring straight
    // away is refused and the split stays cleared.
    let b2 = Address::generate(&te.env);
    assert_eq!(
        te.client
            .try_set_treasury_split(&te.admin, &full_split(&te.env, &b2)),
        Err(Ok(Error::CooldownActive))
    );
    assert_eq!(te.client.get_treasury_split(), None);

    // Once the cooldown elapses the split can be re-established.
    te.env.ledger().with_mut(|l| l.timestamp += CONFIG_COOLDOWN_SECS);
    te.client
        .set_treasury_split(&te.admin, &full_split(&te.env, &b2));
    let restored = te.client.get_treasury_split().expect("split restored");
    assert_eq!(restored.entries.len(), 1);
    assert_eq!(restored.entries.get(0).unwrap().beneficiary, b2);
}

// ── Clear must not disturb the rest of the fee configuration ───────────────

#[test]
fn clear_leaves_protocol_fee_and_single_treasury_untouched() {
    let te = TestEnv::default();
    let treasury = Address::generate(&te.env);
    te.client.set_protocol_fee(&te.admin, &treasury, &333);

    let b1 = Address::generate(&te.env);
    te.client.set_treasury_split(&te.admin, &full_split(&te.env, &b1));
    te.client.clear_treasury_split(&te.admin);

    assert_eq!(te.client.get_treasury_split(), None);
    assert_eq!(te.client.get_protocol_fee_bps(), 333);
    assert_eq!(
        stored_address(&te.env, &te.client.address, &DataKey::Treasury),
        Some(treasury)
    );
}

// ── Validation boundaries reachable through `set_treasury_split` ────────────
//
// These exercise the `entries.get(i)` / `entries.get(j)` indexed reads cited in
// the issue evidence (admin.rs:764 / admin.rs:774).

#[test]
fn set_treasury_split_accepts_a_long_list_of_distinct_beneficiaries_then_clears() {
    let te = TestEnv::default();
    // 100 distinct beneficiaries x 100 bps == exactly 10_000 bps. This drives
    // the full O(n^2) duplicate scan over `entries.get(i)`/`entries.get(j)`.
    let mut entries: Vec<TreasurySplitEntry> = Vec::new(&te.env);
    for _ in 0..100 {
        entries.push_back(TreasurySplitEntry {
            beneficiary: Address::generate(&te.env),
            bps: 100,
        });
    }

    te.client.set_treasury_split(&te.admin, &entries);
    assert_eq!(te.client.get_treasury_split().unwrap().entries.len(), 100);

    te.client.clear_treasury_split(&te.admin);
    assert_eq!(te.client.get_treasury_split(), None);
}

#[test]
fn set_treasury_split_rejects_a_non_adjacent_duplicate_in_a_long_list() {
    let te = TestEnv::default();
    let duplicate = Address::generate(&te.env);
    let mut entries: Vec<TreasurySplitEntry> = Vec::new(&te.env);
    for i in 0..10u32 {
        let beneficiary = if i == 0 || i == 9 {
            duplicate.clone()
        } else {
            Address::generate(&te.env)
        };
        entries.push_back(TreasurySplitEntry {
            beneficiary,
            bps: 1_000,
        });
    }

    assert_eq!(
        te.client.try_set_treasury_split(&te.admin, &entries),
        Err(Ok(Error::InvalidFeeBips))
    );
    assert_eq!(te.client.get_treasury_split(), None);
}

#[test]
fn set_treasury_split_rejects_zero_bps_even_when_the_total_would_be_valid() {
    let te = TestEnv::default();
    let zero = Address::generate(&te.env);
    let rest = Address::generate(&te.env);
    let entries = entries_of(&te.env, &[(zero, 0), (rest, 10_000)]);

    assert_eq!(
        te.client.try_set_treasury_split(&te.admin, &entries),
        Err(Ok(Error::InvalidFeeBips))
    );
    assert_eq!(te.client.get_treasury_split(), None);
}

#[test]
fn set_treasury_split_rejects_a_bps_sum_that_overflows_u32() {
    let te = TestEnv::default();
    let mut entries: Vec<TreasurySplitEntry> = Vec::new(&te.env);
    for _ in 0..3 {
        entries.push_back(TreasurySplitEntry {
            beneficiary: Address::generate(&te.env),
            // 3 x 1_500_000_000 overflows u32::MAX mid-scan.
            bps: 1_500_000_000,
        });
    }

    assert_eq!(
        te.client.try_set_treasury_split(&te.admin, &entries),
        Err(Ok(Error::Overflow))
    );
    assert_eq!(te.client.get_treasury_split(), None);
}
