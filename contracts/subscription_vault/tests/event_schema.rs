#![cfg(test)]

extern crate alloc;

use soroban_sdk::{
    testutils::{Address as _, Events},
    Address, Env, FromVal, TryFromVal,
};
use subscription_vault::{
    AdminRotatedEvent, SubscriptionCreatedEvent, SubscriptionVault, SubscriptionVaultClient,
    EVENT_SCHEMA_VERSION,
};

#[test]
fn test_nonce_consumed_and_admin_rotated_events_emitted() {
    let env = Env::default();
    env.mock_all_auths();

    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();

    let admin = Address::generate(&env);
    let new_admin = Address::generate(&env);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    client.init(&token_address, &7u32, &admin, &1_000_000i128, &3600u64);
    client.rotate_admin(&admin, &new_admin, &0u64);

    let events = env.events().all();
    assert!(
        events.len() >= 2,
        "rotate_admin must emit at least two events"
    );

    let admin_rotated: AdminRotatedEvent = FromVal::from_val(
        &env,
        &events
            .last()
            .expect("admin rotation event must be emitted")
            .2,
    );
    assert_eq!(admin_rotated.schema_version, EVENT_SCHEMA_VERSION);
}

#[test]
fn test_subscription_created_event_emitted() {
    let env = Env::default();
    env.mock_all_auths();

    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();

    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    client.init(&token_address, &7u32, &admin, &1_000_000i128, &3600u64);

    client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &None,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
);

    let events = env.events().all();
    assert!(
        events.len() >= 1,
        "create_subscription must emit at least one event"
    );

    let event = &events.last().expect("subscription created event must be emitted");
    let topics = event.1.clone();
    let topic0: soroban_sdk::Symbol = FromVal::from_val(&env, &topics.get(0).unwrap());
    assert_eq!(topic0, soroban_sdk::Symbol::new(&env, "created"));

    let created: SubscriptionCreatedEvent = FromVal::from_val(
        &env,
        &event.2,
    );
    assert_eq!(created.schema_version, EVENT_SCHEMA_VERSION);
}

#[test]
fn test_subscription_charged_event_emitted() {
    use subscription_vault::SubscriptionChargedEvent;
    
    let env = Env::default();
    env.mock_all_auths();

    let token_admin = Address::generate(&env);
    let token_address = env.register_stellar_asset_contract_v2(token_admin.clone()).address();
    let token = soroban_sdk::token::StellarAssetClient::new(&env, &token_address);

    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    StellarAssetClient::new(&env, &token_address).mint(&subscriber, &10_000_000_000);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    client.init(&token_address, &7u32, &admin, &1_000_000i128, &3600u64);

    let sub_id = client.create_subscription(
        &subscriber, &merchant, &1_000_000i128, &(30 * 24 * 60 * 60u64), &false, &None, &None::<u64>,
        &None::<u32>, &None::<soroban_sdk::Symbol>,
    );
    
    client.deposit_funds(&sub_id, &subscriber, &5_000_000i128, &None);

    client.batch_charge(&soroban_sdk::vec![&env, sub_id], &0u64);

    let events = env.events().all();
    
    let mut found = false;
    for event in events.iter() {
        let topics = event.1.clone();
        if topics.len() > 0 {
            let topic0 = Option::<soroban_sdk::Symbol>::from_val(&env, &topics.get(0).unwrap());
            if topic0 == Some(soroban_sdk::Symbol::new(&env, "charged")) {
                let charged: SubscriptionChargedEvent = FromVal::from_val(&env, &event.2);
                assert_eq!(charged.schema_version, EVENT_SCHEMA_VERSION);
                found = true;
                break;
            }
        }
    }
    assert!(found, "SubscriptionChargedEvent not found");
}

#[test]
fn test_merchant_withdrawal_event_emitted() {
    use subscription_vault::MerchantWithdrawalEvent;
    
    let env = Env::default();
    env.mock_all_auths();

    let token_admin = Address::generate(&env);
    let token_address = env.register_stellar_asset_contract_v2(token_admin.clone()).address();
    let token = soroban_sdk::token::StellarAssetClient::new(&env, &token_address);

    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    StellarAssetClient::new(&env, &token_address).mint(&subscriber, &10_000_000_000);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    client.init(&token_address, &7u32, &admin, &1_000_000i128, &3600u64);

    let sub_id = client.create_subscription(
        &subscriber, &merchant, &1_000_000i128, &(30 * 24 * 60 * 60u64), &false, &None, &None::<u64>,
        &None::<u32>, &None::<soroban_sdk::Symbol>,
    );
    
    client.deposit_funds(&sub_id, &subscriber, &5_000_000i128, &None);
    client.batch_charge(&soroban_sdk::vec![&env, sub_id], &0u64);
    
    client.withdraw_merchant_token_funds(&merchant, &token_address, &500_000i128);

    let events = env.events().all();
    
    let mut found = false;
    for event in events.iter() {
        let topics = event.1.clone();
        if topics.len() > 0 {
            let topic0 = Option::<soroban_sdk::Symbol>::from_val(&env, &topics.get(0).unwrap());
            if topic0 == Some(soroban_sdk::Symbol::new(&env, "withdrawn")) {
                let withdrawn: MerchantWithdrawalEvent = FromVal::from_val(&env, &event.2);
                assert_eq!(withdrawn.schema_version, EVENT_SCHEMA_VERSION);
                found = true;
                break;
            }
        }
    }
    assert!(found, "MerchantWithdrawalEvent not found");
}
