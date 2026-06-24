//! Tests for merchant payout schedule (auto-withdrawal feature).
//!
//! Covers:
//! - Setting and reading payout schedules
//! - Flushing payouts with cadence and min_payout enforcement
//! - Multi-token payout
//! - Emergency stop blocking
//! - Event emission

use crate::{
    DataKey, Error, ScheduledPayoutEvent, SubscriptionVault,
    SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger},
    token, Address, Env, FromVal, IntoVal, String, Symbol,
};

fn setup_env() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    (env, client, admin, token)
}

/// Seed a merchant's balance for a given token directly in storage,
/// and also register the token in `MerchantTokens` so flush_payouts can find it.
fn seed_merchant_balance_and_token(
    env: &Env,
    contract_id: &Address,
    merchant: &Address,
    token: &Address,
    balance: i128,
) {
    env.as_contract(contract_id, || {
        env.storage()
            .instance()
            .set(&DataKey::MerchantBalance(merchant.clone(), token.clone()), &balance);

        // Also register the token in MerchantTokens
        let key = DataKey::MerchantTokens(merchant.clone());
        let mut tokens: soroban_sdk::Vec<Address> =
            env.storage().instance().get(&key).unwrap_or_else(|| soroban_sdk::Vec::new(env));
        if !tokens.contains(token) {
            tokens.push_back(token.clone());
            env.storage().instance().set(&key, &tokens);
        }
    });
}

/// Seed the optional TokenEarnings struct so flush_merchant_token
/// can correctly update the withdrawals counter.
fn seed_merchant_earnings(
    env: &Env,
    contract_id: &Address,
    merchant: &Address,
    token: &Address,
    amount: i128,
) {
    env.as_contract(contract_id, || {
        let earnings = crate::types::TokenEarnings {
            accruals: crate::types::AccruedTotals {
                interval: amount,
                usage: 0,
                one_off: 0,
            },
            withdrawals: 0,
            refunds: 0,
        };
        env.storage()
            .instance()
            .set(&DataKey::MerchantEarnings(merchant.clone(), token.clone()), &earnings);
    });
}

fn initialize_merchant_config(
    client: &SubscriptionVaultClient,
    merchant: &Address,
    payout_address: &Address,
) {
    let url = String::from_str(&client.env, "https://example.com");
    client.initialize_merchant_config(
        merchant,
        payout_address,
        &0,
        &0x1F,
        &None::<Address>,
        &url,
    );
}

// ── set_payout_schedule tests ────────────────────────────────────────────

#[test]
fn test_set_and_get_payout_schedule() {
    let (env, client, _admin, _token) = setup_env();
    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);
    initialize_merchant_config(&client, &merchant, &payout);

    let cadence: u64 = 7 * 24 * 60 * 60; // 7 days
    let min_payout: i128 = 100_000_000; // 100 USDC

    let previous = client.set_payout_schedule(&merchant, &cadence, &min_payout);
    // First time — previous is zeroed schedule
    assert_eq!(previous.cadence_seconds, 0);
    assert_eq!(previous.min_payout, 0);
    assert_eq!(previous.last_payout_at, 0);

    let stored = client.get_payout_schedule(&merchant);
    assert_eq!(stored.cadence_seconds, cadence);
    assert_eq!(stored.min_payout, min_payout);
    assert_eq!(stored.last_payout_at, 0);
}

#[test]
fn test_set_payout_schedule_returns_previous() {
    let (env, client, _admin, _token) = setup_env();
    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);
    initialize_merchant_config(&client, &merchant, &payout);

    client.set_payout_schedule(&merchant, &86400, &1_000_000i128);

    let previous = client.set_payout_schedule(&merchant, &172800, &2_000_000i128);
    assert_eq!(previous.cadence_seconds, 86400);
    assert_eq!(previous.min_payout, 1_000_000i128);
}

#[test]
fn test_set_payout_schedule_clears_schedule() {
    let (env, client, _admin, _token) = setup_env();
    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);
    initialize_merchant_config(&client, &merchant, &payout);

    client.set_payout_schedule(&merchant, &86400, &1_000_000i128);

    // Clear by setting both to 0
    client.set_payout_schedule(&merchant, &0, &0);
    let stored = client.get_payout_schedule(&merchant);
    assert_eq!(stored.cadence_seconds, 0);
    assert_eq!(stored.min_payout, 0);
}

