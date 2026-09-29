#![cfg(test)]

use crate::nonce::DOMAIN_BATCH_CHARGE;
use crate::test_utils::setup::TestEnv;
use crate::{Error, SubscriptionStatus};
use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, vec, Address, Vec};

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const DEPOSIT: i128 = 25_000_000;

fn create_subscription(te: &TestEnv, funded: bool) -> u32 {
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let id = te.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    if funded {
        te.stellar_token_client().mint(&subscriber, &DEPOSIT);
        te.client
            .deposit_funds(&id, &subscriber, &DEPOSIT, &None);
    }
    id
}

fn due(te: &TestEnv) {
    te.env.ledger().with_mut(|ledger| ledger.timestamp += INTERVAL + 1);
}

fn nonce(te: &TestEnv) -> u64 {
    te.client.get_admin_nonce(&te.admin, &DOMAIN_BATCH_CHARGE)
}

#[test]
fn batch_charge_charges_funded_and_reports_per_subscription_failures() {
    let te = TestEnv::default();
    let funded = create_subscription(&te, true);
    let unfunded = create_subscription(&te, false);
    due(&te);

    let results = te
        .client
        .batch_charge(&vec![&te.env, funded, unfunded, u32::MAX], &0u64);

    assert_eq!(results.len(), 3);
    assert!(results.get(0).unwrap().success);
    assert_eq!(
        results.get(1).unwrap().error_code,
        Error::InsufficientBalance.to_code()
    );
    assert_eq!(
        results.get(2).unwrap().error_code,
        Error::NotFound.to_code()
    );
    assert_eq!(
        te.client.get_subscription(&funded).prepaid_balance,
        DEPOSIT - AMOUNT
    );
    assert_eq!(
        te.client.get_subscription(&unfunded).status,
        SubscriptionStatus::InsufficientBalance
    );
    assert_eq!(nonce(&te), 1);
}

#[test]
fn empty_batch_is_accepted_and_consumes_nonce() {
    let te = TestEnv::default();
    let empty: Vec<u32> = Vec::new(&te.env);

    let result = te.client.batch_charge(&empty, &0u64);

    assert!(result.is_empty());
    assert_eq!(nonce(&te), 1);
}

#[test]
fn batch_charge_rejects_wrong_and_replayed_nonces_without_charging() {
    let te = TestEnv::default();
    let id = create_subscription(&te, true);
    due(&te);
    let before = te.client.get_subscription(&id);

    assert_eq!(
        te.client.try_batch_charge(&vec![&te.env, id], &1u64),
        Err(Ok(Error::NonceAlreadyUsed))
    );
    assert_eq!(nonce(&te), 0);
    assert_eq!(te.client.get_subscription(&id), before);

    te.client.batch_charge(&vec![&te.env, id], &0u64);
    let after_success = te.client.get_subscription(&id);
    assert_eq!(
        te.client.try_batch_charge(&vec![&te.env, id], &0u64),
        Err(Ok(Error::NonceAlreadyUsed))
    );
    assert_eq!(nonce(&te), 1);
    assert_eq!(te.client.get_subscription(&id), after_success);
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn missing_admin_authorization_does_not_consume_nonce() {
    let env = soroban_sdk::Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let client = crate::SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    client.batch_charge(&Vec::new(&env), &0u64);
}

#[test]
fn batch_charge_is_blocked_by_emergency_stop_without_state_changes() {
    let te = TestEnv::default();
    let id = create_subscription(&te, true);
    due(&te);
    te.client.enable_emergency_stop(&te.admin);
    let before = te.client.get_subscription(&id);

    assert_eq!(
        te.client
            .try_batch_charge(&vec![&te.env, id], &0u64),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(nonce(&te), 0);
    assert_eq!(te.client.get_subscription(&id), before);
}

#[test]
fn max_u64_nonce_is_rejected_without_mutating_charge_state() {
    let te = TestEnv::default();
    let id = create_subscription(&te, true);
    due(&te);
    let before = te.client.get_subscription(&id);

    assert_eq!(
        te.client
            .try_batch_charge(&vec![&te.env, id], &u64::MAX),
        Err(Ok(Error::NonceAlreadyUsed))
    );
    assert_eq!(nonce(&te), 0);
    assert_eq!(te.client.get_subscription(&id), before);

    te.env.as_contract(&te.client.address, || {
        te.env.storage().persistent().set(
            &crate::types::DataKey::AdminNonce(te.admin.clone(), DOMAIN_BATCH_CHARGE),
            &u64::MAX,
        );
    });
    assert_eq!(
        te.client
            .try_batch_charge(&vec![&te.env, id], &u64::MAX),
        Err(Ok(Error::Overflow))
    );
    assert_eq!(nonce(&te), u64::MAX);
    assert_eq!(te.client.get_subscription(&id), before);
}
