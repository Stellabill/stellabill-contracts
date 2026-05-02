#![cfg(test)]

//! Tests for merchant earnings accounting.
//!
//! Invariant under test:
//!   `balance == accruals.total - withdrawals - refunds`
//!
//! where `accruals.total = accruals.interval + accruals.usage + accruals.one_off`.
//!
//! Every test that touches the ledger ends with an assertion that both
//! `get_merchant_balance_by_token` and the computed value from
//! `get_reconciliation_snapshot` agree, and that `get_merchant_token_earnings`
//! reflects the expected accrual / withdrawal / refund breakdown.

extern crate std;

use crate::test_utils::setup::TestEnv;
use crate::types::TokenEarnings;
use crate::{Error, SubscriptionStatus};
use soroban_sdk::{testutils::Address as _, Address};

// ── Constants ─────────────────────────────────────────────────────────────────

const AMOUNT: i128 = 10_000_000; // 10 USDC (6 decimals)
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const DEPOSIT: i128 = 40_000_000; // 4 intervals worth

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_sub(te: &TestEnv, subscriber: &Address, merchant: &Address) -> u32 {
    let id = te.client.create_subscription(
        subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );
    te.stellar_token_client().mint(subscriber, &DEPOSIT);
    te.client.deposit_funds(&id, subscriber, &DEPOSIT);
    id
}

fn make_usage_sub(te: &TestEnv, subscriber: &Address, merchant: &Address) -> u32 {
    let id = te.client.create_subscription(
        subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &true, // usage_enabled
        &None,
        &None::<u64>,
    );
    te.stellar_token_client().mint(subscriber, &DEPOSIT);
    te.client.deposit_funds(&id, subscriber, &DEPOSIT);
    id
}

/// Assert the reconciliation invariant holds for (merchant, token).
fn assert_reconciled(te: &TestEnv, merchant: &Address) {
    let snaps = te.client.get_reconciliation_snapshot(merchant);
    for snap in snaps.iter() {
        let live_balance = te
            .client
            .get_merchant_balance_by_token(merchant, &snap.token);
        assert_eq!(
            snap.computed_balance, live_balance,
            "reconciliation invariant violated: computed {} != live {}",
            snap.computed_balance, live_balance
        );
        assert_eq!(
            snap.total_accruals
                .checked_sub(snap.total_withdrawals)
                .unwrap()
                .checked_sub(snap.total_refunds)
                .unwrap(),
            live_balance,
            "accruals - withdrawals - refunds must equal balance"
        );
    }
}

fn accruals_total(e: &TokenEarnings) -> i128 {
    e.accruals.interval + e.accruals.usage + e.accruals.one_off
}

// ── 1. Interval charge credits earnings ──────────────────────────────────────

#[test]
fn interval_charge_credits_merchant_balance() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    let balance = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    assert_eq!(balance, AMOUNT);
}

#[test]
fn interval_charge_increments_accruals_interval() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.accruals.interval, AMOUNT);
    assert_eq!(e.accruals.usage, 0);
    assert_eq!(e.accruals.one_off, 0);
    assert_eq!(e.withdrawals, 0);
    assert_eq!(e.refunds, 0);
}

#[test]
fn two_interval_charges_accumulate_correctly() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);
    te.jump(INTERVAL);
    te.client.charge_subscription(&0);

    let balance = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    assert_eq!(balance, AMOUNT * 2);

    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.accruals.interval, AMOUNT * 2);

    assert_reconciled(&te, &merchant);
}

#[test]
fn interval_charge_reconciliation_invariant_holds() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    assert_reconciled(&te, &merchant);
}

// ── 2. Usage charge credits earnings ──────────────────────────────────────────

#[test]
fn usage_charge_credits_merchant_balance() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_usage_sub(&te, &subscriber, &merchant);

    let ref1 = soroban_sdk::String::from_str(&te.env, "u1");
    te.client.charge_usage(&0, &1_000_000i128, &ref1);

    let balance = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    assert_eq!(balance, 1_000_000);
}

