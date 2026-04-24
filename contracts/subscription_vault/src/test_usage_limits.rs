#![cfg(test)]

use crate::{Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, String,
};

const T0: u64 = 1700000000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, client, token, admin)
}

#[test]
fn test_valid_usage_charging() {
    let (env, client, token, _) = setup();
    let subscriber = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let merchant = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let sub_id =
        client.create_subscription(&subscriber, &merchant, &1i128, &INTERVAL, &true, &None::<i128>, &None::<u64>);
    client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    client.charge_usage_with_reference(&sub_id, &5_000_000i128, &String::from_str(&env, "ref1"));
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 95_000_000i128);
    assert_eq!(sub.lifetime_charged, 5_000_000i128);

    let merchant_bal = client.get_merchant_balance_by_token(&merchant, &token);
    assert_eq!(merchant_bal, 5_000_000i128);
}

#[test]
fn test_usage_disabled() {
    let (env, client, token, _) = setup();
    let subscriber = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let merchant = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &1i128,
        &INTERVAL,
        &false, // usage_enabled = false
        &None, &None
    );
    client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    let result = client.try_charge_usage(&sub_id, &5_000_000i128);
    assert_eq!(result, Err(Ok(Error::UsageNotEnabled)));
}

#[test]
fn test_zero_or_negative_usage() {
    let (env, client, token, _) = setup();
    let subscriber = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let merchant = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let sub_id =
        client.create_subscription(&subscriber, &merchant, &1i128, &INTERVAL, &true, &None::<i128>, &None::<u64>);
    client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    let result = client.try_charge_usage(&sub_id, &0i128);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));

    let result = client.try_charge_usage(&sub_id, &-5i128);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_exact_prepaid_balance_usage() {
    let (env, client, token, _) = setup();
    let subscriber = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let merchant = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &10_000_000i128);

    let sub_id =
        client.create_subscription(&subscriber, &merchant, &1i128, &INTERVAL, &true, &None::<i128>, &None::<u64>);
    client.deposit_funds(&sub_id, &subscriber, &10_000_000i128);

    client.charge_usage(&sub_id, &10_000_000i128);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 0i128);
    assert_eq!(
        sub.status,
        crate::types::SubscriptionStatus::InsufficientBalance
    );
}

#[test]
fn test_exact_lifetime_cap_boundary() {
    let (env, client, token, _) = setup();
    let subscriber = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let merchant = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &1i128,
        &INTERVAL,
        &true,
        &Some(50_000_000i128),
        &None,
    );
    client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    client.charge_usage(&sub_id, &50_000_000i128);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.lifetime_charged, 50_000_000i128);
    assert_eq!(sub.status, crate::types::SubscriptionStatus::Cancelled);
}

#[test]
fn test_burst_usage_attempts() {
    let (env, client, token, _) = setup();
    let subscriber = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let merchant = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let sub_id =
        client.create_subscription(&subscriber, &merchant, &1i128, &INTERVAL, &true, &None::<i128>, &None::<u64>);
    client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    client.configure_usage_limits(
        &merchant, &sub_id, &None,  // rate_limit_max_calls
        &60u64, // rate_window_secs
        &2u64,  // burst_min_interval_secs
        &None,  // usage_cap_units
    );

    client.charge_usage_with_reference(&sub_id, &1_000_000i128, &String::from_str(&env, "ref1"));

    // Exact same timestamp -> should fail with burst limit
    let result = client.try_charge_usage_with_reference(
        &sub_id,
        &1_000_000i128,
        &String::from_str(&env, "ref2"),
    );
    assert_eq!(result, Err(Ok(Error::BurstLimitExceeded)));

    // 1 second later -> still fails
    env.ledger().set_timestamp(T0 + 1);
    let result = client.try_charge_usage_with_reference(
        &sub_id,
        &1_000_000i128,
        &String::from_str(&env, "ref3"),
    );
    assert_eq!(result, Err(Ok(Error::BurstLimitExceeded)));

    // 2 seconds later -> succeeds
    env.ledger().set_timestamp(T0 + 2);
    client.charge_usage_with_reference(&sub_id, &1_000_000i128, &String::from_str(&env, "ref4"));
}

#[test]
fn test_rate_limit_violations() {
    let (env, client, token, _) = setup();
    let subscriber = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let merchant = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let sub_id =
        client.create_subscription(&subscriber, &merchant, &1i128, &INTERVAL, &true, &None::<i128>, &None::<u64>);
    client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    client.configure_usage_limits(
        &merchant,
        &sub_id,
        &Some(3u32), // rate_limit_max_calls = 3
        &60u64,      // rate_window_secs = 60
        &0u64,       // burst_min_interval_secs
        &None,       // usage_cap_units
    );

    client.charge_usage_with_reference(&sub_id, &1_000_000i128, &String::from_str(&env, "ref1"));
    client.charge_usage_with_reference(&sub_id, &1_000_000i128, &String::from_str(&env, "ref2"));
    client.charge_usage_with_reference(&sub_id, &1_000_000i128, &String::from_str(&env, "ref3"));

    // 4th call should fail
    let result = client.try_charge_usage_with_reference(
        &sub_id,
        &1_000_000i128,
        &String::from_str(&env, "ref4"),
    );
    assert_eq!(result, Err(Ok(Error::RateLimitExceeded)));

    // Move time forward past window
    env.ledger().set_timestamp(T0 + 60);
    client.charge_usage_with_reference(&sub_id, &1_000_000i128, &String::from_str(&env, "ref5"));
}

#[test]
fn test_replay_attacks() {
    let (env, client, token, _) = setup();
    let subscriber = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let merchant = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let sub_id =
        client.create_subscription(&subscriber, &merchant, &1i128, &INTERVAL, &true, &None::<i128>, &None::<u64>);
    client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    client.charge_usage_with_reference(
        &sub_id,
        &1_000_000i128,
        &String::from_str(&env, "my-unique-ref"),
    );

    env.ledger().set_timestamp(T0 + 10);

    // Try same reference again
    let result = client.try_charge_usage_with_reference(
        &sub_id,
        &1_000_000i128,
        &String::from_str(&env, "my-unique-ref"),
    );
    assert_eq!(result, Err(Ok(Error::Replay)));
}

#[test]
fn test_usage_cap_enforcement() {
    let (env, client, token, _) = setup();
    let subscriber = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let merchant = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let sub_id =
        client.create_subscription(&subscriber, &merchant, &1i128, &INTERVAL, &true, &None::<i128>, &None::<u64>);
    client.deposit_funds(&sub_id, &subscriber, &100_000_000i128);

    client.configure_usage_limits(
        &merchant,
        &sub_id,
        &None,                 // rate_limit_max_calls
        &60u64,                // rate_window_secs
        &0u64,                 // burst_min_interval_secs
        &Some(10_000_000i128), // usage_cap_units = 10m per period
    );

    client.charge_usage_with_reference(&sub_id, &6_000_000i128, &String::from_str(&env, "ref1"));

    // Another 5m should exceed the 10m cap
    let result = client.try_charge_usage_with_reference(
        &sub_id,
        &5_000_000i128,
        &String::from_str(&env, "ref2"),
    );
    assert_eq!(result, Err(Ok(Error::UsageCapExceeded)));

    // Another 4m is perfectly fine
    client.charge_usage_with_reference(&sub_id, &4_000_000i128, &String::from_str(&env, "ref3"));

    // Moving to next period resets the cap
    env.ledger().set_timestamp(T0 + INTERVAL + 1);
    client.charge_usage_with_reference(&sub_id, &6_000_000i128, &String::from_str(&env, "ref4"));
}
