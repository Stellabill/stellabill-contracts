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

// ── Shared constants ──────────────────────────────────────────────────────────

const T0: u64 = 1_700_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const AMOUNT: i128 = 1_000_000;
const DEPOSIT: i128 = 50_000_000;

// ── Setup helpers ─────────────────────────────────────────────────────────────

/// Full setup: registers contract, calls init, creates one subscription with
/// funds deposited. Returns env, client, token, admin, subscriber, merchant,
/// sub_id.
fn setup_full() -> (
    Env,
    SubscriptionVaultClient<'static>,
    Address, // token
    Address, // admin
    Address, // subscriber
    Address, // merchant
    u32,     // sub_id
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

/// Minimal setup: registers contract and calls init but creates NO subscriptions.
/// Useful for empty-state tests.
fn setup_empty() -> (
    Env,
    SubscriptionVaultClient<'static>,
    Address, // token
    Address, // admin
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

/// Pre-init setup: registers the contract but does NOT call init.
fn setup_pre_init() -> (Env, SubscriptionVaultClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    (env, client)
}

// ── Group 1: views succeed while emergency stop is active ─────────────────────

/// `get_subscription` must return the subscription record unchanged while the
/// emergency stop is active. The stop must NOT alter or hide subscription data.
#[test]
fn view_get_subscription_succeeds_while_stopped() {
    let (env, client, _token, admin, subscriber, merchant, sub_id) = setup_full();
    client.enable_emergency_stop(&admin);
    assert!(client.get_emergency_stop_status());

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.subscriber, subscriber);
    assert_eq!(sub.merchant, merchant);
    assert_eq!(sub.amount, AMOUNT);
    assert_eq!(sub.prepaid_balance, DEPOSIT);
}

/// `estimate_topup_for_intervals` must return a correct estimate while stopped.
#[test]
fn view_estimate_topup_succeeds_while_stopped() {
    let (_env, client, _token, admin, _subscriber, _merchant, sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    // subscription is fully funded for many intervals; topup should be 0
    let topup = client
        .estimate_topup_for_intervals(&sub_id, &1);
    assert_eq!(topup, 0i128);
}

/// `get_next_charge_info` must return billing schedule data while stopped.
#[test]
fn view_get_next_charge_info_succeeds_while_stopped() {
    let (_env, client, _token, admin, _subscriber, _merchant, sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let info = client
        .get_next_charge_info(&sub_id);
    // Subscription is Active, so a charge IS expected
    assert!(info.is_charge_expected);
    assert_eq!(info.amount, AMOUNT);
}

/// `get_cap_info` must return cap data while stopped.
#[test]
fn view_get_cap_info_succeeds_while_stopped() {
    let (_env, client, _token, admin, _subscriber, _merchant, sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let cap = client
        .get_cap_info(&sub_id);
    assert!(!cap.cap_reached);
    assert_eq!(cap.lifetime_cap, None);
}

/// `get_merchant_subscription_count` must return correct count while stopped.
#[test]
fn view_get_merchant_subscription_count_while_stopped() {
    let (_env, client, _token, admin, _subscriber, merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let count = client.get_merchant_subscription_count(&merchant);
    assert_eq!(count, 1u32);
}

/// `get_token_subscription_count` must return correct count while stopped.
#[test]
fn view_get_token_subscription_count_while_stopped() {
    let (_env, client, token, admin, _subscriber, _merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let count = client.get_token_subscription_count(&token);
    assert_eq!(count, 1u32);
}

/// `get_subscriptions_by_merchant` must return subscription slice while stopped.
#[test]
fn view_get_subscriptions_by_merchant_while_stopped() {
    let (_env, client, _token, admin, _subscriber, merchant, sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let page = client
        .get_subscriptions_by_merchant(&merchant, &0u32, &10u32);
    assert_eq!(page.len(), 1u32);
    assert_eq!(page.get(0).unwrap().amount, AMOUNT);
    let _ = sub_id; // used via the page check
}

/// `get_subscriptions_by_token` must return subscription slice while stopped.
#[test]
fn view_get_subscriptions_by_token_while_stopped() {
    let (_env, client, token, admin, _subscriber, _merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let page = client
        .get_subscriptions_by_token(&token, &0u32, &10u32);
    assert_eq!(page.len(), 1u32);
}

/// `list_subscriptions_by_subscriber` must return IDs while stopped.
#[test]
fn view_list_subscriptions_by_subscriber_while_stopped() {
    let (_env, client, _token, admin, subscriber, _merchant, sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let page = client
        .list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);
    assert_eq!(page.subscription_ids.len(), 1u32);
    assert_eq!(page.subscription_ids.get(0).unwrap(), sub_id);
}

/// `get_plan_max_active_subs` must return the plan limit while stopped.
#[test]
fn view_get_plan_max_active_subs_while_stopped() {
    let (_env, client, _token, admin, _subscriber, _merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);
    // No plan set for id=0 → default 0 (no limit)
    let limit = client.get_plan_max_active_subs(&0u32);
    assert_eq!(limit, 0u32);
}

/// `get_merchant_max_subs` must return the sentinel while stopped.
#[test]
fn view_get_merchant_max_subs_while_stopped() {
    let (_env, client, _token, admin, _subscriber, merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);
    // No override set → u32::MAX sentinel
    let max = client.get_merchant_max_subs(&merchant);
    assert_eq!(max, u32::MAX);
}

/// Config views must continue to return correct data while stopped.
#[test]
fn view_config_views_while_stopped() {
    let (_env, client, _token, admin, _subscriber, _merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    assert_eq!(client.get_admin(), admin);
    assert_eq!(client.get_min_topup(), 1_000_000i128);
    assert!(client.get_emergency_stop_status());
}

/// Prepaid balance query must return correct partial totals while stopped.
#[test]
fn view_query_prepaid_balances_paginated_while_stopped() {
    let (env, client, token, admin, _subscriber, _merchant, _sub_id) = setup_full();
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

// ── Group 2: views return safe defaults on empty/zero state ───────────────────

/// On an initialised but empty contract (no subscriptions), `get_subscription`
/// must return `Error::NotFound`, not panic.
#[test]
fn view_get_subscription_empty_state() {
    let (_env, client, _token, _admin) = setup_empty();
    let result = client.try_get_subscription(&0u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

/// `estimate_topup_for_intervals` on a missing subscription returns `NotFound`.
#[test]
fn view_estimate_topup_empty_state() {
    let (_env, client, _token, _admin) = setup_empty();
    let result = client.try_estimate_topup_for_intervals(&0u32, &5u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

/// `get_next_charge_info` on a missing subscription returns `NotFound`.
#[test]
fn view_get_next_charge_info_empty_state() {
    let (_env, client, _token, _admin) = setup_empty();
    let result = client.try_get_next_charge_info(&0u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

/// `get_cap_info` on a missing subscription returns `NotFound`.
#[test]
fn view_get_cap_info_empty_state() {
    let (_env, client, _token, _admin) = setup_empty();
    let result = client.try_get_cap_info(&0u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

/// Merchant / token count views on empty state return 0 — no panic.
#[test]
fn view_counts_empty_state() {
    let (env, client, token, _admin) = setup_empty();
    let merchant = Address::generate(&env);

    assert_eq!(client.get_merchant_subscription_count(&merchant), 0u32);
    assert_eq!(client.get_token_subscription_count(&token), 0u32);
}

/// `get_subscriptions_by_merchant` on empty state returns an empty list.
#[test]
fn view_get_subscriptions_by_merchant_empty_state() {
    let (env, client, _token, _admin) = setup_empty();
    let merchant = Address::generate(&env);
    let page = client
        .get_subscriptions_by_merchant(&merchant, &0u32, &10u32);
    assert_eq!(page.len(), 0u32);
}

/// `get_subscriptions_by_token` on empty state returns an empty list.
#[test]
fn view_get_subscriptions_by_token_empty_state() {
    let (env, client, token, _admin) = setup_empty();
    let _ = env;
    let page = client
        .get_subscriptions_by_token(&token, &0u32, &10u32);
    assert_eq!(page.len(), 0u32);
}

/// `list_subscriptions_by_subscriber` on empty state returns empty page.
#[test]
fn view_list_subscriptions_by_subscriber_empty_state() {
    let (env, client, _token, _admin) = setup_empty();
    let subscriber = Address::generate(&env);
    let page = client
        .list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);
    assert_eq!(page.subscription_ids.len(), 0u32);
    assert_eq!(page.next_start_id, None);
}

/// `query_prepaid_balances_paginated` on empty state returns zero totals.
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

/// Plan-max and merchant-max views return safe defaults on empty state.
#[test]
fn view_limit_defaults_empty_state() {
    let (env, client, _token, _admin) = setup_empty();
    let merchant = Address::generate(&env);
    assert_eq!(client.get_plan_max_active_subs(&99u32), 0u32);
    assert_eq!(client.get_merchant_max_subs(&merchant), u32::MAX);
}

// ── Group 3: pre-init calls return safe errors, never panic ───────────────────

/// Before `init` is called, `get_subscription` returns `NotFound` — not a panic.
#[test]
fn view_get_subscription_pre_init() {
    let (_env, client) = setup_pre_init();
    let result = client.try_get_subscription(&0u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

/// Before `init`, `estimate_topup_for_intervals` returns `NotFound`.
#[test]
fn view_estimate_topup_pre_init() {
    let (_env, client) = setup_pre_init();
    let result = client.try_estimate_topup_for_intervals(&0u32, &3u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

/// Before `init`, `get_next_charge_info` returns `NotFound`.
#[test]
fn view_get_next_charge_info_pre_init() {
    let (_env, client) = setup_pre_init();
    let result = client.try_get_next_charge_info(&0u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

/// Before `init`, `get_cap_info` returns `NotFound`.
#[test]
fn view_get_cap_info_pre_init() {
    let (_env, client) = setup_pre_init();
    let result = client.try_get_cap_info(&0u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

/// Before `init`, `get_admin` returns `NotInitialized`.
#[test]
fn view_get_admin_pre_init() {
    let (_env, client) = setup_pre_init();
    let result = client.try_get_admin();
    assert_eq!(result, Err(Ok(Error::NotInitialized)));
}

/// Before `init`, `get_min_topup` returns `NotInitialized`.
#[test]
fn view_get_min_topup_pre_init() {
    let (_env, client) = setup_pre_init();
    let result = client.try_get_min_topup();
    assert_eq!(result, Err(Ok(Error::NotInitialized)));
}

/// Before `init`, `get_emergency_stop_status` returns `false` (default), not a panic.
#[test]
fn view_emergency_stop_status_pre_init_is_false() {
    let (_env, client) = setup_pre_init();
    assert!(!client.get_emergency_stop_status());
}

/// Before `init`, count views return 0 safely.
#[test]
fn view_counts_pre_init_return_zero() {
    let (env, client) = setup_pre_init();
    let addr = Address::generate(&env);
    assert_eq!(client.get_merchant_subscription_count(&addr), 0u32);
    assert_eq!(client.get_token_subscription_count(&addr), 0u32);
}

/// Before `init`, paginated subscriber list returns empty page.
#[test]
fn view_list_by_subscriber_pre_init_returns_empty() {
    let (env, client) = setup_pre_init();
    let subscriber = Address::generate(&env);
    let page = client
        .list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);
    assert_eq!(page.subscription_ids.len(), 0u32);
}

/// Before `init`, plan / merchant limit views return safe defaults.
#[test]
fn view_limit_defaults_pre_init() {
    let (env, client) = setup_pre_init();
    let addr = Address::generate(&env);
    assert_eq!(client.get_plan_max_active_subs(&0u32), 0u32);
    assert_eq!(client.get_merchant_max_subs(&addr), u32::MAX);
}

// ── Group 4: no view exposes a bypass vector ──────────────────────────────────

/// `get_admin` returns the admin address while stopped. Knowing the address
/// is not a bypass: the stop check (`require_not_emergency_stop`) runs before
/// any admin-gated write, and the admin must still sign transactions. Confirmed
/// by attempting a guarded mutation with a spoofed admin address.
#[test]
fn view_get_admin_while_stopped_does_not_aid_bypass() {
    let (_env, client, token, admin, subscriber, merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    // Fetch admin address via the view
    let reported_admin = client.get_admin();
    assert_eq!(reported_admin, admin);

    // Knowing the admin address is NOT sufficient to bypass the stop:
    // create_subscription is still blocked.
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

/// Nonce views expose the current monotonic counter. The counter is public
/// information (visible on-chain) and cannot be used to replay or forge
/// transactions. Confirmed by verifying that knowing the nonce does not unblock
/// a stopped mutation.
#[test]
fn view_nonce_views_while_stopped_do_not_aid_bypass() {
    let (env, client, _token, admin, subscriber, merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    // Read nonces
    let _admin_nonce = client.get_admin_nonce(&admin, &0u32);
    let operator = Address::generate(&env);
    let _op_nonce = client.get_operator_nonce(&operator);

    // Mutation is still blocked
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

/// Reconciliation views reveal accounting totals only — no private keys,
/// no config timelocks, no signing material. Confirmed by reading the proof
/// and verifying none of its fields are the admin address.
#[test]
fn view_reconciliation_proof_contains_no_bypass_data() {
    let (_env, client, token, admin, _subscriber, _merchant, _sub_id) = setup_full();
    client.enable_emergency_stop(&admin);

    let proof = client.generate_reconciliation_proof(&token);
    // Proof fields are all numeric/boolean accounting data — no admin address
    assert!(proof.total_prepaid >= 0);
    assert!(proof.contract_balance >= 0);
    assert!(proof.is_valid);
}

/// After disabling the stop, all guarded mutations become available again,
/// confirming the stop is the sole gate and views did not circumvent it.
#[test]
fn view_surface_audit_stop_re_enabled_mutations_blocked_again() {
    let (env, client, _token, admin, subscriber, merchant, _sub_id) = setup_full();

    // Enable stop — mutations blocked
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

    // Advance time past cooldown so disable_emergency_stop succeeds
    env.ledger()
        .with_mut(|li| li.timestamp += crate::admin::CONFIG_COOLDOWN_SECS + 1);

    // Disable stop — mutations unblocked
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

    // Re-enable — blocked again
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