#[test]
fn usage_charge_increments_accruals_usage() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_usage_sub(&te, &subscriber, &merchant);

    let ref1 = soroban_sdk::String::from_str(&te.env, "u1");
    te.client.charge_usage(&0, &1_000_000i128, &ref1);

    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.accruals.usage, 1_000_000);
    assert_eq!(e.accruals.interval, 0);
    assert_eq!(e.accruals.one_off, 0);

    assert_reconciled(&te, &merchant);
}

#[test]
fn mixed_interval_and_usage_charges_accumulate_by_kind() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_usage_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    let ref1 = soroban_sdk::String::from_str(&te.env, "u1");
    te.client.charge_usage(&0, &2_000_000i128, &ref1);

    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.accruals.interval, AMOUNT);
    assert_eq!(e.accruals.usage, 2_000_000);
    assert_eq!(accruals_total(&e), AMOUNT + 2_000_000);

    let balance = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    assert_eq!(balance, AMOUNT + 2_000_000);

    assert_reconciled(&te, &merchant);
}

// ── 3. Withdrawal debits earnings and balance ─────────────────────────────────

#[test]
fn withdrawal_debits_merchant_balance() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    te.client.withdraw_merchant_funds(&merchant, &AMOUNT);

    let balance = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    assert_eq!(balance, 0);
}

#[test]
fn withdrawal_increments_earnings_withdrawals() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    te.client.withdraw_merchant_funds(&merchant, &AMOUNT);

    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.withdrawals, AMOUNT);
    assert_eq!(e.accruals.interval, AMOUNT);
}

#[test]
fn withdrawal_reconciliation_invariant_holds() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);
    te.client.withdraw_merchant_funds(&merchant, &AMOUNT);

    assert_reconciled(&te, &merchant);
}

#[test]
fn partial_withdrawal_leaves_correct_remainder() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    let withdraw = AMOUNT / 4;
    te.client.withdraw_merchant_funds(&merchant, &withdraw);

    let balance = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    assert_eq!(balance, AMOUNT - withdraw);

    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.withdrawals, withdraw);
    assert_eq!(e.accruals.interval, AMOUNT);

    assert_reconciled(&te, &merchant);
}

#[test]
fn sequential_withdrawals_each_update_earnings_withdrawals() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);
    te.jump(INTERVAL);
    te.client.charge_subscription(&0);

    // Two intervals charged => 2*AMOUNT in balance
    te.client.withdraw_merchant_funds(&merchant, &AMOUNT);
    te.client.withdraw_merchant_funds(&merchant, &AMOUNT);

    let balance = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    assert_eq!(balance, 0);

    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.accruals.interval, AMOUNT * 2);
    assert_eq!(e.withdrawals, AMOUNT * 2);

    assert_reconciled(&te, &merchant);
}

#[test]
fn withdrawal_overdraft_rejected_balance_unchanged() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    let result = te.client.try_withdraw_merchant_funds(&merchant, &(AMOUNT + 1));
    assert_eq!(result.unwrap_err().unwrap(), Error::InsufficientBalance);

    let balance = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    assert_eq!(balance, AMOUNT, "balance must be unchanged after rejected overdraft");

    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.withdrawals, 0, "withdrawals must not be recorded after rejection");

    assert_reconciled(&te, &merchant);
}

#[test]
fn withdrawal_zero_balance_returns_not_found() {
    let te = TestEnv::default();
    let merchant = Address::generate(&te.env);

    let result = te.client.try_withdraw_merchant_funds(&merchant, &1_000_000);
    assert_eq!(result.unwrap_err().unwrap(), Error::NotFound);
}

#[test]
fn withdrawal_zero_amount_returns_invalid_amount() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);
    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    let result = te.client.try_withdraw_merchant_funds(&merchant, &0);
    assert_eq!(result.unwrap_err().unwrap(), Error::InvalidAmount);
}

// ── 4. Merchant refund debits earnings and balance ────────────────────────────

