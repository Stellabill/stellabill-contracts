#![cfg(test)]

//! Adversarial coverage for `admin::execute_treasury_change` (issue #1002).
//!
//! `execute_treasury_change` had no directly associated test fixture, so this
//! module pins down its full boundary surface:
//!
//! - **Authorization** — only the *stored* admin may execute. The guard runs
//!   before the pending-change lookup, so a rejected caller can never consume,
//!   overwrite, or observe the queued change, and a rotated-away admin loses
//!   the ability to execute immediately.
//! - **Timelock boundaries** — `now < effective_at` is rejected with
//!   `Error::TimelockNotElapsed`; `now == effective_at` (the exact boundary)
//!   succeeds.
//! - **Missing / replayed state** — executing with nothing queued, replaying an
//!   already-executed change, or executing after a cancel all return
//!   `Error::NotFound` and leave state untouched.
//! - **State invariants** — every rejected call leaves the pending record, the
//!   configured treasury and the fee bps untouched, and emits no
//!   `treasury_change_executed` event.
//! - **Queue-side boundaries** — `fee_bps > 10_000` and a treasury equal to the
//!   contract's own address are rejected before anything is persisted.

use crate::types::{DataKey, Error, PendingTreasuryChange};
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    Address, Env,
};

/// Mirror of the private `admin::TREASURY_CHANGE_DELAY_SECS` (48 hours).
const TREASURY_CHANGE_DELAY_SECS: u64 = 48 * 24 * 60 * 60;
const MIN_TOPUP: i128 = 1_000_000;
const GRACE_PERIOD: u64 = 86_400;
const START_TS: u64 = 1_000_000;

// ── Harness ──────────────────────────────────────────────────────────────────

/// Mocked-auth fixture. Returns `(env, client, admin)`.
fn setup() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(START_TS);
    build_vault(env)
}

/// Same vault, but with **no** auth mocking: `require_auth` must fail at the
/// host level. Used to prove auth is enforced before the pending lookup.
fn setup_without_mocked_auths() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.ledger().set_timestamp(START_TS);
    build_vault(env)
}

fn build_vault(env: Env) -> (Env, SubscriptionVaultClient<'static>, Address) {
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(admin.clone()).address();
    client.init(&token, &6, &admin, &MIN_TOPUP, &GRACE_PERIOD);
    (env, client, admin)
}

fn advance(env: &Env, seconds: u64) {
    env.ledger().with_mut(|l| l.timestamp += seconds);
}

/// Read the queued change straight out of contract storage.
fn pending(env: &Env, client: &SubscriptionVaultClient) -> Option<PendingTreasuryChange> {
    env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .get::<_, PendingTreasuryChange>(&DataKey::PendingTreasuryChange)
    })
}

/// Read `DataKey::Treasury`, tolerating the legacy instance-storage layout.
fn stored_treasury(env: &Env, client: &SubscriptionVaultClient) -> Option<Address> {
    env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .get::<_, Address>(&DataKey::Treasury)
            .or_else(|| env.storage().instance().get::<_, Address>(&DataKey::Treasury))
    })
}

/// Read `DataKey::FeeBps`, tolerating the legacy instance-storage layout.
fn stored_fee_bps(env: &Env, client: &SubscriptionVaultClient) -> Option<u32> {
    env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .get::<_, u32>(&DataKey::FeeBps)
            .or_else(|| env.storage().instance().get::<_, u32>(&DataKey::FeeBps))
    })
}

/// Count `treasury_change_executed` events emitted so far.
fn executed_event_count(env: &Env) -> usize {
    env.events()
        .all()
        .iter()
        .filter(|(_addr, topics, _data)| {
            topics.len() != 0
                && format!("{:?}", topics.get(0).unwrap()).contains("treasury_change_executed")
        })
        .count()
}

