//! Focused adversarial coverage for the public `get_operator` query.

use crate::test_utils::setup::TestEnv;
use crate::{DataKey, Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

#[test]
fn get_operator_returns_none_for_an_uninitialized_contract_without_auth() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    // The query is read-only and has no caller argument or authorization check.
    assert_eq!(client.get_operator(), None);
}

#[test]
fn get_operator_reads_legacy_instance_storage_only_before_schema_version_three() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let legacy_operator = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::SchemaVersion, &2u32);
        env.storage()
            .instance()
            .set(&DataKey::Operator, &legacy_operator);
    });
    assert_eq!(client.get_operator(), Some(legacy_operator.clone()));

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::SchemaVersion, &3u32);
    });
    assert_eq!(client.get_operator(), None);
}

#[test]
fn get_operator_tracks_set_and_remove_transitions() {
    let test_env = TestEnv::default();
    let operator = Address::generate(&test_env.env);

    assert_eq!(test_env.client.get_operator(), None);
    test_env.client.set_operator(&test_env.admin, &operator);
    assert_eq!(test_env.client.get_operator(), Some(operator.clone()));

    test_env
        .env
        .ledger()
        .with_mut(|ledger| ledger.timestamp += crate::admin::CONFIG_COOLDOWN_SECS);
    test_env.client.remove_operator(&test_env.admin);
    assert_eq!(test_env.client.get_operator(), None);
}

#[test]
fn rejected_operator_mutations_preserve_the_current_value() {
    let test_env = TestEnv::default();
    let operator = Address::generate(&test_env.env);
    let replacement = Address::generate(&test_env.env);
    let stranger = Address::generate(&test_env.env);

    test_env.client.set_operator(&test_env.admin, &operator);

    let rejected_set = test_env.client.try_set_operator(&stranger, &replacement);
    assert_eq!(rejected_set, Err(Ok(Error::Forbidden)));
    assert_eq!(test_env.client.get_operator(), Some(operator.clone()));

    let rejected_remove = test_env.client.try_remove_operator(&stranger);
    assert_eq!(rejected_remove, Err(Ok(Error::Forbidden)));
    assert_eq!(test_env.client.get_operator(), Some(operator));
}
