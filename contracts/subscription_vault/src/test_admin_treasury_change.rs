//! Adversarial coverage for the two-step treasury flow in `src/admin.rs`:
/// `queue_treasury_change`, `cancel_treasury_change` and `execute_treasury_change`.
///
/// The two-step treasury flow lets an admin queue a protocol-fee / treasury
/// change, abort it before the 48h timelock elapses, and then execute it once
/// the timelock has passed. These tests pin down the security-relevant edges of
/// every step, focusing on `execute_treasury_change`:
///
/// - the happy path applies the queued treasury / fee and clears the slot,
/// - a non-admin caller is rejected with `Error::Forbidden` and cannot apply the
///   pending change (authorization is checked before storage),
/// - executing with nothing queued is `Error::NotFound`, repeatedly, with no
///   state drift,
/// - executing before the timelock elapses is `Error::InvalidInput` and leaves the
///   pending change intact and executable later,
/// - a cancelled change can never be executed,
/// - a rejected execute is state-neutral: it emits no event and leaves the
///   stored treasury / fee-bps configuration exactly as it was,
/// - a successful execute frees the pending slot so a fresh change can be
///   queued once the admin-config cooldown has elapsed.

#![cfg(test)]

use crate::admin::CONFIG_COOLDOWN_SECS;
use crate::test_utils::setup::TestEnv;
use crate::types::{DataKey, Error, PendingTreasuryChange};
use soroban_sdk::{testutils::Address as _, testutils::Events as _, Address, Env};

const TREASURY_CHANGE_DELAY_SECS: u64 = 48 * 24 * 60 * 60;

/// Read the raw pending-change slot straight from contract storage.
///
/// `execute_treasury_change` exposes no view of its own, so tests observe the
/// effect through the same `DataKey::PendingTreasuryChange` entry the contract
/// writes.
fn read_pending(env: &Env, contract: &Address) -> Option<PendingTreasuryChange> {
    env.as_contract(contract, || {
        env.storage()
            .persistent()
            .get::<_, PendingTreasuryChange>(&DataKey::PendingTreasuryChange)
    })
}

/// Queue a change and let its timelock elapse so it is ready to execute.
fn queue_and_mature(t: &TestEnv, new_treasury: &Address, fee_bps: u32) {
    t.client
        .queue_treasury_change(&t.admin, new_treasury, &fee_bps);
    t.jump(TREASURY_CHANGE_DELAY_SECS + 1);
}

///-----------------------------------------------------------------------------
/// Execute: success path
///----------------------------------------------------------------------------

#[test]
fn execute_applies_pending_change_and_clears_slot() {
    let t = TestEnv::default();
    let new_treasury = Address::generate(&t.env);

    // Baseline: no treasury configured, zero fee.
    assert_eq(t.client.get_treasury(), None);
    assert_eq(t.client.get_protocol_fee_bps(), 0u32);

    queue_and_mature(&t, &new_treasury, 500u32);
    assert!(read_pending(&t.env, &t.client.address).is_some());

    t.client.execute_treasury_change(&t.admin);

    // The queued config is applied and the pending slot is drained.
    assert!(read_pending(&t.env, &t.client.address).is_none());
    assert_eq(t.client.get_treasury(), Some(new_treasury));
    assert_eq(t.client.get_protocol_fee_bps(), 500u32);
}

///-----------------------------------------------------------------------------
/// Execute: authorization
///-----------------------------------------------------------------------------

#[test]
fn non_admin_cannot_execute_and_pending_survives() {
    let t = TestEnv::default();
    let new_treasury = Address::generate(&t.env);
    let attacker = Address::generate(&t.env);

    queue_and_mature(&t, &new_treasury, 500u32);
    let pending_before = read_pending(&t.env, &t.client.address).expect("queued change");
    let treasury_before = t.client.get_treasury();
    let fee_before = t.client.get_protocol_fee_bps();
    let events_before = t.env.events().all().len();

    assert_eq!(
        t.client.try_execute_treasury_change(&attacker),
        Err(Ok(Error::Forbidden))
    );

    // Rejected execute is side-effect free: the queued change is still there,
    // unchanged, the live config is untouched and no event was published.
    let pending_after =
        read_pending(&t.env, &t.client.address).expect("pending survives rejection");
    assert_eq(pending_after, pending_before);
    assert_eq(t.client.get_treasury(), treasury_before);
    assert_eq(t.client.get_protocol_fee_bps(), fee_before);
    assert_eq(
        t.env.events().all().len(),
        events_before,
        "rejected execute_treasury_change must not publish an event"
    );

    // The genuine admin still can execute the change.
    t.client.execute_treasury_change(&t.admin);
    assert!(read_pending(&t.env, &t.client.address).is_none());
    assert_eq(t.client.get_protocol_fee_bps(), 500u32);
    assert_eq(t.client.get_treasury(), Some(new_treasury));
}

