#![cfg(test)]

use crate::test_utils::setup::TestEnv;
use crate::{Error, SubscriptionStatus};
use soroban_sdk::{testutils::Address as _, Address};

const SUB_AMOUNT: i128 = 10_000_000;
const SUB_INTERVAL: u64 = 30 * 24 * 60 * 60;
const DEPOSIT_1: i128 = 30_000_000;
const DEPOSIT_2: i128 = 20_000_000;
const ONE_OFF_AMOUNT: i128 = 2_000_000;

#[test]
fn test_e2e_happy_path_create() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &SUB_AMOUNT,
        &SUB_INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );

    let sub = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub.subscriber, subscriber);
    assert_eq!(sub.merchant, merchant);
    assert_eq!(sub.amount, SUB_AMOUNT);
    assert_eq!(sub.interval_seconds, SUB_INTERVAL);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(sub.prepaid_balance, 0);
}

#[test]
fn test_e2e_happy_path_deposit() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    test_env.stellar_token_client().mint(&subscriber, &(DEPOSIT_1 * 2));

    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &SUB_AMOUNT,
        &SUB_INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );

    test_env.client.deposit_funds(&sub_id, &subscriber, &DEPOSIT_1);

    let sub = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, DEPOSIT_1);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

#[test]
fn test_e2e_happy_path_charge() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    test_env.stellar_token_client().mint(&subscriber, &(DEPOSIT_1 * 2));

    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &SUB_AMOUNT,
        &SUB_INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );

    test_env.client.deposit_funds(&sub_id, &subscriber, &DEPOSIT_1);

    test_env.env.jump(SUB_INTERVAL + 1);

    let result = test_env.client.charge_subscription(&sub_id);

    let sub = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, DEPOSIT_1 - SUB_AMOUNT);
    assert!(sub.last_payment_timestamp > 0);
}

#[test]
fn test_e2e_happy_path_one_off_charge() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    test_env.stellar_token_client().mint(&subscriber, &(DEPOSIT_1 * 2));

    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &SUB_AMOUNT,
        &SUB_INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );

    test_env.client.deposit_funds(&sub_id, &subscriber, &DEPOSIT_1);

    test_env.client.charge_one_off(&sub_id, &merchant, &ONE_OFF_AMOUNT);

    let sub = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, DEPOSIT_1 - ONE_OFF_AMOUNT);
}

#[test]
fn test_e2e_happy_path_merchant_withdraw() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    test_env.stellar_token_client().mint(&subscriber, &(DEPOSIT_1 * 2));

    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &SUB_AMOUNT,
        &SUB_INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );

    test_env.client.deposit_funds(&sub_id, &subscriber, &DEPOSIT_1);

    test_env.env.jump(SUB_INTERVAL + 1);

    test_env.client.charge_subscription(&sub_id);

    let merchant_balance_before = test_env.client.get_merchant_balance(&merchant);

    test_env.client.withdraw_merchant_funds(&merchant, &SUB_AMOUNT);

    let merchant_balance_after = test_env.client.get_merchant_balance(&merchant);
    assert_eq!(merchant_balance_before - merchant_balance_after, SUB_AMOUNT);
}

#[test]
fn test_e2e_happy_path_cancel_refund() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    test_env.stellar_token_client().mint(&subscriber, &(DEPOSIT_1 * 2));

    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &SUB_AMOUNT,
        &SUB_INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );

    test_env.client.deposit_funds(&sub_id, &subscriber, &DEPOSIT_1);

    let sub_before = test_env.client.get_subscription(&sub_id);
    let balance_before = sub_before.prepaid_balance;

    test_env.client.cancel_subscription(&sub_id, &subscriber);

    let sub = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(sub.prepaid_balance, balance_before);

    test_env.client.withdraw_subscriber_funds(&sub_id, &subscriber);

    let sub_after = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub_after.prepaid_balance, 0);
}

