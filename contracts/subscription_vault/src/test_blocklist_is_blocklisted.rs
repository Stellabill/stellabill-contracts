//! Focused adversarial coverage for the public `is_blocklisted` query.

use crate::test_utils::setup::TestEnv;
use crate::{Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

#[test]
fn is_blocklisted_is_false_for_uninitialized_env_and_unknown_addresses() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let address = Address::generate(&env);

    // The read-only query does not require initialization or caller auth.
    assert!(!client.is_blocklisted(&address));
}

#[test]
fn is_blocklisted_tracks_add_remove_and_isolated_addresses() {
    let test_env = TestEnv::default();
    let blocked = Address::generate(&test_env.env);
    let unrelated = Address::generate(&test_env.env);

    assert!(!test_env.client.is_blocklisted(&blocked));
    assert!(!test_env.client.is_blocklisted(&unrelated));

    test_env
        .client
        .add_to_blocklist(&test_env.admin, &blocked, &None);
    assert!(test_env.client.is_blocklisted(&blocked));
    assert!(!test_env.client.is_blocklisted(&unrelated));

    test_env
        .client
        .remove_from_blocklist(&test_env.admin, &blocked);
    assert!(!test_env.client.is_blocklisted(&blocked));
    assert!(!test_env.client.is_blocklisted(&unrelated));
}

#[test]
fn rejected_duplicate_add_preserves_blocklisted_state() {
    let test_env = TestEnv::default();
    let blocked = Address::generate(&test_env.env);

    test_env
        .client
        .add_to_blocklist(&test_env.admin, &blocked, &None);
    let before = test_env.client.get_blocklist_entry(&blocked);

    let rejected = test_env
        .client
        .try_add_to_blocklist(&test_env.admin, &blocked, &None);
    assert_eq!(rejected, Err(Ok(Error::InvalidInput)));

    assert!(test_env.client.is_blocklisted(&blocked));
    let after = test_env.client.get_blocklist_entry(&blocked);
    assert_eq!(after.subscriber, before.subscriber);
    assert_eq!(after.added_by, before.added_by);
    assert_eq!(after.added_at, before.added_at);
    assert_eq!(after.reason, before.reason);
}

#[test]
fn unauthorized_remove_preserves_blocklisted_state() {
    let test_env = TestEnv::default();
    let blocked = Address::generate(&test_env.env);
    let unauthorized = Address::generate(&test_env.env);

    test_env
        .client
        .add_to_blocklist(&test_env.admin, &blocked, &None);
    let before = test_env.client.get_blocklist_entry(&blocked);

    let rejected = test_env
        .client
        .try_remove_from_blocklist(&unauthorized, &blocked);
    assert_eq!(rejected, Err(Ok(Error::Forbidden)));

    assert!(test_env.client.is_blocklisted(&blocked));
    let after = test_env.client.get_blocklist_entry(&blocked);
    assert_eq!(after.subscriber, before.subscriber);
    assert_eq!(after.added_by, before.added_by);
    assert_eq!(after.added_at, before.added_at);
    assert_eq!(after.reason, before.reason);
}