#[test]
fn authorization_is_checked_before_pending_state_on_execute() {
    let t = TestEnv::default();
    let attacker = Address::generate(&t.env);

    // Nothing is queued, yet an unauthorized caller must be rejected with
    // Forbidden (auth runs first) rather than leaking the NotFound state.
    assert_eq!(
        t.client.try_execute_treasury_change(&attacker),
        Err(Ok(Error::Forbidden))
    );
    assert!(read_pending(&t.env, &t.client.address).is_none());
    assert_eq!(t.client.get_treasury(), None);
    assert_eq!(t.client.get_protocol_fee_bps(), 0u32);
}

///----------------------------------------------------------------------------
/// Execute: nothing queued
///---------------------------------------------------------------------------

#[test]
fn execute_without_pending_is_not_found_and_idempotent() {
    let t = TestEnv::default();

    assert_eq!(t.client.get_treasury(), None);
    assert_eq!(t.client.get_protocol_fee_bps(), 0u32);

    assert_eq!(
        t.client.try_execute_treasury_change(&t.admin),
        Err(Ok(Error::NotFound))
    );
    // Repeating the rejected call must not mutate any state.
    assert_eq!(
        t.client.try_execute_treasury_change(&t.admin),
        Err(Ok(Error::NotFound))
    );
    assert!(read_pending(&t.env, &t.client.address).is_none());
    assert_eq!(t.client.get_treasury(), None);
    assert_eq!(t.client.get_protocol_fee_bps(), 0u32);
}

///----------------------------------------------------------------------------
/// Execute: timelock boundary
///----------------------------------------------------------------------------

#[test]
fn execute_before_timelock_is_rejected_and_pending_survives() {
    let t = TestEnv::default();
    let new_treasury = Address::generate(&t.env);

    t.client
        .queue_treasury_change(&t.admin, &new_treasury, &500u32);
    let pending_before = read_pending(&t.env, &t.client.address).expect("queued change");
    let treasury_before = t.client.get_treasury();
    let fee_before = t.client.get_protocol_fee_bps();

    // One second before the timelock expires the change is not yet executable.
    t.jump(TREASURY_CHANGE_DELAY_SECS - 1);
    assert_eq!(
        t.client.try_execute_treasury_change(&t.admin),
        Err(Ok(Error::InvalidInput))
    );

    // Rejected execute leaves the pending change intact and the config untouched.
    let pending_after =
        read_pending(&t.env, &t.client.address).expect("pending survives rejection");
    assert_eq!(pending_after, pending_before);
    assert_eq!(t.client.get_treasury(), treasury_before);
    assert_eq!(t.client.get_protocol_fee_bps(), fee_before);

    // Once the timelock elapses the same change becomes executable.
    t.jump(2);
    t.client.execute_treasury_change(&t.admin);
    assert!(read_pending(&t.env, &t.client.address).is_none());
    assert_eq!(t.client.get_protocol_fee_bps(), 500u32);
    assert_eq!(t.client.get_treasury(), Some(new_treasury));
}

///----------------------------------------------------------------------------
/// Execute: cancelled change is never executable
///----------------------------------------------------------------------------

#[test]
fn cancelled_change_cannot_be_executed() {
    let t = TestEnv::default();
    let new_treasury = Address::generate(&t.env);

    t.client
        .queue_treasury_change(&t.admin, &new_treasury, &500u32);
    t.client.cancel_treasury_change(&t.admin);
    assert!(read_pending(&t.env, &t.client.address).is_none());

    // A cancelled change must never become executable, not even after the
    // original timelock would have elapsed.
    t.jump(TREASURY_CHANGE_DELAY_SECS + 1);
    assert_eq!(
        t.client.try_execute_treasury_change(&t.admin),
        Err(Ok(Error::NotFound))
    );
    assert_eq!(t.client.get_treasury(), None);
    assert_eq!(t.client.get_protocol_fee_bps(), 0u32);
}

///----------------------------------------------------------------------------
/// Execute: successful execute frees the pending slot
///----------------------------------------------------------------------------

#[test]
fn execute_frees_pending_slot_for_a_new_change() {
    let t = TestEnv::default();
    let first = Address::generate(&t.env);
    let second = Address::generate(&t.env);

    queue_and_mature(&t, &first, 100u32);
    t.client.execute_treasury_change(&t.admin);
    assert!(read_pending(&t.env, &t.client.address).is_none());
    t.jump(CONFIG_COOLDOWN_SECS);

    // Without the execute clearing the slot this would fail with InvalidInput
    // ("pending change already exists"); proving the slot is free is the point.
    t.client
        .queue_treasury_change(&t.admin, &second, &200u32);
    let pending = read_pending(&t.env, &t.client.address).expect("second change queued");
    assert_eq!(pending.new_fee_bps, 200u32);
    assert_eq!(pending.new_treasury, second);

    // The fresh change is executable once its own timelock elapses.
    t.jump(TREASURY_CHANGE_DELAY_SECS + 1);
    t.client.execute_treasury_change(&t.admin);
    assert!(read_pending(&t.env, &t.client.address).is_none());
    assert_eq!(t.client.get_protocol_fee_bps(), 200u32);
    assert_eq!(t.client.get_treasury(), Some(second));
}