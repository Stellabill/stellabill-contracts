//! Tests for bulk_pause_subscriptions
#![cfg(test)]

extern crate alloc;

use soroban_sdk::{Address, Env};
use subscription_vault::{Error, SubscriptionStatus, SubscriptionVaultClient};

fn setup() -> (Env, SubscriptionVaultClient<'static>, Vec<u32>, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_admin_client = soroban_sdk::token::Client::new(&env, &token_address);

    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let stranger = Address::generate(&env);

    let contract_id = env.register(subscription_vault::SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    client.init(&token_address, &7u32, &admin, &1_000_000i128, &36_00u64);

    // Fund subscriber
    token_admin_client.mint(&subscriber, &100_000_000);

    // Create several subscriptions
    let mut ids = Vec::new(&env);
    for _ in 0..3 {
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
            &None::<u32>,
            &None::<soroban_sdk::Symbol>,
        );
        ids.push_back(sub_id);
    }

    (env, client, ids, subscriber, merchant, stranger)
}

#[test]
fn test_bulk_pause_success() {
    let (env, client, ids, _subscriber, _merchant, _stranger) = setup();
    let admin = client.get_admin(&env).unwrap(); // admin from init
    let nonce = client.get_admin_nonce(&env, &admin, &2).unwrap(); // domain 2 for bulk pause
    let result = client.bulk_pause_subscriptions(&env, &admin, &ids, &nonce).unwrap();
    // All should be changed
    assert_eq!(result.len(), 3);
    for r in result.iter() {
        assert!(r.changed);
        assert_eq!(r.error, None);
    }
    // Verify status
    for id in ids.iter() {
        let sub = client.get_subscription(&env, &id);
        assert_eq!(sub.status, SubscriptionStatus::Paused);
    }
}

#[test]
fn test_bulk_pause_unauthorized() {
    let (env, client, ids, _subscriber, _merchant, stranger) = setup();
    let nonce = client.get_operator_nonce(&env, &stranger, &2).unwrap_or(0);
    let result = client.try_bulk_pause_subscriptions(&env, &stranger, &ids, &nonce);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
    // Ensure state unchanged
    for id in ids.iter() {
        let sub = client.get_subscription(&env, &id);
        assert_eq!(sub.status, SubscriptionStatus::Active);
    }
}

#[test]
fn test_bulk_pause_batch_too_large() {
    let (env, client, ids, admin, _merchant, _stranger) = setup();
    // create a huge vector exceeding BATCH_MAX_SIZE (100)
    let mut huge = Vec::new(&env);
    for i in 0..101u32 {
        huge.push_back(i);
    }
    let nonce = client.get_admin_nonce(&env, &admin, &2).unwrap();
    let result = client.try_bulk_pause_subscriptions(&env, &admin, &huge, &nonce);
    assert_eq!(result, Err(Ok(Error::BatchTooLarge)));
}

#[test]
fn test_bulk_pause_nonce_replay() {
    let (env, client, ids, admin, _merchant, _stranger) = setup();
    let nonce = client.get_admin_nonce(&env, &admin, &2).unwrap();
    // First call succeeds
    let _ = client.bulk_pause_subscriptions(&env, &admin, &ids, &nonce).unwrap();
    // Second call with same nonce should fail
    let result = client.try_bulk_pause_subscriptions(&env, &admin, &ids, &nonce);
    assert_eq!(result, Err(Ok(Error::NonceAlreadyUsed)));
}

#[test]
fn test_bulk_pause_idempotent_and_missing() {
    let (env, client, ids, admin, _merchant, _stranger) = setup();
    // Pause first two, keep third active
    let first_two = ids.slice(&env, 0, 2);
    let nonce1 = client.get_admin_nonce(&env, &admin, &2).unwrap();
    let _ = client.bulk_pause_subscriptions(&env, &admin, &first_two, &nonce1).unwrap();
    // Attempt bulk pause with a missing id, a paused id, and an active id
    let mut mixed = Vec::new(&env);
    mixed.push_back(9999u32); // non-existent
    mixed.push_back(ids.get(0).unwrap()); // already paused
    mixed.push_back(ids.get(2).unwrap()); // still active
    let nonce2 = client.get_admin_nonce(&env, &admin, &2).unwrap();
    let result = client.bulk_pause_subscriptions(&env, &admin, &mixed, &nonce2).unwrap();
    assert_eq!(result.len(), 3);
    // first entry error NotFound
    assert_eq!(result.get(0).unwrap().error, Some(Error::NotFound));
    // second entry changed false (idempotent)
    assert!(!result.get(1).unwrap().changed);
    // third entry changed true
    assert!(result.get(2).unwrap().changed);
    // Verify states
    let sub0 = client.get_subscription(&env, &ids.get(0).unwrap());
    assert_eq!(sub0.status, SubscriptionStatus::Paused);
    let sub2 = client.get_subscription(&env, &ids.get(2).unwrap());
    assert_eq!(sub2.status, SubscriptionStatus::Paused);
}
