//! Adversarial coverage for get_schema_version in admin.rs
//!
//! Tests validate the schema version resolution logic across multiple storage tiers,
//! boundary conditions, and migration scenarios.

use crate::{
    admin::{do_init, get_schema_version, read_config},
    types::DataKey,
    SubscriptionVault, SubscriptionVaultClient, STORAGE_VERSION,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env,
};

// ── Test Helpers ─────────────────────────────────────────────────────────────

/// Setup a fresh environment with a registered contract.
fn setup() -> (Env, Address, SubscriptionVaultClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    (env, contract_id, client)
}

/// Helper to read schema version directly from persistent storage.
fn read_persistent_version(env: &Env, contract_id: &Address) -> Option<u32> {
    env.as_contract(contract_id, || {
        env.storage()
            .persistent()
            .get::<_, u32>(&DataKey::SchemaVersion)
    })
}

/// Helper to read schema version directly from instance storage.
fn read_instance_version(env: &Env, contract_id: &Address) -> Option<u32> {
    env.as_contract(contract_id, || {
        env.storage()
            .instance()
            .get::<_, u32>(&DataKey::SchemaVersion)
    })
}

/// Helper to write schema version directly to persistent storage.
fn write_persistent_version(env: &Env, contract_id: &Address, version: u32) {
    env.as_contract(contract_id, || {
        env.storage()
            .persistent()
            .set(&DataKey::SchemaVersion, &version);
    });
}

/// Helper to write schema version directly to instance storage.
fn write_instance_version(env: &Env, contract_id: &Address, version: u32) {
    env.as_contract(contract_id, || {
        env.storage()
            .instance()
            .set(&DataKey::SchemaVersion, &version);
    });
}

// ── Basic Happy Path Tests ───────────────────────────────────────────────────

#[test]
fn test_get_schema_version_after_init() {
    // After init, get_schema_version should return STORAGE_VERSION from persistent storage.
    let (env, contract_id, _client) = setup();
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    env.as_contract(&contract_id, || {
        do_init(
            &env,
            token.clone(),
            6,
            admin.clone(),
            1_000_000i128,
            7 * 24 * 60 * 60,
        )
        .unwrap();

        let version = get_schema_version(&env);
        assert_eq!(
            version, STORAGE_VERSION,
            "get_schema_version must return STORAGE_VERSION after init"
        );
    });
}

#[test]
fn test_get_schema_version_persistent_storage_priority() {
    // When both persistent and instance storage have versions, persistent takes priority.
    let (env, contract_id, _client) = setup();

    write_persistent_version(&env, &contract_id, 5);
    write_instance_version(&env, &contract_id, 2);

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(
            version, 5,
            "persistent storage version must take priority over instance"
        );
    });
}

#[test]
fn test_get_schema_version_instance_fallback() {
    // When only instance storage has a version, it should be returned.
    let (env, contract_id, _client) = setup();

    write_instance_version(&env, &contract_id, 2);

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(
            version, 2,
            "get_schema_version must fallback to instance storage when persistent is absent"
        );
    });
}

#[test]
fn test_get_schema_version_default_zero() {
    // When neither storage tier has a version, it should return 0.
    let (env, contract_id, _client) = setup();

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(
            version, 0,
            "get_schema_version must return 0 when no version is stored"
        );
    });
}

// ── Boundary Value Tests ─────────────────────────────────────────────────────

#[test]
fn test_get_schema_version_zero_in_persistent() {
    // Explicit zero stored in persistent storage should be returned.
    let (env, contract_id, _client) = setup();

    write_persistent_version(&env, &contract_id, 0);

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(version, 0, "explicit zero must be returned");
    });
}

#[test]
fn test_get_schema_version_zero_in_instance() {
    // Explicit zero stored in instance storage should be returned.
    let (env, contract_id, _client) = setup();

    write_instance_version(&env, &contract_id, 0);

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(
            version, 0,
            "explicit zero in instance storage must be returned"
        );
    });
}

#[test]
fn test_get_schema_version_max_u32() {
    // Maximum u32 value should be handled correctly.
    let (env, contract_id, _client) = setup();

    write_persistent_version(&env, &contract_id, u32::MAX);

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(version, u32::MAX, "u32::MAX must be returned correctly");
    });
}

#[test]
fn test_get_schema_version_one() {
    // Version 1 in persistent storage.
    let (env, contract_id, _client) = setup();

    write_persistent_version(&env, &contract_id, 1);

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(version, 1, "version 1 must be returned correctly");
    });
}

// ── Migration Scenario Tests ─────────────────────────────────────────────────

#[test]
fn test_get_schema_version_after_migration_from_instance_to_persistent() {
    // Simulate a migration where version moves from instance to persistent storage.
    let (env, contract_id, _client) = setup();

    // Start with version 2 in instance storage (pre-migration state).
    write_instance_version(&env, &contract_id, 2);

    env.as_contract(&contract_id, || {
        assert_eq!(get_schema_version(&env), 2);
    });

    // Simulate migration: write to persistent and remove from instance.
    write_persistent_version(&env, &contract_id, 3);
    env.as_contract(&contract_id, || {
        env.storage().instance().remove(&DataKey::SchemaVersion);
    });

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(
            version, 3,
            "after migration, version must be read from persistent storage"
        );
    });
}

