use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{testutils::Address as _, Address, Env, String};

#[test]
fn test_merchant_config_governance_enforced() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant_a = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let redirect_url = String::from_str(&env, "https://stellabill.io/success");

    // Success: Merchant A can set their own config
    client.set_merchant_config(&merchant_a, &None, &redirect_url, &false);

    let config = client.get_merchant_config(&merchant_a).unwrap();
    assert_eq!(config.redirect_url, redirect_url);
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_unauthorized_merchant_config_update() {
    let env = Env::default();
    // No mock_all_auths here. Calling require_auth() without a signature
    // will trigger an Auth error from the Soroban host.
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let redirect_url = String::from_str(&env, "https://malicious.com");

    client.set_merchant_config(&merchant, &None, &redirect_url, &false);
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_unauthorized_protocol_fee_update() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let stranger = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let treasury = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

    // Should fail because stranger is not the stored admin
    client.set_protocol_fee(&stranger, &treasury, &500);
}