// ═════════════════════════════════════════════════════════════════════════════
// Happy path + timelock boundaries
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn execute_before_timelock_is_rejected_and_leaves_state_untouched() {
    let (env, client, admin) = setup();
    let new_treasury = Address::generate(&env);

    client.queue_treasury_change(&admin, &new_treasury, &250u32);
    let queued = pending(&env, &client).expect("queue must persist the pending change");

    // Snapshot immediately before the rejected call.
    let treasury_before = stored_treasury(&env, &client);
    let bps_before = stored_fee_bps(&env, &client);
    let events_before = executed_event_count(&env);

    // One second short of the timelock.
    advance(&env, TREASURY_CHANGE_DELAY_SECS - 1);
    let result = client.try_execute_treasury_change(&admin);
    assert_eq!(result, Err(Ok(Error::TimelockNotElapsed)));

    // Rejected: the pending record must survive verbatim ...
    let after = pending(&env, &client).expect("rejected execute must not clear the pending change");
    assert_eq!(after.new_treasury, queued.new_treasury);
    assert_eq!(after.new_fee_bps, queued.new_fee_bps);
    assert_eq!(after.effective_at, queued.effective_at);
    // ... and neither the config nor the event log may move.
    assert_eq!(stored_treasury(&env, &client), treasury_before);
    assert_eq!(stored_fee_bps(&env, &client), bps_before);
    assert_eq!(executed_event_count(&env), events_before);
}

#[test]
fn execute_exactly_at_effective_at_succeeds() {
    let (env, client, admin) = setup();
    let new_treasury = Address::generate(&env);

    client.queue_treasury_change(&admin, &new_treasury, &250u32);
    let queued = pending(&env, &client).unwrap();

    // Land exactly on `effective_at`: the guard is `now < effective_at`, so
    // equality must be accepted.
    advance(&env, queued.effective_at - env.ledger().timestamp());
    assert_eq!(env.ledger().timestamp(), queued.effective_at);

    client.execute_treasury_change(&admin);

    assert!(pending(&env, &client).is_none(), "pending change is consumed");
    assert_eq!(stored_treasury(&env, &client), Some(new_treasury));
    assert_eq!(stored_fee_bps(&env, &client), Some(250u32));
    assert_eq!(executed_event_count(&env), 1);
}

#[test]
fn execute_one_second_after_effective_at_succeeds_and_emits_event() {
    let (env, client, admin) = setup();
    let new_treasury = Address::generate(&env);

    client.queue_treasury_change(&admin, &new_treasury, &500u32);
    let queued = pending(&env, &client).unwrap();

    advance(&env, queued.effective_at - env.ledger().timestamp() + 1);
    client.execute_treasury_change(&admin);

    assert!(pending(&env, &client).is_none());
    assert_eq!(stored_treasury(&env, &client), Some(new_treasury));
    assert_eq!(stored_fee_bps(&env, &client), Some(500u32));
    assert_eq!(executed_event_count(&env), 1);
}

// ═════════════════════════════════════════════════════════════════════════════
// Missing / replayed state
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn execute_without_queued_change_returns_not_found() {
    let (env, client, admin) = setup();

    let treasury_before = stored_treasury(&env, &client);
    let bps_before = stored_fee_bps(&env, &client);

    let result = client.try_execute_treasury_change(&admin);
    assert_eq!(result, Err(Ok(Error::NotFound)));

    assert!(pending(&env, &client).is_none());
    assert_eq!(stored_treasury(&env, &client), treasury_before);
    assert_eq!(stored_fee_bps(&env, &client), bps_before);
    assert_eq!(executed_event_count(&env), 0);
}

#[test]
fn execute_is_not_replayable_after_success() {
    let (env, client, admin) = setup();
    let new_treasury = Address::generate(&env);

    client.queue_treasury_change(&admin, &new_treasury, &100u32);
    advance(&env, TREASURY_CHANGE_DELAY_SECS + 1);
    client.execute_treasury_change(&admin);

    let treasury_after = stored_treasury(&env, &client);
    let bps_after = stored_fee_bps(&env, &client);

    // Second execution must be refused, and must not emit a second event.
    let result = client.try_execute_treasury_change(&admin);
    assert_eq!(result, Err(Ok(Error::NotFound)));
    assert_eq!(stored_treasury(&env, &client), treasury_after);
    assert_eq!(stored_fee_bps(&env, &client), bps_after);
    assert_eq!(executed_event_count(&env), 1);
}