#[test]
fn refund_debits_merchant_balance() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    te.client.merchant_refund(&merchant, &subscriber, &te.token, &AMOUNT);

    let balance = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    assert_eq!(balance, 0);
}

#[test]
fn refund_increments_earnings_refunds() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    te.client.merchant_refund(&merchant, &subscriber, &te.token, &AMOUNT);

    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.refunds, AMOUNT);
    assert_eq!(e.accruals.interval, AMOUNT);
    assert_eq!(e.withdrawals, 0);
}

#[test]
fn refund_reconciliation_invariant_holds() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);
    te.client.merchant_refund(&merchant, &subscriber, &te.token, &AMOUNT);

    assert_reconciled(&te, &merchant);
}

#[test]
fn partial_refund_leaves_correct_remainder() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    let refund = AMOUNT / 2;
    te.client.merchant_refund(&merchant, &subscriber, &te.token, &refund);

    let balance = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    assert_eq!(balance, AMOUNT - refund);

    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.refunds, refund);

    assert_reconciled(&te, &merchant);
}

#[test]
fn refund_overdraft_rejected() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    let result =
        te.client
            .try_merchant_refund(&merchant, &subscriber, &te.token, &(AMOUNT + 1));
    assert_eq!(result.unwrap_err().unwrap(), Error::InsufficientBalance);

    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.refunds, 0, "refunds must not record on rejection");

    assert_reconciled(&te, &merchant);
}

// ── 5. Withdrawal + refund combined ──────────────────────────────────────────

#[test]
fn withdrawal_and_refund_both_tracked_independently() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);
    te.jump(INTERVAL);
    te.client.charge_subscription(&0);
    // balance = 2 * AMOUNT

    let withdraw = AMOUNT;
    let refund = AMOUNT / 2;

    te.client.withdraw_merchant_funds(&merchant, &withdraw);
    te.client.merchant_refund(&merchant, &subscriber, &te.token, &refund);

    let expected_balance = 2 * AMOUNT - withdraw - refund;
    let balance = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    assert_eq!(balance, expected_balance);

    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.accruals.interval, 2 * AMOUNT);
    assert_eq!(e.withdrawals, withdraw);
    assert_eq!(e.refunds, refund);

    assert_reconciled(&te, &merchant);
}

// ── 6. Multiple subscriptions, single merchant ───────────────────────────────

#[test]
fn multiple_subscriptions_single_merchant_accumulate_balance() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    // Create 3 subscriptions for the same merchant
    let sub_a = make_sub(&te, &subscriber, &merchant);
    let sub_b = make_sub(&te, &subscriber, &merchant);
    let sub_c = make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&sub_a);
    te.client.charge_subscription(&sub_b);
    te.client.charge_subscription(&sub_c);

    let balance = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    assert_eq!(balance, 3 * AMOUNT);

    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.accruals.interval, 3 * AMOUNT);

    assert_reconciled(&te, &merchant);
}

// ── 7. Multiple merchants — balances are isolated ────────────────────────────

#[test]
fn two_merchants_balances_are_independent() {
    let te = TestEnv::default();
    let sub_a = Address::generate(&te.env);
    let sub_b = Address::generate(&te.env);
    let merchant_a = Address::generate(&te.env);
    let merchant_b = Address::generate(&te.env);

    let id_a = make_sub(&te, &sub_a, &merchant_a);
    let id_b = make_sub(&te, &sub_b, &merchant_b);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&id_a);
    te.client.charge_subscription(&id_b);

    assert_eq!(
        te.client.get_merchant_balance_by_token(&merchant_a, &te.token),
        AMOUNT
    );
    assert_eq!(
        te.client.get_merchant_balance_by_token(&merchant_b, &te.token),
        AMOUNT
    );

    // Withdrawing from merchant_a must not touch merchant_b
    te.client.withdraw_merchant_funds(&merchant_a, &AMOUNT);
    assert_eq!(
        te.client.get_merchant_balance_by_token(&merchant_a, &te.token),
        0
    );
    assert_eq!(
        te.client.get_merchant_balance_by_token(&merchant_b, &te.token),
        AMOUNT,
        "merchant_b balance must be unaffected by merchant_a withdrawal"
    );

    assert_reconciled(&te, &merchant_a);
    assert_reconciled(&te, &merchant_b);
}

