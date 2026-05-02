use crate::{
    ChargeExecutionResult, Error, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env};

const T0: u64 = 1_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const AMOUNT: i128 = 10_000_000;
const GRACE_PERIOD: u64 = 7 * 24 * 60 * 60;

fn setup_test_env() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(admin.clone()).address();
    client.init(&token, &6, &admin, &1_000_000i128, &GRACE_PERIOD);
    (env, client, token)
}

fn create_subscription(env: &Env, client: &SubscriptionVaultClient) -> (u32, Address, Address) {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    (id, subscriber, merchant)
}

#[test]
fn repeated_failed_charges_preserve_financial_state() {
    let (env, client, _) = setup_test_env();
    env.ledger().set_timestamp(T0);
    let (id, _subscriber, merchant) = create_subscription(&env, &client);
    env.ledger().set_timestamp(T0 + INTERVAL + 1);
    assert_eq!(client.try_charge_subscription(&id), Ok(Ok(ChargeExecutionResult::InsufficientBalance)));
    let first = client.get_subscription(&id);
    assert_eq!(first.status, SubscriptionStatus::GracePeriod);
    assert_eq!(first.prepaid_balance, 0);
    assert_eq!(first.lifetime_charged, 0);
    assert_eq!(client.get_merchant_balance(&merchant), 0);
    env.ledger().set_timestamp(T0 + INTERVAL + GRACE_PERIOD + 1);
    assert_eq!(client.try_charge_subscription(&id), Ok(Ok(ChargeExecutionResult::InsufficientBalance)));
    let after = client.get_subscription(&id);
    assert_eq!(after.status, SubscriptionStatus::InsufficientBalance);
    assert_eq!(after.lifetime_charged, 0);
    assert_eq!(client.get_merchant_balance(&merchant), 0);
}

#[test]
fn resume_from_underfunded_state_requires_sufficient_topup() {
    let (env, client, token) = setup_test_env();
    env.ledger().set_timestamp(T0);
    let (id, subscriber, _merchant) = create_subscription(&env, &client);
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &20_000_000i128);
    env.ledger().set_timestamp(T0 + INTERVAL + GRACE_PERIOD + 1);
    assert_eq!(client.try_charge_subscription(&id), Ok(Ok(ChargeExecutionResult::InsufficientBalance)));
    assert_eq!(client.get_subscription(&id).status, SubscriptionStatus::InsufficientBalance);
    client.deposit_funds(&id, &subscriber, &5_000_000i128);
    assert_eq!(client.try_resume_subscription(&id, &subscriber), Err(Ok(Error::InsufficientBalance)));
    client.deposit_funds(&id, &subscriber, &5_000_000i128);
    client.resume_subscription(&id, &subscriber);
    let resumed = client.get_subscription(&id);
    assert_eq!(resumed.status, SubscriptionStatus::Active);
    assert_eq!(resumed.prepaid_balance, AMOUNT);
}

#[test]
fn deposit_auto_resumes_from_insufficient_balance() {
    let (env, client, token) = setup_test_env();
    env.ledger().set_timestamp(T0);
    let (id, subscriber, _merchant) = create_subscription(&env, &client);
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &20_000_000i128);
    env.ledger().set_timestamp(T0 + INTERVAL + GRACE_PERIOD + 1);
    assert_eq!(client.try_charge_subscription(&id), Ok(Ok(ChargeExecutionResult::InsufficientBalance)));
    client.deposit_funds(&id, &subscriber, &AMOUNT);
    let resumed = client.get_subscription(&id);
    assert_eq!(resumed.status, SubscriptionStatus::Active);
    assert_eq!(resumed.prepaid_balance, AMOUNT);
}

#[test]
fn charge_with_exactly_equal_balance_succeeds() {
    let (env, client, token) = setup_test_env();
    env.ledger().set_timestamp(T0);
    let (id, subscriber, _merchant) = create_subscription(&env, &client);
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &AMOUNT);
    client.deposit_funds(&id, &subscriber, &AMOUNT);
    env.ledger().set_timestamp(T0 + INTERVAL + 1);
    assert_eq!(client.try_charge_subscription(&id), Ok(Ok(ChargeExecutionResult::Charged)));
    let after = client.get_subscription(&id);
    assert_eq!(after.prepaid_balance, 0);
    assert_eq!(after.status, SubscriptionStatus::Active);
    assert_eq!(after.lifetime_charged, AMOUNT);
}

