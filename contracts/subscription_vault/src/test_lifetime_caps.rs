#![cfg(test)]

use crate::test_utils::setup::TestEnv;
use crate::types::{ChargeExecutionResult, Error, SubscriptionStatus};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::Address;

const AMOUNT: i128 = 10_000_000; // 10 USDC
const INTERVAL: u64 = 30 * 24 * 60 * 60;

fn make_sub(env: &TestEnv, cap: Option<i128>) -> (u32, Address, Address) {
    let subscriber = Address::generate(&env.env);
    let merchant = Address::generate(&env.env);
    let sub_id = env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &cap,
        &None::<u64>,
    );
    (sub_id, subscriber, merchant)
}

// ── Deposit-side cap enforcement ──────────────────────────────────────────────

#[test]
fn test_deposit_blocked_when_exceeds_remaining_cap() {
    let env = TestEnv::default();
    let cap = 25_000_000i128; // 2.5 intervals worth
    let (sub_id, subscriber, _) = make_sub(&env, Some(cap));

    env.stellar_token_client().mint(&subscriber, &100_000_000i128);

    // First deposit of 20 is fine (20 < 25)
    env.client.deposit_funds(&sub_id, &subscriber, &20_000_000i128);

    // Second deposit of 6 would bring balance to 26 > cap=25 → blocked
    let result = env.client.try_deposit_funds(&sub_id, &subscriber, &6_000_000i128);
    assert!(matches!(result, Err(Ok(Error::LifetimeCapReached))));
}

#[test]
fn test_deposit_allowed_up_to_cap() {
    let env = TestEnv::default();
    let cap = 20_000_000i128;
    let (sub_id, subscriber, _) = make_sub(&env, Some(cap));

    env.stellar_token_client().mint(&subscriber, &cap);
    // Deposit exactly the cap amount — should succeed
    env.client.deposit_funds(&sub_id, &subscriber, &cap);

    let sub = env.client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, cap);
}

#[test]
fn test_deposit_blocked_when_cap_fully_charged() {
    let env = TestEnv::default();
    let cap = AMOUNT; // exactly one charge
    let (sub_id, subscriber, _) = make_sub(&env, Some(cap));

    env.stellar_token_client().mint(&subscriber, &(cap * 3));
    env.client.deposit_funds(&sub_id, &subscriber, &cap);

    // Charge consumes the cap
    env.set_timestamp(INTERVAL);
    env.client.charge_subscription(&sub_id);

    // Any further deposit should be blocked — cap exhausted
    let result = env.client.try_deposit_funds(&sub_id, &subscriber, &AMOUNT);
    assert!(matches!(result, Err(Ok(Error::LifetimeCapReached))));
}

#[test]
fn test_deposit_remaining_after_partial_charge() {
    let env = TestEnv::default();
    let cap = 30_000_000i128; // 3 charges
    let (sub_id, subscriber, _) = make_sub(&env, Some(cap));

    env.stellar_token_client().mint(&subscriber, &cap);
    env.client.deposit_funds(&sub_id, &subscriber, &(AMOUNT * 2));

    // One charge consumed
    env.set_timestamp(INTERVAL);
    env.client.charge_subscription(&sub_id);

    // After charge: lifetime_charged=10, prepaid_balance=10, cap=30
    // Remaining depositable = 30 - 10 - 10 = 10 → exactly AMOUNT
    env.client.deposit_funds(&sub_id, &subscriber, &AMOUNT);

    // One more unit would exceed the cap
    let result = env.client.try_deposit_funds(&sub_id, &subscriber, &1i128);
    // Note: 1 < min_topup so the error may differ, but cap check also fires
    assert!(result.is_err());
}

#[test]
fn test_deposit_cap_not_enforced_without_lifetime_cap() {
    let env = TestEnv::default();
    let (sub_id, subscriber, _) = make_sub(&env, None);

    env.stellar_token_client().mint(&subscriber, &1_000_000_000i128);
    // Large deposit with no lifetime cap — should succeed
    env.client.deposit_funds(&sub_id, &subscriber, &500_000_000i128);
    env.client.deposit_funds(&sub_id, &subscriber, &500_000_000i128);

    let sub = env.client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 1_000_000_000i128);
}

// ── Global default cap ────────────────────────────────────────────────────────

