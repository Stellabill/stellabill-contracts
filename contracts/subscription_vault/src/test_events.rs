//! Comprehensive event emission tests for the subscription_vault contract.
//!
//! Verifies that every critical state transition and fund movement emits exactly
//! the right event with the correct schema, and that failures do NOT emit events.
//!
//! # Security invariants tested
//! - Events are never emitted on failed operations
//! - Event ordering is deterministic for batch operations
//! - Events do not leak optional sensitive metadata
//! - Failure paths leave event count unchanged

#![cfg(test)]

use crate::test_utils::setup::TestEnv;
use crate::{
    FundsDepositedEvent, MerchantWithdrawalEvent, SubscriptionCancelledEvent,
    SubscriptionChargedEvent, SubscriptionCreatedEvent, SubscriptionPausedEvent,
    SubscriptionResumedEvent,
};
use soroban_sdk::{
    testutils::{Address as _, Events},
    Address, IntoVal, Symbol,
};

const AMOUNT: i128 = 5_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const DEPOSIT: i128 = 20_000_000;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn setup() -> (TestEnv, Address, Address) {
    let t = TestEnv::default();
    let subscriber = Address::generate(&t.env);
    let merchant = Address::generate(&t.env);
    t.stellar_token_client().mint(&subscriber, &100_000_000);
    (t, subscriber, merchant)
}

fn create_funded_sub(t: &TestEnv, subscriber: &Address, merchant: &Address) -> u32 {
    let id = t.client.create_subscription(
        subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    t.client.deposit_funds(&id, subscriber, &DEPOSIT);
    id
}

fn event_count(t: &TestEnv) -> usize {
    t.env.events().all().len()
}

// ── SubscriptionCreatedEvent ──────────────────────────────────────────────────

#[test]
fn test_create_subscription_emits_event() {
    let (t, subscriber, merchant) = setup();
    let before = event_count(&t);

    let id = t.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    let events = t.env.events().all();
    assert_eq!(events.len(), before + 1, "exactly one event on create");

    let ev = events.last().unwrap();
    assert_eq!(ev.0, t.client.address);

    // Topic[0] = "created", Topic[1] = subscription_id
    let topic0 = Symbol::from_val(&t.env, &ev.1.get(0).unwrap());
    assert_eq!(topic0, Symbol::new(&t.env, "created"));
    let topic1 = u32::from_val(&t.env, &ev.1.get(1).unwrap());
    assert_eq!(topic1, id);

    // Decode and verify payload
    let data: SubscriptionCreatedEvent = ev.2.into_val(&t.env);
    assert_eq!(data.subscription_id, id);
    assert_eq!(data.subscriber, subscriber);
    assert_eq!(data.merchant, merchant);
    assert_eq!(data.token, t.token);
    assert_eq!(data.amount, AMOUNT);
    assert_eq!(data.interval_seconds, INTERVAL);
    assert_eq!(data.lifetime_cap, None);
    assert_eq!(data.expires_at, None);
}

#[test]
fn test_create_subscription_event_includes_lifetime_cap() {
    let (t, subscriber, merchant) = setup();
    let cap = 50_000_000i128;

    let id = t.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &Some(cap),
        &None::<u64>,
    );

    let events = t.env.events().all();
    let data: SubscriptionCreatedEvent = events.last().unwrap().2.into_val(&t.env);
    assert_eq!(data.subscription_id, id);
    assert_eq!(data.lifetime_cap, Some(cap));
}

// ── FundsDepositedEvent ───────────────────────────────────────────────────────

#[test]
fn test_deposit_funds_emits_event() {
    let (t, subscriber, merchant) = setup();
    let id = t.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    let before = event_count(&t);

    t.client.deposit_funds(&id, &subscriber, &DEPOSIT);

    let events = t.env.events().all();
    assert_eq!(events.len(), before + 1, "exactly one event on deposit");

    let ev = events.last().unwrap();
    let topic0 = Symbol::from_val(&t.env, &ev.1.get(0).unwrap());
    assert_eq!(topic0, Symbol::new(&t.env, "deposited"));

    let data: FundsDepositedEvent = ev.2.into_val(&t.env);
    assert_eq!(data.subscription_id, id);
    assert_eq!(data.subscriber, subscriber);
    assert_eq!(data.token, t.token);
    assert_eq!(data.amount, DEPOSIT);
    assert_eq!(data.new_balance, DEPOSIT);
}

