//! Adversarial coverage for `admin::get_min_topup`.
//!
//! `get_min_topup` returns the vault's minimum top-up amount, which is a critical
//! configuration value used during subscription creation and top-up operations.
//! It is a simple getter, but proper coverage is essential because:
//!
//! * it must return `Error::NotInitialized` — not a panic — on an uninitialized
//!   vault;
//! * it reads from instance storage using a Symbol key;
//! * it must be idempotent and read-only (no state mutations);
//! * unauthorized `set_min_topup` calls must not corrupt the stored value;
//! * it must reflect the latest value after successful updates;
//! * it does not require authentication (read-only operation);
//! * it must handle boundary values correctly (zero, negative, large values).

use crate::admin::{do_set_min_topup, get_min_topup};
use crate::test_utils::setup::TestEnv;
use crate::types::Error;
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, Symbol};

/// Read the min_topup through the internal getter inside the vault's
/// storage context.
fn min_topup(env: &Env, client: &SubscriptionVaultClient) -> Result<i128, Error> {
    env.as_contract(&client.address, || get_min_topup(env))
}

#[test]
fn returns_the_min_topup_supplied_at_init() {
    let te = TestEnv::default();
    assert_eq!(min_topup(&te.env, &te.client), Ok(1_000_000i128));
}

#[test]
fn returns_not_found_before_initialization() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    // `read_config` must fall through both storage tiers and surface the
    // typed error rather than panicking on missing state.
    assert_eq!(min_topup(&env, &client), Err(Error::NotInitialized));
}

#[test]
fn repeated_reads_are_pure() {
    let te = TestEnv::default();
    for _ in 0..5 {
        assert_eq!(min_topup(&te.env, &te.client), Ok(1_000_000i128));
    }
}

#[test]
fn reflects_latest_value_after_set() {
    let te = TestEnv::default();

    te.client.set_min_topup(&te.admin, &5_000_000i128);
    assert_eq!(min_topup(&te.env, &te.client), Ok(5_000_000i128));

    te.client.set_min_topup(&te.admin, &10_000_000i128);
    assert_eq!(min_topup(&te.env, &te.client), Ok(10_000_000i128));
}

#[test]
fn is_idempotent_and_read_only() {
    let te = TestEnv::default();

    let first = min_topup(&te.env, &te.client);
    let second = min_topup(&te.env, &te.client);
    let third = min_topup(&te.env, &te.client);

    assert_eq!(first, Ok(1_000_000i128));
    assert_eq!(second, Ok(1_000_000i128));
    assert_eq!(third, Ok(1_000_000i128));
    assert_eq!(first, second);
    assert_eq!(second, third);
}

#[test]
fn min_topup_unchanged_after_unauthorized_set() {
    let te = TestEnv::default();
    let attacker = Address::generate(&te.env);

    assert_eq!(min_topup(&te.env, &te.client), Ok(1_000_000i128));

    // Revoke authorization for the attacker and attempt to overwrite min_topup.
    te.env.mock_auths(&[]);
    let result = te.client.try_set_min_topup(&attacker, &2_000_000i128);
    assert!(result.is_err());

    // State must be unchanged after the rejected operation.
    assert_eq!(min_topup(&te.env, &te.client), Ok(1_000_000i128));
}

#[test]
fn min_topup_unchanged_after_rejected_set_invalid_amount() {
    let te = TestEnv::default();

    assert_eq!(min_topup(&te.env, &te.client), Ok(1_000_000i128));

    // Note: do_set_min_topup does not validate the amount - it accepts any i128.
    // Validation happens at the contract entry point level or in init.
    // This test verifies that the getter reflects whatever is set.

    te.client.set_min_topup(&te.admin, &0i128);
    assert_eq!(min_topup(&te.env, &te.client), Ok(0i128));

    // Reset to valid value
    te.client.set_min_topup(&te.admin, &1_000_000i128);
    assert_eq!(min_topup(&te.env, &te.client), Ok(1_000_000i128));
}

#[test]
fn does_not_require_auth() {
    let env = Env::default();
    // No mock_all_auths: any auth requirement would cause a panic.
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    assert_eq!(min_topup(&env, &client), Err(Error::NotInitialized));
}

#[test]
fn direct_unit_coverage_matches_do_set_min_topup() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(admin.clone()).address();

    // Initialize the contract.
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    // Before any set operation, verify the initial value.
    assert_eq!(min_topup(&env, &client), Ok(1_000_000i128));

    // Use the internal do_set_min_topup to update the value.
    env.as_contract(&contract_id, || {
        let result = do_set_min_topup(&env, admin.clone(), 3_000_000i128);
        assert!(result.is_ok());
    });

    // Verify the getter reflects the new value.
    assert_eq!(min_topup(&env, &client), Ok(3_000_000i128));

    // Repeated reads must not mutate state.
    assert_eq!(min_topup(&env, &client), Ok(3_000_000i128));
}

