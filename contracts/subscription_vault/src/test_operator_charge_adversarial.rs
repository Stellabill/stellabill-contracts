#![cfg(test)]

//! Adversarial coverage for `do_operator_charge_subscription`
//! (`contracts/subscription_vault/src/lib.rs`, re-exported through the
//! `operator_charge_subscription` entrypoint).
//!
//! The existing suite checks the sunny path, that a stranger is rejected, and
//! that removal/emergency-stop blocks charging. These tests fill the gaps that
//! really decide whether the operator charge path is safe:
//!
//! * no operator configured at all ⇒ `Unauthorized`,
//! * an unknown subscription ⇒ `NotFound` rather than a silent success,
//! * the interval guard fires one second *before* the boundary and is `Err`,
//!   while the exact boundary charges exactly once,
//! * a second charge in the same period is `Replay`-protected,
//! * an underfunded subscription yields the `InsufficientBalance` *result*
//!   (not an error), so batch callers can keep going,
//! * rejected charges leave balance, `last_payment_timestamp`, and status
//!   untouched, and an unauthorized caller emits no event at all,
//! * the module helper and the public entrypoint agree.

extern crate std;

use crate::test_utils::setup::TestEnv;
use crate::{ChargeExecutionResult, Error, SubscriptionStatus};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    Address,
};

// ── Shared constants ──────────────────────────────────────────────────────────

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const DEPOSIT: i128 = 25_000_000; // enough for two intervals

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_subscription(te: &TestEnv, subscriber: &Address, merchant: &Address) -> u32 {
    te.client.create_subscription(
        subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &false, // usage_enabled
        &None,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    )
}

fn make_funded_subscription(
    te: &TestEnv,
    subscriber: &Address,
    merchant: &Address,
    deposit: i128,
) -> u32 {
    let sub_id = make_subscription(te, subscriber, merchant);
    te.stellar_token_client().mint(subscriber, &deposit);
    te.client
        .deposit_funds(&sub_id, subscriber, &deposit, &None::<soroban_sdk::BytesN<32>>);
    sub_id
}

// ── Authorization ─────────────────────────────────────────────────────────────

#[test]
fn operator_charge_without_a_configured_operator_is_unauthorized() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let caller = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant, DEPOSIT);
    te.jump(INTERVAL + 1);

    assert_eq!(te.client.get_operator(), None);
    let before = te.client.get_subscription(&sub_id);
    let res = te
        .client
        .try_operator_charge_subscription(&caller, &sub_id);
    assert_eq!(
        res.err().unwrap().unwrap().to_code(),
        Error::Unauthorized.to_code()
    );

    let after = te.client.get_subscription(&sub_id);
    assert_eq!(after.prepaid_balance, DEPOSIT);
    assert_eq!(after.last_payment_timestamp, before.last_payment_timestamp);
}

#[test]
fn unauthorized_operator_charge_emits_no_charge_event() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant, DEPOSIT);
    te.client.set_operator(&te.admin, &operator);
    te.jump(INTERVAL + 1);

    let before = te.env.events().all().len();
    let res = te
        .client
        .try_operator_charge_subscription(&stranger, &sub_id);
    assert!(res.is_err());
    // Rejected at the auth gate, before `charge_one` can publish
    // `charge_failed_v2`.
    assert_eq!(te.env.events().all().len(), before);
}

#[test]
fn operator_replacement_revokes_the_previous_operator() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let old_operator = Address::generate(&te.env);
    let new_operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant, DEPOSIT);
    te.client.set_operator(&te.admin, &old_operator);

    // Respect the config cooldown between operator writes.
    te.env.ledger().with_mut(|li| {
        li.timestamp += crate::admin::CONFIG_COOLDOWN_SECS
    });
    te.client.set_operator(&te.admin, &new_operator);

    te.jump(INTERVAL + 1);

    let old_res = te
        .client
        .try_operator_charge_subscription(&old_operator, &sub_id);
    assert_eq!(
        old_res.err().unwrap().unwrap().to_code(),
        Error::Unauthorized.to_code()
    );
    assert_eq!(te.client.get_subscription(&sub_id).prepaid_balance, DEPOSIT);

    let new_res = te.client.operator_charge_subscription(&new_operator, &sub_id);
    assert_eq!(new_res as u32, ChargeExecutionResult::Charged as u32);
}

// ── Unknown subscription ──────────────────────────────────────────────────────

#[test]
fn operator_charge_for_an_unknown_subscription_is_not_found() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);

    te.client.set_operator(&te.admin, &operator);
    te.jump(INTERVAL + 1);

    let res = te
        .client
        .try_operator_charge_subscription(&operator, &9_999);
    assert_eq!(
        res.err().unwrap().unwrap().to_code(),
        Error::NotFound.to_code()
    );
}