#[test]
fn test_deposit_cumulative_balance_in_event() {
    let (t, subscriber, merchant) = setup();
    let id = t.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    t.client.deposit_funds(&id, &subscriber, &DEPOSIT);
    t.client.deposit_funds(&id, &subscriber, &DEPOSIT);

    let events = t.env.events().all();
    let data: FundsDepositedEvent = events.last().unwrap().2.into_val(&t.env);
    // Second deposit: new_balance = DEPOSIT * 2
    assert_eq!(data.new_balance, DEPOSIT * 2);
    assert_eq!(data.amount, DEPOSIT);
}

#[test]
fn test_failed_deposit_below_min_topup_no_event() {
    let (t, subscriber, merchant) = setup();
    let id = t.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    let before = event_count(&t);

    // min_topup is 1_000_000; deposit 1 is below threshold
    let result = t.client.try_deposit_funds(&id, &subscriber, &1i128);
    assert!(result.is_err(), "should fail below min topup");
    assert_eq!(event_count(&t), before, "no event on failed deposit");
}

// ── SubscriptionChargedEvent ──────────────────────────────────────────────────

#[test]
fn test_charge_subscription_emits_event() {
    let (t, subscriber, merchant) = setup();
    let id = create_funded_sub(&t, &subscriber, &merchant);
    t.jump(INTERVAL + 1);
    let before = event_count(&t);

    t.client.charge_subscription(&id);

    let events = t.env.events().all();
    // At minimum one "charged" event
    let charged_events: Vec<_> = events
        .iter()
        .filter(|ev| {
            ev.1.get(0)
                .map(|v| Symbol::from_val(&t.env, &v) == Symbol::new(&t.env, "charged"))
                .unwrap_or(false)
        })
        .collect();
    assert!(!charged_events.is_empty(), "charged event must be emitted");

    let data: SubscriptionChargedEvent = charged_events[0].2.into_val(&t.env);
    assert_eq!(data.subscription_id, id);
    assert_eq!(data.subscriber, subscriber);
    assert_eq!(data.merchant, merchant);
    assert_eq!(data.token, t.token);
    assert_eq!(data.amount, AMOUNT);
    assert_eq!(data.lifetime_charged, AMOUNT);
    assert_eq!(data.remaining_balance, DEPOSIT - AMOUNT);
}

#[test]
fn test_failed_charge_interval_not_elapsed_no_event() {
    let (t, subscriber, merchant) = setup();
    let id = create_funded_sub(&t, &subscriber, &merchant);
    // Do NOT advance time — interval has not elapsed
    let before = event_count(&t);

    let result = t.client.try_charge_subscription(&id);
    assert!(result.is_err(), "should fail: interval not elapsed");
    assert_eq!(event_count(&t), before, "no event on failed charge");
}

#[test]
fn test_charge_insufficient_balance_emits_charge_failed_event() {
    let (t, subscriber, merchant) = setup();
    // Create subscription with zero balance
    let id = t.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    t.jump(INTERVAL + 1);
    let before = event_count(&t);

    // charge_subscription returns Ok(InsufficientBalance) — not an Err
    t.client.charge_subscription(&id);

    let events = t.env.events().all();
    assert!(
        events.len() > before,
        "charge_failed event must be emitted on insufficient balance"
    );
    // Verify no "charged" event was emitted (only "charge_failed")
    let charged = events.iter().skip(before).any(|ev| {
        ev.1.get(0)
            .map(|v| Symbol::from_val(&t.env, &v) == Symbol::new(&t.env, "charged"))
            .unwrap_or(false)
    });
    assert!(!charged, "no 'charged' event on insufficient balance");
}

// ── SubscriptionPausedEvent / SubscriptionResumedEvent ────────────────────────

