#![cfg(test)]

extern crate std;

use crate::test_utils::setup::TestEnv;
use crate::types::{DataKey, Error, SchemaMigratedEvent};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    Address, Env, IntoVal,
};

#[test]
fn test_fresh_init_stores_in_persistent() {
    let te = TestEnv::default();
    te.env.as_contract(&te.client.address, || {
        let storage = te.env.storage();
        assert!(storage.persistent().has(&DataKey::Token));
        assert!(storage.persistent().has(&DataKey::Admin));
        assert!(storage.persistent().has(&DataKey::MinTopup));
        assert!(storage.persistent().has(&DataKey::SchemaVersion));

        assert!(!storage.instance().has(&DataKey::Token));
        assert!(!storage.instance().has(&DataKey::Admin));
        assert!(!storage.instance().has(&DataKey::MinTopup));
        assert!(!storage.instance().has(&DataKey::SchemaVersion));
    });
}

#[test]
fn test_fallback_reads_on_v2() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let client = crate::SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let min_topup = 1_000_000i128;

    env.as_contract(&contract_id, || {
        let storage = env.storage();
        storage.instance().set(&DataKey::SchemaVersion, &2u32);
        storage.instance().set(&DataKey::Token, &token);
        storage.instance().set(&DataKey::Admin, &admin);
        storage.instance().set(&DataKey::MinTopup, &min_topup);
    });

    // Check fallback reads
    let stored_token = env.as_contract(&contract_id, || {
        crate::admin::read_config::<Address>(&env, &DataKey::Token).unwrap()
    });
    assert_eq!(stored_token, token);
    assert_eq!(client.get_min_topup(), min_topup);
    
    // Auth mocking
    env.mock_all_auths();
    // Setting min topup should work and verify auth using fallback admin read
    client.set_min_topup(&admin, &2_000_000i128);
    assert_eq!(client.get_min_topup(), 2_000_000i128);
}

#[test]
fn test_migration_moves_all_keys() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let client = crate::SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let min_topup = 1_000_000i128;
    let next_id = 42u32;
    let emergency_stop = true;
    let treasury = Address::generate(&env);
    let fee_bps = 250u32;
    let operator = Address::generate(&env);

    env.as_contract(&contract_id, || {
        let storage = env.storage();
        storage.instance().set(&DataKey::SchemaVersion, &2u32);
        storage.instance().set(&DataKey::Token, &token);
        storage.instance().set(&DataKey::Admin, &admin);
        storage.instance().set(&DataKey::MinTopup, &min_topup);
        storage.instance().set(&DataKey::NextId, &next_id);
        storage.instance().set(&DataKey::EmergencyStop, &emergency_stop);
        storage.instance().set(&DataKey::Treasury, &treasury);
        storage.instance().set(&DataKey::FeeBps, &fee_bps);
        storage.instance().set(&DataKey::Operator, &operator);
    });

    env.mock_all_auths();
    env.ledger().with_mut(|li| li.timestamp = 5_000);
    
    // Call migrate_config_to_persistent
    client.migrate_config_to_persistent(&admin);

    // Verify events
    let events = env.events().all();
    let last = events.last().expect("no events");
    let payload: SchemaMigratedEvent = last.2.into_val(&env);
    assert_eq!(payload.admin, admin);
    assert_eq!(payload.from_version, 2);
    assert_eq!(payload.to_version, 3);
    assert_eq!(payload.timestamp, 5_000);

    // Verify that the keys are now in persistent storage and removed from instance storage
    env.as_contract(&contract_id, || {
        let storage = env.storage();
        
        // Persistent has them
        assert_eq!(storage.persistent().get::<_, u32>(&DataKey::SchemaVersion), Some(3u32));
        assert_eq!(storage.persistent().get::<_, Address>(&DataKey::Token), Some(token));
        assert_eq!(storage.persistent().get::<_, Address>(&DataKey::Admin), Some(admin.clone()));
        assert_eq!(storage.persistent().get::<_, i128>(&DataKey::MinTopup), Some(min_topup));
        assert_eq!(storage.persistent().get::<_, u32>(&DataKey::NextId), Some(next_id));
        assert_eq!(storage.persistent().get::<_, bool>(&DataKey::EmergencyStop), Some(emergency_stop));
        assert_eq!(storage.persistent().get::<_, Address>(&DataKey::Treasury), Some(treasury));
        assert_eq!(storage.persistent().get::<_, u32>(&DataKey::FeeBps), Some(fee_bps));
        assert_eq!(storage.persistent().get::<_, Address>(&DataKey::Operator), Some(operator));

        // Instance does NOT have them
        assert!(!storage.instance().has(&DataKey::SchemaVersion));
        assert!(!storage.instance().has(&DataKey::Token));
        assert!(!storage.instance().has(&DataKey::Admin));
        assert!(!storage.instance().has(&DataKey::MinTopup));
        assert!(!storage.instance().has(&DataKey::NextId));
        assert!(!storage.instance().has(&DataKey::EmergencyStop));
        assert!(!storage.instance().has(&DataKey::Treasury));
        assert!(!storage.instance().has(&DataKey::FeeBps));
        assert!(!storage.instance().has(&DataKey::Operator));
    });
}

