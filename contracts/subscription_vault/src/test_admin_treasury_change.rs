//! Adversarial coverage for `cancel_treasury_change` (see `src/admin.rs`).
//!
//! The two-step treasury flow lets an admin abort a queued protocol-fee /
//! treasury change before its 48h timelock elapses. These tests pin down the
//! security-relevant edges of that abort path:
//!
//! - the happy path removes the pending change and makes it unexecutable,
//! - a non-admin caller is rejected with `Error::Forbidden` and cannot drop
//!   the pending change (authorization is checked before storage),
//! - cancelling with nothing queued is `Error::NotFound`, repeatedly, with no
//!   state drift,
//! - cancellation is state-neutral: it emits no event and leaves the stored
//!   treasury / fee-bps configuration exactly as it was,
//! - a successful cancel frees the pending slot so a fresh change can be queued
//!   once the admin-config cooldown has elapsed.

#![cfg(test)]

use crate::admin::CONFIG_COOLDOWN_SECS;
use crate::test_utils::setup::TestEnv;
use crate::types::{DataKey, Error, PendingTreasuryChange};
use soroban_sdk::{testutils::Address as _, testutils::Events as _, Address, Env};

const TREASURY_CHANGE_DELAY_SECS: u64 = 48 * 24 * 60 * 60;

/// Read the raw pending-change slot straight from contract storage.
///
/// `cancel_treasury_change` exposes no view of its own, so tests observe the
/// effect through the same `DataKey::PendingTreasuryChange` entry the contract
/// writes.
fn read_pending(env: &Env, contract: &Address) -> Option<PendingTreasuryChange> {
    env.as_contract(contract, || {
        env.storage()
            .persistent()
            .get::<_, PendingTreasuryChange>(&DataKey::PendingTreasuryChange)
    })
}

/// Read the live treasury address via the internal admin getter, because the
/// contract client does not expose a `get_treasury` entry point.
fn get_treasury(t: &TestEnv) -> Option<Address> {
    t.env
        .as_contract(&t.client.address, || crate::admin::get_treasury(&t.env))
}

#[test]
fn cancel_removes_pending_change_and_blocks_execution() {
    let t = TestEnv::default();
    let new_treasury = Address::generate(&t.env);

    t.client
        .queue_treasury_change(&t.admin, &new_treasury, &500u32);
    assert!(read_pending(&t.env, &t.client.address).is_some());

    t.client.cancel_treasury_change(&t.admin);
    assert!(read_pending(&t.env, &t.client.address).is_none());

    // A cancelled change must never become executable, not even after the
    // original timelock would have elapsed.
    t.jump(TREASURY_CHANGE_DELAY_SECS + 1);
    assert_eq!(
        t.client.try_execute_treasury_change(&t.admin),
        Err(Ok(Error::NotFound))
    );
}

#[test]
fn non_admin_cannot_cancel_and_pending_survives() {
    let t = TestEnv::default();
    let new_treasury = Address::generate(&t.env);
    let attacker = Address::generate(&t.env);

    t.client
        .queue_treasury_change(&t.admin, &new_treasury, &500u32);
    let pending_before = read_pending(&t.env, &t.client.address).expect("queued change");
    let treasury_before = get_treasury(&t);
    let fee_before = t.client.get_protocol_fee_bps();

    assert_eq!(
        t.client.try_cancel_treasury_change(&attacker),
        Err(Ok(Error::Forbidden))
    );

    // Rejected cancel is side-effect free: the queued change is still there,
    // unchanged, and the live config is untouched.
    let pending_after =
        read_pending(&t.env, &t.client.address).expect("pending survives rejection");
    assert_eq!(pending_after, pending_before);
    assert_eq!(get_treasury(&t), treasury_before);
    assert_eq!(t.client.get_protocol_fee_bps(), fee_before);

    // The genuine admin still can cancel the change.
    t.client.cancel_treasury_change(&t.admin);
    assert!(read_pending(&t.env, &t.client.address).is_none());
}

#[test]
fn authorization_is_checked_before_pending_state() {
    let t = TestEnv::default();
    let attacker = Address::generate(&t.env);

    // Nothing is queued, yet an unauthorized caller must be rejected with
    // Forbidden (auth runs first) rather than leaking the NotFound state.
    assert_eq!(
        t.client.try_cancel_treasury_change(&attacker),
        Err(Ok(Error::Forbidden))
    );
    assert!(read_pending(&t.env, &t.client.address).is_none());
}

#[test]
fn cancel_without_pending_is_not_found_and_idempotent() {
    let t = TestEnv::default();

    assert_eq!(get_treasury(&t), None);
    assert_eq!(t.client.get_protocol_fee_bps(), 0u32);

    assert_eq!(
        t.client.try_cancel_treasury_change(&t.admin),
        Err(Ok(Error::NotFound))
    );
    // Repeating the rejected call must not mutate any state.
    assert_eq!(
        t.client.try_cancel_treasury_change(&t.admin),
        Err(Ok(Error::NotFound))
    );
    assert!(read_pending(&t.env, &t.client.address).is_none());
    assert_eq!(get_treasury(&t), None);
    assert_eq!(t.client.get_protocol_fee_bps(), 0u32);
}

#[test]
fn cancel_emits_no_event_and_leaves_config_unchanged() {
    let t = TestEnv::default();
    let new_treasury = Address::generate(&t.env);

    t.client
        .queue_treasury_change(&t.admin, &new_treasury, &700u32);

    let treasury_after_queue = get_treasury(&t);
    let fee_after_queue = t.client.get_protocol_fee_bps();
    let events_before_cancel = t.env.events().all().len();

    t.client.cancel_treasury_change(&t.admin);

    assert_eq!(
        t.env.events().all().len(),
        events_before_cancel,
        "cancel_treasury_change must not publish an event"
    );
    assert_eq!(get_treasury(&t), treasury_after_queue);
    assert_eq!(t.client.get_protocol_fee_bps(), fee_after_queue);
}

#[test]
fn cancel_frees_pending_slot_for_a_new_change() {
    let t = TestEnv::default();
    let first = Address::generate(&t.env);
    let second = Address::generate(&t.env);

    t.client.queue_treasury_change(&t.admin, &first, &100u32);
    t.client.cancel_treasury_change(&t.admin);
    t.jump(CONFIG_COOLDOWN_SECS);

    // Without the cancel this would fail with InvalidInput ("pending change
    // already exists"); proving the slot is free is the point of the test.
    t.client.queue_treasury_change(&t.admin, &second, &200u32);
    let pending = read_pending(&t.env, &t.client.address).expect("second change queued");
    assert_eq!(pending.new_fee_bps, 200u32);
    assert_eq!(pending.new_treasury, second);

    // The fresh change is executable once its own timelock elapses.
    t.jump(TREASURY_CHANGE_DELAY_SECS + 1);
    t.client.execute_treasury_change(&t.admin);
    assert!(read_pending(&t.env, &t.client.address).is_none());
    assert_eq!(t.client.get_protocol_fee_bps(), 200u32);
    assert_eq!(get_treasury(&t), Some(second));
}
