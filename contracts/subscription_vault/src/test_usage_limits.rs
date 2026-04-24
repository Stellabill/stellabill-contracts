#![cfg(test)]

use crate::{Error, SubscriptionStatus};
use crate::test_utils::{fixtures, setup::TestEnv};
use soroban_sdk::{
    testutils::Address as _,
    String,
};

const T0: u64 = 1700000000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days

#[test]
fn test_valid_usage_charging() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let (sub_id, subscriber, merchant) = fixtures::create_usage_subscription(
        &test_env.env, &test_env.client, SubscriptionStatus::Active, 1i128, INTERVAL
    );
    test_env.stellar_token_client().mint(&subscriber, &100_000_000i128);
    test_env.client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    test_env.client.charge_usage_with_reference(&sub_id, &5_000_000i128, &String::from_str(&test_env.env, "ref1"));
    let sub = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 95_000_000i128);
    assert_eq!(sub.lifetime_charged, 5_000_000i128);

    let merchant_bal = test_env.client.get_merchant_balance_by_token(&merchant, &test_env.token);
    assert_eq!(merchant_bal, 5_000_000i128);
}

#[test]
fn test_usage_disabled() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let (sub_id, subscriber, _) = fixtures::create_test_subscription(
        &test_env.env, &test_env.client, SubscriptionStatus::Active
    );
    test_env.stellar_token_client().mint(&subscriber, &100_000_000i128);
    test_env.client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    let result = test_env.client.try_charge_usage(&sub_id, &5_000_000i128);
    assert_eq!(result, Err(Ok(Error::UsageNotEnabled)));
}

#[test]
fn test_zero_or_negative_usage() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let (sub_id, subscriber, _) = fixtures::create_usage_subscription(
        &test_env.env, &test_env.client, SubscriptionStatus::Active, 1i128, INTERVAL
    );
    test_env.stellar_token_client().mint(&subscriber, &100_000_000i128);
    test_env.client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    let result = test_env.client.try_charge_usage(&sub_id, &0i128);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));

    let result = test_env.client.try_charge_usage(&sub_id, &-5i128);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_exact_prepaid_balance_usage() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let (sub_id, subscriber, _) = fixtures::create_usage_subscription(
        &test_env.env, &test_env.client, SubscriptionStatus::Active, 1i128, INTERVAL
    );
    test_env.stellar_token_client().mint(&subscriber, &10_000_000i128);
    test_env.client.deposit_funds(&sub_id, &subscriber, &10_000_000i128);

    test_env.client.charge_usage(&sub_id, &10_000_000i128);
    let sub = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 0i128);
    assert_eq!(
        sub.status,
        crate::types::SubscriptionStatus::InsufficientBalance
    );
}

#[test]
fn test_exact_lifetime_cap_boundary() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let (sub_id, subscriber, _) = fixtures::create_capped_subscription(
        &test_env.env, &test_env.client, 1i128, INTERVAL, Some(50_000_000i128), true
    );
    test_env.stellar_token_client().mint(&subscriber, &100_000_000i128);
    test_env.client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    test_env.client.charge_usage(&sub_id, &50_000_000i128);
    let sub = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub.lifetime_charged, 50_000_000i128);
    assert_eq!(sub.status, crate::types::SubscriptionStatus::Cancelled);
}

#[test]
fn test_burst_usage_attempts() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let (sub_id, subscriber, merchant) = fixtures::create_usage_subscription(
        &test_env.env, &test_env.client, SubscriptionStatus::Active, 1i128, INTERVAL
    );
    test_env.stellar_token_client().mint(&subscriber, &100_000_000i128);
    test_env.client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    test_env.client.configure_usage_limits(
        &merchant, &sub_id, &None,  // rate_limit_max_calls
        &60u64, // rate_window_secs
        &2u64,  // burst_min_interval_secs
        &None,  // usage_cap_units
    );

    test_env.client.charge_usage_with_reference(&sub_id, &1_000_000i128, &String::from_str(&test_env.env, "ref1"));

    // Exact same timestamp -> should fail with burst limit
    let result = test_env.client.try_charge_usage_with_reference(
        &sub_id,
        &1_000_000i128,
        &String::from_str(&test_env.env, "ref2"),
    );
    assert_eq!(result, Err(Ok(Error::BurstLimitExceeded)));

    // 1 second later -> still fails
    test_env.set_timestamp(T0 + 1);
    let result = test_env.client.try_charge_usage_with_reference(
        &sub_id,
        &1_000_000i128,
        &String::from_str(&test_env.env, "ref3"),
    );
    assert_eq!(result, Err(Ok(Error::BurstLimitExceeded)));

    // 2 seconds later -> succeeds
    test_env.set_timestamp(T0 + 2);
    test_env.client.charge_usage_with_reference(&sub_id, &1_000_000i128, &String::from_str(&test_env.env, "ref4"));
}

