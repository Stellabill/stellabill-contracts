#![cfg(test)]

extern crate std;

use crate::test_utils::setup::TestEnv;
use crate::{Error, OperatorRemovedEvent, OperatorSetEvent, SubscriptionStatus};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    vec, Address, IntoVal,
};

// ── Shared constants ──────────────────────────────────────────────────────────

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const DEPOSIT: i128 = 25_000_000; // enough for two intervals

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Create a subscription with prepaid balance and return its ID.
fn make_funded_subscription(te: &TestEnv, subscriber: &Address, merchant: &Address) -> u32 {
    let sub_id = te.client.create_subscription(
        subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );
    te.stellar_token_client().mint(subscriber, &DEPOSIT);
    te.client.deposit_funds(&sub_id, subscriber, &DEPOSIT);
    sub_id
}

// ── set_operator ─────────────────────────────────────────────────────────────

#[test]
fn set_operator_by_admin_stores_address_and_emits_event() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);

    te.env.ledger().with_mut(|li| li.timestamp = 1_000);
    te.client.set_operator(&te.admin, &operator);

    assert_eq!(te.client.get_operator(), Some(operator.clone()));

    let events = te.env.events().all();
    let last = events.last().expect("no events");
    let payload: OperatorSetEvent = last.2.into_val(&te.env);
    assert_eq!(payload.admin, te.admin);
    assert_eq!(payload.operator, operator);
    assert_eq!(payload.timestamp, 1_000);
}

#[test]
fn set_operator_replaces_previous_operator() {
    let te = TestEnv::default();
    let op1 = Address::generate(&te.env);
    let op2 = Address::generate(&te.env);

    te.client.set_operator(&te.admin, &op1);
    te.client.set_operator(&te.admin, &op2);

    assert_eq!(te.client.get_operator(), Some(op2));
}

#[test]
fn set_operator_non_admin_rejected() {
    let te = TestEnv::default();
    let stranger = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let result = te.client.try_set_operator(&stranger, &operator);
    assert!(result.is_err(), "non-admin should not be able to set operator");
}

#[test]
fn set_operator_stale_admin_after_rotation_rejected() {
    let te = TestEnv::default();
    let new_admin = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    te.client.rotate_admin(&te.admin, &new_admin, &0u64);

    let result = te.client.try_set_operator(&te.admin, &operator);
    assert!(result.is_err(), "old admin should not be able to set operator after rotation");
}

#[test]
fn set_operator_contract_address_rejected() {
    let te = TestEnv::default();
    let contract_addr = te.client.address.clone();

    let result = te.client.try_set_operator(&te.admin, &contract_addr);
    assert!(
        result == Err(Ok(Error::InvalidInput)),
        "setting contract as operator should return InvalidInput"
    );
}

// ── remove_operator ───────────────────────────────────────────────────────────

#[test]
fn remove_operator_clears_address_and_emits_event() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);

    te.env.ledger().with_mut(|li| li.timestamp = 2_000);
    te.client.set_operator(&te.admin, &operator);
    te.client.remove_operator(&te.admin);

    assert_eq!(te.client.get_operator(), None);

    let events = te.env.events().all();
    let last = events.last().expect("no events");
    let payload: OperatorRemovedEvent = last.2.into_val(&te.env);
    assert_eq!(payload.admin, te.admin);
    assert_eq!(payload.timestamp, 2_000);
}

#[test]
fn remove_operator_when_none_set_is_noop() {
    let te = TestEnv::default();
    // No operator set; this should succeed without panicking.
    te.client.remove_operator(&te.admin);
    assert_eq!(te.client.get_operator(), None);
}

#[test]
fn remove_operator_non_admin_rejected() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);

    te.client.set_operator(&te.admin, &operator);

    let result = te.client.try_remove_operator(&stranger);
    assert!(result.is_err(), "non-admin should not be able to remove operator");

    // Operator should still be set.
    assert_eq!(te.client.get_operator(), Some(operator));
}

// ── operator_batch_charge ─────────────────────────────────────────────────────

#[test]
fn operator_batch_charge_succeeds_and_decrements_balance() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);

    te.jump(INTERVAL + 1);

    let results = te
        .client
        .operator_batch_charge(&operator, &vec![&te.env, sub_id], &0u64);

    assert_eq!(results.len(), 1);
    assert!(results.get(0).unwrap().success);

    let sub = te.client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, DEPOSIT - AMOUNT);
}

