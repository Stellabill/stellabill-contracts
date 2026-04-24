#![cfg(test)]

use crate::{ChargeExecutionResult, Error, SubscriptionStatus};
use crate::test_utils::{fixtures, setup::TestEnv};
use soroban_sdk::{
    testutils::Events,
    FromVal, String, Symbol, Val, Vec,
    Address,
};

const T0: u64 = 1_700_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const DEPOSIT: i128 = 100_000_000;

fn topic0(env: &soroban_sdk::Env, event: &(Address, Vec<Val>, Val)) -> Symbol {
    Symbol::from_val(env, &event.1.get(0).unwrap())
}

#[test]
fn test_emergency_stop_blocks_all_critical_create_deposit_charge_paths() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let (sub_id, subscriber, merchant) = fixtures::create_usage_subscription(
        &test_env.env, &test_env.client, SubscriptionStatus::Active, 1_000_000i128, INTERVAL
    );
    test_env.stellar_token_client().mint(&subscriber, &DEPOSIT);
    test_env.client.deposit_funds(&sub_id, &subscriber, &10_000_000i128);

    let plan_id = test_env.client.create_plan_template(
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
    );

    test_env.client.enable_emergency_stop(&test_env.admin);
    assert!(test_env.client.get_emergency_stop_status());

    assert_eq!(
        test_env.client.try_create_subscription(
            &subscriber,
            &merchant,
            &1_000_000i128,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>
        ),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(
        test_env.client.try_create_subscription_with_token(
            &subscriber,
            &merchant,
            &test_env.token,
            &1_000_000i128,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>
        ),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(
        test_env.client.try_create_subscription_from_plan(&subscriber, &plan_id),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(
        test_env.client.try_deposit_funds(&sub_id, &subscriber, &1_000_000i128),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(
        test_env.client.try_charge_subscription(&sub_id),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(
        test_env.client.try_charge_usage(&sub_id, &100_000i128),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(
        test_env.client.try_charge_usage_with_reference(
            &sub_id,
            &100_000i128,
            &String::from_str(&test_env.env, "usage-ref"),
        ),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(
        test_env.client.try_charge_one_off(&sub_id, &merchant, &100_000i128),
        Err(Ok(Error::EmergencyStopActive))
    );

    // Read paths remain available during emergency stop.
    let sub = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(test_env.client.get_admin(), test_env.admin);

    test_env.client.disable_emergency_stop(&test_env.admin);
    assert!(!test_env.client.get_emergency_stop_status());

    let resumed_id = test_env.client.create_subscription_from_plan(&subscriber, &plan_id);
    assert_eq!(test_env.client.get_subscription(&resumed_id).status, SubscriptionStatus::Active);
}

#[test]
fn test_emergency_stop_toggle_is_idempotent_and_emits_events_once_per_transition() {
    let test_env = TestEnv::default();

    test_env.client.enable_emergency_stop(&test_env.admin);
    let enabled_events = test_env.env.events().all();
    assert_eq!(enabled_events.len(), 1);
    assert_eq!(
        topic0(&test_env.env, &enabled_events.get(0).unwrap()),
        Symbol::new(&test_env.env, "emergency_stop_enabled")
    );

    test_env.client.enable_emergency_stop(&test_env.admin);
    assert!(test_env.env.events().all().is_empty());
    assert!(test_env.client.get_emergency_stop_status());

    test_env.client.disable_emergency_stop(&test_env.admin);
    let disabled_events = test_env.env.events().all();
    assert_eq!(disabled_events.len(), 1);
    assert_eq!(
        topic0(&test_env.env, &disabled_events.get(0).unwrap()),
        Symbol::new(&test_env.env, "emergency_stop_disabled")
    );

    test_env.client.disable_emergency_stop(&test_env.admin);
    assert!(test_env.env.events().all().is_empty());
    assert!(!test_env.client.get_emergency_stop_status());
}

#[test]
#[should_panic(expected = "Error(Contract, #1009)")]
fn test_emergency_stop_blocks_batch_charge() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let (sub_id, subscriber, _) = fixtures::create_test_subscription(
        &test_env.env, &test_env.client, SubscriptionStatus::Active
    );
    test_env.stellar_token_client().mint(&subscriber, &DEPOSIT);

    test_env.client.deposit_funds(&sub_id, &subscriber, &10_000_000i128);
    test_env.set_timestamp(T0 + INTERVAL + 1);

    test_env.client.enable_emergency_stop(&test_env.admin);
    let ids = Vec::from_array(&test_env.env, [sub_id]);
    test_env.client.batch_charge(&ids);
}

#[test]
fn test_batch_charge_resumes_normally_after_emergency_stop_disabled() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let (sub_id, subscriber, _) = fixtures::create_test_subscription(
        &test_env.env, &test_env.client, SubscriptionStatus::Active
    );
    test_env.stellar_token_client().mint(&subscriber, &DEPOSIT);

    test_env.client.deposit_funds(&sub_id, &subscriber, &10_000_000i128);
    test_env.set_timestamp(T0 + INTERVAL + 1);

    test_env.client.enable_emergency_stop(&test_env.admin);
    test_env.client.disable_emergency_stop(&test_env.admin);

    let ids = Vec::from_array(&test_env.env, [sub_id]);
    let results = test_env.client.batch_charge(&ids);
    assert_eq!(results.len(), 1);
    assert!(results.get(0).unwrap().success);
}

#[test]
fn test_lifetime_cap_interval_overrun_cancels_without_debiting_or_crediting() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let amount = 10_000_000i128;
    let cap = (2 * amount) - 1;
    let (sub_id, subscriber, merchant) = fixtures::create_capped_subscription(
        &test_env.env, &test_env.client, amount, INTERVAL, Some(cap), false
    );
    test_env.stellar_token_client().mint(&subscriber, &DEPOSIT);

    test_env.client.deposit_funds(&sub_id, &subscriber, &(3 * amount));

    test_env.set_timestamp(T0 + INTERVAL + 1);
    assert_eq!(
        test_env.client.try_charge_subscription(&sub_id),
        Ok(Ok(ChargeExecutionResult::Charged))
    );
    let after_first = test_env.client.get_subscription(&sub_id);
    let merchant_after_first = test_env.client.get_merchant_balance(&merchant);

    test_env.set_timestamp(T0 + (2 * INTERVAL) + 1);
    assert_eq!(
        test_env.client.try_charge_subscription(&sub_id),
        Ok(Ok(ChargeExecutionResult::Charged))
    );

    let after_second = test_env.client.get_subscription(&sub_id);
    assert_eq!(after_second.status, SubscriptionStatus::Cancelled);
    assert_eq!(after_second.prepaid_balance, after_first.prepaid_balance);
    assert_eq!(after_second.lifetime_charged, after_first.lifetime_charged);
    assert_eq!(test_env.client.get_merchant_balance(&merchant), merchant_after_first);
}

#[test]
fn test_lifetime_cap_usage_exact_hit_charges_then_auto_cancels() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let cap = 50_000_000i128;
    let (sub_id, subscriber, merchant) = fixtures::create_capped_subscription(
        &test_env.env, &test_env.client, 1i128, INTERVAL, Some(cap), true
    );
    test_env.stellar_token_client().mint(&subscriber, &DEPOSIT);

    test_env.client.deposit_funds(&sub_id, &subscriber, &DEPOSIT);
    test_env.client.charge_usage_with_reference(
        &sub_id,
        &cap,
        &String::from_str(&test_env.env, "cap-exact-usage"),
    );

    let sub = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, DEPOSIT - cap);
    assert_eq!(sub.lifetime_charged, cap);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(test_env.client.get_merchant_balance(&merchant), cap);
}

#[test]
fn test_lifetime_cap_usage_overrun_cancels_without_financial_side_effects() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let cap = 50_000_000i128;
    let (sub_id, subscriber, merchant) = fixtures::create_capped_subscription(
        &test_env.env, &test_env.client, 1i128, INTERVAL, Some(cap), true
    );
    test_env.stellar_token_client().mint(&subscriber, &DEPOSIT);

    test_env.client.deposit_funds(&sub_id, &subscriber, &DEPOSIT);

    // Simulate a nearly exhausted cap while still active.
    let mut sub = test_env.client.get_subscription(&sub_id);
    sub.lifetime_charged = cap - 1;
    test_env.env.as_contract(&test_env.client.address, || {
        test_env.env.storage().instance().set(&sub_id, &sub);
    });

    test_env.client.charge_usage_with_reference(
        &sub_id,
        &2i128,
        &String::from_str(&test_env.env, "cap-overrun-usage"),
    );

    let updated = test_env.client.get_subscription(&sub_id);
    assert_eq!(updated.status, SubscriptionStatus::Cancelled);
    assert_eq!(updated.prepaid_balance, DEPOSIT);
    assert_eq!(updated.lifetime_charged, cap - 1);
    assert_eq!(test_env.client.get_merchant_balance(&merchant), 0);
}

#[test]
fn test_lifetime_cap_oneoff_exact_hit_auto_cancels_and_emits_single_cap_event() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let cap = 5_000_000i128;
    let (sub_id, subscriber, merchant) = fixtures::create_capped_subscription(
        &test_env.env, &test_env.client, 1_000_000i128, INTERVAL, Some(cap), false
    );
    test_env.stellar_token_client().mint(&subscriber, &DEPOSIT);

    test_env.client.deposit_funds(&sub_id, &subscriber, &20_000_000i128);
    test_env.client.charge_one_off(&sub_id, &merchant, &cap);
    let events = test_env.env.events().all();

    let sub = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(sub.lifetime_charged, cap);
    assert_eq!(sub.prepaid_balance, 15_000_000i128);
    assert_eq!(test_env.client.get_merchant_balance(&merchant), cap);
    let mut cap_events = 0u32;
    for event in events.iter() {
        if event.0 != test_env.client.address { continue; }
        if event.1.len() == 0 { continue; }
        let topic: Symbol = Symbol::from_val(&test_env.env, &event.1.get(0).unwrap());
        if topic == Symbol::new(&test_env.env, "lifetime_cap_reached") {
            cap_events += 1;
        }
    }
    assert_eq!(cap_events, 1);
}