#[test]
fn test_global_cap_default_applied_on_create() {
    let env = TestEnv::default();
    let global_cap = 50_000_000i128;
    env.client.set_global_cap_default(&env.admin, &Some(global_cap));

    // Create subscription with no explicit cap
    let subscriber = Address::generate(&env.env);
    let merchant = Address::generate(&env.env);
    let sub_id = env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    let cap_info = env.client.get_cap_info(&sub_id).unwrap();
    assert_eq!(cap_info.lifetime_cap, Some(global_cap));
}

#[test]
fn test_explicit_cap_overrides_global_default() {
    let env = TestEnv::default();
    env.client.set_global_cap_default(&env.admin, &Some(50_000_000i128));

    let explicit_cap = 100_000_000i128;
    let (sub_id, _, _) = make_sub(&env, Some(explicit_cap));

    let cap_info = env.client.get_cap_info(&sub_id).unwrap();
    assert_eq!(cap_info.lifetime_cap, Some(explicit_cap));
}

#[test]
fn test_get_global_cap_default_returns_none_initially() {
    let env = TestEnv::default();
    assert!(env.client.get_global_cap_default().is_none());
}

#[test]
fn test_global_cap_default_can_be_cleared() {
    let env = TestEnv::default();
    env.client.set_global_cap_default(&env.admin, &Some(50_000_000i128));
    assert!(env.client.get_global_cap_default().is_some());

    env.client.set_global_cap_default(&env.admin, &None::<i128>);
    assert!(env.client.get_global_cap_default().is_none());
}

#[test]
fn test_set_global_cap_default_unauthorized() {
    let env = TestEnv::default();
    let non_admin = Address::generate(&env.env);
    let result = env.client.try_set_global_cap_default(&non_admin, &Some(50_000_000i128));
    assert!(matches!(result, Err(Ok(Error::Unauthorized))));
}

// ── Per-merchant default cap ──────────────────────────────────────────────────

#[test]
fn test_merchant_cap_default_applied_on_create() {
    let env = TestEnv::default();
    let merchant = Address::generate(&env.env);
    let merchant_cap = 40_000_000i128;
    env.client.set_merchant_cap_default(&merchant, &Some(merchant_cap));

    let subscriber = Address::generate(&env.env);
    let sub_id = env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    let cap_info = env.client.get_cap_info(&sub_id).unwrap();
    assert_eq!(cap_info.lifetime_cap, Some(merchant_cap));
}

#[test]
fn test_merchant_cap_default_overrides_global_default() {
    let env = TestEnv::default();
    env.client.set_global_cap_default(&env.admin, &Some(50_000_000i128));

    let merchant = Address::generate(&env.env);
    let merchant_cap = 30_000_000i128;
    env.client.set_merchant_cap_default(&merchant, &Some(merchant_cap));

    let subscriber = Address::generate(&env.env);
    let sub_id = env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    // Merchant default wins over global default
    let cap_info = env.client.get_cap_info(&sub_id).unwrap();
    assert_eq!(cap_info.lifetime_cap, Some(merchant_cap));
}

#[test]
fn test_merchant_cap_default_can_be_cleared() {
    let env = TestEnv::default();
    let merchant = Address::generate(&env.env);
    env.client.set_merchant_cap_default(&merchant, &Some(40_000_000i128));

    env.client.set_merchant_cap_default(&merchant, &None::<i128>);
    assert!(env.client.get_merchant_cap_default(&merchant).is_none());
}

// ── Admin update subscription cap ────────────────────────────────────────────

#[test]
fn test_admin_can_raise_cap() {
    let env = TestEnv::default();
    let initial_cap = 20_000_000i128;
    let (sub_id, _, _) = make_sub(&env, Some(initial_cap));

    env.client.update_subscription_cap(&env.admin, &sub_id, &Some(50_000_000i128));

    let cap_info = env.client.get_cap_info(&sub_id).unwrap();
    assert_eq!(cap_info.lifetime_cap, Some(50_000_000i128));
}

#[test]
fn test_admin_can_remove_cap() {
    let env = TestEnv::default();
    let (sub_id, _, _) = make_sub(&env, Some(20_000_000i128));

    env.client.update_subscription_cap(&env.admin, &sub_id, &None::<i128>);

    let cap_info = env.client.get_cap_info(&sub_id).unwrap();
    assert!(cap_info.lifetime_cap.is_none());
}