#[test]
fn test_pause_subscription_emits_event() {
    let (t, subscriber, merchant) = setup();
    let id = create_funded_sub(&t, &subscriber, &merchant);
    let before = event_count(&t);

    t.client.pause_subscription(&id, &subscriber);

    let events = t.env.events().all();
    assert_eq!(events.len(), before + 1, "exactly one event on pause");

    let ev = events.last().unwrap();
    let topic0 = Symbol::from_val(&t.env, &ev.1.get(0).unwrap());
    assert_eq!(topic0, Symbol::new(&t.env, "sub_paused"));

    let data: SubscriptionPausedEvent = ev.2.into_val(&t.env);
    assert_eq!(data.subscription_id, id);
    assert_eq!(data.subscriber, subscriber);
    assert_eq!(data.merchant, merchant);
    assert_eq!(data.authorizer, subscriber);
}

#[test]
fn test_resume_subscription_emits_event() {
    let (t, subscriber, merchant) = setup();
    let id = create_funded_sub(&t, &subscriber, &merchant);
    t.client.pause_subscription(&id, &subscriber);
    let before = event_count(&t);

    t.client.resume_subscription(&id, &subscriber);

    let events = t.env.events().all();
    assert_eq!(events.len(), before + 1, "exactly one event on resume");

    let ev = events.last().unwrap();
    let topic0 = Symbol::from_val(&t.env, &ev.1.get(0).unwrap());
    assert_eq!(topic0, Symbol::new(&t.env, "sub_resumed"));

    let data: SubscriptionResumedEvent = ev.2.into_val(&t.env);
    assert_eq!(data.subscription_id, id);
    assert_eq!(data.subscriber, subscriber);
    assert_eq!(data.merchant, merchant);
    assert_eq!(data.authorizer, subscriber);
}

#[test]
fn test_failed_pause_unauthorized_no_event() {
    let (t, subscriber, merchant) = setup();
    let id = create_funded_sub(&t, &subscriber, &merchant);
    let intruder = Address::generate(&t.env);
    let before = event_count(&t);

    let result = t.client.try_pause_subscription(&id, &intruder);
    assert!(result.is_err(), "unauthorized pause should fail");
    assert_eq!(event_count(&t), before, "no event on failed pause");
}

#[test]
fn test_failed_resume_unauthorized_no_event() {
    let (t, subscriber, merchant) = setup();
    let id = create_funded_sub(&t, &subscriber, &merchant);
    t.client.pause_subscription(&id, &subscriber);
    let intruder = Address::generate(&t.env);
    let before = event_count(&t);

    let result = t.client.try_resume_subscription(&id, &intruder);
    assert!(result.is_err(), "unauthorized resume should fail");
    assert_eq!(event_count(&t), before, "no event on failed resume");
}

// ── SubscriptionCancelledEvent ────────────────────────────────────────────────

#[test]
fn test_cancel_subscription_emits_event() {
    let (t, subscriber, merchant) = setup();
    let id = create_funded_sub(&t, &subscriber, &merchant);
    let before = event_count(&t);

    t.client.cancel_subscription(&id, &subscriber);

    let events = t.env.events().all();
    assert_eq!(events.len(), before + 1, "exactly one event on cancel");

    let ev = events.last().unwrap();
    let topic0 = Symbol::from_val(&t.env, &ev.1.get(0).unwrap());
    assert_eq!(topic0, Symbol::new(&t.env, "subscription_cancelled"));

    let data: SubscriptionCancelledEvent = ev.2.into_val(&t.env);
    assert_eq!(data.subscription_id, id);
    assert_eq!(data.subscriber, subscriber);
    assert_eq!(data.merchant, merchant);
    assert_eq!(data.token, t.token);
    assert_eq!(data.authorizer, subscriber);
    assert_eq!(data.refund_amount, DEPOSIT);
}

#[test]
fn test_failed_cancel_unauthorized_no_event() {
    let (t, subscriber, merchant) = setup();
    let id = create_funded_sub(&t, &subscriber, &merchant);
    let intruder = Address::generate(&t.env);
    let before = event_count(&t);

    let result = t.client.try_cancel_subscription(&id, &intruder);
    assert!(result.is_err(), "unauthorized cancel should fail");
    assert_eq!(event_count(&t), before, "no event on failed cancel");
}

// ── MerchantWithdrawalEvent ───────────────────────────────────────────────────

