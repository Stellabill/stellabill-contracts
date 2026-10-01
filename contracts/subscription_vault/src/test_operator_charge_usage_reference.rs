//! Adversarial coverage for `operator::do_operator_charge_usage_with_reference`
//! (issue #1085).
//!
//! The helper is the only path that charges metered usage *with* a caller
//! supplied reference, and the reference doubles as the per-subscription
//! idempotency key. The tests below pin:
//!
//! - authorization: the stored operator is required, and a rejected call must
//!   not debit the subscriber or burn the reference for a later valid call;
//! - boundary `usage_amount` values (`0`, negative, exactly the prepaid
//!   balance, one unit above it);
//! - reference semantics: replay protection per subscription, scope across
//!   subscriptions, and the empty-string boundary;
//! - subscription-state guards (`usage_enabled`, `Active`, unknown id,
//!   emergency stop) leaving balances untouched.

#![cfg(test)]

extern crate std;

use crate::test_utils::setup::TestEnv;
use crate::{Error, UsageChargeResult, UsageStatementEvent};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    Address, BytesN, IntoVal, String, Symbol,
};

// ── Shared constants ──────────────────────────────────────────────────────────

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const DEPOSIT: i128 = 25_000_000;
const USAGE: i128 = 1_000_000;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_subscription(
    te: &TestEnv,
    subscriber: &Address,
    merchant: &Address,
    usage_enabled: bool,
) -> u32 {
    let sub_id = te.client.create_subscription(
        subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &usage_enabled,
        &None,
        &None::<u64>,
        &None::<u32>,
        &None::<Symbol>,
    );
    te.stellar_token_client().mint(subscriber, &DEPOSIT);
    te.client
        .deposit_funds(&sub_id, subscriber, &DEPOSIT, &None::<BytesN<32>>);
    sub_id
}

fn reference(te: &TestEnv, value: &str) -> String {
    String::from_str(&te.env, value)
}

fn balance(te: &TestEnv, sub_id: u32) -> i128 {
    te.client.get_subscription(&sub_id).prepaid_balance
}

/// Invoke the helper named by issue #1085 directly, inside the contract
/// context, so its own guard is exercised rather than the public wrapper's.
fn call_helper(
    te: &TestEnv,
    op: &Address,
    sub_id: u32,
    usage_amount: i128,
    reference: String,
) -> Result<UsageChargeResult, Error> {
    let contract = te.client.address.clone();
    te.env.as_contract(&contract, || {
        crate::operator::do_operator_charge_usage_with_reference(
            &te.env,
            op.clone(),
            sub_id,
            usage_amount,
            reference,
        )
    })
}

// ── Authorization ─────────────────────────────────────────────────────────────

#[test]
fn helper_rejects_a_caller_that_is_not_the_stored_operator() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);

    let sub_id = make_subscription(&te, &subscriber, &merchant, true);
    te.client.set_operator(&te.admin, &operator);

    let result = call_helper(&te, &stranger, sub_id, USAGE, reference(&te, "inv-1"));

    assert_eq!(result, Err(Error::Unauthorized));
    assert_eq!(balance(&te, sub_id), DEPOSIT);

    // The rejected call must not burn the reference, and it must not have
    // debited anything: the same reference still charges for the real operator.
    let accepted = te
        .client
        .operator_charge_usage_with_ref(&operator, &sub_id, &USAGE, &reference(&te, "inv-1"));
    assert_eq!(accepted, UsageChargeResult::Charged);
    assert_eq!(balance(&te, sub_id), DEPOSIT - USAGE);
}

#[test]
fn helper_rejects_every_caller_when_no_operator_is_configured() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let caller = Address::generate(&te.env);

    let sub_id = make_subscription(&te, &subscriber, &merchant, true);
    // No `set_operator` call at all.

    let result = call_helper(&te, &caller, sub_id, USAGE, reference(&te, "inv-2"));

    assert_eq!(result, Err(Error::Unauthorized));
    assert_eq!(balance(&te, sub_id), DEPOSIT);
}

#[test]
fn public_entry_rejects_a_caller_that_is_not_the_stored_operator() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);

    let sub_id = make_subscription(&te, &subscriber, &merchant, true);
    te.client.set_operator(&te.admin, &operator);

    let result = te.client.try_operator_charge_usage_with_ref(
        &stranger,
        &sub_id,
        &USAGE,
        &reference(&te, "inv-3"),
    );

    assert!(result.is_err(), "a non-operator must not charge usage");
    assert_eq!(balance(&te, sub_id), DEPOSIT);
}

// ── `usage_amount` boundaries ─────────────────────────────────────────────────

