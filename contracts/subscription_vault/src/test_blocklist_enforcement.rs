#![cfg(test)]

extern crate std;

use crate::test_utils::setup::TestEnv;
use crate::{Error, SubscriptionStatus};
use soroban_sdk::{testutils::Address as _, Address};

// ── Shared constants ──────────────────────────────────────────────────────────

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const DEPOSIT: i128 = 30_000_000;
const USAGE_AMOUNT: i128 = 1_000_000;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Create a funded subscription and return (sub_id, subscriber, merchant).
fn make_funded_subscription(te: &TestEnv) -> (u32, Address, Address) {
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &true,
        &None,
        &None::<u64>,
    );
    te.stellar_token_client().mint(&subscriber, &DEPOSIT);
    te.client.deposit_funds(&sub_id, &subscriber, &DEPOSIT);
    (sub_id, subscriber, merchant)
}

/// Blocklist a subscriber via admin.
fn blocklist(te: &TestEnv, subscriber: &Address) {
    te.client
        .add_to_blocklist(&te.admin, subscriber, &None);
}

/// Advance the ledger past one interval so charge_one succeeds.
fn advance_interval(te: &TestEnv) {
    te.jump(INTERVAL + 1);
}

// ── 1. create_subscription ────────────────────────────────────────────────────

#[test]
fn blocklisted_subscriber_cannot_create_subscription() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    blocklist(&te, &subscriber);

    let result = te.client.try_create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );
    assert_eq!(
        result.unwrap_err().unwrap(),
        Error::SubscriberBlocklisted
    );
}

#[test]
fn non_blocklisted_subscriber_can_create_subscription() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    let result = te.client.try_create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );
    assert!(result.is_ok());
}

// ── 2. deposit_funds ─────────────────────────────────────────────────────────

#[test]
fn blocklisted_subscriber_cannot_deposit_funds() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _merchant) = make_funded_subscription(&te);

    blocklist(&te, &subscriber);

    te.stellar_token_client().mint(&subscriber, &AMOUNT);
    let result = te.client.try_deposit_funds(&sub_id, &subscriber, &AMOUNT);
    assert_eq!(
        result.unwrap_err().unwrap(),
        Error::SubscriberBlocklisted
    );
}

#[test]
fn block_after_subscription_creation_prevents_subsequent_deposit() {
    let te = TestEnv::default();
    // subscription created before block
    let (sub_id, subscriber, _merchant) = make_funded_subscription(&te);

    // block after creation — existing subscription and balance intact
    blocklist(&te, &subscriber);

    te.stellar_token_client().mint(&subscriber, &AMOUNT);
    let result = te.client.try_deposit_funds(&sub_id, &subscriber, &AMOUNT);
    assert_eq!(
        result.unwrap_err().unwrap(),
        Error::SubscriberBlocklisted
    );
}

// ── 3. charge (interval-based) ────────────────────────────────────────────────

#[test]
fn charge_subscription_skips_blocklisted_subscriber() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _merchant) = make_funded_subscription(&te);

    blocklist(&te, &subscriber);
    advance_interval(&te);

    let result = te.client.try_charge_subscription(&sub_id);
    assert_eq!(
        result.unwrap_err().unwrap(),
        Error::SubscriberBlocklisted
    );
}

#[test]
fn charge_subscription_succeeds_for_non_blocklisted_subscriber() {
    let te = TestEnv::default();
    let (sub_id, _subscriber, _merchant) = make_funded_subscription(&te);

    advance_interval(&te);

    let result = te.client.try_charge_subscription(&sub_id);
    assert!(result.is_ok());
}

// ── 4. charge_usage ──────────────────────────────────────────────────────────

#[test]
fn charge_usage_blocked_for_blocklisted_subscriber() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _merchant) = make_funded_subscription(&te);

    blocklist(&te, &subscriber);

    let ref_str = soroban_sdk::String::from_str(&te.env, "ref-001");
    let result = te
        .client
        .try_charge_usage(&sub_id, &USAGE_AMOUNT, &ref_str);
    assert_eq!(
        result.unwrap_err().unwrap(),
        Error::SubscriberBlocklisted
    );
}

