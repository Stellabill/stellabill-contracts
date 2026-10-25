#![cfg(test)]

use crate::{admin, Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{testutils::Address as _, Address, Env, Symbol};

fn setup_admin_env() -> (Env, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(admin.clone()).address();

    // The init function gets called by client.init
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, contract_id, token, admin)
}

#[test]
fn test_do_set_grace_period_success() {
    let (env, contract_id, _, admin) = setup_admin_env();

    let new_grace_period: u64 = 30 * 24 * 60 * 60; // 30 days

    env.as_contract(&contract_id, || {
        let result = admin::do_set_grace_period(&env, admin.clone(), new_grace_period);
        assert!(result.is_ok());

        let fetched = admin::get_grace_period(&env).unwrap();
        assert_eq!(fetched, new_grace_period);
    });
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_do_set_grace_period_unauthorized_stranger() {
    let env = Env::default();
    
    // We do not mock all auths here, because we want the auth signature to fail for the stranger
    // Actually, Soroban SDK require_auth fails with Auth/InvalidAction when not mocked and not signed
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let stranger = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(admin.clone()).address();

    env.mock_all_auths();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    // Turn off mock_all_auths to test explicit auth failure
    env.mock_auths(&[]);

    env.as_contract(&contract_id, || {
        let _ = admin::do_set_grace_period(&env, stranger, 100);
    });
}

#[test]
fn test_do_set_grace_period_unauthorized_wrong_admin_mocked() {
    let (env, contract_id, _, admin) = setup_admin_env();
    let stranger = Address::generate(&env);

    // Get the old grace period to ensure it's unchanged
    let old_grace_period = env.as_contract(&contract_id, || {
        admin::get_grace_period(&env).unwrap()
    });

    // We can call do_set_grace_period with stranger. The auth will succeed because mock_all_auths is on,
    // but our manual check `if admin != stored_admin` will return Error::Unauthorized
    env.as_contract(&contract_id, || {
        let result = admin::do_set_grace_period(&env, stranger, 100);
        assert_eq!(result.err(), Some(Error::Unauthorized));

        // Ensure state wasn't modified
        let current_grace = admin::get_grace_period(&env).unwrap();
        assert_eq!(current_grace, old_grace_period);
    });
}

#[test]
fn test_do_set_grace_period_boundary_zero() {
    let (env, contract_id, _, admin) = setup_admin_env();

    env.as_contract(&contract_id, || {
        let result = admin::do_set_grace_period(&env, admin.clone(), 0);
        assert!(result.is_ok());

        assert_eq!(admin::get_grace_period(&env).unwrap(), 0);
    });
}

#[test]
fn test_do_set_grace_period_boundary_max() {
    let (env, contract_id, _, admin) = setup_admin_env();

    env.as_contract(&contract_id, || {
        let result = admin::do_set_grace_period(&env, admin.clone(), u64::MAX);
        assert!(result.is_ok());

        assert_eq!(admin::get_grace_period(&env).unwrap(), u64::MAX);
    });
}

#[test]
fn test_do_set_grace_period_uninitialized_state() {
    let env = Env::default();
    env.mock_all_auths();
    
    // Contract registered but NOT initialized
    let contract_id = env.register(SubscriptionVault, ());
    let some_admin = Address::generate(&env);

    env.as_contract(&contract_id, || {
        // Should fail with NotInitialized because stored_admin doesn't exist
        let result = admin::do_set_grace_period(&env, some_admin.clone(), 3600);
        assert_eq!(result.err(), Some(Error::NotInitialized));
    });
}
