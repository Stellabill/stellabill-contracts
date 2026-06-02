#![cfg(test)]

extern crate alloc;

use soroban_sdk::{
    testutils::Address as _,
    token::{Client as TokenClient, StellarAssetClient as TokenAdminClient},
    Address, Env,
};
use subscription_vault::{Error, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient};

fn setup() -> (
    Env,
    SubscriptionVaultClient<'static>,
    u32,
    Address,
    Address,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();

    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_admin_client = TokenAdminClient::new(&env, &token_address);

    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let stranger = Address::generate(&env);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let min_topup: i128 = 1_000_000;
    let grace_period: u64 = 3 * 24 * 60 * 60;

    client.init(&token_address, &7u32, &admin, &min_topup, &grace_period);

    // Fund subscriber with initial tokens
    token_admin_client.mint(&subscriber, &100_000_000);

    // Create subscription
    let amount = 5_000_000i128;
    let interval = 30 * 24 * 60 * 60u64;
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval,
        &false,
        &None,
        &None::<u64>,
    );

    // Deposit funds
    client.deposit_funds(&sub_id, &subscriber, &30_000_000);

    (
        env, client, sub_id, subscriber, merchant, stranger,
    )
}

#[test]
fn test_cancel_by_subscriber() {
    let (_env, client, sub_id, subscriber, _merchant, _stranger) = setup();

    client.cancel_subscription(&sub_id, &subscriber);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(sub.prepaid_balance, 0);
}

#[test]
fn test_cancel_by_merchant() {
    let (_env, client, sub_id, _subscriber, merchant, _stranger) = setup();

    client.cancel_subscription(&sub_id, &merchant);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(sub.prepaid_balance, 0);
}

#[test]
fn test_cancel_by_stranger_rejected() {
    let (_env, client, sub_id, _subscriber, _merchant, stranger) = setup();

    let result = client.try_cancel_subscription(&sub_id, &stranger);
    assert_eq!(result, Err(Ok(Error::Forbidden)));

    // Subscription unchanged
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

#[test]
fn test_cancel_twice_rejected() {
    let (_env, client, sub_id, subscriber, _merchant, _stranger) = setup();

    // First cancel succeeds
    client.cancel_subscription(&sub_id, &subscriber);

    // Second cancel should fail
    let result = client.try_cancel_subscription(&sub_id, &subscriber);
    assert_eq!(result, Err(Ok(Error::InvalidStatusTransition)));
}

#[test]
fn test_cancel_nonexistent_subscription() {
    let (_env, client, _sub_id, _subscriber, _merchant, _stranger) = setup();

    let result = client.try_cancel_subscription(&99999, &Address::generate(&_env));
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn test_cancel_with_zero_balance_refunds_nothing() {
    let env = Env::default();
    env.mock_all_auths();

    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();

    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    client.init(&token_address, &7u32, &admin, &1_000_000i128, &3600u64);

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &5_000_000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &None,
        &None::<u64>,
    );

    // Cancel with zero balance — should succeed with no refund
    client.cancel_subscription(&sub_id, &subscriber);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(sub.prepaid_balance, 0);
}

#[test]
fn test_cancel_refunds_prepaid_balance() {
    let env = Env::default();
    env.mock_all_auths();

    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_admin_client = TokenAdminClient::new(&env, &token_address);
    let token_client = TokenClient::new(&env, &token_address);

    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    client.init(&token_address, &7u32, &admin, &1_000_000i128, &3600u64);

    // Fund subscriber
    let deposit = 30_000_000i128;
    token_admin_client.mint(&subscriber, &100_000_000);

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &5_000_000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &None,
        &None::<u64>,
    );
    client.deposit_funds(&sub_id, &subscriber, &deposit);

    // Confirm vault holds the deposit
    let contract_balance_before = token_client.balance(&contract_id);
    let subscriber_balance_before = token_client.balance(&subscriber);

    client.cancel_subscription(&sub_id, &subscriber);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(sub.prepaid_balance, 0);

    // Subscriber got their refund
    let subscriber_balance_after = token_client.balance(&subscriber);
    assert_eq!(subscriber_balance_after, subscriber_balance_before + deposit);

    // Vault no longer holds the refunded amount
    let contract_balance_after = token_client.balance(&contract_id);
    assert_eq!(contract_balance_after, contract_balance_before - deposit);
}
