#![cfg(test)]

use crate::test_utils::setup::TestEnv;
use crate::types::{ChargeExecutionResult, Error};
use soroban_sdk::{testutils::Address as _, Address, String};

#[test]
fn test_interval_charge_determinism() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let amount = 100_000_000i128; // 100 USDC
    let interval = 30 * 24 * 60 * 60; // 30 days
    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval,
        &false,
        &None,
        &None::<u64>,
    );

    test_env
        .stellar_token_client()
        .mint(&subscriber, &500_000_000i128);
    test_env
        .client
        .deposit_funds(&sub_id, &subscriber, &500_000_000i128);

    // T=0 is creation. First charge allowed at T=interval.
    test_env.set_timestamp(interval);

    // First charge should succeed
    let res1 = test_env.client.charge_subscription(&sub_id);
    assert_eq!(res1, ChargeExecutionResult::Charged);

    // Identical second charge at same timestamp should be rejected as Replay
    let res2 = test_env.client.try_charge_subscription(&sub_id);
    assert!(matches!(res2, Err(Ok(Error::Replay))));
}

#[test]
fn test_usage_charge_determinism() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let amount = 100_000_000i128; // 100 USDC
    let interval = 30 * 24 * 60 * 60;
    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval,
        &true,
        &None,
        &None,
    );

    test_env
        .stellar_token_client()
        .mint(&subscriber, &500_000_000i128);
    test_env
        .client
        .deposit_funds(&sub_id, &subscriber, &500_000_000i128);

    let usage_amount = 10_000_000i128;
    let reference = String::from_str(&test_env.env, "req_123");

    // First usage charge should succeed
    test_env
        .client
        .charge_usage_with_reference(&sub_id, &usage_amount, &reference);

    // Replay with identical reference returns Replay variant
    let res2 = test_env
        .client
        .try_charge_usage_with_reference(&sub_id, &usage_amount, &reference);
    assert!(matches!(
        res2,
        Ok(Ok(crate::types::UsageChargeResult::Replay))
    ));

    // Different reference at same timestamp should succeed
    let reference2 = String::from_str(&test_env.env, "req_124");
    test_env
        .client
        .charge_usage_with_reference(&sub_id, &usage_amount, &reference2);
}

#[test]
fn test_idempotent_failure_codes() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &100_000_000,
        &3600,
        &false,
        &None,
        &None::<u64>,
    );

    test_env
        .stellar_token_client()
        .mint(&subscriber, &1_000_000_000i128);
    test_env
        .client
        .deposit_funds(&sub_id, &subscriber, &1_000_000_000i128);

    test_env.set_timestamp(3600);
    test_env.client.charge_subscription(&sub_id);

    // Multiple replay attempts should all return the same error code
    for _ in 0..5 {
        let res = test_env.client.try_charge_subscription(&sub_id);
        assert!(matches!(res, Err(Ok(Error::Replay))));
    }
}

#[test]
fn test_boundary_period_transitions() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let interval = 3600;
    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &100_000_000,
        &interval,
        &false,
        &None,
        &None::<u64>,
    );

    test_env
        .stellar_token_client()
        .mint(&subscriber, &1_000_000_000i128);
    test_env
        .client
        .deposit_funds(&sub_id, &subscriber, &1_000_000_000i128);

    // Exactly one second before first interval elapsed
    test_env.set_timestamp(interval - 1);
    let res_early = test_env.client.try_charge_subscription(&sub_id);
    assert!(matches!(res_early, Err(Ok(Error::IntervalNotElapsed))));

    // Exactly at interval boundary
    test_env.set_timestamp(interval);
    let res_at = test_env.client.charge_subscription(&sub_id);
    assert_eq!(res_at, ChargeExecutionResult::Charged);

    // Replay at same boundary
    let res_replay = test_env.client.try_charge_subscription(&sub_id);
    assert!(matches!(res_replay, Err(Ok(Error::Replay))));

    // Move to next interval boundary (2*interval)
    test_env.set_timestamp(2 * interval);
    let res_next = test_env.client.charge_subscription(&sub_id);
    assert_eq!(res_next, ChargeExecutionResult::Charged);
}

#[test]
fn test_same_timestamp_repeated_calls() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let interval = 4000;
    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &50_000_000,
        &interval,
        &true,
        &None,
        &None,
    );

    test_env
        .stellar_token_client()
        .mint(&subscriber, &1_000_000_000i128);
    test_env
        .client
        .deposit_funds(&sub_id, &subscriber, &1_000_000_000i128);

    // Set time far in the future
    let future_time = 40000;
    test_env.set_timestamp(future_time);

    // First charge for current period (index 10)
    let res = test_env.client.charge_subscription(&sub_id);
    assert_eq!(res, ChargeExecutionResult::Charged);

    // Repeated call at same timestamp should fail
    let res_rep = test_env.client.try_charge_subscription(&sub_id);
    assert!(matches!(res_rep, Err(Ok(Error::Replay))));

    // Usage charge at same timestamp with same reference returns Replay variant
    let ref_str = String::from_str(&test_env.env, "t1");
    test_env
        .client
        .charge_usage_with_reference(&sub_id, &1, &ref_str);
    let res_usage_rep = test_env
        .client
        .try_charge_usage_with_reference(&sub_id, &1, &ref_str);
    assert!(matches!(
        res_usage_rep,
        Ok(Ok(crate::types::UsageChargeResult::Replay))
    ));
}

#[test]
fn test_usage_charging_parity() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &100_000_000,
        &3600,
        &true,
        &None,
        &None,
    );

    test_env
        .stellar_token_client()
        .mint(&subscriber, &1_000_000_000i128);
    test_env
        .client
        .deposit_funds(&sub_id, &subscriber, &1_000_000_000i128);

    // Verify that interval charge and usage charge both update lifetime_charged correctly
    test_env.set_timestamp(3600);
    test_env.client.charge_subscription(&sub_id);

    let sub1 = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub1.lifetime_charged, 100_000_000);

    test_env.client.charge_usage_with_reference(
        &sub_id,
        &50_000_000,
        &String::from_str(&test_env.env, "u1"),
    );
    let sub2 = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub2.lifetime_charged, 150_000_000);

    // Both should contribute to merchant balance identically
    let balance = test_env.client.get_merchant_balance(&merchant);
    assert_eq!(balance, 150_000_000);
}
