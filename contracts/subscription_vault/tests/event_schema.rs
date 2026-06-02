#![cfg(test)]

extern crate alloc;

use soroban_sdk::{
    testutils::{Address as _, Events},
    Address, Env, FromVal, Symbol,
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
    assert!(!events.is_empty(), "rotate_admin must emit at least one event");

    let ts = env.ledger().timestamp();

    // Find and verify the admin_rotated event
    let admin_rotated_ev = events.iter().find(|ev| {
        ev.1.get(0).map(|t| Symbol::from_val(&env, &t) == Symbol::new(&env, "admin_rotated")).unwrap_or(false)
    }).expect("admin_rotated event must be emitted");

    assert_eq!(admin_rotated_ev.0, contract_id);
    let admin_evt: AdminRotatedEvent = FromVal::from_val(&env, &admin_rotated_ev.2);
    assert_eq!(admin_evt.old_admin, admin);
    assert_eq!(admin_evt.new_admin, new_admin);
    assert_eq!(admin_evt.timestamp, ts);
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

    let subscription_id = client.create_subscription(
        &subscriber, &merchant, &amount, &interval_seconds, &false, &None, &None::<u64>,
    );

    let events = env.events().all();
    let last = events.last().unwrap();

    assert_eq!(last.0, contract_id);
    assert_eq!(
        Symbol::from_val(&env, &last.1.get(0).unwrap()),
        Symbol::new(&env, "created")
    );
    let evt: SubscriptionCreatedEvent = FromVal::from_val(&env, &last.2);
    assert_eq!(evt.subscription_id, subscription_id);
    assert_eq!(evt.subscriber, subscriber);
    assert_eq!(evt.merchant, merchant);
    assert_eq!(evt.token, token_address);
    assert_eq!(evt.amount, amount);
    assert_eq!(evt.interval_seconds, interval_seconds);
    assert_eq!(evt.lifetime_cap, None);
    assert_eq!(evt.expires_at, None);
}
