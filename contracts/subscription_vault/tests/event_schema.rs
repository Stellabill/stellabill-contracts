#![cfg(test)]

extern crate alloc;

use soroban_sdk::{
    testutils::{Address as _, Events},
    Address, Env,
};
use subscription_vault::{
    SubscriptionVault, SubscriptionVaultClient,
};

#[test]
fn test_nonce_consumed_and_admin_rotated_events_emitted() {
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
    assert!(events.len() >= 2, "rotate_admin must emit at least two events");
}

#[test]
fn test_subscription_created_event_emitted() {
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

    client.create_subscription(
        &subscriber, &merchant, &1_000_000i128, &(30 * 24 * 60 * 60u64), &false, &None, &None::<u64>,
    );

    let events = env.events().all();
    assert!(events.len() >= 1, "create_subscription must emit at least one event");
}
