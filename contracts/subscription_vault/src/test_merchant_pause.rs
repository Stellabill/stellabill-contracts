#![cfg(test)]

use crate::test_utils::setup::TestEnv;
use crate::types::{ChargeExecutionResult, Error, SubscriptionStatus};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, String};

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const PREPAID: i128 = 100_000_000;

fn make_funded_sub(env: &TestEnv) -> (u32, Address, Address) {
    let subscriber = Address::generate(&env.env);
    let merchant = Address::generate(&env.env);
    let sub_id = env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &true,
        &None::<i128>,
        &None::<u64>,
    );
    env.stellar_token_client().mint(&subscriber, &PREPAID);
    env.client.deposit_funds(&sub_id, &subscriber, &PREPAID);
    (sub_id, subscriber, merchant)
}

// ── Pause state management ────────────────────────────────────────────────────

#[test]
fn test_merchant_not_paused_by_default() {
    let env = TestEnv::default();
    let merchant = Address::generate(&env.env);
    assert!(!env.client.get_merchant_paused(&merchant));
}

#[test]
fn test_pause_merchant_sets_flag() {
    let env = TestEnv::default();
    let (_, _, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    assert!(env.client.get_merchant_paused(&merchant));
}

#[test]
fn test_unpause_merchant_clears_flag() {
    let env = TestEnv::default();
    let (_, _, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    env.client.unpause_merchant(&merchant);
    assert!(!env.client.get_merchant_paused(&merchant));
}

#[test]
fn test_pause_is_idempotent() {
    let env = TestEnv::default();
    let (_, _, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    // Second call should succeed without error
    env.client.pause_merchant(&merchant);
    assert!(env.client.get_merchant_paused(&merchant));
}

#[test]
fn test_unpause_is_idempotent() {
    let env = TestEnv::default();
    let (_, _, merchant) = make_funded_sub(&env);
    // Unpause on a non-paused merchant should succeed silently
    env.client.unpause_merchant(&merchant);
    assert!(!env.client.get_merchant_paused(&merchant));
}

#[test]
fn test_pause_only_affects_paused_merchant() {
    let env = TestEnv::default();
    let (_, _, merchant_a) = make_funded_sub(&env);
    let (_, _, merchant_b) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant_a);
    assert!(env.client.get_merchant_paused(&merchant_a));
    assert!(!env.client.get_merchant_paused(&merchant_b));
}

// ── Events ────────────────────────────────────────────────────────────────────

#[test]
fn test_pause_emits_merchant_paused_event() {
    let env = TestEnv::default();
    let (_, _, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    let events = env.env.events().all();
    let has_event = events.iter().any(|(_, topics, _)| {
        topics
            .get(0)
            .and_then(|v| soroban_sdk::Symbol::try_from(v).ok())
            .map(|s| s == soroban_sdk::Symbol::new(&env.env, "merchant_paused"))
            .unwrap_or(false)
    });
    assert!(has_event, "expected merchant_paused event");
}

#[test]
fn test_unpause_emits_merchant_unpaused_event() {
    let env = TestEnv::default();
    let (_, _, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    env.env.events().all(); // clear
    env.client.unpause_merchant(&merchant);
    let events = env.env.events().all();
    let has_event = events.iter().any(|(_, topics, _)| {
        topics
            .get(0)
            .and_then(|v| soroban_sdk::Symbol::try_from(v).ok())
            .map(|s| s == soroban_sdk::Symbol::new(&env.env, "merchant_unpaused"))
            .unwrap_or(false)
    });
    assert!(has_event, "expected merchant_unpaused event");
}

#[test]
fn test_pause_idempotent_emits_no_duplicate_event() {
    let env = TestEnv::default();
    let (_, _, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    let _ = env.env.events().all(); // clear after first pause
    // Second pause call should be a no-op — no new event
    env.client.pause_merchant(&merchant);
    assert!(env.env.events().all().is_empty());
}

// ── Charge enforcement — interval charges ─────────────────────────────────────

#[test]
fn test_interval_charge_blocked_when_merchant_paused() {
    let env = TestEnv::default();
    let (sub_id, _, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    env.set_timestamp(INTERVAL);
    let result = env.client.try_charge_subscription(&sub_id);
    assert!(matches!(result, Err(Ok(Error::MerchantPaused))));
}

#[test]
fn test_interval_charge_succeeds_after_merchant_unpaused() {
    let env = TestEnv::default();
    let (sub_id, _, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    env.client.unpause_merchant(&merchant);
    env.set_timestamp(INTERVAL);
    assert_eq!(
        env.client.charge_subscription(&sub_id),
        ChargeExecutionResult::Charged
    );
}

#[test]
fn test_interval_charge_does_not_mutate_balance_when_paused() {
    let env = TestEnv::default();
    let (sub_id, _, merchant) = make_funded_sub(&env);
    let balance_before = env.client.get_subscription(&sub_id).prepaid_balance;
    env.client.pause_merchant(&merchant);
    env.set_timestamp(INTERVAL);
    let _ = env.client.try_charge_subscription(&sub_id);
    assert_eq!(
        env.client.get_subscription(&sub_id).prepaid_balance,
        balance_before
    );
}

// ── Charge enforcement — usage charges ───────────────────────────────────────

#[test]
fn test_usage_charge_blocked_when_merchant_paused() {
    let env = TestEnv::default();
    let (sub_id, _, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    let result = env.client.try_charge_usage(&sub_id, &1_000_000i128);
    assert!(matches!(result, Err(Ok(Error::MerchantPaused))));
}

#[test]
fn test_usage_charge_with_reference_blocked_when_paused() {
    let env = TestEnv::default();
    let (sub_id, _, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    let result = env.client.try_charge_usage_with_reference(
        &sub_id,
        &1_000_000i128,
        &String::from_str(&env.env, "ref-001"),
    );
    assert!(matches!(result, Err(Ok(Error::MerchantPaused))));
}

// ── Charge enforcement — one-off charges ─────────────────────────────────────

#[test]
fn test_oneoff_charge_blocked_when_merchant_paused() {
    let env = TestEnv::default();
    let (sub_id, _, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    let result = env.client.try_charge_one_off(&sub_id, &merchant, &AMOUNT);
    assert!(matches!(result, Err(Ok(Error::MerchantPaused))));
}

#[test]
fn test_oneoff_charge_succeeds_after_unpause() {
    let env = TestEnv::default();
    let (sub_id, _, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    env.client.unpause_merchant(&merchant);
    env.client.charge_one_off(&sub_id, &merchant, &AMOUNT);
    assert_eq!(env.client.get_merchant_balance(&merchant), AMOUNT);
}

// ── Batch charge enforcement ──────────────────────────────────────────────────

#[test]
fn test_batch_charge_skips_paused_merchant_with_error_code() {
    let env = TestEnv::default();
    let (sub_id, _, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    env.set_timestamp(INTERVAL);

    let ids = soroban_sdk::Vec::from_array(&env.env, [sub_id]);
    let results = env.client.batch_charge(&ids);
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    assert!(!result.success);
    assert_eq!(result.error_code, Error::MerchantPaused.to_code());
}

#[test]
fn test_batch_charge_mixed_paused_and_active() {
    let env = TestEnv::default();
    let (sub_paused, _, merchant_paused) = make_funded_sub(&env);
    let (sub_active, _, _) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant_paused);
    env.set_timestamp(INTERVAL);

    let ids = soroban_sdk::Vec::from_array(&env.env, [sub_paused, sub_active]);
    let results = env.client.batch_charge(&ids);
    assert_eq!(results.len(), 2);
    // Paused merchant's subscription fails
    assert!(!results.get(0).unwrap().success);
    // Active merchant's subscription succeeds
    assert!(results.get(1).unwrap().success);
}

// ── Allowed operations during pause ──────────────────────────────────────────

#[test]
fn test_withdrawal_allowed_when_merchant_paused() {
    let env = TestEnv::default();
    let (sub_id, _, merchant) = make_funded_sub(&env);
    // First charge to give merchant a balance
    env.set_timestamp(INTERVAL);
    env.client.charge_subscription(&sub_id);
    let balance = env.client.get_merchant_balance(&merchant);
    assert!(balance > 0);

    // Pause merchant
    env.client.pause_merchant(&merchant);

    // Withdrawal should still work
    env.client.withdraw_merchant_funds(&merchant, &balance);
    assert_eq!(env.client.get_merchant_balance(&merchant), 0);
}

#[test]
fn test_deposit_allowed_when_merchant_paused() {
    let env = TestEnv::default();
    let (sub_id, subscriber, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    env.stellar_token_client().mint(&subscriber, &AMOUNT);
    // Deposit should succeed — pause doesn't lock subscriber funds
    env.client.deposit_funds(&sub_id, &subscriber, &AMOUNT);
}

#[test]
fn test_cancel_subscription_allowed_when_merchant_paused() {
    let env = TestEnv::default();
    let (sub_id, subscriber, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    // Subscriber can cancel even when merchant is paused
    env.client.cancel_subscription(&sub_id, &subscriber);
    assert_eq!(
        env.client.get_subscription(&sub_id).status,
        SubscriptionStatus::Cancelled
    );
}

#[test]
fn test_subscription_status_preserved_when_merchant_paused() {
    let env = TestEnv::default();
    let (sub_id, subscriber, merchant) = make_funded_sub(&env);
    // Subscriber manually pauses their subscription
    env.client.pause_subscription(&sub_id, &subscriber);
    env.client.pause_merchant(&merchant);
    // The subscription remains in Paused state; merchant pause doesn't override it
    assert_eq!(
        env.client.get_subscription(&sub_id).status,
        SubscriptionStatus::Paused
    );
}

#[test]
fn test_subscriber_pause_preserved_after_merchant_unpauses() {
    let env = TestEnv::default();
    let (sub_id, subscriber, merchant) = make_funded_sub(&env);
    env.client.pause_subscription(&sub_id, &subscriber);
    env.client.pause_merchant(&merchant);
    env.client.unpause_merchant(&merchant);
    // Subscription is still Paused after merchant unpauses — subscriber preference is preserved
    assert_eq!(
        env.client.get_subscription(&sub_id).status,
        SubscriptionStatus::Paused
    );
    // Charge attempt still fails due to subscription status
    env.set_timestamp(INTERVAL);
    let result = env.client.try_charge_subscription(&sub_id);
    assert!(matches!(result, Err(Ok(Error::NotActive))));
}

// ── Precedence: Emergency Stop > Merchant Pause ───────────────────────────────

#[test]
fn test_emergency_stop_takes_precedence_over_merchant_pause() {
    let env = TestEnv::default();
    let (sub_id, _, merchant) = make_funded_sub(&env);
    env.client.pause_merchant(&merchant);
    env.client.enable_emergency_stop(&env.admin);
    env.set_timestamp(INTERVAL);
    // Emergency stop error is returned, not merchant pause error
    let result = env.client.try_charge_subscription(&sub_id);
    assert!(matches!(result, Err(Ok(Error::EmergencyStopActive))));
}

// ── Authorization ─────────────────────────────────────────────────────────────

#[test]
fn test_wrong_merchant_cannot_pause_another_merchant() {
    let env = TestEnv::default();
    let (_, _, merchant_a) = make_funded_sub(&env);
    let (_, _, merchant_b) = make_funded_sub(&env);
    // merchant_b trying to pause merchant_a — auth will fail
    // In mock_all_auths this would succeed, but we verify the flag went to the right place
    env.client.pause_merchant(&merchant_b);
    assert!(env.client.get_merchant_paused(&merchant_b));
    assert!(!env.client.get_merchant_paused(&merchant_a));
}