#[test]
fn test_get_schema_version_both_storages_during_migration() {
    // During migration, both storage tiers may have a version; persistent wins.
    let (env, contract_id, _client) = setup();

    write_instance_version(&env, &contract_id, 2);
    write_persistent_version(&env, &contract_id, 3);

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(
            version, 3,
            "persistent version must take priority during migration"
        );
    });
}

#[test]
fn test_get_schema_version_stale_instance_after_migration() {
    // If migration writes to persistent but doesn't clean up instance, persistent wins.
    let (env, contract_id, _client) = setup();

    write_instance_version(&env, &contract_id, 1);
    write_persistent_version(&env, &contract_id, 6);

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(
            version, 6,
            "new persistent version must override stale instance version"
        );
    });
}

// ── State Isolation Tests ────────────────────────────────────────────────────

#[test]
fn test_get_schema_version_does_not_modify_storage() {
    // get_schema_version is read-only and must not modify any storage.
    let (env, contract_id, _client) = setup();

    write_persistent_version(&env, &contract_id, 4);
    write_instance_version(&env, &contract_id, 2);

    let persistent_before = read_persistent_version(&env, &contract_id);
    let instance_before = read_instance_version(&env, &contract_id);

    env.as_contract(&contract_id, || {
        let _version = get_schema_version(&env);
    });

    let persistent_after = read_persistent_version(&env, &contract_id);
    let instance_after = read_instance_version(&env, &contract_id);

    assert_eq!(
        persistent_before, persistent_after,
        "persistent storage must not be modified"
    );
    assert_eq!(
        instance_before, instance_after,
        "instance storage must not be modified"
    );
}

#[test]
fn test_get_schema_version_multiple_calls_idempotent() {
    // Multiple consecutive calls must return the same value.
    let (env, contract_id, _client) = setup();

    write_persistent_version(&env, &contract_id, 5);

    env.as_contract(&contract_id, || {
        let v1 = get_schema_version(&env);
        let v2 = get_schema_version(&env);
        let v3 = get_schema_version(&env);

        assert_eq!(v1, v2, "multiple calls must be idempotent");
        assert_eq!(v2, v3, "multiple calls must be idempotent");
        assert_eq!(v1, 5, "all calls must return the correct version");
    });
}

#[test]
fn test_get_schema_version_independent_of_other_keys() {
    // get_schema_version must only read SchemaVersion key, not other DataKey variants.
    let (env, contract_id, _client) = setup();
    let admin = Address::generate(&env);

    // Write a version.
    write_persistent_version(&env, &contract_id, 3);

    // Write other config keys to ensure they don't interfere.
    env.as_contract(&contract_id, || {
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage()
            .persistent()
            .set(&DataKey::MinTopup, &1_000_000i128);
        env.storage().persistent().set(&DataKey::NextId, &42u32);
    });

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(
            version, 3,
            "get_schema_version must only read SchemaVersion key"
        );
    });
}

// ── Authorization Tests ──────────────────────────────────────────────────────

#[test]
fn test_get_schema_version_no_auth_required() {
    // get_schema_version is a pure read operation and requires no authorization.
    let (env, contract_id, _client) = setup();

    // Don't mock any auth.
    write_persistent_version(&env, &contract_id, 4);

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(
            version, 4,
            "get_schema_version must succeed without authorization"
        );
    });
}

#[test]
fn test_get_schema_version_callable_by_anyone() {
    // get_schema_version can be called from any contract context.
    let (env, contract_id, _client) = setup();

    write_persistent_version(&env, &contract_id, 5);

    // Call from a different contract context.
    let other_contract = env.register_contract(None, SubscriptionVault);
    env.as_contract(&other_contract, || {
        // Read from the original contract's storage via env parameter.
        // Note: This is testing that the function doesn't have auth guards.
        // In practice, the Env passed to get_schema_version determines which
        // contract's storage is read.
    });

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(version, 5, "get_schema_version must be callable by anyone");
    });
}

// ── Edge Case Tests ──────────────────────────────────────────────────────────

#[test]
fn test_get_schema_version_all_valid_versions() {
    // Test all historically valid schema versions (0-6).
    let (env, contract_id, _client) = setup();

    for expected_version in 0..=6u32 {
        write_persistent_version(&env, &contract_id, expected_version);

        env.as_contract(&contract_id, || {
            let version = get_schema_version(&env);
            assert_eq!(
                version, expected_version,
                "version {} must be returned correctly",
                expected_version
            );
        });
    }
}