#[test]
fn operator_batch_charge_advances_nonce() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);

    te.jump(INTERVAL + 1);
    te.client.operator_batch_charge(&operator, &vec![&te.env, sub_id], &0u64);

    assert_eq!(te.client.get_operator_nonce(&operator), 1u64);
}

#[test]
fn operator_batch_charge_wrong_nonce_rejected() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);

    te.jump(INTERVAL + 1);

    // Nonce 0 is expected but we pass 1.
    let result = te
        .client
        .try_operator_batch_charge(&operator, &vec![&te.env, sub_id], &1u64);
    assert!(
        result == Err(Ok(Error::NonceAlreadyUsed)),
        "wrong nonce should return NonceAlreadyUsed"
    );
}

#[test]
fn operator_batch_charge_replay_rejected() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);

    te.jump(INTERVAL + 1);
    te.client.operator_batch_charge(&operator, &vec![&te.env, sub_id], &0u64);

    // Replay the exact same call — nonce 0 is already consumed.
    te.jump(INTERVAL + 1);
    let result = te
        .client
        .try_operator_batch_charge(&operator, &vec![&te.env, sub_id], &0u64);
    assert!(result == Err(Ok(Error::NonceAlreadyUsed)));
}

#[test]
fn operator_batch_charge_without_operator_set_rejected() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    // No operator set.

    te.jump(INTERVAL + 1);
    let result = te
        .client
        .try_operator_batch_charge(&stranger, &vec![&te.env, sub_id], &0u64);
    assert!(result.is_err(), "batch charge without operator set should fail");
}

#[test]
fn operator_batch_charge_wrong_operator_rejected() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);
    let wrong_operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);

    te.jump(INTERVAL + 1);
    let result = te
        .client
        .try_operator_batch_charge(&wrong_operator, &vec![&te.env, sub_id], &0u64);
    assert!(result.is_err(), "wrong operator address should fail");
}

#[test]
fn operator_nonce_independent_from_admin_nonce() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);

    // Admin uses domain 0 (DOMAIN_BATCH_CHARGE). Advance it.
    te.jump(INTERVAL + 1);
    te.client.batch_charge(&vec![&te.env, sub_id], &0u64);

    // Operator's domain 2 (DOMAIN_OPERATOR_BATCH_CHARGE) should still be 0.
    assert_eq!(te.client.get_operator_nonce(&operator), 0u64);

    // Make another subscription so the operator has something to charge.
    let sub2 = make_funded_subscription(&te, &subscriber, &merchant);
    te.jump(INTERVAL + 1);
    let results = te
        .client
        .operator_batch_charge(&operator, &vec![&te.env, sub2], &0u64);
    assert!(results.get(0).unwrap().success);
}

// ── operator_charge_subscription ─────────────────────────────────────────────

#[test]
fn operator_charge_subscription_succeeds() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);

    te.jump(INTERVAL + 1);

    let result = te.client.operator_charge_subscription(&operator, &sub_id);
    // ChargeExecutionResult::Charged = 0
    assert_eq!(result as u32, 0u32);

    let sub = te.client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, DEPOSIT - AMOUNT);
}

#[test]
fn operator_charge_subscription_wrong_operator_rejected() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);

    te.jump(INTERVAL + 1);
    let result = te.client.try_operator_charge_subscription(&stranger, &sub_id);
    assert!(result.is_err());
}

// ── operator_charge_usage ─────────────────────────────────────────────────────

#[test]
fn operator_charge_usage_succeeds() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    // Create a usage-enabled subscription.
    let sub_id = te.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &true, // usage_enabled
        &None,
        &None::<u64>,
    );
    te.stellar_token_client().mint(&subscriber, &DEPOSIT);
    te.client.deposit_funds(&sub_id, &subscriber, &DEPOSIT);

    te.client.set_operator(&te.admin, &operator);

    let usage = 1_000_000i128;
    te.client.operator_charge_usage(&operator, &sub_id, &usage);

    let sub = te.client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, DEPOSIT - usage);
}

#[test]
fn operator_charge_usage_wrong_operator_rejected() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);

    let sub_id = te.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &true,
        &None,
        &None::<u64>,
    );
    te.stellar_token_client().mint(&subscriber, &DEPOSIT);
    te.client.deposit_funds(&sub_id, &subscriber, &DEPOSIT);

    te.client.set_operator(&te.admin, &operator);

    let result = te.client.try_operator_charge_usage(&stranger, &sub_id, &1_000_000i128);
    assert!(result.is_err());
}