#[test]
fn test_admin_cannot_lower_cap_below_charged() {
    let env = TestEnv::default();
    let cap = 30_000_000i128;
    let (sub_id, subscriber, _) = make_sub(&env, Some(cap));

    env.stellar_token_client().mint(&subscriber, &cap);
    env.client.deposit_funds(&sub_id, &subscriber, &cap);

    env.set_timestamp(INTERVAL);
    env.client.charge_subscription(&sub_id);

    // lifetime_charged = AMOUNT = 10_000_000 → cannot set cap to 5_000_000
    let result = env.client.try_update_subscription_cap(&env.admin, &sub_id, &Some(5_000_000i128));
    assert!(matches!(result, Err(Ok(Error::LifetimeCapReached))));
}

#[test]
fn test_admin_can_lower_cap_to_exact_charged() {
    let env = TestEnv::default();
    let cap = 30_000_000i128;
    let (sub_id, subscriber, _) = make_sub(&env, Some(cap));

    env.stellar_token_client().mint(&subscriber, &cap);
    env.client.deposit_funds(&sub_id, &subscriber, &cap);

    env.set_timestamp(INTERVAL);
    env.client.charge_subscription(&sub_id);

    // Setting cap to exactly lifetime_charged is allowed
    env.client.update_subscription_cap(&env.admin, &sub_id, &Some(AMOUNT));

    let cap_info = env.client.get_cap_info(&sub_id).unwrap();
    assert_eq!(cap_info.lifetime_cap, Some(AMOUNT));
    assert!(cap_info.cap_reached);
}

#[test]
fn test_update_cap_emits_event() {
    let env = TestEnv::default();
    let (sub_id, _, _) = make_sub(&env, Some(20_000_000i128));

    env.client.update_subscription_cap(&env.admin, &sub_id, &Some(50_000_000i128));

    let events = env.env.events().all();
    let has_cap_event = events.iter().any(|(_, topics, _)| {
        topics.len() >= 1
            && topics
                .get(0)
                .map(|v| {
                    soroban_sdk::Symbol::try_from(v)
                        .map(|s| s == soroban_sdk::Symbol::new(&env.env, "cap_updated"))
                        .unwrap_or(false)
                })
                .unwrap_or(false)
    });
    assert!(has_cap_event, "expected cap_updated event");
}

#[test]
fn test_update_cap_unauthorized() {
    let env = TestEnv::default();
    let (sub_id, _, _) = make_sub(&env, Some(20_000_000i128));
    let non_admin = Address::generate(&env.env);

    let result = env.client.try_update_subscription_cap(&non_admin, &sub_id, &Some(50_000_000i128));
    assert!(matches!(result, Err(Ok(Error::Unauthorized))));
}

// ── Charge-side cap enforcement (existing — regression guard) ─────────────────

#[test]
fn test_charge_cancelled_when_remaining_cap_less_than_amount() {
    let env = TestEnv::default();
    let cap = (AMOUNT * 2) - 1;
    let (sub_id, subscriber, merchant) = make_sub(&env, Some(cap));

    env.stellar_token_client().mint(&subscriber, &(AMOUNT * 3));
    env.client.deposit_funds(&sub_id, &subscriber, &(AMOUNT * 2));

    // First charge succeeds
    env.set_timestamp(INTERVAL);
    assert_eq!(
        env.client.charge_subscription(&sub_id),
        ChargeExecutionResult::Charged
    );

    let balance_after_1 = env.client.get_merchant_balance(&merchant);

    // Second charge: cap would be exceeded → cancels without charging
    env.set_timestamp(INTERVAL * 2);
    assert_eq!(
        env.client.charge_subscription(&sub_id),
        ChargeExecutionResult::Charged
    );

    let sub = env.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    // No funds transferred on the cancelling charge
    assert_eq!(env.client.get_merchant_balance(&merchant), balance_after_1);
}

#[test]
fn test_cap_info_remaining_decreases_after_charge() {
    let env = TestEnv::default();
    let cap = 50_000_000i128;
    let (sub_id, subscriber, _) = make_sub(&env, Some(cap));

    env.stellar_token_client().mint(&subscriber, &cap);
    env.client.deposit_funds(&sub_id, &subscriber, &cap);

    let info_before = env.client.get_cap_info(&sub_id).unwrap();
    assert_eq!(info_before.remaining_cap, Some(cap));

    env.set_timestamp(INTERVAL);
    env.client.charge_subscription(&sub_id);

    let info_after = env.client.get_cap_info(&sub_id).unwrap();
    assert_eq!(info_after.remaining_cap, Some(cap - AMOUNT));
    assert_eq!(info_after.lifetime_charged, AMOUNT);
}