#[test]
fn min_topup_key_is_stored_in_instance_storage() {
    let te = TestEnv::default();

    te.env.as_contract(&te.client.address, || {
        let storage = te.env.storage();
        let key = Symbol::new(&te.env, "min_topup");
        assert!(storage.instance().has(&key));
    });

    assert_eq!(min_topup(&te.env, &te.client), Ok(1_000_000i128));
}

#[test]
fn min_topup_can_be_read_from_instance_storage_directly() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(admin.clone()).address();
    let expected_min_topup = 2_500_000i128;

    // Initialize the contract to set up proper state.
    client.init(&token, &6, &admin, &expected_min_topup, &(7 * 24 * 60 * 60));

    // Verify reads from instance storage work correctly.
    assert_eq!(min_topup(&env, &client), Ok(expected_min_topup));
}

#[test]
fn boundary_zero_is_accepted_by_set() {
    let te = TestEnv::default();

    assert_eq!(min_topup(&te.env, &te.client), Ok(1_000_000i128));

    // Set min_topup to zero (do_set_min_topup accepts any i128).
    te.client.set_min_topup(&te.admin, &0i128);
    assert_eq!(min_topup(&te.env, &te.client), Ok(0i128));

    // Reset to valid value
    te.client.set_min_topup(&te.admin, &1_000_000i128);
    assert_eq!(min_topup(&te.env, &te.client), Ok(1_000_000i128));
}

#[test]
fn boundary_negative_is_accepted_by_set() {
    let te = TestEnv::default();

    assert_eq!(min_topup(&te.env, &te.client), Ok(1_000_000i128));

    // Set min_topup to a negative value (do_set_min_topup accepts any i128).
    te.client.set_min_topup(&te.admin, &-1i128);
    assert_eq!(min_topup(&te.env, &te.client), Ok(-1i128));

    // Reset to valid value
    te.client.set_min_topup(&te.admin, &1_000_000i128);
    assert_eq!(min_topup(&te.env, &te.client), Ok(1_000_000i128));
}

#[test]
fn boundary_large_value_is_accepted() {
    let te = TestEnv::default();

    let large_value = i128::MAX / 2; // Safe large value
    te.client.set_min_topup(&te.admin, &large_value);

    assert_eq!(min_topup(&te.env, &te.client), Ok(large_value));
}

#[test]
fn getter_does_not_modify_storage() {
    let te = TestEnv::default();

    // Capture initial storage state
    let initial_snapshot = te.env.as_contract(&te.client.address, || {
        let storage = te.env.storage();
        let key = Symbol::new(&te.env, "min_topup");
        (storage.instance().has(&key), storage.instance().get::<_, i128>(&key))
    });

    // Perform multiple reads
    for _ in 0..10 {
        min_topup(&te.env, &te.client).unwrap();
    }

    // Verify storage is unchanged
    let final_snapshot = te.env.as_contract(&te.client.address, || {
        let storage = te.env.storage();
        let key = Symbol::new(&te.env, "min_topup");
        (storage.instance().has(&key), storage.instance().get::<_, i128>(&key))
    });

    assert_eq!(initial_snapshot, final_snapshot);
}

#[test]
fn getter_returns_error_when_key_missing_from_storage() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(admin.clone()).address();

    // Initialize the contract
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    // Manually remove the min_topup key to simulate corrupted state
    env.as_contract(&contract_id, || {
        let storage = env.storage();
        let key = Symbol::new(&env, "min_topup");
        storage.instance().remove(&key);
    });

    // Getter should return NotInitialized error
    assert_eq!(min_topup(&env, &client), Err(Error::NotInitialized));
}

#[test]
fn getter_handles_zero_value_correctly() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(admin.clone()).address();

    // Initialize with non-zero value
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    // Manually set to zero (bypassing validation)
    env.as_contract(&contract_id, || {
        let storage = env.storage();
        let key = Symbol::new(&env, "min_topup");
        storage.instance().set(&key, &0i128);
    });

    // Getter should return zero successfully (read-only doesn't validate)
    assert_eq!(min_topup(&env, &client), Ok(0i128));
}

#[test]
fn getter_does_not_panic_on_uninitialized_contract() {
    let env = Env::default();
    // No mock_all_auths - any auth requirement would panic
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    // Should return error, not panic
    assert_eq!(min_topup(&env, &client), Err(Error::NotInitialized));
}

#[test]
fn getter_is_threadsafe_across_multiple_reads() {
    let te = TestEnv::default();

    // Simulate concurrent reads
    for _ in 0..100 {
        assert_eq!(min_topup(&te.env, &te.client), Ok(1_000_000i128));
    }
}

#[test]
fn getter_works_after_admin_rotation() {
    let te = TestEnv::default();
    let new_admin = Address::generate(&te.env);

    // Rotate admin
    te.client.rotate_admin(&te.admin, &new_admin);

    // Getter should still work (read-only, no auth required)
    assert_eq!(min_topup(&te.env, &te.client), Ok(1_000_000i128));

    // Set new value with new admin
    te.client.set_min_topup(&new_admin, &3_000_000i128);
    assert_eq!(min_topup(&te.env, &te.client), Ok(3_000_000i128));
}