#[test]
fn charge_usage_succeeds_for_non_blocklisted_subscriber() {
    let te = TestEnv::default();
    let (sub_id, _subscriber, _merchant) = make_funded_subscription(&te);

    let ref_str = soroban_sdk::String::from_str(&te.env, "ref-001");
    let result = te
        .client
        .try_charge_usage(&sub_id, &USAGE_AMOUNT, &ref_str);
    assert!(result.is_ok());
}

// ── 5. pause_subscription ────────────────────────────────────────────────────

#[test]
fn blocklisted_subscriber_cannot_self_pause() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _merchant) = make_funded_subscription(&te);

    blocklist(&te, &subscriber);

    let result = te.client.try_pause_subscription(&sub_id, &subscriber);
    assert_eq!(
        result.unwrap_err().unwrap(),
        Error::SubscriberBlocklisted
    );
}

#[test]
fn merchant_can_pause_subscription_of_blocklisted_subscriber() {
    let te = TestEnv::default();
    let (sub_id, subscriber, merchant) = make_funded_subscription(&te);

    blocklist(&te, &subscriber);

    // Merchant should still be able to pause
    let result = te.client.try_pause_subscription(&sub_id, &merchant);
    assert!(result.is_ok());

    let sub = te.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Paused);
}

// ── 6. cancel_subscription ───────────────────────────────────────────────────

#[test]
fn blocklisted_subscriber_cannot_self_cancel() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _merchant) = make_funded_subscription(&te);

    blocklist(&te, &subscriber);

    let result = te.client.try_cancel_subscription(&sub_id, &subscriber);
    assert_eq!(
        result.unwrap_err().unwrap(),
        Error::SubscriberBlocklisted
    );
}

#[test]
fn merchant_can_cancel_subscription_of_blocklisted_subscriber() {
    let te = TestEnv::default();
    let (sub_id, subscriber, merchant) = make_funded_subscription(&te);

    blocklist(&te, &subscriber);

    let result = te.client.try_cancel_subscription(&sub_id, &merchant);
    assert!(result.is_ok());

    let sub = te.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

// ── 7. resume_subscription ───────────────────────────────────────────────────

#[test]
fn blocklisted_subscriber_cannot_self_resume() {
    let te = TestEnv::default();
    let (sub_id, subscriber, merchant) = make_funded_subscription(&te);

    // Pause via merchant first
    te.client.pause_subscription(&sub_id, &merchant);
    blocklist(&te, &subscriber);

    let result = te.client.try_resume_subscription(&sub_id, &subscriber);
    assert_eq!(
        result.unwrap_err().unwrap(),
        Error::SubscriberBlocklisted
    );
}

#[test]
fn merchant_can_resume_subscription_of_blocklisted_subscriber() {
    let te = TestEnv::default();
    let (sub_id, subscriber, merchant) = make_funded_subscription(&te);

    te.client.pause_subscription(&sub_id, &merchant);
    blocklist(&te, &subscriber);

    let result = te.client.try_resume_subscription(&sub_id, &merchant);
    assert!(result.is_ok());

    let sub = te.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

// ── 8. withdraw_merchant_funds ───────────────────────────────────────────────

#[test]
fn blocklisted_merchant_cannot_withdraw_funds() {
    let te = TestEnv::default();
    let (sub_id, _subscriber, merchant) = make_funded_subscription(&te);

    // Charge so merchant has a balance
    advance_interval(&te);
    te.client.charge_subscription(&sub_id);

    // Blocklist the merchant address
    blocklist(&te, &merchant);

    let result = te.client.try_withdraw_merchant_funds(&merchant, &AMOUNT);
    assert_eq!(
        result.unwrap_err().unwrap(),
        Error::SubscriberBlocklisted
    );
}

#[test]
fn non_blocklisted_merchant_can_withdraw_funds() {
    let te = TestEnv::default();
    let (sub_id, _subscriber, merchant) = make_funded_subscription(&te);

    advance_interval(&te);
    te.client.charge_subscription(&sub_id);

    // Merchant earnings should be AMOUNT; withdraw half
    let result = te.client.try_withdraw_merchant_funds(&merchant, &(AMOUNT / 2));
    assert!(result.is_ok());
}

// ── 9. Unblock restores full access ──────────────────────────────────────────

#[test]
fn unblocked_subscriber_can_deposit_again() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _merchant) = make_funded_subscription(&te);

    blocklist(&te, &subscriber);
    // While blocked, deposit fails
    te.stellar_token_client().mint(&subscriber, &AMOUNT);
    assert!(te
        .client
        .try_deposit_funds(&sub_id, &subscriber, &AMOUNT)
        .is_err());

    // Admin removes from blocklist
    te.client.remove_from_blocklist(&te.admin, &subscriber);

    // Now deposit should succeed
    let result = te.client.try_deposit_funds(&sub_id, &subscriber, &AMOUNT);
    assert!(result.is_ok());
}