// ── Revocation (remove_operator revokes access immediately) ───────────────────

#[test]
fn remove_operator_revokes_batch_charge_immediately() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);
    te.client.remove_operator(&te.admin);

    te.jump(INTERVAL + 1);
    let result = te
        .client
        .try_operator_batch_charge(&operator, &vec![&te.env, sub_id], &0u64);
    assert!(
        result.is_err(),
        "operator should not be able to charge after being removed"
    );
}

#[test]
fn remove_operator_revokes_single_charge_immediately() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);
    te.client.remove_operator(&te.admin);

    te.jump(INTERVAL + 1);
    let result = te.client.try_operator_charge_subscription(&operator, &sub_id);
    assert!(result.is_err());
}

// ── Privilege isolation ───────────────────────────────────────────────────────

#[test]
fn operator_cannot_set_min_topup() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);

    let result = te.client.try_set_min_topup(&operator, &5_000_000);
    assert!(result.is_err(), "operator must not be able to set_min_topup");
}

#[test]
fn operator_cannot_rotate_admin() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    let new_admin = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);

    let result = te.client.try_rotate_admin(&operator, &new_admin, &0u64);
    assert!(result.is_err(), "operator must not be able to rotate_admin");
}

#[test]
fn operator_cannot_enable_emergency_stop() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);

    let result = te.client.try_enable_emergency_stop(&operator);
    assert!(result.is_err(), "operator must not be able to enable_emergency_stop");
}

#[test]
fn operator_cannot_set_another_operator() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    let new_op = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);

    let result = te.client.try_set_operator(&operator, &new_op);
    assert!(result.is_err(), "operator must not be able to escalate to set_operator");
}

#[test]
fn operator_cannot_remove_operator() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);

    let result = te.client.try_remove_operator(&operator);
    assert!(result.is_err(), "operator must not be able to remove_operator");

    // Operator should still be set.
    assert_eq!(te.client.get_operator(), Some(operator));
}

#[test]
fn operator_cannot_add_accepted_token() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    let dummy_token = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);

    let result = te.client.try_add_accepted_token(&operator, &dummy_token, &6u32);
    assert!(result.is_err(), "operator must not be able to add_accepted_token");
}

#[test]
fn operator_cannot_export_contract_snapshot() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);

    let result = te.client.try_export_contract_snapshot(&operator);
    assert!(result.is_err(), "operator must not be able to export_contract_snapshot");
}

// ── Emergency stop blocks operator charges ────────────────────────────────────

#[test]
fn operator_batch_charge_blocked_by_emergency_stop() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);
    te.client.enable_emergency_stop(&te.admin);

    te.jump(INTERVAL + 1);
    let result = te
        .client
        .try_operator_batch_charge(&operator, &vec![&te.env, sub_id], &0u64);
    assert!(
        result == Err(Ok(Error::EmergencyStopActive)),
        "emergency stop should block operator_batch_charge"
    );
}

#[test]
fn operator_charge_subscription_blocked_by_emergency_stop() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);
    te.client.enable_emergency_stop(&te.admin);

    te.jump(INTERVAL + 1);
    let result = te.client.try_operator_charge_subscription(&operator, &sub_id);
    assert!(result == Err(Ok(Error::EmergencyStopActive)));
}

// ── Admin rotation interaction ────────────────────────────────────────────────

#[test]
fn admin_rotation_preserves_operator() {
    let te = TestEnv::default();
    let new_admin = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    te.client.set_operator(&te.admin, &operator);
    te.client.rotate_admin(&te.admin, &new_admin, &0u64);

    // Operator should survive admin rotation unchanged.
    assert_eq!(
        te.client.get_operator(),
        Some(operator.clone()),
        "operator must persist across admin rotation"
    );
}

#[test]
fn new_admin_can_replace_operator_after_rotation() {
    let te = TestEnv::default();
    let new_admin = Address::generate(&te.env);
    let operator = Address::generate(&te.env);
    let new_op = Address::generate(&te.env);

    te.client.set_operator(&te.admin, &operator);
    te.client.rotate_admin(&te.admin, &new_admin, &0u64);
    te.client.set_operator(&new_admin, &new_op);

    assert_eq!(te.client.get_operator(), Some(new_op));
}

