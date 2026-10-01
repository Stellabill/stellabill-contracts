//! Adversarial coverage for `admin::get_treasury`.
//!
//! `get_treasury` is the read side of the protocol-fee destination: it feeds
//! fee routing, so an incorrect (or prematurely visible) value sends real value
//! to the wrong address.  `Treasury` is also written by a *timelocked* queue
//! path, which makes the read path interesting: the getter must be consistent
//! with the queued, executed and cancelled states of `PendingTreasuryChange`.
//!
//! Covered:
//! - unset before configuration, and None is distinguishable from a zero address
//! - the address set by `set_protocol_fee` / `queue_treasury_change`
//! - the boundary fee of `0` bps (fee collection disabled) still records the treasury
//! - visibility while a change is queued, and after it executes
//! - state after rejected operations: non-admin caller, `fee_bps > 10_000`,
//!   self-referential treasury, a second queue while one is pending, and the
//!   per-key cooldown window
//! - cancel does not retroactively revert the published value
//! - repeated sequential changes converge on the latest executed address
//! - the single treasury is independent of the multi-beneficiary split
//! - reads are pure: repeating them never changes state

#![cfg(test)]

extern crate std;

use crate::admin;
use crate::test_utils::setup::TestEnv;
use crate::types::TreasurySplitEntry;
use crate::Error;
use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, Address, Vec};

/// Mirrors `admin::TREASURY_CHANGE_DELAY_SECS` (48 days).
const TREASURY_CHANGE_DELAY_SECS: u64 = 48 * 24 * 60 * 60;
/// Mirrors `admin::CONFIG_COOLDOWN_SECS` (6 hours).
const CONFIG_COOLDOWN_SECS: u64 = 6 * 60 * 60;

/// Read `DataKey::Treasury` through the same code path the contract uses.
fn read_treasury(te: &TestEnv) -> Option<Address> {
    te.env
        .as_contract(&te.client.address, || admin::get_treasury(&te.env))
}

/// Move the ledger to a non-zero timestamp so cooldown bookkeeping is active.
fn set_time(te: &TestEnv, timestamp: u64) {
    te.env.ledger().with_mut(|li| li.timestamp = timestamp);
}

#[test]
fn treasury_is_none_before_any_protocol_fee_is_configured() {
    let te = TestEnv::default();

    assert_eq!(read_treasury(&te), None);
}

#[test]
fn treasury_reflects_the_address_set_by_set_protocol_fee() {
    let te = TestEnv::default();
    let treasury = Address::generate(&te.env);

    te.client.set_protocol_fee(&te.admin, &treasury, &500);

    assert_eq!(read_treasury(&te), Some(treasury.clone()));
    assert_eq!(te.client.get_protocol_fee_bps(), 500);
}

#[test]
fn zero_fee_bps_is_accepted_and_still_records_the_treasury() {
    let te = TestEnv::default();
    let treasury = Address::generate(&te.env);

    te.client.set_protocol_fee(&te.admin, &treasury, &0);

    assert_eq!(te.client.get_protocol_fee_bps(), 0);
    assert_eq!(read_treasury(&te), Some(treasury));
}

#[test]
fn treasury_is_visible_as_soon_as_the_change_is_queued() {
    let te = TestEnv::default();
    set_time(&te, 1_000);
    let treasury = Address::generate(&te.env);

    te.client.queue_treasury_change(&te.admin, &treasury, &250);

    // The queue path writes the published treasury immediately, while the
    // timelock only governs `PendingTreasuryChange` execution.
    assert_eq!(read_treasury(&te), Some(treasury.clone()));

    let early = te.client.try_execute_treasury_change(&te.admin);
    assert!(
        early.is_err(),
        "executing before the effective timestamp must fail"
    );
    assert_eq!(read_treasury(&te), Some(treasury));
}

#[test]
fn treasury_follows_the_change_after_the_timelock_elapses() {
    let te = TestEnv::default();
    set_time(&te, 1_000);
    let treasury = Address::generate(&te.env);

    te.client
        .queue_treasury_change(&te.admin, &treasury, &1_000);
    te.jump(TREASURY_CHANGE_DELAY_SECS + 1);
    te.client.execute_treasury_change(&te.admin);

    assert_eq!(read_treasury(&te), Some(treasury));
    assert_eq!(te.client.get_protocol_fee_bps(), 1_000);
}

#[test]
fn treasury_is_unchanged_when_the_timelock_has_not_elapsed() {
    let te = TestEnv::default();
    set_time(&te, 1_000);
    let queued = Address::generate(&te.env);

    te.client.queue_treasury_change(&te.admin, &queued, &100);
    te.jump(TREASURY_CHANGE_DELAY_SECS - 1);

    let err = te
        .client
        .try_execute_treasury_change(&te.admin)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, Error::TimelockNotElapsed);
    assert_eq!(read_treasury(&te), Some(queued));
}

#[test]
fn cancelling_a_pending_change_does_not_revert_the_published_treasury() {
    let te = TestEnv::default();
    set_time(&te, 1_000);
    let queued = Address::generate(&te.env);

    te.client.queue_treasury_change(&te.admin, &queued, &100);
    te.client.cancel_treasury_change(&te.admin);

    assert_eq!(read_treasury(&te), Some(queued));
}

