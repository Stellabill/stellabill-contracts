//! Unit test: Merchant full-balance drain via withdraw_merchant_funds.
//!
//! Verifies that:
//! 1. `withdraw_merchant_funds` with amount equal to current earnings drains the merchant balance cleanly to zero.
//! 2. `MerchantEarnings` balance (computed balance) is reduced to exactly zero.
//! 3. `MerchantBalanceSnapshotEvent` is emitted with a zero balance.
//! 4. Vault token balance and merchant token balance update consistently.
//! 5. Subsequent withdrawal attempts return an error (`Error::NotFound` or `Error::InsufficientBalance`).

#![cfg(test)]

use crate::types::MerchantBalanceSnapshotEvent;
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger},
    token, Address, Env, TryFromVal,
};

const INTERVAL: u64 = 30 * 24 * 60 * 60;
const AMOUNT: i128 = 10_000_000;
const DEPOSIT_AMOUNT: i128 = 50_000_000;

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000_000);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token, &7, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    (env, client, token, admin)
}

fn create_and_fund_sub(
    env: &Env,
    client: &SubscriptionVaultClient,
    token: &Address,
) -> (u32, Address, Address) {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);

    // Mint tokens to subscriber
    token::StellarAssetClient::new(env, token).mint(&subscriber, &DEPOSIT_AMOUNT);

    let id = client.create_subscription(
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

    client.deposit_funds(&id, &subscriber, &DEPOSIT_AMOUNT);
    (id, subscriber, merchant)
}

/// Helper to parse published events for a specific event type.
fn find_snapshot_events(env: &Env) -> Vec<MerchantBalanceSnapshotEvent> {
    let mut matching = Vec::new();
    let expected_topic = soroban_sdk::Symbol::new(env, "merchant_balance_snapshot");
    for (_contract_id, topics, data) in env.events().all() {
        if let Some(topic_val) = topics.get(0) {
            if let Ok(topic_sym) = soroban_sdk::Symbol::try_from_val(env, &topic_val) {
                if topic_sym == expected_topic {
                    if let Ok(event) = MerchantBalanceSnapshotEvent::try_from_val(env, &data) {
                        matching.push(event);
                    }
                }
            }
        }
    }
    matching
}

#[test]
fn test_merchant_full_balance_drain() {
    let (env, client, token, _) = setup();
    let (id, _, merchant) = create_and_fund_sub(&env, &client, &token);
    let token_client = token::Client::new(&env, &token);

    // 1. Charge subscription to accumulate merchant earnings
    let now = 1_000_000 + INTERVAL + 1;
    env.ledger().set_timestamp(now);
    client.charge_subscription(&id);

    // Verify initial merchant earnings and balance
    let initial_balance = client.get_merchant_balance(&merchant);
    assert_eq!(initial_balance, AMOUNT, "Merchant balance should equal charge amount");

    let initial_vault_tokens = token_client.balance(&client.address);
    let initial_merchant_tokens = token_client.balance(&merchant);

    // 2. Withdraw exact full balance
    client.withdraw_merchant_funds(&merchant, &initial_balance);

    // 3. Immediately inspect events from the withdrawal invocation
    let snapshot_events = find_snapshot_events(&env);
    assert!(
        !snapshot_events.is_empty(),
        "MerchantBalanceSnapshotEvent should be emitted"
    );
    let last_snapshot = snapshot_events.last().unwrap();
    assert_eq!(last_snapshot.merchant, merchant);
    assert_eq!(last_snapshot.token, token);
    assert_eq!(last_snapshot.balance, 0, "Snapshot balance must be zero");

    // 4. Assert final storage state: balance is exactly 0
    let final_balance = client.get_merchant_balance(&merchant);
    assert_eq!(final_balance, 0, "Merchant balance must be zero after full drain");

    // Verify token movements
    assert_eq!(
        token_client.balance(&merchant),
        initial_merchant_tokens + initial_balance,
        "Merchant token balance must reflect full payout"
    );
    assert_eq!(
        token_client.balance(&client.address),
        initial_vault_tokens - initial_balance,
        "Vault token balance must decrease by exact withdrawal amount"
    );

    // 5. Subsequent withdrawal when balance is zero must fail
    let err = client.try_withdraw_merchant_funds(&merchant, &1i128);
    assert!(err.is_err(), "Withdrawal from zero balance must be rejected");
}

#[test]
fn test_merchant_partial_drain_then_full_drain() {
    let (env, client, token, _) = setup();
    let (id, _, merchant) = create_and_fund_sub(&env, &client, &token);

    // Charge subscription
    let now = 1_000_000 + INTERVAL + 1;
    env.ledger().set_timestamp(now);
    client.charge_subscription(&id);

    let total_earnings = client.get_merchant_balance(&merchant);
    let partial_amount = total_earnings / 2;
    let remaining_amount = total_earnings - partial_amount;

    // Withdraw partial
    client.withdraw_merchant_funds(&merchant, &partial_amount);
    let partial_events = find_snapshot_events(&env);
    assert_eq!(partial_events.last().unwrap().balance, remaining_amount);

    assert_eq!(client.get_merchant_balance(&merchant), remaining_amount);

    // Withdraw remaining exact balance to complete full drain
    client.withdraw_merchant_funds(&merchant, &remaining_amount);
    let snapshot_events = find_snapshot_events(&env);
    let last_snapshot = snapshot_events.last().unwrap();
    assert_eq!(last_snapshot.balance, 0);

    assert_eq!(client.get_merchant_balance(&merchant), 0);
}

#[test]
fn test_merchant_dust_balance_drain() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let dust_amount = 1_000_000i128;
    token::StellarAssetClient::new(&env, &token).mint(&subscriber, &dust_amount);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &dust_amount,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    client.deposit_funds(&id, &subscriber, &dust_amount);

    let now = 1_000_000 + INTERVAL + 1;
    env.ledger().set_timestamp(now);
    client.charge_subscription(&id);

    assert_eq!(client.get_merchant_balance(&merchant), dust_amount);

    client.withdraw_merchant_funds(&merchant, &dust_amount);
    let snapshot_events = find_snapshot_events(&env);
    let last_snapshot = snapshot_events.last().unwrap();
    assert_eq!(last_snapshot.balance, 0);

    assert_eq!(client.get_merchant_balance(&merchant), 0);
}
