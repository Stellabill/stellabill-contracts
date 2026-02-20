use crate::{Subscription, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

#[test]
fn test_init_and_struct() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin);
    // TODO: add create_subscription test with mock token
}

#[test]
fn test_pause_and_resume() {
    let env = Env::default();
    env.mock_all_auths();
    
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let other = Address::generate(&env);

    let sub_id = client.create_subscription(&subscriber, &merchant, &1000, &3600, &false);

    // Initial state should be Active
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);

    // Pause by subscriber
    client.pause_subscription(&sub_id, &subscriber);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Paused);

    // Resume by merchant
    client.resume_subscription(&sub_id, &merchant);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);

    // Pause by merchant
    client.pause_subscription(&sub_id, &merchant);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Paused);

    // Attempt to charge paused subscription
    let result = client.try_charge_subscription(&sub_id);
    assert!(result.is_err());

    // Resume by subscriber
    client.resume_subscription(&sub_id, &subscriber);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    
    // Attempt to charge active subscription (should succeed call-wise, even if logic is TODO)
    let result = client.try_charge_subscription(&sub_id);
    assert!(result.is_ok());
}

#[test]
#[should_panic]
fn test_unauthorized_pause() {
    let env = Env::default();
    env.mock_all_auths();
    
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let other = Address::generate(&env);

    let sub_id = client.create_subscription(&subscriber, &merchant, &1000, &3600, &false);

    // Other user attempts to pause
    client.pause_subscription(&sub_id, &other);
}

#[test]
fn test_invalid_state_transitions() {
    let env = Env::default();
    env.mock_all_auths();
    
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let sub_id = client.create_subscription(&subscriber, &merchant, &1000, &3600, &false);

    // Resume already active
    let res = client.try_resume_subscription(&sub_id, &subscriber);
    assert!(res.is_err());

    // Pause active
    client.pause_subscription(&sub_id, &subscriber);

    // Pause already paused
    let res = client.try_pause_subscription(&sub_id, &subscriber);
    assert!(res.is_err());

    // Cancel
    client.cancel_subscription(&sub_id, &subscriber);

    // Try to pause cancelled
    let res = client.try_pause_subscription(&sub_id, &subscriber);
    assert!(res.is_err());

    // Try to resume cancelled
    let res = client.try_resume_subscription(&sub_id, &subscriber);
    assert!(res.is_err());
}
