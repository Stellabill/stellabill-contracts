//! Unit tests confirming view function surface safety under `emergency_stop` active, pre-initialization, and empty state.

#![cfg(test)]

use crate::{
    types::PrepaidQueryRequest, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{testutils::Address as _, Address, Env};

fn setup_vault() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    client.init(&token, &7, &admin, &100_000, &86400);

    (env, client, admin, token)
}

#[test]
fn test_view_functions_with_emergency_stop_active() {
    let (env, client, admin, token) = setup_vault();

    // Enable emergency stop
    client.enable_emergency_stop(&admin);
    assert!(client.get_emergency_stop_status());

    // 1. get_subscription for non-existent ID returns error
    let res = client.try_get_subscription(&99);
    assert!(res.is_err());

    // 2. estimate_topup_for_intervals returns error
    let res = client.try_estimate_topup_for_intervals(&99, &3);
    assert!(res.is_err());

    // 3. get_subscriptions_by_merchant returns empty list
    let merchant = Address::generate(&env);
    let subs = client.get_subscriptions_by_merchant(&merchant, &0, &10);
    assert_eq!(subs.len(), 0);

    // 4. get_merchant_subscription_count returns 0
    let count = client.get_merchant_subscription_count(&merchant);
    assert_eq!(count, 0);

    // 5. get_token_subscription_count returns 0
    let count = client.get_token_subscription_count(&token);
    assert_eq!(count, 0);

    // 6. get_subscriptions_by_token returns empty list
    let subs = client.get_subscriptions_by_token(&token, &0, &10);
    assert_eq!(subs.len(), 0);

    // 7. get_next_charge_info returns error
    let res = client.try_get_next_charge_info(&99);
    assert!(res.is_err());

    // 8. get_cap_info returns error
    let res = client.try_get_cap_info(&99);
    assert!(res.is_err());

    // 9. get_plan_max_active_subs returns 0
    let plan_max = client.get_plan_max_active_subs(&1);
    assert_eq!(plan_max, 0);

    // 10. list_subscriptions_by_subscriber returns empty page
    let subscriber = Address::generate(&env);
    let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &10);
    assert_eq!(page.subscription_ids.len(), 0);
    assert_eq!(page.next_start_id, None);

    // 11. query_prepaid_balances_paginated returns empty result
    let req = PrepaidQueryRequest {
        token: token.clone(),
        start_subscription_id: 0,
        scan_limit: 10,
    };
    let prep_res = client.query_prepaid_balances_paginated(&req);
    assert_eq!(prep_res.partial_total, 0);
    assert_eq!(prep_res.subscriptions_count, 0);
    assert!(!prep_res.has_more);
}

#[test]
fn test_view_functions_uninitialized_and_empty_state() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    // Prior to init:
    assert!(!client.get_emergency_stop_status());

    let merchant = Address::generate(&env);
    let token = Address::generate(&env);
    let subscriber = Address::generate(&env);

    assert_eq!(client.get_merchant_subscription_count(&merchant), 0);
    assert_eq!(client.get_token_subscription_count(&token), 0);
    assert_eq!(client.get_plan_max_active_subs(&1), 0);

    let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &10);
    assert_eq!(page.subscription_ids.len(), 0);
}
