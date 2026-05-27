#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Env,
};

fn setup() -> (Env, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let token_admin = Address::generate(&env);
    let token_contract_id = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_address = token_contract_id.address();
    let contract_id = env.register_contract(None, SubscriptionVault);
    let admin = Address::generate(&env);
    SubscriptionVaultClient::new(&env, &contract_id).init(&token_address, &admin);
    (env, contract_id, token_address, admin)
}

fn fund_subscription(
    env: &Env,
    token_address: &Address,
    contract_id: &Address,
    subscription_id: u64,
    subscriber: &Address,
    amount: i128,
) {
    StellarAssetClient::new(env, token_address).mint(subscriber, &amount);
    SubscriptionVaultClient::new(env, contract_id).deposit_funds(&subscription_id, &amount);
}

fn make_subscription(
    env: &Env,
    contract_id: &Address,
    subscriber: &Address,
    merchant: &Address,
    amount: i128,
    interval_seconds: u64,
) -> u64 {
    SubscriptionVaultClient::new(env, contract_id)
        .create_subscription(subscriber, merchant, &amount, &interval_seconds, &false)
}

#[test]
fn test_charge_subscription_success() {
    let (env, contract_id, token_address, _admin) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = make_subscription(&env, &contract_id, &subscriber, &merchant, 100, 86_400);
    env.ledger().set_timestamp(env.ledger().timestamp() + 86_401);
    fund_subscription(&env, &token_address, &contract_id, id, &subscriber, 100);
    SubscriptionVaultClient::new(&env, &contract_id).charge_subscription(&id);
    let sub = SubscriptionVaultClient::new(&env, &contract_id).get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 0);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

#[test]
fn test_charge_subscription_insufficient_balance() {
    let (env, contract_id, _token_address, _admin) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = make_subscription(&env, &contract_id, &subscriber, &merchant, 100, 0);
    env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    let result = SubscriptionVaultClient::new(&env, &contract_id).try_charge_subscription(&id);
    assert!(result.is_err());
    let sub = SubscriptionVaultClient::new(&env, &contract_id).get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::InsufficientBalance);
}

#[test]
fn test_charge_subscription_not_yet_due() {
    let (env, contract_id, token_address, _admin) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = make_subscription(&env, &contract_id, &subscriber, &merchant, 100, 86_400);
    fund_subscription(&env, &token_address, &contract_id, id, &subscriber, 1000);
    let result = SubscriptionVaultClient::new(&env, &contract_id).try_charge_subscription(&id);
    assert!(result.is_err());
}

#[test]
fn test_batch_charge_all_active() {
    let (env, contract_id, token_address, _admin) = setup();
    let merchant = Address::generate(&env);
    let mut ids = vec![&env];
    for _ in 0..3 {
        let sub = Address::generate(&env);
        let id = make_subscription(&env, &contract_id, &sub, &merchant, 50, 0);
        env.ledger().set_timestamp(env.ledger().timestamp() + 1);
        fund_subscription(&env, &token_address, &contract_id, id, &sub, 50);
        ids.push_back(id);
    }
    env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    let results = SubscriptionVaultClient::new(&env, &contract_id).batch_charge(&ids);
    for r in results.iter() {
        assert!(r.success);
    }
}

#[test]
fn test_batch_charge_mixed() {
    let (env, contract_id, token_address, _admin) = setup();
    let merchant = Address::generate(&env);
    let sub_a = Address::generate(&env);
    let id_a = make_subscription(&env, &contract_id, &sub_a, &merchant, 100, 0);
    env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    fund_subscription(&env, &token_address, &contract_id, id_a, &sub_a, 100);
    let sub_b = Address::generate(&env);
    let id_b = make_subscription(&env, &contract_id, &sub_b, &merchant, 100, 0);
    env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    let sub_c = Address::generate(&env);
    let id_c = make_subscription(&env, &contract_id, &sub_c, &merchant, 100, 999_999);
    fund_subscription(&env, &token_address, &contract_id, id_c, &sub_c, 100);
    env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    let ids = vec![&env, id_a, id_b, id_c];
    let results = SubscriptionVaultClient::new(&env, &contract_id).batch_charge(&ids);
    let find = |target: u64| results.iter().find(|r| r.subscription_id == target).unwrap();
    assert!(find(id_a).success);
    assert!(!find(id_b).success);
    assert!(!find(id_c).success);
}

#[test]
fn test_batch_charge_empty_list() {
    let (env, contract_id, _token_address, _admin) = setup();
    let ids: soroban_sdk::Vec<u64> = vec![&env];
    let results = SubscriptionVaultClient::new(&env, &contract_id).batch_charge(&ids);
    assert_eq!(results.len(), 0);
}

#[test]
fn test_batch_charge_nonexistent_id() {
    let (env, contract_id, _token_address, _admin) = setup();
    let ids = vec![&env, 9999u64];
    let results = SubscriptionVaultClient::new(&env, &contract_id).batch_charge(&ids);
    assert!(!results.get(0).unwrap().success);
    assert_eq!(results.get(0).unwrap().message, soroban_sdk::String::from_str(&env, "not_found"));
}

#[test]
fn test_batch_charge_duplicate_ids() {
    let (env, contract_id, token_address, _admin) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let id = make_subscription(&env, &contract_id, &subscriber, &merchant, 50, 0);
    env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    fund_subscription(&env, &token_address, &contract_id, id, &subscriber, 200);
    let ids = vec![&env, id, id];
    let results = SubscriptionVaultClient::new(&env, &contract_id).batch_charge(&ids);
    assert_eq!(results.len(), 2);
    assert!(results.get(0).unwrap().success);
    assert!(!results.get(1).unwrap().success);
}

#[test]
fn test_batch_charge_paused_skipped() {
    let (env, contract_id, token_address, _admin) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let id = make_subscription(&env, &contract_id, &subscriber, &merchant, 100, 0);
    env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    fund_subscription(&env, &token_address, &contract_id, id, &subscriber, 100);
    SubscriptionVaultClient::new(&env, &contract_id).pause_subscription(&id);
    let ids = vec![&env, id];
    let results = SubscriptionVaultClient::new(&env, &contract_id).batch_charge(&ids);
    assert!(!results.get(0).unwrap().success);
}

#[test]
fn test_batch_charge_cancelled_skipped() {
    let (env, contract_id, token_address, _admin) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let id = make_subscription(&env, &contract_id, &subscriber, &merchant, 100, 0);
    env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    fund_subscription(&env, &token_address, &contract_id, id, &subscriber, 100);
    SubscriptionVaultClient::new(&env, &contract_id).cancel_subscription(&id);
    let ids = vec![&env, id];
    let results = SubscriptionVaultClient::new(&env, &contract_id).batch_charge(&ids);
    assert!(!results.get(0).unwrap().success);
}