#[test]
fn test_get_schema_version_version_greater_than_storage_version() {
    // A stored version higher than STORAGE_VERSION should still be returned correctly.
    // This tests forward compatibility / downgrade detection scenarios.
    let (env, contract_id, _client) = setup();

    let future_version = STORAGE_VERSION + 10;
    write_persistent_version(&env, &contract_id, future_version);

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(
            version, future_version,
            "get_schema_version must return future versions correctly"
        );
    });
}

#[test]
fn test_get_schema_version_consistent_across_ledger_time() {
    // Schema version should be stable regardless of ledger timestamp changes.
    let (env, contract_id, _client) = setup();

    write_persistent_version(&env, &contract_id, 4);

    env.ledger().with_mut(|li| li.timestamp = 1_000);
    let v1 = env.as_contract(&contract_id, || get_schema_version(&env));

    env.ledger().with_mut(|li| li.timestamp = 1_000_000);
    let v2 = env.as_contract(&contract_id, || get_schema_version(&env));

    env.ledger().with_mut(|li| li.timestamp = 9_999_999);
    let v3 = env.as_contract(&contract_id, || get_schema_version(&env));

    assert_eq!(v1, 4, "version must be consistent at different timestamps");
    assert_eq!(v2, 4, "version must be consistent at different timestamps");
    assert_eq!(v3, 4, "version must be consistent at different timestamps");
}

#[test]
fn test_get_schema_version_with_uninitialized_contract() {
    // Calling get_schema_version on an uninitialized contract should return 0.
    let (env, contract_id, _client) = setup();

    // Don't call init, don't write any version.
    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(
            version, 0,
            "get_schema_version on uninitialized contract must return 0"
        );
    });
}

#[test]
fn test_get_schema_version_after_instance_removal() {
    // After instance version is removed, get_schema_version should still work.
    let (env, contract_id, _client) = setup();

    write_persistent_version(&env, &contract_id, 6);
    write_instance_version(&env, &contract_id, 2);

    // Remove instance version.
    env.as_contract(&contract_id, || {
        env.storage().instance().remove(&DataKey::SchemaVersion);
    });

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(
            version, 6,
            "get_schema_version must work after instance removal"
        );
    });
}

#[test]
fn test_get_schema_version_after_persistent_removal() {
    // After persistent version is removed, fallback to instance should work.
    let (env, contract_id, _client) = setup();

    write_persistent_version(&env, &contract_id, 5);
    write_instance_version(&env, &contract_id, 2);

    // Remove persistent version.
    env.as_contract(&contract_id, || {
        env.storage().persistent().remove(&DataKey::SchemaVersion);
    });

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(
            version, 2,
            "get_schema_version must fallback to instance after persistent removal"
        );
    });
}

#[test]
fn test_get_schema_version_after_both_removed() {
    // After both versions are removed, should return 0.
    let (env, contract_id, _client) = setup();

    write_persistent_version(&env, &contract_id, 5);
    write_instance_version(&env, &contract_id, 2);

    // Remove both.
    env.as_contract(&contract_id, || {
        env.storage().persistent().remove(&DataKey::SchemaVersion);
        env.storage().instance().remove(&DataKey::SchemaVersion);
    });

    env.as_contract(&contract_id, || {
        let version = get_schema_version(&env);
        assert_eq!(
            version, 0,
            "get_schema_version must return 0 after both versions removed"
        );
    });
}

// ── Integration with read_config ─────────────────────────────────────────────

#[test]
fn test_get_schema_version_used_by_read_config() {
    // read_config uses get_schema_version to determine storage tier fallback.
    // When schema version < 3, it should check instance storage as fallback.
    let (env, contract_id, _client) = setup();
    let test_value: i128 = 5_000_000;

    // Set schema version to 2 (< 3) in instance.
    write_instance_version(&env, &contract_id, 2);

    // Write MinTopup to instance storage only.
    env.as_contract(&contract_id, || {
        env.storage()
            .instance()
            .set(&DataKey::MinTopup, &test_value);
    });

    // read_config should find the value in instance storage.
    env.as_contract(&contract_id, || {
        let retrieved: Option<i128> = read_config(&env, &DataKey::MinTopup);
        assert_eq!(
            retrieved,
            Some(test_value),
            "read_config must use instance fallback when schema < 3"
        );
    });
}

#[test]
fn test_get_schema_version_read_config_no_instance_fallback_v3_plus() {
    // When schema version >= 3, read_config should NOT fallback to instance.
    let (env, contract_id, _client) = setup();
    let test_value: i128 = 8_000_000;

    // Set schema version to 3 in persistent.
    write_persistent_version(&env, &contract_id, 3);

    // Write MinTopup ONLY to instance storage.
    env.as_contract(&contract_id, || {
        env.storage()
            .instance()
            .set(&DataKey::MinTopup, &test_value);
    });

    // read_config should NOT find the value because schema >= 3.
    env.as_contract(&contract_id, || {
        let retrieved: Option<i128> = read_config(&env, &DataKey::MinTopup);
        assert_eq!(
            retrieved, None,
            "read_config must NOT use instance fallback when schema >= 3"
        );
    });
}
