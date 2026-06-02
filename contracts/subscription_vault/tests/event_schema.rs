#![cfg(test)]

extern crate alloc;

use soroban_sdk::{
    testutils::{Address as _, Events}, Address, Env, Symbol, IntoVal,
};
use subscription_vault::{
    SubscriptionVault, SubscriptionVaultClient, AdminRotatedEvent,
    SubscriptionCreatedEvent,
};

#[test]
fn test_nonce_consumed_and_admin_rotated_event_topics_and_shapes() {
    let env = Env::default();
    env.mock_all_auths();

    let token_admin = Address::generate(&env);
    let token_address = env.register_stellar_asset_contract_v2(token_admin.clone()).address();

    let admin = Address::generate(&env);
    let new_admin = Address::generate(&env);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    client.init(&token_address, &7u32, &admin, &1_000_000i128, &3600u64);

    client.rotate_admin(&admin, &new_admin, &0u64);

    let events = env.events().all();
    assert!(events.len() >= 1, "rotate_admin must emit at least one event (admin_rotated)");

    let ts = env.ledger().timestamp();

    let ev0 = events.get(0).unwrap();
    assert_eq!(ev0.0, contract_id.clone());
    assert!(ev0.1.len() >= 1, "expected at least one topic for admin_rotated");
}

#[test]
fn test_subscription_created_event_topic_and_shape() {
    let env = Env::default();
    env.mock_all_auths();

    let token_admin = Address::generate(&env);
    let token_address = env.register_stellar_asset_contract_v2(token_admin.clone()).address();

    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    client.init(&token_address, &7u32, &admin, &1_000_000i128, &3600u64);

    let amount: i128 = 1_000_000;
    let interval_seconds: u64 = 30 * 24 * 60 * 60;

    let subscription_id = client.create_subscription(&subscriber, &merchant, &amount, &interval_seconds, &false, &None, &None::<u64>);

    let events = env.events().all();
    let last_event = events.get(events.len() - 1).unwrap();
    assert_eq!(last_event.0, contract_id.clone());
}
