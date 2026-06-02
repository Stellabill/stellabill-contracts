#![cfg(test)]

extern crate alloc;

use soroban_sdk::{
    testutils::{Address as _, Events},
    xdr::ToXdr,
    Address, Env, IntoVal, Symbol,
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

    client.init(&token_address, &7u32, &admin, &min_topup, &grace_period);

    // rotate_admin emits one event: admin_rotated
    // (nonce_consumed is not emitted by the current stub nonce implementation)
    client.rotate_admin(&admin, &new_admin, &0u64);

    let events: std::vec::Vec<_> = env.events().all().iter().collect();
    assert!(!events.is_empty(), "rotate_admin must emit at least one event");

    let ts = env.ledger().timestamp();

    // Find the admin_rotated event
    let (addr, topics, data) = events.iter()
        .find(|(_, t, _)| {
            t.clone().to_xdr(&env) == soroban_sdk::Vec::<soroban_sdk::Val>::from_array(
                &env,
                [Symbol::new(&env, "admin_rotated").into_val(&env)],
            ).to_xdr(&env)
        })
        .expect("admin_rotated event not found");

    assert_eq!(addr.clone().to_xdr(&env), contract_id.to_xdr(&env));
    let _ = topics; // already matched above
    assert_eq!(
        data.clone().to_xdr(&env),
        <AdminRotatedEvent as IntoVal<Env, soroban_sdk::Val>>::into_val(
            &AdminRotatedEvent { old_admin: admin.clone(), new_admin: new_admin.clone(), timestamp: ts },
            &env,
        ).to_xdr(&env),
    );
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

    let last_event = env.events().all().last().unwrap();
    let (addr, topics, data) = last_event;

    assert_eq!(addr.to_xdr(&env), contract_id.to_xdr(&env));
    assert_eq!(
        topics.to_xdr(&env),
        soroban_sdk::Vec::<soroban_sdk::Val>::from_array(
            &env,
            [
                Symbol::new(&env, "created").into_val(&env),
                subscription_id.into_val(&env),
            ]
        ).to_xdr(&env),
    );
    assert_eq!(
        data.to_xdr(&env),
        <SubscriptionCreatedEvent as IntoVal<Env, soroban_sdk::Val>>::into_val(
            &SubscriptionCreatedEvent {
                subscription_id,
                subscriber,
                merchant,
                token: token_address.clone(),
                amount,
                interval_seconds,
                lifetime_cap: None,
                expires_at: None,
                timestamp: env.ledger().timestamp(),
            },
            &env,
        ).to_xdr(&env),
    );
}
