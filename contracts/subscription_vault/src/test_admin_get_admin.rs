use super::*;

use crate::admin::{do_get_admin, do_set_admin, Admin};
use crate::error::Error;
use crate::{SubscriptionVault, SubscriptionVaultClient};

use soroban_sdk::testutils:{Address, Env};

fn setup() -> (Env, SubscriptionVaultClient<'_>) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register({}, SubscriptionVault::contract_id());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    (env, client)
}

// -----------------------------------------------------------------------------
// Happy path: admin is unset before initialization.
// -----------------------------------------------------------------------------

#[test]
fn get_admin_returns_none_before_init() {
    let (_env, client) = setup();
    assert!(client.get_admin().is_none());
}

// -----------------------------------------------------------------------------
// Happy path: admin returns the exact value that was set.
// -----------------------------------------------------------------------------

#[test]
fn get_admin_returns_set_admin() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.set_admin(&admin);
    assert_eq!(client.get_admin(), Some(admin.clone()));
}

// -----------------------------------------------------------------------------
// Boundary: get_admin is a read-only operation and must not mutate state.
// -----------------------------------------------------------------------------

#[test]
fn get_admin_is_idempotent_and_read_only() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.set_admin(&admin);

    let first = client.get_admin();
    let second = client.get_admin();
    let third = client.get_admin();

    assert_eq!(first, Some(admin.clone()));
    assert_eq!(second, Some(admin.clone()));
    assert_eq!(third, Some(admin.clone()));
    assert_eq!(first, second);
    assert_eq!(second, third);
}

// -----------------------------------------------------------------------------
// Boundary: setting a new admin overwrites the previous value and get_admin
// observes the latest value.
// -----------------------------------------------------------------------------

#[test]
fn get_admin_reflects_latest_overwrite() {
    let (env, client) = setup();
    let admin_a = Address::generate(&env);
    let admin_b = Address::generate(&env);

    client.set_admin(&admin_a);
    assert_eq!(client.get_admin(), Some(admin_a.clone()));

    client.set_admin(&admin_b);
    assert_eq!(client.get_admin(), Some(admin_b.clone()));
    assert_ne!(client.get_admin(), Some(admin_a.clone()));
}

// -----------------------------------------------------------------------------
// Authorization: an unauthorized caller must not be able to change the admin,
// and get_admin must still report the original admin after the rejected call.
// -----------------------------------------------------------------------------

#[test]
fn get_admin_unchanged_after_unauthorized_set() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    let attacker = Address::generate(&env);

    client.set_admin(&admin);
    assert_eq!(client.get_admin(), Some(admin.clone()));

    // Revoke authorization for the attacker and attempt to overwrite the admin.
    env.mock_auth(soroban_sdk::testutils::MockAuth::Next){auth_{
        public_key: attacker.clone(),
        ..Default::default()
    }});
    let result = client.try_set_admin(&attacker);
    assert!(result.is_error());

    // State must be unchanged after the rejected operation.
    assert_eq!(client.get_admin(), Some(admin.clone()));
    assert_ne!(client.get_admin(), Some(attacker.clone()));
}

// -----------------------------------------------------------------------------
// Error behavior: do_get_admin does not require authorization and must not
// panic when called without any auth entry.
// -----------------------------------------------------------------------------

#[test]
fn get_admin_does_not_require_auth() {
    let env = Env::default();
    // No mock_all_auths: any auth requirement would cause a panic.
    let contract_id = env.register({}, SubscriptionVault::contract_id());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    assert!(client.get_admin().is_none());
}

// -----------------------------------------------------------------------------
// Direct unit coverage of the admin module accessors to ensure get/set stay
// consistent without going through the client wrapper.
// -----------------------------------------------------------------------------

#[test]
fn do_get_admin_matches_do_set_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register({}, SubscriptionVault::contract_id());

    // Before initialization the admin is absent.
    assert!(do_get_admin(&env, &contract_id).is_none());

    let admin = Address::generate(&env);
    do_set_admin(&env, &contract_id, &admin);
    assert_eq!(do_get_admin(&env, &contract_id), Some(admin.clone()));

    // Repeated reads must not mutate state.
    assert_eq!(do_get_admin(&env, &contract_id), Some(admin.clone()));
}

// -----------------------------------------------------------------------------
// Adversarial: a rejected set must leave the admin slot unchanged and the
// error must be observable and deterministic.
// -----------------------------------------------------------------------------

#[test]
fn do_get_admin_unchanged_after_rejected_set() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register({}, SubscriptionVault::contract_id());

    let admin = Address::generate(&env);
    do_set_admin(&env, &contract_id, &admin);
    assert_eq!(do_get_admin(&env, &contract_id), Some(admin.clone()));

    // Revoke auth and attempt to overwrite the admin.
    let attacker = Address::generate(&env);
    env.mock_auth(soroban_sdk::testutils::MockAuth::Next){auth_{
        public_key: attacker.clone(),
        ..Default::default()
    }});

    let result = do_set_admin(&env, &contract_id, &attacker);
    assert_eq!(result, Error::Unauthorized);

    // State must be unchanged after the rejected operation.
    assert_eq!(do_get_admin(&env, &contract_id), Some(admin.clone()));
    assert_ne!(do_get_admin(&env, &contract_id), Some(attacker.clone()));
}
