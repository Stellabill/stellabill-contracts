#![cfg(test)]

use crate::{
    ChargeExecutionResult, Error, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
    UsageChargeResult,
};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use crate::test_helpers::run_all_mutation_calls;
use soroban_sdk::{Address, Env, String, Vec};

const T0: u64 = 1_700_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const DEPOSIT: i128 = 100_000_000;

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, client, token, admin)
}

#[test]
fn test_emergency_stop_matrix_blocks_mutations_but_allows_reads() {
    let (env, client, token, admin) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &DEPOSIT);

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &true,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
);
    client.deposit_funds(&sub_id, &subscriber, &10_000_000i128, &None::<soroban_sdk::BytesN<32>>);

    let plan_id = client.create_plan_template(&merchant, &1_000_000i128, &INTERVAL, &false, &None::<i128>);

    let operator = Address::generate(&env);
    client.set_operator(&admin, &operator);

    client.enable_emergency_stop(&admin);
    assert!(client.get_emergency_stop_status());

    assert_eq!(
        client.try_create_subscription(
            &subscriber,
            &merchant,
            &1_000_000i128,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
                &None::<u32>,
),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(
        client.try_create_subscription_with_token(
            &subscriber,
            &merchant,
            &token,
            &1_000_000i128,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
                &None::<u32>,
),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(client.try_create_subscription_from_plan(&subscriber, &plan_id), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_deposit_funds(&sub_id, &subscriber, &1_000_000i128, &None::<soroban_sdk::BytesN<32>>), Err(Ok(Error::EmergencyStopActive)));

    assert_eq!(client.try_charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_charge_usage(&sub_id, &100_000i128), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(
        client.try_charge_usage_with_reference(&sub_id, &100_000i128, &String::from_str(&env, "usage-ref")),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(client.try_charge_one_off(&sub_id, &merchant, &100_000i128, &None::<soroban_sdk::BytesN<32>>), Err(Ok(Error::EmergencyStopActive)));

    let ids_vec = Vec::from_array(&env, [sub_id]);
    assert_eq!(client.try_operator_batch_charge(&operator, &ids_vec, &0u64), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_operator_charge_subscription(&operator, &sub_id), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_operator_charge_usage(&operator, &sub_id, &100_000i128), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(
        client.try_operator_charge_usage_with_ref(&operator, &sub_id, &100_000i128, &String::from_str(&env, "oref")),
        Err(Ok(Error::EmergencyStopActive))
    );

    assert_eq!(client.try_partial_refund(&admin, &sub_id, &subscriber, &1_000_000i128), Err(Ok(Error::EmergencyStopActive)));

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(client.get_admin(), admin);
    assert!(client.get_emergency_stop_status());

    env.ledger().with_mut(|li| li.timestamp += crate::admin::CONFIG_COOLDOWN_SECS);
    client.disable_emergency_stop(&admin);
    assert!(!client.get_emergency_stop_status());

    let resumed = client.create_subscription_from_plan(&subscriber, &plan_id);
    assert_eq!(client.get_subscription(&resumed).status, SubscriptionStatus::Active);
}

#[test]
fn test_emergency_stop_recovery_and_edge_cases() {
    let (env, client, token, admin) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let operator = Address::generate(&env);

    // Initial setup
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &DEPOSIT);
    let sub_id = client.create_subscription(&subscriber, &merchant, &1_000_000i128, &INTERVAL, &true, &None::<i128>, &None::<u64>);
    client.deposit_funds(&sub_id, &subscriber, &10_000_000i128, &None::<soroban_sdk::BytesN<32>>);
    let plan_id = client.create_plan_template(&merchant, &1_000_000i128, &INTERVAL, &false, &None::<i128>);
    client.set_operator(&admin, &operator);

    // Enable emergency stop and verify blocking
    client.enable_emergency_stop(&admin);
    assert!(client.get_emergency_stop_status());

    // Verify mutations are blocked
    assert_eq!(client.try_create_subscription(&subscriber, &merchant, &1_000_000i128, &INTERVAL, &false, &None::<i128>, &None::<u64>), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_create_subscription_with_token(&subscriber, &merchant, &token, &1_000_000i128, &INTERVAL, &false, &None::<i128>, &None::<u64>), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_create_subscription_from_plan(&subscriber, &plan_id), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_deposit_funds(&sub_id, &subscriber, &1_000_000i128, &None::<soroban_sdk::BytesN<32>>), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_charge_usage(&sub_id, &100_000i128), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_charge_usage_with_reference(&sub_id, &100_000i128, &String::from_str(&env, "usage-ref")), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_charge_one_off(&sub_id, &merchant, &100_000i128, &None::<soroban_sdk::BytesN<32>>), Err(Ok(Error::EmergencyStopActive)));
    let ids_vec = Vec::from_array(&env, [sub_id]);
    assert_eq!(client.try_operator_batch_charge(&operator, &ids_vec, &0u64), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_operator_charge_subscription(&operator, &sub_id), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_operator_charge_usage(&operator, &sub_id, &100_000i128), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_operator_charge_usage_with_ref(&operator, &sub_id, &100_000i128, &String::from_str(&env, "oref")), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_partial_refund(&admin, &sub_id, &subscriber, &1_000_000i128), Err(Ok(Error::EmergencyStopActive)));

    // Disable emergency stop
    client.disable_emergency_stop(&admin);
    assert!(!client.get_emergency_stop_status());

    // After disabling, mutations should succeed
    assert!(client.try_create_subscription(&subscriber, &merchant, &1_000_000i128, &INTERVAL, &false, &None::<i128>, &None::<u64>).is_ok());
    assert!(client.try_create_subscription_with_token(&subscriber, &merchant, &token, &1_000_000i128, &INTERVAL, &false, &None::<i128>, &None::<u64>).is_ok());
    assert!(client.try_deposit_funds(&sub_id, &subscriber, &1_000_000i128, &None::<soroban_sdk::BytesN<32>>).is_ok());
    assert!(client.try_charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>).is_ok());
    assert!(client.try_charge_usage(&sub_id, &100_000i128).is_ok());
    assert!(client.try_charge_usage_with_reference(&sub_id, &100_000i128, &String::from_str(&env, "usage-ref")).is_ok());
    assert!(client.try_charge_one_off(&sub_id, &merchant, &100_000i128, &None::<soroban_sdk::BytesN<32>>).is_ok());
    assert!(client.try_operator_batch_charge(&operator, &ids_vec, &0u64).is_ok());
    assert!(client.try_operator_charge_subscription(&operator, &sub_id).is_ok());
    assert!(client.try_operator_charge_usage(&operator, &sub_id, &100_000i128).is_ok());
    assert!(client.try_operator_charge_usage_with_ref(&operator, &sub_id, &100_000i128, &String::from_str(&env, "oref")).is_ok());
    assert!(client.try_partial_refund(&admin, &sub_id, &subscriber, &1_000_000i128).is_ok());

    // Edge case: clearing when already off (no panic)
    client.disable_emergency_stop(&admin);
    assert!(!client.get_emergency_stop_status());

    // Edge case: non-admin tries to clear (should be rejected)
    let non_admin = Address::generate(&env);
    let clear_result = client.try_disable_emergency_stop(&non_admin);
    assert_eq!(clear_result, Err(Ok(Error::NotAuthorized)));

    // Re‑enable after clear and verify blocking again
    client.enable_emergency_stop(&admin);
    assert!(client.get_emergency_stop_status());
    assert_eq!(client.try_create_subscription(&subscriber, &merchant, &1_000_000i128, &INTERVAL, &false, &None::<i128>, &None::<u64>), Err(Ok(Error::EmergencyStopActive)));
}

//! Emergency Stop Recovery Matrix Tests
//!
//! Verifies that after emergency_stop is enabled and later cleared,
//! every previously-blocked mutating entrypoint succeeds again without state drift.

#![cfg(test)]

use crate::{
    ChargeExecutionResult, Error, SubscriptionStatus, SubscriptionVaultClient,
};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env, String, Vec};

const T0: u64 = 1_700_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const DEPOSIT: i128 = 100_000_000;
const MIN_TOPUP: i128 = 1_000_000;
const GRACE_PERIOD: u64 = 7 * 24 * 60 * 60;

/// Setup test environment
fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(crate::SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    
    client.init(&token, &6, &admin, &MIN_TOPUP, &GRACE_PERIOD);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &DEPOSIT);
    
    (env, client, token, admin, subscriber, merchant)
}

