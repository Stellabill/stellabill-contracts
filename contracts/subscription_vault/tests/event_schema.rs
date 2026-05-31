#![cfg(test)]

extern crate alloc;

use soroban_sdk::{
    testutils::Address as _, Address, Env, Symbol,
};
use subscription_vault::{
    SubscriptionVault, SubscriptionVaultClient, AdminRotatedEvent, NonceConsumedEvent,
    SubscriptionCreatedEvent, nonce,
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

    let min_topup: i128 = 1_000_000;
    let grace_period: u64 = 3600;

    client.init(&token_address, &7u32, &admin, &min_topup, &grace_period);

    // rotate_admin should consume the admin nonce and emit two events: nonce_consumed, admin_rotated
    client.rotate_admin(&admin, &new_admin, &0u64);

    let events = env.events().all();
    assert!(events.len() >= 2, "rotate_admin must emit at least two events (nonce + admin_rotated)");

    let ts = env.ledger().timestamp();

    assert_eq!(
        &events[0],
        &(
            contract_id.clone(),
            (Symbol::new(&env, "nonce_consumed"), admin.clone(), Symbol::new(&env, "adm_rot")).into_val(&env),
            NonceConsumedEvent { signer: admin.clone(), domain: nonce::DOMAIN_ADMIN_ROTATION, nonce: 0u64, timestamp: ts }.into_val(&env),
        )
    );

    assert_eq!(
        &events[1],
        &(
            contract_id.clone(),
            (Symbol::new(&env, "admin_rotated"),).into_val(&env),
            AdminRotatedEvent { old_admin: admin.clone(), new_admin: new_admin.clone(), timestamp: ts }.into_val(&env),
        )
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

    assert_eq!(
        last_event,
        (
            contract_id.clone(),
            (Symbol::new(&env, "created"), subscription_id).into_val(&env),
            SubscriptionCreatedEvent {
                subscription_id,
                subscriber,
                merchant,
                amount,
                interval_seconds,
                lifetime_cap: None,
                expires_at: None,
            }.into_val(&env),
        )
    );
}