#[test]
fn test_upgrade_via_migrate() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let client = crate::SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let min_topup = 1_000_000i128;

    env.as_contract(&contract_id, || {
        let storage = env.storage();
        storage.instance().set(&DataKey::SchemaVersion, &2u32);
        storage.instance().set(&DataKey::Token, &token);
        storage.instance().set(&DataKey::Admin, &admin);
        storage.instance().set(&DataKey::MinTopup, &min_topup);
    });

    env.mock_all_auths();
    client.migrate(&admin);

    env.as_contract(&contract_id, || {
        let storage = env.storage();
        assert_eq!(storage.persistent().get::<_, u32>(&DataKey::SchemaVersion), Some(3u32));
        assert_eq!(storage.persistent().get::<_, Address>(&DataKey::Token), Some(token));
        assert_eq!(storage.persistent().get::<_, Address>(&DataKey::Admin), Some(admin));
        assert_eq!(storage.persistent().get::<_, i128>(&DataKey::MinTopup), Some(min_topup));

        assert!(!storage.instance().has(&DataKey::SchemaVersion));
        assert!(!storage.instance().has(&DataKey::Token));
        assert!(!storage.instance().has(&DataKey::Admin));
        assert!(!storage.instance().has(&DataKey::MinTopup));
    });
}

#[test]
fn test_migration_idempotency_and_crash_recovery() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let client = crate::SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let min_topup = 1_000_000i128;

    // Simulate mid-migration crash state:
    // Token is in persistent storage (migrated).
    // Admin and MinTopup are still in instance storage.
    env.as_contract(&contract_id, || {
        let storage = env.storage();
        storage.instance().set(&DataKey::SchemaVersion, &2u32);
        storage.persistent().set(&DataKey::Token, &token);
        storage.instance().set(&DataKey::Admin, &admin);
        storage.instance().set(&DataKey::MinTopup, &min_topup);
    });

    env.mock_all_auths();
    
    // First run (resuming from crash state)
    client.migrate_config_to_persistent(&admin);

    env.as_contract(&contract_id, || {
        let storage = env.storage();
        assert_eq!(storage.persistent().get::<_, u32>(&DataKey::SchemaVersion), Some(3u32));
        assert_eq!(storage.persistent().get::<_, Address>(&DataKey::Token), Some(token.clone()));
        assert_eq!(storage.persistent().get::<_, Address>(&DataKey::Admin), Some(admin.clone()));
        assert_eq!(storage.persistent().get::<_, i128>(&DataKey::MinTopup), Some(min_topup));

        assert!(!storage.instance().has(&DataKey::SchemaVersion));
        assert!(!storage.instance().has(&DataKey::Token));
        assert!(!storage.instance().has(&DataKey::Admin));
        assert!(!storage.instance().has(&DataKey::MinTopup));
    });

    // Second run (idempotency check: should be a safe no-op)
    client.migrate_config_to_persistent(&admin);

    env.as_contract(&contract_id, || {
        let storage = env.storage();
        assert_eq!(storage.persistent().get::<_, u32>(&DataKey::SchemaVersion), Some(3u32));
        assert_eq!(storage.persistent().get::<_, Address>(&DataKey::Token), Some(token));
        assert_eq!(storage.persistent().get::<_, Address>(&DataKey::Admin), Some(admin));
        assert_eq!(storage.persistent().get::<_, i128>(&DataKey::MinTopup), Some(min_topup));
    });
}

#[test]
fn test_rejection_of_schema_downgrades() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let client = crate::SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    // Simulate version 4
    env.as_contract(&contract_id, || {
        env.storage().persistent().set(&DataKey::SchemaVersion, &4u32);
        env.storage().persistent().set(&DataKey::Admin, &admin);
    });

    env.mock_all_auths();
    
    // migrate_config_to_persistent should fail
    let err1 = client.try_migrate_config_to_persistent(&admin);
    assert_eq!(err1, Err(Ok(Error::SchemaMigrationDowngrade)));

    // migrate to version 3 should also fail
    let err2 = client.try_migrate(&admin);
    assert_eq!(err2, Err(Ok(Error::SchemaMigrationDowngrade)));
}