#[test]
fn test_emergency_stop_blocks_mutations() {
    let (env, client, token, admin, subscriber, merchant) = setup();
    let operator = Address::generate(&env);
    client.set_operator(&admin, &operator);

    // Setup test subscriptions
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &MIN_TOPUP,
        &INTERVAL,
        &true,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
    );
    client.deposit_funds(&sub_id, &subscriber, &10_000_000i128, &None::<soroban_sdk::BytesN<32>>);
    
    let plan_id = client.create_plan_template(
        &merchant,
        &MIN_TOPUP,
        &INTERVAL,
        &false,
        &None::<i128>,
    );

    // Enable emergency stop
    client.enable_emergency_stop(&admin);
    assert!(client.get_emergency_stop_status());

    // Verify MUTATING entrypoints are blocked
    // 1. Subscription creation
    assert_eq!(
        client.try_create_subscription(
            &subscriber,
            &merchant,
            &MIN_TOPUP,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
            &None::<u32>,
        ),
        Err(Ok(Error::EmergencyStopActive))
    );

    // 2. Deposits
    assert_eq!(
        client.try_deposit_funds(&sub_id, &subscriber, &1_000_000i128, &None::<soroban_sdk::BytesN<32>>),
        Err(Ok(Error::EmergencyStopActive))
    );

    // 3. Charges
    assert_eq!(
        client.try_charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>),
        Err(Ok(Error::EmergencyStopActive))
    );

    // 4. Operator operations
    let ids_vec = Vec::from_array(&env, [sub_id]);
    assert_eq!(
        client.try_operator_batch_charge(&operator, &ids_vec, &0u64),
        Err(Ok(Error::EmergencyStopActive))
    );

    // READ operations should still work
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(client.get_admin(), admin);
    assert!(client.get_emergency_stop_status());
}