#[test]
fn test_rate_limit_violations() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let (sub_id, subscriber, merchant) = fixtures::create_usage_subscription(
        &test_env.env, &test_env.client, SubscriptionStatus::Active, 1i128, INTERVAL
    );
    test_env.stellar_token_client().mint(&subscriber, &100_000_000i128);
    test_env.client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    test_env.client.configure_usage_limits(
        &merchant,
        &sub_id,
        &Some(3u32), // rate_limit_max_calls = 3
        &60u64,      // rate_window_secs = 60
        &0u64,       // burst_min_interval_secs
        &None,       // usage_cap_units
    );

    test_env.client.charge_usage_with_reference(&sub_id, &1_000_000i128, &String::from_str(&test_env.env, "ref1"));
    test_env.client.charge_usage_with_reference(&sub_id, &1_000_000i128, &String::from_str(&test_env.env, "ref2"));
    test_env.client.charge_usage_with_reference(&sub_id, &1_000_000i128, &String::from_str(&test_env.env, "ref3"));

    // 4th call should fail
    let result = test_env.client.try_charge_usage_with_reference(
        &sub_id,
        &1_000_000i128,
        &String::from_str(&test_env.env, "ref4"),
    );
    assert_eq!(result, Err(Ok(Error::RateLimitExceeded)));

    // Move time forward past window
    test_env.set_timestamp(T0 + 60);
    test_env.client.charge_usage_with_reference(&sub_id, &1_000_000i128, &String::from_str(&test_env.env, "ref5"));
}

#[test]
fn test_replay_attacks() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let (sub_id, subscriber, _) = fixtures::create_usage_subscription(
        &test_env.env, &test_env.client, SubscriptionStatus::Active, 1i128, INTERVAL
    );
    test_env.stellar_token_client().mint(&subscriber, &100_000_000i128);
    test_env.client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    test_env.client.charge_usage_with_reference(
        &sub_id,
        &1_000_000i128,
        &String::from_str(&test_env.env, "my-unique-ref"),
    );

    test_env.set_timestamp(T0 + 10);

    // Try same reference again
    let result = test_env.client.try_charge_usage_with_reference(
        &sub_id,
        &1_000_000i128,
        &String::from_str(&test_env.env, "my-unique-ref"),
    );
    assert_eq!(result, Err(Ok(Error::Replay)));
}

#[test]
fn test_usage_cap_enforcement() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let (sub_id, subscriber, merchant) = fixtures::create_usage_subscription(
        &test_env.env, &test_env.client, SubscriptionStatus::Active, 1i128, INTERVAL
    );
    test_env.stellar_token_client().mint(&subscriber, &100_000_000i128);
    test_env.client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    test_env.client.configure_usage_limits(
        &merchant,
        &sub_id,
        &None,                 // rate_limit_max_calls
        &60u64,                // rate_window_secs
        &0u64,                 // burst_min_interval_secs
        &Some(10_000_000i128), // usage_cap_units = 10m per period
    );

    test_env.client.charge_usage_with_reference(&sub_id, &6_000_000i128, &String::from_str(&test_env.env, "ref1"));

    // Another 5m should exceed the 10m cap
    let result = test_env.client.try_charge_usage_with_reference(
        &sub_id,
        &5_000_000i128,
        &String::from_str(&test_env.env, "ref2"),
    );
    assert_eq!(result, Err(Ok(Error::UsageCapExceeded)));

    // Another 4m is perfectly fine
    test_env.client.charge_usage_with_reference(&sub_id, &4_000_000i128, &String::from_str(&test_env.env, "ref3"));

    // Moving to next period resets the cap
    test_env.set_timestamp(T0 + INTERVAL + 1);
    test_env.client.charge_usage_with_reference(&sub_id, &6_000_000i128, &String::from_str(&test_env.env, "ref4"));
}