#[test]
fn helper_rejects_zero_and_negative_usage_amounts_without_debiting() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_subscription(&te, &subscriber, &merchant, true);
    te.client.set_operator(&te.admin, &operator);

    for amount in [0i128, -1i128, i128::MIN] {
        let result = call_helper(
            &te,
            &operator,
            sub_id,
            amount,
            reference(&te, "zero-or-negative"),
        );
        assert_eq!(
            result,
            Err(Error::InvalidAmount),
            "usage amount {amount} must be rejected",
        );
        assert_eq!(balance(&te, sub_id), DEPOSIT);
    }

    // None of the rejected attempts consumed the reference.
    let accepted = te.client.operator_charge_usage_with_ref(
        &operator,
        &sub_id,
        &USAGE,
        &reference(&te, "zero-or-negative"),
    );
    assert_eq!(accepted, UsageChargeResult::Charged);
    assert_eq!(balance(&te, sub_id), DEPOSIT - USAGE);
}

#[test]
fn usage_charge_accepts_exactly_the_prepaid_balance_and_then_blocks_more() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_subscription(&te, &subscriber, &merchant, true);
    te.client.set_operator(&te.admin, &operator);

    // Boundary: debiting the whole prepaid balance is allowed.
    let drained = te.client.operator_charge_usage_with_ref(
        &operator,
        &sub_id,
        &DEPOSIT,
        &reference(&te, "drain"),
    );
    assert_eq!(drained, UsageChargeResult::Charged);
    assert_eq!(balance(&te, sub_id), 0);

    // One unit past the balance is rejected, and the reference stays unused.
    let over = te.client.try_operator_charge_usage_with_ref(
        &operator,
        &sub_id,
        &1i128,
        &reference(&te, "over"),
    );
    assert!(over.is_err());
    assert_eq!(balance(&te, sub_id), 0);
    assert_eq!(te.client.get_operator_nonce(&operator), 0u64);
}

#[test]
fn helper_rejects_usage_above_the_prepaid_balance_and_keeps_the_reference() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_subscription(&te, &subscriber, &merchant, true);
    te.client.set_operator(&te.admin, &operator);

    let result = call_helper(
        &te,
        &operator,
        sub_id,
        DEPOSIT + 1,
        reference(&te, "too-much"),
    );
    assert_eq!(result, Err(Error::InsufficientPrepaidBalance));
    assert_eq!(balance(&te, sub_id), DEPOSIT);

    // A rejected over-balance attempt must not be recorded as a replay.
    let accepted = te.client.operator_charge_usage_with_ref(
        &operator,
        &sub_id,
        &USAGE,
        &reference(&te, "too-much"),
    );
    assert_eq!(accepted, UsageChargeResult::Charged);
    assert_eq!(balance(&te, sub_id), DEPOSIT - USAGE);
}

// ── Reference semantics ───────────────────────────────────────────────────────

#[test]
fn usage_charge_replays_when_the_reference_is_reused_for_the_same_subscription() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_subscription(&te, &subscriber, &merchant, true);
    te.client.set_operator(&te.admin, &operator);

    let first = te.client.operator_charge_usage_with_ref(
        &operator,
        &sub_id,
        &USAGE,
        &reference(&te, "inv-replay"),
    );
    assert_eq!(first, UsageChargeResult::Charged);
    assert_eq!(balance(&te, sub_id), DEPOSIT - USAGE);

    // Replaying the exact reference must be a no-op, not a second debit.
    let second = te.client.operator_charge_usage_with_ref(
        &operator,
        &sub_id,
        &USAGE,
        &reference(&te, "inv-replay"),
    );
    assert_eq!(second, UsageChargeResult::Replay);
    assert_eq!(balance(&te, sub_id), DEPOSIT - USAGE);

    // A fresh reference charges again.
    let third = te.client.operator_charge_usage_with_ref(
        &operator,
        &sub_id,
        &USAGE,
        &reference(&te, "inv-fresh"),
    );
    assert_eq!(third, UsageChargeResult::Charged);
    assert_eq!(balance(&te, sub_id), DEPOSIT - 2 * USAGE);
}

#[test]
fn usage_charge_reference_is_scoped_to_its_subscription() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let first_sub = make_subscription(&te, &subscriber, &merchant, true);
    let second_sub = make_subscription(&te, &subscriber, &merchant, true);
    te.client.set_operator(&te.admin, &operator);

    let shared = reference(&te, "shared-invoice");

    let first = te
        .client
        .operator_charge_usage_with_ref(&operator, &first_sub, &USAGE, &shared);
    let second = te
        .client
        .operator_charge_usage_with_ref(&operator, &second_sub, &USAGE, &shared);

    // The idempotency key is `(subscription_id, reference)`, so one reference
    // must not block a charge on a different subscription.
    assert_eq!(first, UsageChargeResult::Charged);
    assert_eq!(second, UsageChargeResult::Charged);
    assert_eq!(balance(&te, first_sub), DEPOSIT - USAGE);
    assert_eq!(balance(&te, second_sub), DEPOSIT - USAGE);
}