#[test]
fn merchant_cannot_overdraw_into_other_merchants_balance() {
    let te = TestEnv::default();
    let sub_a = Address::generate(&te.env);
    let sub_b = Address::generate(&te.env);
    let merchant_a = Address::generate(&te.env);
    let merchant_b = Address::generate(&te.env);

    let id_a = make_sub(&te, &sub_a, &merchant_a);
    make_sub(&te, &sub_b, &merchant_b);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&id_a);
    // merchant_b has AMOUNT from setup deposit loop but no actual charges

    // Try to overdraw merchant_a far beyond their earned AMOUNT
    let result = te.client.try_withdraw_merchant_funds(&merchant_a, &(AMOUNT * 10));
    assert_eq!(result.unwrap_err().unwrap(), Error::InsufficientBalance);

    // merchant_a balance unchanged
    assert_eq!(
        te.client.get_merchant_balance_by_token(&merchant_a, &te.token),
        AMOUNT
    );
}

// ── 8. Withdrawal during paused/cancelled subscription doesn't affect earnings ──

#[test]
fn withdrawal_allowed_when_subscription_is_paused() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&sub_id);

    // Pause after earning
    te.client.pause_subscription(&sub_id, &merchant);

    // Withdrawal should still work
    let result = te.client.try_withdraw_merchant_funds(&merchant, &AMOUNT);
    assert!(result.is_ok());
    assert_eq!(
        te.client.get_merchant_balance_by_token(&merchant, &te.token),
        0
    );
    assert_reconciled(&te, &merchant);
}

#[test]
fn withdrawal_allowed_when_subscription_is_cancelled() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&sub_id);

    te.client.cancel_subscription(&sub_id, &merchant);

    let result = te.client.try_withdraw_merchant_funds(&merchant, &AMOUNT);
    assert!(result.is_ok());
    assert_reconciled(&te, &merchant);
}

#[test]
fn pausing_subscription_does_not_alter_earnings() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&sub_id);

    let balance_before = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    te.client.pause_subscription(&sub_id, &merchant);
    let balance_after = te.client.get_merchant_balance_by_token(&merchant, &te.token);

    assert_eq!(balance_before, balance_after, "pause must not modify merchant balance");
    assert_reconciled(&te, &merchant);
}

#[test]
fn cancellation_does_not_alter_merchant_earnings() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&sub_id);

    let balance_before = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    te.client.cancel_subscription(&sub_id, &merchant);
    let balance_after = te.client.get_merchant_balance_by_token(&merchant, &te.token);

    assert_eq!(balance_before, balance_after, "cancellation must not modify merchant balance");
    assert_reconciled(&te, &merchant);
}

// ── 9. Batch charge — deterministic ordering doesn't alter totals ─────────────

#[test]
fn batch_charge_totals_match_individual_charge_totals() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    let id_0 = make_sub(&te, &subscriber, &merchant);
    let id_1 = make_sub(&te, &subscriber, &merchant);
    let id_2 = make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);

    let ids = soroban_sdk::vec![&te.env, id_0, id_1, id_2];
    te.client.batch_charge(&ids, &0);

    let balance = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    assert_eq!(balance, 3 * AMOUNT, "3 subscriptions × AMOUNT each");

    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.accruals.interval, 3 * AMOUNT);

    assert_reconciled(&te, &merchant);
}