// ── Interval guard and its boundary ───────────────────────────────────────────

#[test]
fn operator_charge_one_second_before_the_interval_is_interval_not_elapsed() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant, DEPOSIT);
    te.client.set_operator(&te.admin, &operator);

    te.jump(INTERVAL - 1);
    let before = te.client.get_subscription(&sub_id);

    let res = te
        .client
        .try_operator_charge_subscription(&operator, &sub_id);
    assert_eq!(
        res.err().unwrap().unwrap().to_code(),
        Error::IntervalNotElapsed.to_code()
    );

    let after = te.client.get_subscription(&sub_id);
    assert_eq!(after.prepaid_balance, before.prepaid_balance);
    assert_eq!(after.last_payment_timestamp, before.last_payment_timestamp);
    assert_eq!(after.status, before.status);
}

#[test]
fn operator_charge_at_the_exact_interval_boundary_succeeds_once() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant, DEPOSIT);
    te.client.set_operator(&te.admin, &operator);

    // `jump(INTERVAL - 1)` must fail, `jump(+1)` must succeed: this is the
    // off-by-one boundary the guard is supposed to honour.
    te.jump(INTERVAL - 1);
    assert!(te
        .client
        .try_operator_charge_subscription(&operator, &sub_id)
        .is_err());

    te.jump(1);
    let result = te.client.operator_charge_subscription(&operator, &sub_id);
    assert_eq!(result as u32, ChargeExecutionResult::Charged as u32);

    let sub = te.client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, DEPOSIT - AMOUNT);
    assert_eq!(sub.last_payment_timestamp, INTERVAL);
    assert_eq!(sub.lifetime_charged, AMOUNT);
}

// ── Replay protection ─────────────────────────────────────────────────────────

#[test]
fn operator_charge_twice_in_the_same_period_is_replay_protected() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant, DEPOSIT);
    te.client.set_operator(&te.admin, &operator);
    te.jump(INTERVAL + 1);

    let first = te.client.operator_charge_subscription(&operator, &sub_id);
    assert_eq!(first as u32, ChargeExecutionResult::Charged as u32);
    let after_first = te.client.get_subscription(&sub_id);

    let second = te
        .client
        .try_operator_charge_subscription(&operator, &sub_id);
    assert_eq!(
        second.err().unwrap().unwrap().to_code(),
        Error::Replay.to_code()
    );

    let after_second = te.client.get_subscription(&sub_id);
    assert_eq!(after_second.prepaid_balance, after_first.prepaid_balance);
    assert_eq!(
        after_second.last_payment_timestamp,
        after_first.last_payment_timestamp
    );
    assert_eq!(after_second.lifetime_charged, after_first.lifetime_charged);
}

// ── Underfunded subscription ──────────────────────────────────────────────────

#[test]
fn operator_charge_with_insufficient_balance_returns_the_result_not_an_error() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    // The contract's configured min top-up is 1_000_000, so this deposit is
    // accepted but is far short of one `AMOUNT` charge.
    let sub_id = make_funded_subscription(&te, &subscriber, &merchant, 1_000_000);
    te.client.set_operator(&te.admin, &operator);
    te.jump(INTERVAL + 1);

    let result = te.client.operator_charge_subscription(&operator, &sub_id);
    assert_eq!(
        result as u32,
        ChargeExecutionResult::InsufficientBalance as u32,
        "an underfunded charge must be reported as a result so batch callers continue"
    );

    let sub = te.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::InsufficientBalance);
    assert_eq!(sub.prepaid_balance, 1_000_000, "nothing may be debited");
}

// ── Helper / entrypoint parity ────────────────────────────────────────────────

#[test]
fn do_operator_charge_subscription_matches_the_public_entrypoint() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);
    te.jump(INTERVAL + 1);

    // Route A: the public contract entrypoint.
    let sub_a = make_funded_subscription(
        &te,
        &Address::generate(&te.env),
        &Address::generate(&te.env),
        DEPOSIT,
    );
    let via_entrypoint = te.client.operator_charge_subscription(&operator, &sub_a);

    // Route B: the module helper invoked directly inside the contract context.
    let sub_b = make_funded_subscription(
        &te,
        &Address::generate(&te.env),
        &Address::generate(&te.env),
        DEPOSIT,
    );
    let via_helper = te.env.as_contract(&te.client.address, || {
        crate::operator::do_operator_charge_subscription(&te.env, operator.clone(), sub_b)
    });

    assert_eq!(via_entrypoint as u32, ChargeExecutionResult::Charged as u32);
    assert_eq!(via_helper.unwrap() as u32, ChargeExecutionResult::Charged as u32);
    assert_eq!(
        te.client.get_subscription(&sub_a).prepaid_balance,
        te.client.get_subscription(&sub_b).prepaid_balance
    );
}
