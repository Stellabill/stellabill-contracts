#![cfg(test)]

use crate::test_utils::setup::TestEnv;
use crate::types::Error;
use soroban_sdk::{testutils::Address as _, Address};

const AMOUNT: i128 = 10_000_000; // 10 USDC
const INTERVAL: u64 = 86400 * 30; // 30 days

#[test]
fn test_multiple_versions_with_active_subscribers() {
    let test_env = TestEnv::default();
    let merchant = Address::generate(&test_env.env);
    let sub1 = Address::generate(&test_env.env);
    let sub2 = Address::generate(&test_env.env);

    // V1 Plan
    let plan_id1 =
        test_env
            .client
            .create_plan_template(&merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>);

    // Sub1 subscribes to V1
    let sub_id1 = test_env
        .client
        .create_subscription_from_plan(&sub1, &plan_id1);

    // Update Plan to V2
    let plan_id2 = test_env.client.update_plan_template(
        &merchant,
        &plan_id1,
        &(AMOUNT * 2), // Double price
        &INTERVAL,
        &false,
        &None::<i128>,
    );

    // Sub2 subscribes to V2
    let sub_id2 = test_env
        .client
        .create_subscription_from_plan(&sub2, &plan_id2);

    // Validate V1 subscriber continues on old terms
    let subscription1 = test_env.client.get_subscription(&sub_id1);
    assert_eq!(subscription1.amount, AMOUNT);

    // Validate V2 subscriber has new terms
    let subscription2 = test_env.client.get_subscription(&sub_id2);
    assert_eq!(subscription2.amount, AMOUNT * 2);

    // Active migrations work correctly
    test_env
        .client
        .migrate_subscription_to_plan(&sub1, &sub_id1, &plan_id2);
    let migrated_sub1 = test_env.client.get_subscription(&sub_id1);
    assert_eq!(migrated_sub1.amount, AMOUNT * 2);
}

#[test]
fn test_plan_disablement() {
    let test_env = TestEnv::default();
    let merchant = Address::generate(&test_env.env);
    let subscriber = Address::generate(&test_env.env);

    let plan_id =
        test_env
            .client
            .create_plan_template(&merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>);

    // Sub1 subscribes
    let sub_id = test_env
        .client
        .create_subscription_from_plan(&subscriber, &plan_id);

    // Disable plan
    test_env.client.disable_plan_template(&merchant, &plan_id);

    let plan = test_env.client.get_plan_template(&plan_id);
    assert_eq!(plan.is_disabled, true);

    // Existing subscriptions shouldn't be affected (they can still be checked/exist actively)
    let subscription = test_env.client.get_subscription(&sub_id);
    assert_eq!(subscription.amount, AMOUNT);

    // New subscriptions MUST fail
    let user2 = Address::generate(&test_env.env);
    let result = test_env
        .client
        .try_create_subscription_from_plan(&user2, &plan_id);
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}