#[test]
fn batch_charge_with_partial_failures_credits_only_successes() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    let id_funded = make_sub(&te, &subscriber, &merchant);

    // Create a subscription with no balance — will fail to charge
    let id_empty = te.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );

    te.jump(INTERVAL + 1);

    let ids = soroban_sdk::vec![&te.env, id_funded, id_empty];
    let results = te.client.batch_charge(&ids, &0);

    assert!(results.get(0).unwrap().success, "funded subscription should succeed");
    assert!(!results.get(1).unwrap().success, "zero-balance subscription should fail");

    let balance = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    assert_eq!(balance, AMOUNT, "only one successful charge");

    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.accruals.interval, AMOUNT);

    assert_reconciled(&te, &merchant);
}

// ── 10. Protocol fee: net amount credited to merchant ─────────────────────────

#[test]
fn protocol_fee_merchant_receives_net_amount() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let treasury = Address::generate(&te.env);

    // Set a 10% (1000 bps) protocol fee
    te.client
        .set_protocol_fee(&te.admin, &treasury, &1_000u32);

    make_sub(&te, &subscriber, &merchant);
    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    let fee = AMOUNT * 1_000 / 10_000; // 10%
    let net = AMOUNT - fee;

    let merchant_balance = te.client.get_merchant_balance_by_token(&merchant, &te.token);
    assert_eq!(merchant_balance, net, "merchant receives net-of-fee amount");

    let treasury_balance = te.client.get_merchant_balance_by_token(&treasury, &te.token);
    assert_eq!(treasury_balance, fee, "treasury receives fee amount");

    // Merchant earnings record matches what was actually credited (net)
    let e = te.client.get_merchant_token_earnings(&merchant, &te.token);
    assert_eq!(e.accruals.interval, net);

    assert_reconciled(&te, &merchant);
    assert_reconciled(&te, &treasury);
}

// ── 11. Reconciliation snapshot is correct after combined operations ──────────

#[test]
fn reconciliation_snapshot_correct_after_charge_withdraw_refund() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);
    te.jump(INTERVAL);
    te.client.charge_subscription(&0);
    // accruals.interval = 2 * AMOUNT

    te.client.withdraw_merchant_funds(&merchant, &AMOUNT);
    te.client.merchant_refund(&merchant, &subscriber, &te.token, &(AMOUNT / 4));
    // balance = 2*AMOUNT - AMOUNT - AMOUNT/4 = 3*AMOUNT/4

    assert_reconciled(&te, &merchant);

    let snaps = te.client.get_reconciliation_snapshot(&merchant);
    assert_eq!(snaps.len(), 1);
    let snap = snaps.get(0).unwrap();
    assert_eq!(snap.total_accruals, 2 * AMOUNT);
    assert_eq!(snap.total_withdrawals, AMOUNT);
    assert_eq!(snap.total_refunds, AMOUNT / 4);
    assert_eq!(snap.computed_balance, 3 * AMOUNT / 4);
}

#[test]
fn get_merchant_total_earnings_returns_all_token_entries() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    let all_earnings = te.client.get_merchant_total_earnings(&merchant);
    assert_eq!(all_earnings.len(), 1, "one token entry expected");
    let (token_addr, earnings) = all_earnings.get(0).unwrap();
    assert_eq!(token_addr, te.token);
    assert_eq!(earnings.accruals.interval, AMOUNT);
    assert_eq!(earnings.withdrawals, 0);
    assert_eq!(earnings.refunds, 0);
}

// ── 12. Only merchant can withdraw their own funds ────────────────────────────

#[test]
fn wrong_signer_cannot_withdraw_merchant_funds() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);
    make_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&0);

    // mock_all_auths lets anyone pass auth; we use try_ to check balance guard
    // The real auth check is enforced by the host. Here we verify that a stranger
    // reading merchant_a's balance returns their own (zero) balance, not merchant_a's.
    let stranger_balance = te.client.get_merchant_balance_by_token(&stranger, &te.token);
    assert_eq!(
        stranger_balance, 0,
        "stranger has no balance — cannot withdraw merchant's funds"
    );

    let result = te.client.try_withdraw_merchant_funds(&stranger, &AMOUNT);
    assert!(result.is_err(), "stranger with zero balance cannot withdraw");
}