#[test]
fn execute_after_cancel_returns_not_found_and_keeps_state() {
    let (env, client, admin) = setup();
    let new_treasury = Address::generate(&env);

    client.queue_treasury_change(&admin, &new_treasury, &100u32);
    advance(&env, TREASURY_CHANGE_DELAY_SECS + 1);
    client.cancel_treasury_change(&admin);
    assert!(pending(&env, &client).is_none(), "cancel clears the pending change");

    let treasury_before = stored_treasury(&env, &client);
    let bps_before = stored_fee_bps(&env, &client);
    let events_before = executed_event_count(&env);

    let result = client.try_execute_treasury_change(&admin);
    assert_eq!(result, Err(Ok(Error::NotFound)));

    assert_eq!(stored_treasury(&env, &client), treasury_before);
    assert_eq!(stored_fee_bps(&env, &client), bps_before);
    assert_eq!(executed_event_count(&env), events_before);
}

#[test]
fn repeated_premature_executes_never_consume_the_pending_change() {
    let (env, client, admin) = setup();
    let new_treasury = Address::generate(&env);

    client.queue_treasury_change(&admin, &new_treasury, &150u32);
    let queued = pending(&env, &client).unwrap();

    for _ in 0..5 {
        let result = client.try_execute_treasury_change(&admin);
        assert_eq!(result, Err(Ok(Error::TimelockNotElapsed)));
        assert!(pending(&env, &client).is_some());
        assert_eq!(executed_event_count(&env), 0);
    }

    // After the timelock, the untouched change still executes normally.
    advance(&env, queued.effective_at - env.ledger().timestamp() + 1);
    client.execute_treasury_change(&admin);
    assert!(pending(&env, &client).is_none());
    assert_eq!(stored_treasury(&env, &client), Some(new_treasury));
    assert_eq!(stored_fee_bps(&env, &client), Some(150u32));
    assert_eq!(executed_event_count(&env), 1);
}

// ═════════════════════════════════════════════════════════════════════════════
// Authorization
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn stranger_cannot_execute_even_after_timelock() {
    let (env, client, admin) = setup();
    let new_treasury = Address::generate(&env);

    client.queue_treasury_change(&admin, &new_treasury, &300u32);
    let queued = pending(&env, &client).unwrap();
    // Timelock already elapsed — the only thing that must block is auth.
    advance(&env, TREASURY_CHANGE_DELAY_SECS + 1);

    let treasury_before = stored_treasury(&env, &client);
    let bps_before = stored_fee_bps(&env, &client);
    let stranger = Address::generate(&env);

    let result = client.try_execute_treasury_change(&stranger);
    assert_eq!(result, Err(Ok(Error::Forbidden)));

    let after = pending(&env, &client).expect("unauthorized execute must not consume the change");
    assert_eq!(after.new_treasury, queued.new_treasury);
    assert_eq!(after.new_fee_bps, queued.new_fee_bps);
    assert_eq!(after.effective_at, queued.effective_at);
    assert_eq!(stored_treasury(&env, &client), treasury_before);
    assert_eq!(stored_fee_bps(&env, &client), bps_before);
    assert_eq!(executed_event_count(&env), 0);

    // The genuine admin can still execute afterwards.
    client.execute_treasury_change(&admin);
    assert!(pending(&env, &client).is_none());
    assert_eq!(stored_treasury(&env, &client), Some(new_treasury));
    assert_eq!(executed_event_count(&env), 1);
}

#[test]
fn contract_address_as_caller_is_rejected() {
    let (env, client, admin) = setup();
    let new_treasury = Address::generate(&env);

    client.queue_treasury_change(&admin, &new_treasury, &300u32);
    advance(&env, TREASURY_CHANGE_DELAY_SECS + 1);

    // The contract can never be the stored admin, so this must be Forbidden.
    let result = client.try_execute_treasury_change(&client.address);
    assert_eq!(result, Err(Ok(Error::Forbidden)));
    assert!(pending(&env, &client).is_some());
    assert_eq!(executed_event_count(&env), 0);
}

