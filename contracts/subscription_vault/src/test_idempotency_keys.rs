use crate::{
    ChargeExecutionResult, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, BytesN, Env,
};

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 86_400;
const DEPOSIT: i128 = 50_000_000;
const MIN_TOPUP: i128 = 1_000_000;

fn setup_test_env() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000_000);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &MIN_TOPUP, &(7 * 24 * 60 * 60));

    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&contract_id, &1_000_000_000i128);

    (env, client, token)
}

fn create_and_fund_sub(
    env: &Env,
    client: &SubscriptionVaultClient,
    subscriber: &Address,
    merchant: &Address,
    token: &Address,
) -> u32 {
    let id = client.create_subscription(
        subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    let token_client = token::Client::new(env, token);
    if token_client.balance(subscriber) < DEPOSIT {
        token::StellarAssetClient::new(env, token).mint(subscriber, &(DEPOSIT * 2));
    }

    let none_key: Option<BytesN<32>> = None;
    client.deposit_funds(&id, subscriber, &DEPOSIT, &none_key);
    env.ledger().set_timestamp(env.ledger().timestamp() + 1);

    id
}

fn make_key(env: &Env, val: u8) -> BytesN<32> {
    let mut arr = [0u8; 32];
    arr[31] = val;
    BytesN::from_array(env, &arr)
}

#[test]
fn test_charge_subscription_idempotent_replay() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);

    env.ledger().set_timestamp(env.ledger().timestamp() + INTERVAL);

    let key = make_key(&env, 1);
    let r1 = client.charge_subscription(&id, &Some(key.clone()));
    assert_eq!(r1, ChargeExecutionResult::Charged);

    let r2 = client.charge_subscription(&id, &Some(key.clone()));
    assert_eq!(r2, ChargeExecutionResult::Charged);
}

#[test]
fn test_charge_subscription_different_keys_allowed() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);

    env.ledger().set_timestamp(env.ledger().timestamp() + INTERVAL);

    let key1 = make_key(&env, 1);
    let r1 = client.charge_subscription(&id, &Some(key1));
    assert_eq!(r1, ChargeExecutionResult::Charged);

    env.ledger().set_timestamp(env.ledger().timestamp() + INTERVAL);

    let key2 = make_key(&env, 2);
    let r2 = client.charge_subscription(&id, &Some(key2));
    assert_eq!(r2, ChargeExecutionResult::Charged);
}

#[test]
fn test_charge_subscription_none_key_ok() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);

    env.ledger().set_timestamp(env.ledger().timestamp() + INTERVAL);

    let none_key: Option<BytesN<32>> = None;
    let r = client.charge_subscription(&id, &none_key);
    assert_eq!(r, ChargeExecutionResult::Charged);
}

#[test]
fn test_deposit_funds_idempotent_replay() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);

    let key = make_key(&env, 10);
    let extra = 5_000_000i128;
    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &extra);

    client.deposit_funds(&id, &subscriber, &extra, &Some(key.clone()));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, DEPOSIT + extra);

    client.deposit_funds(&id, &subscriber, &extra, &Some(key.clone()));

    let sub2 = client.get_subscription(&id);
    assert_eq!(sub2.prepaid_balance, DEPOSIT + extra);
}

#[test]
fn test_deposit_funds_different_keys_allowed() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);

    let key1 = make_key(&env, 20);
    let key2 = make_key(&env, 21);
    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &20_000_000i128);

    client.deposit_funds(&id, &subscriber, &10_000_000i128, &Some(key1));
    client.deposit_funds(&id, &subscriber, &10_000_000i128, &Some(key2));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, DEPOSIT + 20_000_000i128);
}

#[test]
fn test_charge_one_off_idempotent_replay() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);

    let key = make_key(&env, 30);
    let amount: i128 = 5_000_000;

    client.charge_one_off(&id, &merchant, &amount, &Some(key.clone()));
    client.charge_one_off(&id, &merchant, &amount, &Some(key.clone()));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, DEPOSIT - amount);
}

#[test]
fn test_charge_one_off_different_keys_allowed() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);

    let key1 = make_key(&env, 31);
    let key2 = make_key(&env, 32);

    client.charge_one_off(&id, &merchant, &1_000_000i128, &Some(key1));
    client.charge_one_off(&id, &merchant, &2_000_000i128, &Some(key2));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, DEPOSIT - 3_000_000i128);
}

#[test]
fn test_same_raw_key_different_entrypoints_no_collision() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);
    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &10_000_000i128);

    let key = make_key(&env, 99);

    client.charge_one_off(&id, &merchant, &1_000_000i128, &Some(key.clone()));
    client.deposit_funds(&id, &subscriber, &5_000_000i128, &Some(key.clone()));

    env.ledger().set_timestamp(env.ledger().timestamp() + INTERVAL);
    client.charge_subscription(&id, &Some(key.clone()));
}

#[test]
fn test_ring_buffer_evicts_oldest_key() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);
    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &1_000_000_000i128);

    // Insert 33 unique keys to fill buffer (32) + evict oldest (key 0)
    for i in 0..33u8 {
        let key = make_key(&env, i);
        token_admin.mint(&subscriber, &MIN_TOPUP);
        client.deposit_funds(&id, &subscriber, &MIN_TOPUP, &Some(key));
    }

    // Buffer now holds [32, 1, 2, 3, ..., 31], cursor = 1.
    // Key 0 was evicted (overwritten by key 32 at index 0).

    let balance_before = client.get_subscription(&id).prepaid_balance;

    // Key 1 is still present → idempotent no-op (balance unchanged)
    let key1 = make_key(&env, 1);
    client.deposit_funds(&id, &subscriber, &MIN_TOPUP, &Some(key1));
    assert_eq!(
        client.get_subscription(&id).prepaid_balance,
        balance_before,
        "key 1 should be idempotent (no balance change)"
    );

    // Key 0 was evicted → fresh deposit (balance increases)
    let key0 = make_key(&env, 0);
    token_admin.mint(&subscriber, &MIN_TOPUP);
    client.deposit_funds(&id, &subscriber, &MIN_TOPUP, &Some(key0));
    assert_eq!(
        client.get_subscription(&id).prepaid_balance,
        balance_before + MIN_TOPUP,
        "key 0 should be a fresh deposit"
    );
}
