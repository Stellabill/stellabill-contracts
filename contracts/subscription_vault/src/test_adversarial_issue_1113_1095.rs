#![cfg(test)]

use crate::test_utils::setup::TestEnv;
use crate::{Error, SubscriptionStatus};
use soroban_sdk::{testutils::Address as _, vec, Address};

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const DEPOSIT: i128 = 25_000_000;

fn make_funded_subscription(te: &TestEnv, subscriber: &Address, merchant: &Address) -> u32 {
    let id = te.client.create_subscription(
        subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    te.stellar_token_client().mint(subscriber, &DEPOSIT);
    te.client
        .deposit_funds(&id, subscriber, &DEPOSIT, &None::<soroban_sdk::BytesN<32>>);
    id
}

#[test]
fn disable_emergency_stop_is_idempotent_and_authorized() {
    let te = TestEnv::default();
    let stranger = Address::generate(&te.env);

    te.client.enable_emergency_stop(&te.admin);
    assert!(te.client.get_emergency_stop_status());

    let unauthorized = te.client.try_disable_emergency_stop(&stranger);
    assert_eq!(unauthorized, Err(Ok(Error::Unauthorized)));
    assert!(te.client.get_emergency_stop_status());

    te.client.disable_emergency_stop(&te.admin);
    assert!(!te.client.get_emergency_stop_status());

    // Repeating the disabled state is a safe no-op.
    assert_eq!(te.client.try_disable_emergency_stop(&te.admin), Ok(Ok(())));
    assert!(!te.client.get_emergency_stop_status());
}

#[test]
fn operator_batch_charge_preserves_state_on_invalid_nonce_and_unauthorized_call() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);
    let id = make_funded_subscription(&te, &subscriber, &merchant);
    let ids = vec![&te.env, id];

    te.client.set_operator(&te.admin, &operator);
    te.jump(INTERVAL + 1);
    let before = te.client.get_subscription(&id);

    let wrong_nonce = te.client.try_operator_batch_charge(&operator, &ids, &1);
    assert_eq!(wrong_nonce, Err(Ok(Error::NonceAlreadyUsed)));
    assert_eq!(te.client.get_operator_nonce(&operator), 0);
    assert_eq!(
        te.client.get_subscription(&id).prepaid_balance,
        before.prepaid_balance
    );

    let unauthorized = te.client.try_operator_batch_charge(&stranger, &ids, &0);
    assert!(unauthorized.is_err());
    assert_eq!(te.client.get_operator_nonce(&operator), 0);
    assert_eq!(
        te.client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );

    let results = te.client.operator_batch_charge(&operator, &ids, &0);
    assert!(results.get(0).unwrap().success);
    assert_eq!(te.client.get_operator_nonce(&operator), 1);
    assert_eq!(
        te.client.get_subscription(&id).prepaid_balance,
        DEPOSIT - AMOUNT
    );
}