#[test]
fn admin_rotated_away_from_pending_change_cannot_execute_it() {
    let (env, client, old_admin) = setup();
    let new_treasury = Address::generate(&env);

    client.queue_treasury_change(&old_admin, &new_treasury, &75u32);
    advance(&env, TREASURY_CHANGE_DELAY_SECS + 1);

    // Hand the admin role to a fresh address.
    let new_admin = Address::generate(&env);
    client.rotate_admin(&old_admin, &new_admin, &0u64);
    assert_eq!(client.get_admin(), new_admin);

    // The former admin must lose access immediately, even though they queued
    // the change and the timelock has elapsed.
    let result = client.try_execute_treasury_change(&old_admin);
    assert_eq!(result, Err(Ok(Error::Forbidden)));
    assert!(pending(&env, &client).is_some());

    // The new admin inherits the pending change and can execute it.
    client.execute_treasury_change(&new_admin);
    assert!(pending(&env, &client).is_none());
    assert_eq!(stored_treasury(&env, &client), Some(new_treasury));
    assert_eq!(stored_fee_bps(&env, &client), Some(75u32));
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn execute_without_auth_panics_before_reading_pending_state() {
    // No mocked auths: `admin.require_auth()` must fail at the host level.
    let (env, client, admin) = setup_without_mocked_auths();
    let new_treasury = Address::generate(&env);

    // Plant an already-runnable pending change directly in storage so that, if
    // auth were (incorrectly) checked after the lookup, this call would slip
    // through and mutate config. It must panic instead.
    env.as_contract(&client.address, || {
        env.storage().persistent().set(
            &DataKey::PendingTreasuryChange,
            &PendingTreasuryChange {
                new_treasury: new_treasury.clone(),
                new_fee_bps: 400u32,
                effective_at: 0,
            },
        );
    });

    let _ = client.execute_treasury_change(&admin);
}

// ═════════════════════════════════════════════════════════════════════════════
// Queue-side boundaries that guard `execute_treasury_change`
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn queue_rejects_fee_bps_above_max_and_persists_nothing() {
    let (env, client, admin) = setup();
    let new_treasury = Address::generate(&env);

    let result = client.try_queue_treasury_change(&admin, &new_treasury, &10_001u32);
    assert_eq!(result, Err(Ok(Error::InvalidInput)));

    assert!(pending(&env, &client).is_none());
    assert_eq!(stored_treasury(&env, &client), None);
    assert_eq!(stored_fee_bps(&env, &client), None);
    assert_eq!(executed_event_count(&env), 0);
}

#[test]
fn queue_accepts_exact_max_fee_bps() {
    let (env, client, admin) = setup();
    let new_treasury = Address::generate(&env);

    client.queue_treasury_change(&admin, &new_treasury, &10_000u32);

    let queued = pending(&env, &client).expect("10_000 is the inclusive upper bound");
    assert_eq!(queued.new_fee_bps, 10_000u32);

    advance(&env, TREASURY_CHANGE_DELAY_SECS + 1);
    client.execute_treasury_change(&admin);
    assert_eq!(stored_fee_bps(&env, &client), Some(10_000u32));
    assert_eq!(stored_treasury(&env, &client), Some(new_treasury));
}

#[test]
fn queue_accepts_zero_fee_bps() {
    let (env, client, admin) = setup();
    let new_treasury = Address::generate(&env);

    client.queue_treasury_change(&admin, &new_treasury, &0u32);

    let queued = pending(&env, &client).expect("0 is the inclusive lower bound");
    assert_eq!(queued.new_fee_bps, 0u32);

    advance(&env, TREASURY_CHANGE_DELAY_SECS + 1);
    client.execute_treasury_change(&admin);
    assert_eq!(stored_fee_bps(&env, &client), Some(0u32));
    assert_eq!(stored_treasury(&env, &client), Some(new_treasury));
}

#[test]
fn queue_rejects_contract_self_treasury_before_persisting() {
    let (env, client, admin) = setup();

    let result = client.try_queue_treasury_change(&admin, &client.address, &500u32);
    assert_eq!(result, Err(Ok(Error::InvalidInput)));

    assert!(pending(&env, &client).is_none());
    assert_eq!(stored_treasury(&env, &client), None);
    assert_eq!(stored_fee_bps(&env, &client), None);
}

#[test]
fn second_queue_while_one_is_pending_is_rejected_without_overwriting() {
    let (env, client, admin) = setup();
    let first_treasury = Address::generate(&env);
    let second_treasury = Address::generate(&env);

    client.queue_treasury_change(&admin, &first_treasury, &111u32);
    let queued = pending(&env, &client).unwrap();

    let result = client.try_queue_treasury_change(&admin, &second_treasury, &222u32);
    assert_eq!(result, Err(Ok(Error::InvalidInput)));

    let after = pending(&env, &client).expect("the first pending change must survive");
    assert_eq!(after.new_treasury, queued.new_treasury);
    assert_eq!(after.new_treasury, first_treasury);
    assert_eq!(after.new_fee_bps, queued.new_fee_bps);
    assert_eq!(after.effective_at, queued.effective_at);
}