#[test]
fn test_merchant_withdrawal_emits_event() {
    let (t, subscriber, merchant) = setup();
    let id = create_funded_sub(&t, &subscriber, &merchant);
    t.jump(INTERVAL + 1);
    t.client.charge_subscription(&id);

    let before = event_count(&t);
    t.client.withdraw_merchant_funds(&merchant, &AMOUNT);

    let events = t.env.events().all();
    assert_eq!(events.len(), before + 1, "exactly one event on withdrawal");

    let ev = events.last().unwrap();
    let topic0 = Symbol::from_val(&t.env, &ev.1.get(0).unwrap());
    assert_eq!(topic0, Symbol::new(&t.env, "withdrawn"));

    let data: MerchantWithdrawalEvent = ev.2.into_val(&t.env);
    assert_eq!(data.merchant, merchant);
    assert_eq!(data.token, t.token);
    assert_eq!(data.amount, AMOUNT);
    assert_eq!(data.remaining_balance, 0);
}

#[test]
fn test_failed_withdrawal_no_balance_no_event() {
    let (t, _subscriber, merchant) = setup();
    let before = event_count(&t);

    let result = t.client.try_withdraw_merchant_funds(&merchant, &AMOUNT);
    assert!(result.is_err(), "withdrawal with no balance should fail");
    assert_eq!(event_count(&t), before, "no event on failed withdrawal");
}

// ── Batch charge event ordering ───────────────────────────────────────────────

#[test]
fn test_batch_charge_emits_one_event_per_success() {
    let (t, subscriber, merchant) = setup();
    t.stellar_token_client().mint(&subscriber, &200_000_000);

    let id0 = create_funded_sub(&t, &subscriber, &merchant);
    let id1 = create_funded_sub(&t, &subscriber, &merchant);
    // id2 has no balance — will fail
    let id2 = t.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    t.jump(INTERVAL + 1);
    let before = event_count(&t);

    t.client.batch_charge(&soroban_sdk::vec![&t.env, id0, id1, id2]);

    let new_events: Vec<_> = t.env.events().all().iter().skip(before).collect();

    // Count "charged" events
    let charged_count = new_events.iter().filter(|ev| {
        ev.1.get(0)
            .map(|v| Symbol::from_val(&t.env, &v) == Symbol::new(&t.env, "charged"))
            .unwrap_or(false)
    }).count();

    assert_eq!(charged_count, 2, "two successful charges → two charged events");
}

#[test]
fn test_batch_charge_empty_no_events() {
    let (t, _subscriber, _merchant) = setup();
    let before = event_count(&t);

    t.client.batch_charge(&soroban_sdk::vec![&t.env]);

    assert_eq!(event_count(&t), before, "empty batch emits no events");
}

#[test]
fn test_batch_charge_event_order_is_deterministic() {
    let (t, subscriber, merchant) = setup();
    t.stellar_token_client().mint(&subscriber, &200_000_000);

    let id0 = create_funded_sub(&t, &subscriber, &merchant);
    let id1 = create_funded_sub(&t, &subscriber, &merchant);

    t.jump(INTERVAL + 1);
    let before = event_count(&t);

    t.client.batch_charge(&soroban_sdk::vec![&t.env, id0, id1]);

    let new_events: Vec<_> = t.env.events().all().iter().skip(before).collect();
    let charged: Vec<_> = new_events.iter().filter(|ev| {
        ev.1.get(0)
            .map(|v| Symbol::from_val(&t.env, &v) == Symbol::new(&t.env, "charged"))
            .unwrap_or(false)
    }).collect();

    assert_eq!(charged.len(), 2);

    // Events must be in the same order as the input IDs
    let first: SubscriptionChargedEvent = charged[0].2.into_val(&t.env);
    let second: SubscriptionChargedEvent = charged[1].2.into_val(&t.env);
    assert_eq!(first.subscription_id, id0);
    assert_eq!(second.subscription_id, id1);
}

// ── Full lifecycle sequence ───────────────────────────────────────────────────

