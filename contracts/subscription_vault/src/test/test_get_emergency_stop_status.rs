#![cfg(test)]

use crate::SubscriptionVaultClient;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

fn setup() -> (Env, SubscriptionVaultClient<'static>) {
    let env = Env::default();
    let contract_id = env.register_contract(None, crate::SubscriptionVault);
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    
    env.mock_all_auths();
    
    // Using the 5-parameter initialization based on the contract signature
    client.init(&admin, &token, &1_000_000i128);
    
    (env, client)
}

#[test]
fn test_get_emergency_stop_status_happy_path() {
    let (_env, client) = setup();
    
    // Boundary/State check: Vault defaults to false
    assert_eq!(client.get_emergency_stop_status(), false, "Emergency stop should default to false");
}

#[test]
fn test_get_emergency_stop_status_no_auth_required() {
    let (env, client) = setup();
    
    // Capture authorization state before the getter
    let auths_before = env.auths().len();
    let initial_status = client.get_emergency_stop_status();
    let auths_after = env.auths().len();
    
    // Verify state remains strictly unchanged
    assert_eq!(client.get_emergency_stop_status(), initial_status);
    
    // Verify caller does NOT need authorization to read the emergency stop state
    assert_eq!(auths_before, auths_after, "Getter should not consume authorizations");
}

#[test]
#[should_panic]
fn test_get_emergency_stop_status_uninitialized() {
    let env = Env::default();
    let contract_id = env.register_contract(None, crate::SubscriptionVault);
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    
    // Adversarial: Reading state before the vault is initialized must deterministically panic/revert
    client.get_emergency_stop_status();
}