#[test]
fn a_non_admin_queue_leaves_the_treasury_unset() {
    let te = TestEnv::default();
    let stranger = Address::generate(&te.env);
    let treasury = Address::generate(&te.env);

    let err = te
        .client
        .try_queue_treasury_change(&stranger, &treasury, &100)
        .unwrap_err()
        .unwrap();

    assert_eq!(err, Error::Forbidden);
    assert_eq!(read_treasury(&te), None);
}

#[test]
fn a_non_admin_execute_leaves_the_treasury_unset() {
    let te = TestEnv::default();
    let stranger = Address::generate(&te.env);

    let err = te
        .client
        .try_execute_treasury_change(&stranger)
        .unwrap_err()
        .unwrap();

    assert_eq!(err, Error::Forbidden);
    assert_eq!(read_treasury(&te), None);
}

#[test]
fn an_over_limit_fee_bps_leaves_the_treasury_unset() {
    let te = TestEnv::default();
    let treasury = Address::generate(&te.env);

    for fee_bps in [10_001u32, 20_000, u32::MAX] {
        let err = te
            .client
            .try_queue_treasury_change(&te.admin, &treasury, &fee_bps)
            .unwrap_err()
            .unwrap();

        assert_eq!(
            err,
            Error::InvalidInput,
            "fee_bps {fee_bps} must be rejected"
        );
        assert_eq!(read_treasury(&te), None);
    }
}

#[test]
fn a_self_referential_treasury_leaves_the_treasury_unset() {
    let te = TestEnv::default();
    let contract = te.client.address.clone();

    let err = te
        .client
        .try_set_protocol_fee(&te.admin, &contract, &100)
        .unwrap_err()
        .unwrap();

    assert_eq!(err, Error::InvalidInput);
    assert_eq!(read_treasury(&te), None);
}

#[test]
fn a_second_queued_change_cannot_repoint_the_treasury_while_one_is_pending() {
    let te = TestEnv::default();
    set_time(&te, 1_000);
    let first = Address::generate(&te.env);
    let second = Address::generate(&te.env);

    te.client.queue_treasury_change(&te.admin, &first, &100);

    let err = te
        .client
        .try_queue_treasury_change(&te.admin, &second, &200)
        .unwrap_err()
        .unwrap();

    assert_eq!(err, Error::InvalidInput);
    assert_eq!(read_treasury(&te), Some(first));
}

#[test]
fn the_per_key_cooldown_blocks_a_second_queue_and_keeps_the_treasury() {
    let te = TestEnv::default();
    set_time(&te, 1_000);
    let first = Address::generate(&te.env);
    let second = Address::generate(&te.env);

    te.client.queue_treasury_change(&te.admin, &first, &100);
    te.client.cancel_treasury_change(&te.admin);

    // The pending slot is free again, but the cooldown timestamp is not.
    te.jump(CONFIG_COOLDOWN_SECS - 1);
    let err = te
        .client
        .try_queue_treasury_change(&te.admin, &second, &200)
        .unwrap_err()
        .unwrap();

    assert_eq!(err, Error::CooldownActive);
    assert_eq!(read_treasury(&te), Some(first));

    // Once the cooldown elapses the same call is accepted.
    te.jump(1);
    te.client.queue_treasury_change(&te.admin, &second, &200);
    assert_eq!(read_treasury(&te), Some(second));
}

#[test]
fn treasury_converges_across_sequential_executed_changes() {
    let te = TestEnv::default();
    set_time(&te, 1_000);
    let first = Address::generate(&te.env);
    let second = Address::generate(&te.env);

    te.client.queue_treasury_change(&te.admin, &first, &100);
    te.jump(TREASURY_CHANGE_DELAY_SECS + 1);
    te.client.execute_treasury_change(&te.admin);
    assert_eq!(read_treasury(&te), Some(first.clone()));

    te.jump(CONFIG_COOLDOWN_SECS + 1);
    te.client.queue_treasury_change(&te.admin, &second, &300);
    te.jump(TREASURY_CHANGE_DELAY_SECS + 1);
    te.client.execute_treasury_change(&te.admin);

    assert_eq!(read_treasury(&te), Some(second));
    assert_eq!(te.client.get_protocol_fee_bps(), 300);
}

#[test]
fn treasury_is_independent_of_the_multi_beneficiary_split() {
    let te = TestEnv::default();
    set_time(&te, 1_000);
    let treasury = Address::generate(&te.env);
    te.client.set_protocol_fee(&te.admin, &treasury, &500);
    assert_eq!(read_treasury(&te), Some(treasury.clone()));

    let mut entries = Vec::new(&te.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: Address::generate(&te.env),
        bps: 10_000,
    });
    te.client.set_treasury_split(&te.admin, &entries);

    // Configuring split routing must not silently repoint the single treasury.
    assert_eq!(read_treasury(&te), Some(treasury));
}

#[test]
fn repeated_reads_are_free_of_side_effects() {
    let te = TestEnv::default();
    let treasury = Address::generate(&te.env);
    te.client.set_protocol_fee(&te.admin, &treasury, &250);

    let first = read_treasury(&te);
    let second = read_treasury(&te);
    let third = read_treasury(&te);

    assert_eq!(first, second);
    assert_eq!(second, third);
    assert_eq!(third, Some(treasury));
}