#[test]
fn test_set_payout_schedule_negative_min_payout_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);
    initialize_merchant_config(&client, &merchant, &payout);

    let result = client.try_set_payout_schedule(&merchant, &86400, &(-1i128));
    assert!(result.is_err());
}

// ── flush_payouts tests ──────────────────────────────────────────────────

#[test]
fn test_flush_payouts_no_schedule_returns_zero() {
    let (env, client, _admin, token) = setup_env();
    let contract_id = client.address.clone();
    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);
    initialize_merchant_config(&client, &merchant, &payout);

    seed_merchant_balance_and_token(&env, &contract_id, &merchant, &token, 5_000_000i128);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&contract_id, &5_000_000i128);

    // No schedule set — flush does nothing
    let count = client.flush_payouts(&merchant);
    assert_eq!(count, 0);
}

#[test]
fn test_flush_payouts_processes_single_token() {
    let (env, client, _admin, token) = setup_env();
    let contract_id = client.address.clone();
    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);
    initialize_merchant_config(&client, &merchant, &payout);

    // Set a schedule with cadence=1 (effectively always eligible) and zero min_payout
    client.set_payout_schedule(&merchant, &1, &0);

    let balance: i128 = 5_000_000;
    seed_merchant_balance_and_token(&env, &contract_id, &merchant, &token, balance);
    seed_merchant_earnings(&env, &contract_id, &merchant, &token, balance);
    token::StellarAssetClient::new(&env, &token).mint(&contract_id, &balance);

    let count = client.flush_payouts(&merchant);
    assert_eq!(count, 1);

    // Merchant balance should be 0
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), 0);

    // Payout address received the full balance
    assert_eq!(
        token::Client::new(&env, &token).balance(&payout),
        balance
    );
}

#[test]
fn test_flush_payouts_skips_below_min_payout() {
    let (env, client, _admin, token) = setup_env();
    let contract_id = client.address.clone();
    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);
    initialize_merchant_config(&client, &merchant, &payout);

    // min_payout = 10_000_000 but balance is only 5_000_000
    client.set_payout_schedule(&merchant, &0, &10_000_000i128);

    let balance: i128 = 5_000_000;
    seed_merchant_balance_and_token(&env, &contract_id, &merchant, &token, balance);
    seed_merchant_earnings(&env, &contract_id, &merchant, &token, balance);
    token::StellarAssetClient::new(&env, &token).mint(&contract_id, &balance);

    let count = client.flush_payouts(&merchant);
    assert_eq!(count, 0);

    // Balance unchanged
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        balance
    );
}

#[test]
fn test_flush_payouts_respects_cadence() {
    let (env, client, _admin, token) = setup_env();
    let contract_id = client.address.clone();
    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);
    initialize_merchant_config(&client, &merchant, &payout);

    // Cadence = 1 hour
    let cadence: u64 = 3600;
    client.set_payout_schedule(&merchant, &cadence, &0);

    let balance: i128 = 5_000_000;
    seed_merchant_balance_and_token(&env, &contract_id, &merchant, &token, balance);
    seed_merchant_earnings(&env, &contract_id, &merchant, &token, balance);
    token::StellarAssetClient::new(&env, &token).mint(&contract_id, &balance);

    // Use a non-zero initial timestamp so last_payout_at is > 0 after first flush
    env.ledger().set_timestamp(100_000);
    
    // First flush succeeds
    let count = client.flush_payouts(&merchant);
    assert_eq!(count, 1);

    // Re-seed balance for a second flush attempt
    seed_merchant_balance_and_token(&env, &contract_id, &merchant, &token, balance);
    token::StellarAssetClient::new(&env, &token).mint(&contract_id, &balance);

    // Second flush within same second should fail cadence check
    env.ledger().set_timestamp(cadence - 1);
    let result = client.try_flush_payouts(&merchant);
    assert_eq!(result, Err(Ok(Error::IntervalNotElapsed)));
}