#[test]
fn unblocked_subscriber_can_create_subscription() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    blocklist(&te, &subscriber);
    te.client.remove_from_blocklist(&te.admin, &subscriber);

    let result = te.client.try_create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );
    assert!(result.is_ok());
}

#[test]
fn unblocked_subscriber_can_self_pause() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _merchant) = make_funded_subscription(&te);

    blocklist(&te, &subscriber);
    te.client.remove_from_blocklist(&te.admin, &subscriber);

    let result = te.client.try_pause_subscription(&sub_id, &subscriber);
    assert!(result.is_ok());
}

// ── 10. Existing subscription state continuity after block ────────────────────

#[test]
fn existing_subscription_balance_preserved_after_block() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _merchant) = make_funded_subscription(&te);

    let sub_before = te.client.get_subscription(&sub_id);
    let balance_before = sub_before.prepaid_balance;

    blocklist(&te, &subscriber);

    let sub_after = te.client.get_subscription(&sub_id);
    assert_eq!(sub_after.prepaid_balance, balance_before, "block must not alter existing balance");
    assert_eq!(sub_after.status, SubscriptionStatus::Active, "block must not alter subscription status");
}

#[test]
fn subscriber_can_withdraw_own_funds_while_blocklisted() {
    let te = TestEnv::default();
    let (sub_id, subscriber, merchant) = make_funded_subscription(&te);

    // Cancel via merchant to unlock funds for withdrawal
    te.client.cancel_subscription(&sub_id, &merchant);
    blocklist(&te, &subscriber);

    // Subscriber should still be able to retrieve their own funds
    let result = te.client.try_withdraw_subscriber_funds(&sub_id, &subscriber);
    assert!(result.is_ok(), "fund-return path must remain open for blocked subscribers");
}

// ── 11. Blocked merchant with existing earnings ───────────────────────────────

#[test]
fn blocked_merchant_earnings_are_preserved_but_locked() {
    let te = TestEnv::default();
    let (sub_id, _subscriber, merchant) = make_funded_subscription(&te);

    advance_interval(&te);
    te.client.charge_subscription(&sub_id);

    blocklist(&te, &merchant);

    // Balance is still there — just can't be withdrawn
    let balance = te
        .client
        .get_merchant_balance_by_token(&merchant, &te.token);
    assert!(balance > 0, "merchant earnings should be preserved");

    let result = te.client.try_withdraw_merchant_funds(&merchant, &balance);
    assert!(
        result.is_err(),
        "blocked merchant cannot withdraw earnings"
    );
}

#[test]
fn unblocked_merchant_can_withdraw_accumulated_earnings() {
    let te = TestEnv::default();
    let (sub_id, _subscriber, merchant) = make_funded_subscription(&te);

    advance_interval(&te);
    te.client.charge_subscription(&sub_id);

    blocklist(&te, &merchant);
    te.client.remove_from_blocklist(&te.admin, &merchant);

    let balance = te
        .client
        .get_merchant_balance_by_token(&merchant, &te.token);
    let result = te.client.try_withdraw_merchant_funds(&merchant, &balance);
    assert!(result.is_ok(), "unblocked merchant can access earnings");
}