#[test]
fn old_admin_cannot_change_operator_after_rotation() {
    let te = TestEnv::default();
    let new_admin = Address::generate(&te.env);
    let operator = Address::generate(&te.env);
    let another_op = Address::generate(&te.env);

    te.client.set_operator(&te.admin, &operator);
    te.client.rotate_admin(&te.admin, &new_admin, &0u64);

    let result = te.client.try_set_operator(&te.admin, &another_op);
    assert!(result.is_err(), "old admin should not be able to change operator after rotation");
}

#[test]
fn old_admin_cannot_remove_operator_after_rotation() {
    let te = TestEnv::default();
    let new_admin = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    te.client.set_operator(&te.admin, &operator);
    te.client.rotate_admin(&te.admin, &new_admin, &0u64);

    let result = te.client.try_remove_operator(&te.admin);
    assert!(result.is_err(), "old admin should not be able to remove operator after rotation");

    // Operator must still be set.
    assert_eq!(te.client.get_operator(), Some(operator));
}

#[test]
fn operator_can_charge_after_admin_rotation() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let new_admin = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);
    te.client.rotate_admin(&te.admin, &new_admin, &0u64);

    te.jump(INTERVAL + 1);
    let results = te
        .client
        .operator_batch_charge(&operator, &vec![&te.env, sub_id], &0u64);
    assert!(
        results.get(0).unwrap().success,
        "operator should still be able to charge after admin rotation"
    );
}

// ── get_operator_nonce ────────────────────────────────────────────────────────

#[test]
fn get_operator_nonce_starts_at_zero() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    assert_eq!(te.client.get_operator_nonce(&operator), 0u64);
}

#[test]
fn get_operator_nonce_increments_per_call() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);

    for expected_nonce in 0u64..3u64 {
        assert_eq!(te.client.get_operator_nonce(&operator), expected_nonce);
        te.jump(INTERVAL + 1);
        te.client.operator_batch_charge(&operator, &vec![&te.env, sub_id], &expected_nonce);

        // Re-fund so the next charge can succeed.
        te.stellar_token_client().mint(&subscriber, &AMOUNT);
        te.client.deposit_funds(&sub_id, &subscriber, &AMOUNT);
    }
    assert_eq!(te.client.get_operator_nonce(&operator), 3u64);
}

// ── operator_charge_usage_with_reference ─────────────────────────────────────

#[test]
fn operator_charge_usage_with_reference_succeeds() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = te.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &true,
        &None,
        &None::<u64>,
    );
    te.stellar_token_client().mint(&subscriber, &DEPOSIT);
    te.client.deposit_funds(&sub_id, &subscriber, &DEPOSIT);

    te.client.set_operator(&te.admin, &operator);

    let ref_str = soroban_sdk::String::from_str(&te.env, "invoice-001");
    let usage = 500_000i128;
    te.client
        .operator_charge_usage_with_reference(&operator, &sub_id, &usage, &ref_str);

    let sub = te.client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, DEPOSIT - usage);
}

// ── Subscription state integrity ──────────────────────────────────────────────

#[test]
fn operator_batch_charge_paused_subscription_returns_not_active_error() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);

    te.client.pause_subscription(&sub_id, &subscriber);

    te.jump(INTERVAL + 1);
    let results = te
        .client
        .operator_batch_charge(&operator, &vec![&te.env, sub_id], &0u64);

    assert!(!results.get(0).unwrap().success);
    assert_eq!(results.get(0).unwrap().error_code, Error::NotActive as u32);
}

#[test]
fn operator_charge_does_not_mutate_subscription_metadata() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let sub_id = make_funded_subscription(&te, &subscriber, &merchant);
    te.client.set_operator(&te.admin, &operator);

    let sub_before = te.client.get_subscription(&sub_id);

    te.jump(INTERVAL + 1);
    te.client
        .operator_batch_charge(&operator, &vec![&te.env, sub_id], &0u64);

    let sub_after = te.client.get_subscription(&sub_id);

    assert_eq!(sub_after.subscriber, sub_before.subscriber);
    assert_eq!(sub_after.merchant, sub_before.merchant);
    assert_eq!(sub_after.amount, sub_before.amount);
    assert_eq!(sub_after.interval_seconds, sub_before.interval_seconds);
    assert_eq!(sub_after.status, SubscriptionStatus::Active);
}