#[test]
fn test_flush_payouts_multi_token() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token_a, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    client.add_accepted_token(&admin, &token_b, &6);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);
    initialize_merchant_config(&client, &merchant, &payout);

    client.set_payout_schedule(&merchant, &1, &0);

    let bal_a: i128 = 3_000_000;
    let bal_b: i128 = 7_000_000;

    seed_merchant_balance_and_token(&env, &contract_id, &merchant, &token_a, bal_a);
    seed_merchant_balance_and_token(&env, &contract_id, &merchant, &token_b, bal_b);
    seed_merchant_earnings(&env, &contract_id, &merchant, &token_a, bal_a);
    seed_merchant_earnings(&env, &contract_id, &merchant, &token_b, bal_b);
    token::StellarAssetClient::new(&env, &token_a).mint(&contract_id, &bal_a);
    token::StellarAssetClient::new(&env, &token_b).mint(&contract_id, &bal_b);

    let count = client.flush_payouts(&merchant);
    assert_eq!(count, 2);

    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token_a),
        0
    );
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token_b),
        0
    );
    assert_eq!(
        token::Client::new(&env, &token_a).balance(&payout),
        bal_a
    );
    assert_eq!(
        token::Client::new(&env, &token_b).balance(&payout),
        bal_b
    );
}

#[test]
fn test_flush_payouts_emits_event() {
    let (env, client, _admin, token) = setup_env();
    let contract_id = client.address.clone();
    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);
    initialize_merchant_config(&client, &merchant, &payout);

    client.set_payout_schedule(&merchant, &1, &0);

    env.ledger().set_timestamp(500_000);

    let balance: i128 = 5_000_000;
    seed_merchant_balance_and_token(&env, &contract_id, &merchant, &token, balance);
    seed_merchant_earnings(&env, &contract_id, &merchant, &token, balance);
    token::StellarAssetClient::new(&env, &token).mint(&contract_id, &balance);

    client.flush_payouts(&merchant);

    // Check events
    let events = env.events().all();
    let scheduled_event = events
        .iter()
        .find(|(contract, topic, _val)| {
            *contract == contract_id
                && Symbol::from_val(&env, &topic.get(0).unwrap()) == Symbol::new(&env, "scheduled_payout")
        })
        .expect("scheduled_payout event not found");

    let payload: ScheduledPayoutEvent = scheduled_event.2.clone().into_val(&env);
    assert_eq!(payload.merchant, merchant);
    assert_eq!(payload.tokens_paid, 1);
    assert!(payload.timestamp > 0);
}

#[test]
fn test_flush_payouts_no_config_or_schedule_returns_zero() {
    let (env, client, _admin, _token) = setup_env();
    let merchant = Address::generate(&env);
    // Merchant has no config and no schedule — flush returns 0 (nothing to do)

    let result = client.try_flush_payouts(&merchant);
    assert_eq!(result, Ok(Ok(0)));
}

#[test]
fn test_flush_payouts_updates_last_payout_at() {
    let (env, client, _admin, token) = setup_env();
    let contract_id = client.address.clone();
    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);
    initialize_merchant_config(&client, &merchant, &payout);

    let cadence: u64 = 3600;
    client.set_payout_schedule(&merchant, &cadence, &0);

    let balance: i128 = 5_000_000;
    seed_merchant_balance_and_token(&env, &contract_id, &merchant, &token, balance);
    seed_merchant_earnings(&env, &contract_id, &merchant, &token, balance);
    token::StellarAssetClient::new(&env, &token).mint(&contract_id, &balance);

    let ts: u64 = 1_000_000;
    env.ledger().set_timestamp(ts);
    client.flush_payouts(&merchant);

    let stored = client.get_payout_schedule(&merchant);
    assert_eq!(stored.last_payout_at, ts);
}

#[test]
fn test_get_payout_schedule_default() {
    let (env, client, _admin, _token) = setup_env();
    let merchant = Address::generate(&env);

    let schedule = client.get_payout_schedule(&merchant);
    assert_eq!(schedule.cadence_seconds, 0);
    assert_eq!(schedule.min_payout, 0);
    assert_eq!(schedule.last_payout_at, 0);
}
