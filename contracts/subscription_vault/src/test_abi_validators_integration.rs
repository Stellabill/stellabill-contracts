//! Integration-level tests confirming that ABI validators are wired into
//! every affected entrypoint.
//!
//! Each test calls the entrypoint through the contract client, which exercises
//! the full dispatch path, and asserts that `Error::InvalidInput` is returned
//! before any auth or storage operations take place.

use crate::{types::Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{testutils::Address as _, Address, Env, String};

fn setup() -> (Env, SubscriptionVaultClient<'static>) {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    (env, client)
}

// ── set_metadata ──────────────────────────────────────────────────────────────

#[test]
fn test_set_metadata_rejects_empty_key() {
    let (env, client) = setup();
    env.mock_all_auths();
    let authorizer = Address::generate(&env);
    let empty_key = String::from_str(&env, "");
    let value = String::from_str(&env, "some_value");
    assert_eq!(
        client.try_set_metadata(&1, &authorizer, &empty_key, &value),
        Err(Ok(Error::InvalidInput))
    );
}

#[test]
fn test_set_metadata_rejects_whitespace_key() {
    let (env, client) = setup();
    env.mock_all_auths();
    let authorizer = Address::generate(&env);
    let ws_key = String::from_str(&env, "   ");
    let value = String::from_str(&env, "some_value");
    assert_eq!(
        client.try_set_metadata(&1, &authorizer, &ws_key, &value),
        Err(Ok(Error::InvalidInput))
    );
}

#[test]
fn test_set_metadata_rejects_empty_value() {
    let (env, client) = setup();
    env.mock_all_auths();
    let authorizer = Address::generate(&env);
    let key = String::from_str(&env, "plan_name");
    let empty_value = String::from_str(&env, "");
    assert_eq!(
        client.try_set_metadata(&1, &authorizer, &key, &empty_value),
        Err(Ok(Error::InvalidInput))
    );
}

// ── delete_metadata ───────────────────────────────────────────────────────────

#[test]
fn test_delete_metadata_rejects_empty_key() {
    let (env, client) = setup();
    env.mock_all_auths();
    let authorizer = Address::generate(&env);
    let empty_key = String::from_str(&env, "");
    assert_eq!(
        client.try_delete_metadata(&1, &authorizer, &empty_key),
        Err(Ok(Error::InvalidInput))
    );
}

#[test]
fn test_delete_metadata_rejects_whitespace_key() {
    let (env, client) = setup();
    env.mock_all_auths();
    let authorizer = Address::generate(&env);
    let ws_key = String::from_str(&env, "\t");
    assert_eq!(
        client.try_delete_metadata(&1, &authorizer, &ws_key),
        Err(Ok(Error::InvalidInput))
    );
}

// ── add_accepted_token ────────────────────────────────────────────────────────

#[test]
fn test_add_accepted_token_rejects_contract_self() {
    let (env, client) = setup();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    // Use the contract's own address as the token.
    let self_addr = client.address.clone();
    assert_eq!(
        client.try_add_accepted_token(&admin, &self_addr, &6),
        Err(Ok(Error::InvalidInput))
    );
}

// ── set_merchant_cap_default ──────────────────────────────────────────────────

#[test]
fn test_set_merchant_cap_default_rejects_contract_self() {
    let (env, client) = setup();
    env.mock_all_auths();
    let self_addr = client.address.clone();
    assert_eq!(
        client.try_set_merchant_cap_default(&self_addr, &Some(1_000_000i128)),
        Err(Ok(Error::InvalidInput))
    );
}

// ── set_protocol_fee ─────────────────────────────────────────────────────────

#[test]
fn test_set_protocol_fee_rejects_contract_self_as_treasury() {
    let (env, client) = setup();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let self_addr = client.address.clone();
    assert_eq!(
        client.try_set_protocol_fee(&admin, &self_addr, &500u32),
        Err(Ok(Error::InvalidInput))
    );
}