#[test]
fn deposit_insufficient_token_balance_reverts_no_state_change() {
    let (env, client, token) = setup_test_env();
    let (id, subscriber, _merchant) = create_subscription(&env, &client);
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &500_000i128);
    let initial_sub = client.get_subscription(&id);
    assert_eq!(initial_sub.prepaid_balance, 0);
    let result = client.try_deposit_funds(&id, &subscriber, &2_000_000i128);
    assert!(result.is_err());
    let after_fail = client.get_subscription(&id);
    assert_eq!(after_fail.prepaid_balance, 0);
    assert_eq!(after_fail.status, initial_sub.status);
}

#[test]
fn deposit_respects_subscriber_credit_limit() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let admin = client.get_admin();
    client.set_subscriber_credit_limit(&admin, &subscriber, &token, &5_000_000i128);
    let id = client.create_subscription(&subscriber, &merchant, &3_000_000i128, &INTERVAL, &false, &None::<i128>, &None::<u64>);
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &10_000_000i128);
    client.deposit_funds(&id, &subscriber, &2_000_000i128);
    assert_eq!(client.get_subscription(&id).prepaid_balance, 2_000_000);
    let result = client.try_deposit_funds(&id, &subscriber, &4_000_000i128);
    assert_eq!(result, Err(Ok(Error::CreditLimitExceeded)));
}

#[test]
fn multiple_failed_charges_then_topup_recovers() {
    let (env, client, token) = setup_test_env();
    env.ledger().set_timestamp(T0);
    let (id, subscriber, _merchant) = create_subscription(&env, &client);
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &30_000_000i128);
    for i in 1..=3 {
        env.ledger().set_timestamp(T0 + (INTERVAL * i) + GRACE_PERIOD + 1);
        assert_eq!(client.try_charge_subscription(&id), Ok(Ok(ChargeExecutionResult::InsufficientBalance)));
    }
    client.deposit_funds(&id, &subscriber, &AMOUNT);
    client.resume_subscription(&id, &subscriber);
    assert_eq!(client.get_subscription(&id).status, SubscriptionStatus::Active);
    env.ledger().set_timestamp(T0 + (INTERVAL * 4) + 1);
    assert_eq!(client.try_charge_subscription(&id), Ok(Ok(ChargeExecutionResult::Charged)));
}

#[test]
fn cancel_from_insufficient_balance_succeeds() {
    let (env, client, _) = setup_test_env();
    env.ledger().set_timestamp(T0);
    let (id, subscriber, _merchant) = create_subscription(&env, &client);
    env.ledger().set_timestamp(T0 + INTERVAL + GRACE_PERIOD + 1);
    assert_eq!(client.try_charge_subscription(&id), Ok(Ok(ChargeExecutionResult::InsufficientBalance)));
    assert_eq!(client.get_subscription(&id).status, SubscriptionStatus::InsufficientBalance);
    client.cancel_subscription(&id, &subscriber);
    assert_eq!(client.get_subscription(&id).status, SubscriptionStatus::Cancelled);
}

#[test]
fn insufficient_balance_never_credits_merchant() {
    let (env, client, _) = setup_test_env();
    env.ledger().set_timestamp(T0);
    let (id, _subscriber, merchant) = create_subscription(&env, &client);
    for i in 1..=5 {
        env.ledger().set_timestamp(T0 + (INTERVAL * i) + GRACE_PERIOD + 1);
        assert_eq!(client.try_charge_subscription(&id), Ok(Ok(ChargeExecutionResult::InsufficientBalance)));
    }
    assert_eq!(client.get_merchant_balance(&merchant), 0);
    assert_eq!(client.get_subscription(&id).lifetime_charged, 0);
}