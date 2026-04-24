use crate::{ChargeExecutionResult, Error, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient};
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

    let admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token, &6, &admin, &1_000_000i128, &GRACE_PERIOD);
    (env, client, token)
}

fn create_subscription(env: &Env, client: &SubscriptionVaultClient) -> (u32, Address, Address) {
    let subscriber = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(env);
    let merchant = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(env);
    let id = client.create_subscription(&subscriber, &merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>, &None::<u64>);
    (id, subscriber, merchant)
}

#[test]
fn repeated_failed_charges_preserve_financial_state() {
    let (env, client, _) = setup_test_env();
    env.ledger().set_timestamp(T0);

    let (id, _subscriber, merchant) = create_subscription(&env, &client);

    env.ledger().set_timestamp(T0 + INTERVAL + 1);
    assert_eq!(
        client.try_charge_subscription(&id),
        Ok(Ok(ChargeExecutionResult::InsufficientBalance))
    );

    let first = client.get_subscription(&id);
    assert_eq!(first.status, SubscriptionStatus::GracePeriod);
    assert_eq!(first.prepaid_balance, 0);
    assert_eq!(first.last_payment_timestamp, T0);
    assert_eq!(first.lifetime_charged, 0);
    assert_eq!(client.get_merchant_balance(&merchant), 0);
    assert_eq!(
        client.get_sub_statements_offset(&id, &0, &10, &true).total,
        0
    );

    env.ledger().set_timestamp(T0 + INTERVAL + 2);
    assert_eq!(
        client.try_charge_subscription(&id),
        Ok(Ok(ChargeExecutionResult::InsufficientBalance))
    );

    let second = client.get_subscription(&id);
    assert_eq!(second.status, SubscriptionStatus::GracePeriod);
    assert_eq!(second.prepaid_balance, 0);
    assert_eq!(second.last_payment_timestamp, T0);
    assert_eq!(second.lifetime_charged, 0);
    assert_eq!(client.get_merchant_balance(&merchant), 0);
    assert_eq!(
        client.get_sub_statements_offset(&id, &0, &10, &true).total,
        0
    );

    env.ledger().set_timestamp(T0 + INTERVAL + GRACE_PERIOD + 1);
    assert_eq!(
        client.try_charge_subscription(&id),
        Ok(Ok(ChargeExecutionResult::InsufficientBalance))
    );

    let after = client.get_subscription(&id);
    assert_eq!(after.status, SubscriptionStatus::InsufficientBalance);
    assert_eq!(after.prepaid_balance, 0);
    assert_eq!(after.last_payment_timestamp, T0);
    assert_eq!(after.lifetime_charged, 0);
    assert_eq!(client.get_merchant_balance(&merchant), 0);
    assert_eq!(
        client.get_sub_statements_offset(&id, &0, &10, &true).total,
        0
    );
}

#[test]
fn resume_from_underfunded_state_requires_sufficient_topup() {
    let (env, client, token) = setup_test_env();
    env.ledger().set_timestamp(T0);

    let (id, subscriber, _merchant) = create_subscription(&env, &client);
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &20_000_000i128);

    env.ledger().set_timestamp(T0 + INTERVAL + GRACE_PERIOD + 1);
    assert_eq!(
        client.try_charge_subscription(&id),
        Ok(Ok(ChargeExecutionResult::InsufficientBalance))
    );
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::InsufficientBalance
    );

    client.deposit_funds(&id, &subscriber, &5_000_000i128);
    assert_eq!(
        client.try_resume_subscription(&id, &subscriber),
        Err(Ok(Error::InsufficientBalance))
    );

    client.deposit_funds(&id, &subscriber, &5_000_000i128);
    client.resume_subscription(&id, &subscriber);

    let resumed = client.get_subscription(&id);
    assert_eq!(resumed.status, SubscriptionStatus::Active);
    assert_eq!(resumed.prepaid_balance, AMOUNT);
}

#[test]
fn cancel_from_insufficient_balance_succeeds() {
    let (env, client, _) = setup_test_env();
    env.ledger().set_timestamp(T0);

    let (id, subscriber, _merchant) = create_subscription(&env, &client);

    env.ledger().set_timestamp(T0 + INTERVAL + GRACE_PERIOD + 1);
    assert_eq!(
        client.try_charge_subscription(&id),
        Ok(Ok(ChargeExecutionResult::InsufficientBalance))
    );
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::InsufficientBalance
    );

    client.cancel_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}


