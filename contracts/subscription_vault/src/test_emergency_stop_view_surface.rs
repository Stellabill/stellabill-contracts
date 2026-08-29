//! Security audit tests — emergency_stop view surface (#847)
//!
//! Verifies that:
//! 1. Every read-only view function succeeds (does not panic or return an
//!    unexpected error) while the emergency stop is active.
//! 2. Every read-only view function succeeds on an empty / not-yet-populated
//!    contract (empty-state calls).
//! 3. View functions called before `init` return safe defaults or
//!    `Error::NotFound` / `Error::NotInitialized` — never a panic.
//! 4. No view function returns data that could help an attacker bypass the
//!    emergency-stop flag (confirmed by inspecting return values).
#![cfg(test)]

use crate::{Error, SubscriptionVault, SubscriptionVaultClient};
use crate::types::PrepaidQueryRequest;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env};

const T0: u64 = 1_700_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const AMOUNT: i128 = 1_000_000;
const DEPOSIT: i128 = 50_000_000;

fn setup_full() -> (
    Env,
    SubscriptionVaultClient<'static>,
    Address,
    Address,
    Address,
    Address,
    u32,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token, &6, &admin, &AMOUNT, &(7 * 24 * 60 * 60));

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &DEPOSIT);

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    client.deposit_funds(
        &sub_id,
        &subscriber,
        &DEPOSIT,
        &None::<soroban_sdk::BytesN<32>>,
    );

    (env, client, token, admin, subscriber, merchant, sub_id)
}

fn setup_empty() -> (
    Env,
    SubscriptionVaultClient<'static>,
    Address,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token, &6, &admin, &AMOUNT, &(7 * 24 * 60 * 60));

    (env, client, token, admin)
}

fn setup_pre_init() -> (Env, SubscriptionVaultClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    (env, client)
}

#[test]
fn view_get_subscription_succeeds_while_stopped() {
    let (_env, client, _token, admin, subscriber, merchant, sub_id) = setup_full();
    client.enable_emergency_stop(&admin);
    assert!(client.get_emergency_stop_status());

    let sub = client.get_subscription(&sub_id).expect("get_subscription failed under stop");
    assert_eq!(sub.subscriber, subscriber);
    assert_eq!(sub.merchant, merchant);
    assert_eq!(sub.amount, AMOUNT);
    assert_eq!(sub.prepaid_balance, DEPOSIT);
}