#[test]
fn usage_charge_treats_an_empty_reference_as_a_sticky_key() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_subscription(&te, &subscriber, &merchant, true);
    te.client.set_operator(&te.admin, &operator);

    let empty = reference(&te, "");

    let first =
        te.client
            .operator_charge_usage_with_ref(&operator, &sub_id, &USAGE, &empty);
    assert_eq!(first, UsageChargeResult::Charged);

    // Documented boundary: an empty reference is a valid key, so the second
    // call with it is a replay rather than a second charge.
    let second =
        te.client
            .operator_charge_usage_with_ref(&operator, &sub_id, &USAGE, &empty);
    assert_eq!(second, UsageChargeResult::Replay);
    assert_eq!(balance(&te, sub_id), DEPOSIT - USAGE);
}

#[test]
fn usage_charge_surfaces_the_reference_on_the_charged_event() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_subscription(&te, &subscriber, &merchant, true);
    te.client.set_operator(&te.admin, &operator);
    te.env.ledger().with_mut(|li| li.timestamp = 5_000);

    let invoice = reference(&te, "inv-event-1");
    te.client
        .operator_charge_usage_with_ref(&operator, &sub_id, &USAGE, &invoice);

    let events = te.env.events().all();
    // The success path publishes `usage_charged` last.
    let (_, _, data) = events.last().expect("no events emitted");
    let charged: UsageStatementEvent = data.into_val(&te.env);

    assert_eq!(charged.subscription_id, sub_id);
    assert_eq!(charged.usage_amount, USAGE);
    assert_eq!(charged.reference, invoice);
    assert_eq!(charged.merchant, merchant);
}

// ── Subscription-state guards ─────────────────────────────────────────────────

#[test]
fn helper_rejects_usage_when_the_subscription_has_usage_disabled() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_subscription(&te, &subscriber, &merchant, false);
    te.client.set_operator(&te.admin, &operator);

    let result = call_helper(&te, &operator, sub_id, USAGE, reference(&te, "no-usage"));

    assert_eq!(result, Err(Error::UsageNotEnabled));
    assert_eq!(balance(&te, sub_id), DEPOSIT);
}

#[test]
fn helper_rejects_an_unknown_subscription_id_without_touching_real_state() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_subscription(&te, &subscriber, &merchant, true);
    te.client.set_operator(&te.admin, &operator);

    let result = call_helper(&te, &operator, u32::MAX, USAGE, reference(&te, "ghost"));

    assert!(result.is_err(), "u32::MAX must not resolve to a subscription");
    assert_eq!(balance(&te, sub_id), DEPOSIT);

    // The failed lookup must not have consumed the reference for real charges.
    let accepted = te
        .client
        .operator_charge_usage_with_ref(&operator, &sub_id, &USAGE, &reference(&te, "ghost"));
    assert_eq!(accepted, UsageChargeResult::Charged);
}

#[test]
fn helper_rejects_usage_on_a_paused_subscription_and_leaves_the_balance() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_subscription(&te, &subscriber, &merchant, true);
    te.client.set_operator(&te.admin, &operator);
    te.client.pause_subscription(&sub_id, &subscriber);

    let result = call_helper(&te, &operator, sub_id, USAGE, reference(&te, "paused"));

    assert_eq!(result, Err(Error::NotActive));
    assert_eq!(balance(&te, sub_id), DEPOSIT);

    // Resuming restores the ability to charge with the same reference.
    te.client.resume_subscription(&sub_id, &subscriber);
    let accepted = te
        .client
        .operator_charge_usage_with_ref(&operator, &sub_id, &USAGE, &reference(&te, "paused"));
    assert_eq!(accepted, UsageChargeResult::Charged);
    assert_eq!(balance(&te, sub_id), DEPOSIT - USAGE);
}

#[test]
fn public_entry_is_blocked_by_the_emergency_stop_and_recovers_afterwards() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_subscription(&te, &subscriber, &merchant, true);
    te.client.set_operator(&te.admin, &operator);
    te.client.enable_emergency_stop(&te.admin);

    let blocked = te.client.try_operator_charge_usage_with_ref(
        &operator,
        &sub_id,
        &USAGE,
        &reference(&te, "stopped"),
    );
    assert!(
        blocked == Err(Ok(Error::EmergencyStopActive)),
        "emergency stop must block the public entry",
    );
    assert_eq!(balance(&te, sub_id), DEPOSIT);

    te.client.disable_emergency_stop(&te.admin);

    let accepted = te
        .client
        .operator_charge_usage_with_ref(&operator, &sub_id, &USAGE, &reference(&te, "stopped"));
    assert_eq!(accepted, UsageChargeResult::Charged);
    assert_eq!(balance(&te, sub_id), DEPOSIT - USAGE);
}
