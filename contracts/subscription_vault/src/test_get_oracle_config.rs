//! Focused adversarial coverage for the public `get_oracle_config` query.

use crate::test_utils::setup::TestEnv;
use crate::{DataKey, Error, OracleConfig, OracleKind, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

#[test]
fn get_oracle_config_returns_disabled_spot_defaults_without_initialization() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let config = client.get_oracle_config();

    assert_eq!(
        config,
        OracleConfig {
            enabled: false,
            oracle: None,
            max_age_seconds: 0,
            kind: OracleKind::Spot,
            window_secs: 0,
            fixed_numerator: 0,
            fixed_denominator: 1,
        }
    );
}

#[test]
fn get_oracle_config_round_trips_the_full_configured_shape() {
    let test_env = TestEnv::default();
    let oracle = Address::generate(&test_env.env);

    test_env.client.set_oracle_config(
        &test_env.admin,
        &true,
        &Some(oracle.clone()),
        &120u64,
        &OracleKind::FixedRate,
        &0u64,
        &3u128,
        &2u128,
    );

    assert_eq!(
        test_env.client.get_oracle_config(),
        OracleConfig {
            enabled: true,
            oracle: Some(oracle),
            max_age_seconds: 120,
            kind: OracleKind::FixedRate,
            window_secs: 0,
            fixed_numerator: 3,
            fixed_denominator: 2,
        }
    );
}

#[test]
fn get_oracle_config_reads_legacy_instance_storage_only_before_schema_three() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let oracle = Address::generate(&env);
    let legacy_config = OracleConfig {
        enabled: true,
        oracle: Some(oracle),
        max_age_seconds: 60,
        kind: OracleKind::Twap,
        window_secs: 300,
        fixed_numerator: 0,
        fixed_denominator: 1,
    };

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::SchemaVersion, &2u32);
        env.storage()
            .instance()
            .set(&DataKey::Oracle, &legacy_config);
    });
    assert_eq!(client.get_oracle_config(), legacy_config);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::SchemaVersion, &3u32);
    });
    assert_eq!(
        client.get_oracle_config(),
        OracleConfig {
            enabled: false,
            oracle: None,
            max_age_seconds: 0,
            kind: OracleKind::Spot,
            window_secs: 0,
            fixed_numerator: 0,
            fixed_denominator: 1,
        }
    );
}

#[test]
fn rejected_oracle_config_mutation_preserves_the_current_value() {
    let test_env = TestEnv::default();
    let oracle = Address::generate(&test_env.env);
    let stranger = Address::generate(&test_env.env);

    test_env.client.set_oracle_config(
        &test_env.admin,
        &true,
        &Some(oracle),
        &120u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );
    let before = test_env.client.get_oracle_config();

    let rejected = test_env.client.try_set_oracle_config(
        &stranger,
        &true,
        &Some(Address::generate(&test_env.env)),
        &120u64,
        &OracleKind::FixedRate,
        &0u64,
        &1u128,
        &0u128,
    );

    assert_eq!(rejected, Err(Ok(Error::Unauthorized)));
    assert_eq!(test_env.client.get_oracle_config(), before);
}