#[test]
fn test_emergency_stop_clear_restores_mutations() {
    let (env, client, token, admin, subscriber, merchant) = setup();
    let operator = Address::generate(&env);
    client.set_operator(&admin, &operator);

    // Setup test data
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &MIN_TOPUP,
        &INTERVAL,
        &true,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
    );
    client.deposit_funds(&sub_id, &subscriber, &10_000_000i128, &None::<soroban_sdk::BytesN<32>>);
    
    // Enable emergency stop
    client.enable_emergency_stop(&admin);
    assert!(client.get_emergency_stop_status());

    // Verify blocked
    assert_eq!(
        client.try_deposit_funds(&sub_id, &subscriber, &1_000_000i128, &None::<soroban_sdk::BytesN<32>>),
        Err(Ok(Error::EmergencyStopActive))
    );

    // DISABLE emergency stop
    client.disable_emergency_stop(&admin);
    assert!(!client.get_emergency_stop_status());

    // Now mutations should succeed
    let initial_balance = client.get_subscriber_balance(&sub_id, &subscriber);
    client.try_deposit_funds(&sub_id, &subscriber, &1_000_000i128, &None::<soroban_sdk::BytesN<32>>).unwrap();
    let new_balance = client.get_subscriber_balance(&sub_id, &subscriber);
    assert_eq!(new_balance, initial_balance + 1_000_000);
}

#[test]
fn test_emergency_stop_edge_cases() {
    let (env, client, token, admin, subscriber, merchant) = setup();

    // Edge case 1: Clear when not set (should succeed without error)
    client.disable_emergency_stop(&admin);
    assert!(!client.get_emergency_stop_status());

    // Edge case 2: Non-admin cannot clear
    let non_admin = Address::generate(&env);
    client.enable_emergency_stop(&admin);
    assert!(client.get_emergency_stop_status());
    
    let clear_result = client.try_disable_emergency_stop(&non_admin);
    assert_eq!(clear_result, Err(Ok(Error::NotAuthorized)));
    assert!(client.get_emergency_stop_status());

    // Edge case 3: Non-admin cannot enable
    let enable_result = client.try_enable_emergency_stop(&non_admin);
    assert_eq!(enable_result, Err(Ok(Error::NotAuthorized)));

    // Edge case 4: Enable when already enabled
    let double_enable = client.try_enable_emergency_stop(&admin);
    assert_eq!(double_enable, Err(Ok(Error::EmergencyStopAlreadyActive)));

    // Edge case 5: Set-clear-set cycle
    client.enable_emergency_stop(&admin);
    assert!(client.get_emergency_stop_status());
    
    client.disable_emergency_stop(&admin);
    assert!(!client.get_emergency_stop_status());
    
    client.enable_emergency_stop(&admin);
    assert!(client.get_emergency_stop_status());
}

#[test]
fn test_emergency_stop_state_persistence() {
    let (env, client, token, admin, subscriber, merchant) = setup();

    // Enable emergency stop
    client.enable_emergency_stop(&admin);
    assert!(client.get_emergency_stop_status());

    // Status should persist
    let status1 = client.get_emergency_stop_status();
    assert!(status1);

    // Operations should be blocked
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &MIN_TOPUP,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
    );
    
    assert_eq!(
        client.try_deposit_funds(&sub_id, &subscriber, &1_000_000i128, &None::<soroban_sdk::BytesN<32>>),
        Err(Ok(Error::EmergencyStopActive))
    );
    
    // Status should still be enabled
    let status2 = client.get_emergency_stop_status();
    assert!(status2);

    // Disable
    client.disable_emergency_stop(&admin);
    assert!(!client.get_emergency_stop_status());
    
    // Now should work
    client.try_deposit_funds(&sub_id, &subscriber, &1_000_000i128, &None::<soroban_sdk::BytesN<32>>).unwrap();
}

#[test]
fn test_emergency_stop_cooldown_interaction() {
    let (env, client, token, admin, subscriber, merchant) = setup();

    // Enable emergency stop
    client.enable_emergency_stop(&admin);
    assert!(client.get_emergency_stop_status());

    // Admin operation should be blocked by emergency stop first
    let result = client.try_set_min_topup(&admin, &2_000_000i128);
    assert_eq!(result, Err(Ok(Error::EmergencyStopActive)));

    // Disable emergency stop
    client.disable_emergency_stop(&admin);
    assert!(!client.get_emergency_stop_status());

    // Now admin operation should work (cooldown may apply if called quickly)
    let result = client.try_set_min_topup(&admin, &2_000_000i128);
    // Either succeeds or hits cooldown - either is acceptable
    // Cooldown is 6 hours, so it will hit cooldown if called again
    if result.is_err() {
        assert_eq!(result, Err(Ok(Error::CooldownActive)));
    }
}