#[test]
fn view_estimate_topup_succeeds_while_stopped() {
    let (_env, client, _token, admin, _subscriber, _merchant, sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let topup = client
        .estimate_topup_for_intervals(&sub_id, &1)
        .expect("estimate_topup failed under stop");
    assert_eq!(topup, 0i128);
}

#[test]
fn view_get_next_charge_info_succeeds_while_stopped() {
    let (_env, client, _token, admin, _subscriber, _merchant, sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let info = client
        .get_next_charge_info(&sub_id)
        .expect("get_next_charge_info failed under stop");
    assert!(info.is_charge_expected);
    assert_eq!(info.amount, AMOUNT);
}

#[test]
fn view_get_cap_info_succeeds_while_stopped() {
    let (_env, client, _token, admin, _subscriber, _merchant, sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let cap = client
        .get_cap_info(&sub_id)
        .expect("get_cap_info failed under stop");
    assert!(!cap.cap_reached);
    assert_eq!(cap.lifetime_cap, None);
}

#[test]
fn view_get_merchant_subscription_count_while_stopped() {
    let (_env, client, _token, admin, _subscriber, merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let count = client.get_merchant_subscription_count(&merchant);
    assert_eq!(count, 1u32);
}

#[test]
fn view_get_token_subscription_count_while_stopped() {
    let (_env, client, token, admin, _subscriber, _merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let count = client.get_token_subscription_count(&token);
    assert_eq!(count, 1u32);
}

#[test]
fn view_get_subscriptions_by_merchant_while_stopped() {
    let (_env, client, _token, admin, _subscriber, merchant, sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let page = client
        .get_subscriptions_by_merchant(&merchant, &0u32, &10u32)
        .expect("get_subscriptions_by_merchant failed");
    assert_eq!(page.len(), 1u32);
    assert_eq!(page.get(0).unwrap().amount, AMOUNT);
    let _ = sub_id;
}

#[test]
fn view_get_subscriptions_by_token_while_stopped() {
    let (_env, client, token, admin, _subscriber, _merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let page = client
        .get_subscriptions_by_token(&token, &0u32, &10u32)
        .expect("get_subscriptions_by_token failed");
    assert_eq!(page.len(), 1u32);
}

#[test]
fn view_list_subscriptions_by_subscriber_while_stopped() {
    let (_env, client, _token, admin, subscriber, _merchant, sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let page = client
        .list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32)
        .expect("list_subscriptions_by_subscriber failed");
    assert_eq!(page.subscription_ids.len(), 1u32);
    assert_eq!(page.subscription_ids.get(0).unwrap(), sub_id);
}

#[test]
fn view_get_plan_max_active_subs_while_stopped() {
    let (_env, client, _token, admin, _subscriber, _merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);
    let limit = client.get_plan_max_active_subs(&0u32);
    assert_eq!(limit, 0u32);
}

#[test]
fn view_get_merchant_max_subs_while_stopped() {
    let (_env, client, _token, admin, _subscriber, merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);
    let max = client.get_merchant_max_subs(&merchant);
    assert_eq!(max, u32::MAX);
}

#[test]
fn view_config_views_while_stopped() {
    let (_env, client, _token, admin, _subscriber, _merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    assert_eq!(client.get_admin().unwrap(), admin);
    assert!(client.get_min_topup().is_ok());
    assert!(client.get_emergency_stop_status());
}

#[test]
fn view_query_prepaid_balances_paginated_while_stopped() {
    let (_env, client, token, admin, _subscriber, _merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let req = PrepaidQueryRequest {
        token: token.clone(),
        start_subscription_id: 0,
        scan_limit: 100,
    };
    let result = client.query_prepaid_balances_paginated(&req);
    assert_eq!(result.token, token);
    assert!(result.partial_total >= 0);
}

#[test]
fn view_get_subscription_empty_state() {
    let (_env, client, _token, _admin) = setup_empty();
    let result = client.try_get_subscription(&0u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn view_estimate_topup_empty_state() {
    let (_env, client, _token, _admin) = setup_empty();
    let result = client.try_estimate_topup_for_intervals(&0u32, &5u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn view_get_next_charge_info_empty_state() {
    let (_env, client, _token, _admin) = setup_empty();
    let result = client.try_get_next_charge_info(&0u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn view_get_cap_info_empty_state() {
    let (_env, client, _token, _admin) = setup_empty();
    let result = client.try_get_cap_info(&0u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn view_counts_empty_state() {
    let (env, client, token, _admin) = setup_empty();
    let merchant = Address::generate(&env);

    assert_eq!(client.get_merchant_subscription_count(&merchant), 0u32);
    assert_eq!(client.get_token_subscription_count(&token), 0u32);
}

#[test]
fn view_get_subscriptions_by_merchant_empty_state() {
    let (env, client, _token, _admin) = setup_empty();
    let merchant = Address::generate(&env);
    let page = client
        .get_subscriptions_by_merchant(&merchant, &0u32, &10u32)
        .expect("should return Ok with empty list");
    assert_eq!(page.len(), 0u32);
}

#[test]
fn view_get_subscriptions_by_token_empty_state() {
    let (env, client, token, _admin) = setup_empty();
    let _ = env;
    let page = client
        .get_subscriptions_by_token(&token, &0u32, &10u32)
        .expect("should return Ok with empty list");
    assert_eq!(page.len(), 0u32);
}

#[test]
fn view_list_subscriptions_by_subscriber_empty_state() {
    let (env, client, _token, _admin) = setup_empty();
    let subscriber = Address::generate(&env);
    let page = client
        .list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32)
        .expect("should return Ok with empty page");
    assert_eq!(page.subscription_ids.len(), 0u32);
    assert_eq!(page.next_start_id, None);
}

#[test]
fn view_query_prepaid_balances_paginated_empty_state() {
    let (env, client, token, _admin) = setup_empty();
    let _ = env;
    let req = PrepaidQueryRequest {
        token: token.clone(),
        start_subscription_id: 0,
        scan_limit: 100,
    };
    let result = client.query_prepaid_balances_paginated(&req);
    assert_eq!(result.partial_total, 0i128);
    assert_eq!(result.subscriptions_count, 0u32);
    assert!(!result.has_more);
}

#[test]
fn view_limit_defaults_empty_state() {
    let (env, client, _token, _admin) = setup_empty();
    let merchant = Address::generate(&env);
    assert_eq!(client.get_plan_max_active_subs(&99u32), 0u32);
    assert_eq!(client.get_merchant_max_subs(&merchant), u32::MAX);
}

#[test]
fn view_get_subscription_pre_init() {
    let (_env, client) = setup_pre_init();
    let result = client.try_get_subscription(&0u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn view_estimate_topup_pre_init() {
    let (_env, client) = setup_pre_init();
    let result = client.try_estimate_topup_for_intervals(&0u32, &3u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn view_get_next_charge_info_pre_init() {
    let (_env, client) = setup_pre_init();
    let result = client.try_get_next_charge_info(&0u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn view_get_cap_info_pre_init() {
    let (_env, client) = setup_pre_init();
    let result = client.try_get_cap_info(&0u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn view_get_admin_pre_init() {
    let (_env, client) = setup_pre_init();
    let result = client.try_get_admin();
    assert_eq!(result, Err(Ok(Error::NotInitialized)));
}

#[test]
fn view_get_min_topup_pre_init() {
    let (_env, client) = setup_pre_init();
    let result = client.try_get_min_topup();
    assert_eq!(result, Err(Ok(Error::NotInitialized)));
}

#[test]
fn view_emergency_stop_status_pre_init_is_false() {
    let (_env, client) = setup_pre_init();
    assert!(!client.get_emergency_stop_status());
}

#[test]
fn view_counts_pre_init_return_zero() {
    let (env, client) = setup_pre_init();
    let addr = Address::generate(&env);
    assert_eq!(client.get_merchant_subscription_count(&addr), 0u32);
    assert_eq!(client.get_token_subscription_count(&addr), 0u32);
}

#[test]
fn view_list_by_subscriber_pre_init_returns_empty() {
    let (env, client) = setup_pre_init();
    let subscriber = Address::generate(&env);
    let page = client
        .list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32)
        .expect("pre-init list should return Ok empty");
    assert_eq!(page.subscription_ids.len(), 0u32);
}

#[test]
fn view_limit_defaults_pre_init() {
    let (env, client) = setup_pre_init();
    let addr = Address::generate(&env);
    assert_eq!(client.get_plan_max_active_subs(&0u32), 0u32);
    assert_eq!(client.get_merchant_max_subs(&addr), u32::MAX);
}

#[test]
fn view_get_admin_while_stopped_does_not_aid_bypass() {
    let (_env, client, token, admin, subscriber, merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let reported_admin = client.get_admin().expect("get_admin failed");
    assert_eq!(reported_admin, admin);

    let result = client.try_create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert_eq!(result, Err(Ok(Error::EmergencyStopActive)));
    let _ = token;
}

#[test]
fn view_nonce_views_while_stopped_do_not_aid_bypass() {
    let (env, client, _token, admin, subscriber, merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let _admin_nonce = client.get_admin_nonce(&admin, &0u32);
    let operator = Address::generate(&env);
    let _op_nonce = client.get_operator_nonce(&operator);

    let result = client.try_create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert_eq!(result, Err(Ok(Error::EmergencyStopActive)));
}

#[test]
fn view_reconciliation_proof_contains_no_bypass_data() {
    let (_env, client, token, admin, _subscriber, _merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let proof = client.generate_reconciliation_proof(&token);
    assert!(proof.total_prepaid >= 0);
    assert!(proof.contract_balance >= 0);
    assert!(proof.is_valid);
}

#[test]
fn view_surface_audit_stop_re_enabled_mutations_blocked_again() {
    let (env, client, _token, admin, subscriber, merchant, _sub_id) = setup_full();

    client.enable_emergency_stop(&admin);
    assert_eq!(
        client.try_create_subscription(
            &subscriber,
            &merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
            &None::<u32>,
            &None::<soroban_sdk::Symbol>,
        ),
        Err(Ok(Error::EmergencyStopActive))
    );

    env.ledger()
        .with_mut(|li| li.timestamp += crate::admin::CONFIG_COOLDOWN_SECS + 1);

    client.disable_emergency_stop(&admin);
    assert!(!client.get_emergency_stop_status());

    let new_sub = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert!(new_sub > 0);

    env.ledger()
        .with_mut(|li| li.timestamp += crate::admin::CONFIG_COOLDOWN_SECS + 1);
    client.enable_emergency_stop(&admin);
    assert_eq!(
        client.try_create_subscription(
            &subscriber,
            &merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
            &None::<u32>,
            &None::<soroban_sdk::Symbol>,
        ),
        Err(Ok(Error::EmergencyStopActive))
    );
}