#[test]
fn test_lifecycle_event_sequence() {
    let (t, subscriber, merchant) = setup();

    // 1. create
    let id = t.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    // 2. deposit
    t.client.deposit_funds(&id, &subscriber, &DEPOSIT);
    // 3. charge
    t.jump(INTERVAL + 1);
    t.client.charge_subscription(&id);
    // 4. pause
    t.client.pause_subscription(&id, &subscriber);
    // 5. resume
    t.client.resume_subscription(&id, &subscriber);
    // 6. cancel
    t.client.cancel_subscription(&id, &subscriber);

    let events = t.env.events().all();

    // Collect topic[0] symbols for all events from this contract
    let topics: Vec<Symbol> = events
        .iter()
        .filter(|ev| ev.0 == t.client.address)
        .filter_map(|ev| ev.1.get(0).map(|v| Symbol::from_val(&t.env, &v)))
        .collect();

    // Verify each expected event type appears at least once
    let has = |name: &str| topics.iter().any(|s| *s == Symbol::new(&t.env, name));
    assert!(has("created"), "missing created event");
    assert!(has("deposited"), "missing deposited event");
    assert!(has("charged"), "missing charged event");
    assert!(has("sub_paused"), "missing sub_paused event");
    assert!(has("sub_resumed"), "missing sub_resumed event");
    assert!(has("subscription_cancelled"), "missing subscription_cancelled event");
}

// ── Security: events must not leak metadata ───────────────────────────────────

#[test]
fn test_events_do_not_contain_metadata_values() {
    let (t, subscriber, merchant) = setup();
    let id = create_funded_sub(&t, &subscriber, &merchant);

    // Set metadata with a sensitive value
    t.client.set_metadata(
        &id,
        &subscriber,
        &soroban_sdk::String::from_str(&t.env, "secret_key"),
        &soroban_sdk::String::from_str(&t.env, "sensitive_value_12345"),
    );

    // The metadata_set event should only contain key + authorizer, not the value
    let events = t.env.events().all();
    let meta_ev = events.iter().find(|ev| {
        ev.1.get(0)
            .map(|v| Symbol::from_val(&t.env, &v) == Symbol::new(&t.env, "metadata_set"))
            .unwrap_or(false)
    });
    assert!(meta_ev.is_some(), "metadata_set event must be emitted");

    let data: crate::MetadataSetEvent = meta_ev.unwrap().2.into_val(&t.env);
    assert_eq!(data.subscription_id, id);
    assert_eq!(data.key, soroban_sdk::String::from_str(&t.env, "secret_key"));
    // authorizer is present (not sensitive)
    assert_eq!(data.authorizer, subscriber);
    // The event struct has no `value` field — metadata values are never emitted
}

// ── Admin events ──────────────────────────────────────────────────────────────

#[test]
fn test_admin_rotation_emits_event() {
    let (t, _subscriber, _merchant) = setup();
    let new_admin = Address::generate(&t.env);
    let before = event_count(&t);

    t.client.rotate_admin(&t.admin, &new_admin);

    let events = t.env.events().all();
    assert_eq!(events.len(), before + 1);

    let ev = events.last().unwrap();
    let topic0 = Symbol::from_val(&t.env, &ev.1.get(0).unwrap());
    assert_eq!(topic0, Symbol::new(&t.env, "admin_rotated"));

    let data: crate::AdminRotatedEvent = ev.2.into_val(&t.env);
    assert_eq!(data.old_admin, t.admin);
    assert_eq!(data.new_admin, new_admin);
}

#[test]
fn test_emergency_stop_events() {
    let (t, _subscriber, _merchant) = setup();

    let before = event_count(&t);
    t.client.enable_emergency_stop(&t.admin);
    assert_eq!(event_count(&t), before + 1, "enable emits one event");

    let before2 = event_count(&t);
    t.client.disable_emergency_stop(&t.admin);
    assert_eq!(event_count(&t), before2 + 1, "disable emits one event");
}

#[test]
fn test_emergency_stop_idempotent_no_duplicate_events() {
    let (t, _subscriber, _merchant) = setup();
    t.client.enable_emergency_stop(&t.admin);
    let before = event_count(&t);

    // Calling enable again when already enabled should be a no-op
    t.client.enable_emergency_stop(&t.admin);
    assert_eq!(event_count(&t), before, "idempotent enable emits no extra event");
}