#[test]
fn test_e2e_happy_path_full_lifecycle() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    test_env.stellar_token_client().mint(&subscriber, &(DEPOSIT_1 * 5));

    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &SUB_AMOUNT,
        &SUB_INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );

    let sub_0 = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub_0.status, SubscriptionStatus::Active);
    assert_eq!(sub_0.prepaid_balance, 0);

    test_env.client.deposit_funds(&sub_id, &subscriber, &DEPOSIT_1);

    let sub_1 = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub_1.prepaid_balance, DEPOSIT_1);
    assert_eq!(sub_1.status, SubscriptionStatus::Active);

    test_env.env.jump(SUB_INTERVAL + 1);

    let charge_result = test_env.client.charge_subscription(&sub_id);
    let sub_2 = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub_2.prepaid_balance, DEPOSIT_1 - SUB_AMOUNT);
    assert!(sub_2.last_payment_timestamp > 0);

    test_env.client.deposit_funds(&sub_id, &subscriber, &DEPOSIT_2);

    let sub_3 = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub_3.prepaid_balance, DEPOSIT_1 - SUB_AMOUNT + DEPOSIT_2);

    test_env.client.charge_one_off(&sub_id, &merchant, &ONE_OFF_AMOUNT);

    let sub_4 = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub_4.prepaid_balance, DEPOSIT_1 - SUB_AMOUNT + DEPOSIT_2 - ONE_OFF_AMOUNT);

    test_env.env.jump(SUB_INTERVAL + 1);

    test_env.client.charge_subscription(&sub_id);

    let sub_5 = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub_5.prepaid_balance, 0);
    assert_eq!(sub_5.status, SubscriptionStatus::InsufficientBalance);

    test_env.client.resume_subscription(&sub_id, &subscriber);

    let sub_6 = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub_6.status, SubscriptionStatus::Active);

    test_env.client.cancel_subscription(&sub_id, &subscriber);

    let sub_7 = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub_7.status, SubscriptionStatus::Cancelled);

    test_env.client.withdraw_subscriber_funds(&sub_id, &subscriber);

    let sub_8 = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub_8.prepaid_balance, 0);
}

#[test]
fn test_e2e_retry_charge_idempotency() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    test_env.stellar_token_client().mint(&subscriber, &(DEPOSIT_1 * 2));

    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &SUB_AMOUNT,
        &SUB_INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );

    test_env.client.deposit_funds(&sub_id, &subscriber, &DEPOSIT_1);

    test_env.env.jump(SUB_INTERVAL + 1);

    let result_1 = test_env.client.charge_subscription(&sub_id);
    let sub_after_first = test_env.client.get_subscription(&sub_id);
    let first_balance = sub_after_first.prepaid_balance;

    let result_2 = test_env.client.charge_subscription(&sub_id);
    let sub_after_second = test_env.client.get_subscription(&sub_id);
    let second_balance = sub_after_second.prepaid_balance;

    assert_eq!(first_balance, second_balance);
}

#[test]
fn test_e2e_pause_resume_cycle() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    test_env.stellar_token_client().mint(&subscriber, &(DEPOSIT_1 * 2));

    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &SUB_AMOUNT,
        &SUB_INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );

    test_env.client.deposit_funds(&sub_id, &subscriber, &DEPOSIT_1);

    test_env.client.pause_subscription(&sub_id, &subscriber);

    let sub_paused = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub_paused.status, SubscriptionStatus::Paused);

    test_env.env.jump(SUB_INTERVAL + 1);

    let result = test_env.client.charge_subscription(&sub_id);
    let sub_still_paused = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub_still_paused.prepaid_balance, DEPOSIT_1);

    test_env.client.resume_subscription(&sub_id, &subscriber);

    let sub_resumed = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub_resumed.status, SubscriptionStatus::Active);

    test_env.client.charge_subscription(&sub_id);

    let sub_after_charge = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub_after_charge.prepaid_balance, DEPOSIT_1 - SUB_AMOUNT);
}