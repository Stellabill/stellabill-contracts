#![cfg(test)]

use crate::{ChargeExecutionResult, Error, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::{Address as _, Events, Ledger as _};
use soroban_sdk::{Address, Env, FromVal, String, Symbol, Val, Vec};

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

fn topic0(env: &Env, event: &(Address, Vec<Val>, Val)) -> Symbol {
    Symbol::from_val(env, &event.1.get(0).unwrap())
}

#[test]
fn test_emergency_stop_blocks_all_critical_create_deposit_charge_paths() {
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
    );
    client.deposit_funds(&sub_id, &subscriber, &10_000_000i128);

    let plan_id = client.create_plan_template(
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
    );

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
            &None::<u64>
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
            &None::<u64>
        ),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(
        client.try_create_subscription_from_plan(&subscriber, &plan_id),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(
        client.try_deposit_funds(&sub_id, &subscriber, &1_000_000i128),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(
        client.try_charge_subscription(&sub_id),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(
        client.try_charge_usage(&sub_id, &100_000i128),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(
        client.try_charge_usage_with_reference(
            &sub_id,
            &100_000i128,
            &String::from_str(&env, "usage-ref"),
        ),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(
        client.try_charge_one_off(&sub_id, &merchant, &100_000i128),
        Err(Ok(Error::EmergencyStopActive))
    );

    // Read paths remain available during emergency stop.
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(client.get_admin(), admin);

    client.disable_emergency_stop(&admin);
    assert!(!client.get_emergency_stop_status());

    let resumed_id = client.create_subscription_from_plan(&subscriber, &plan_id);
    assert_eq!(client.get_subscription(&resumed_id).status, SubscriptionStatus::Active);
}

#[test]
fn test_emergency_stop_toggle_is_idempotent_and_emits_events_once_per_transition() {
    let (env, client, _, admin) = setup();

    client.enable_emergency_stop(&admin);
    let enabled_events = env.events().all();
    assert_eq!(enabled_events.len(), 1);
    assert_eq!(
        topic0(&env, &enabled_events.get(0).unwrap()),
        Symbol::new(&env, "emergency_stop_enabled")
    );

    client.enable_emergency_stop(&admin);
    assert!(env.events().all().is_empty());
    assert!(client.get_emergency_stop_status());

    client.disable_emergency_stop(&admin);
    let disabled_events = env.events().all();
    assert_eq!(disabled_events.len(), 1);
    assert_eq!(
        topic0(&env, &disabled_events.get(0).unwrap()),
        Symbol::new(&env, "emergency_stop_disabled")
    );

    client.disable_emergency_stop(&admin);
    assert!(env.events().all().is_empty());
    assert!(!client.get_emergency_stop_status());
}

#[test]
#[should_panic(expected = "Error(Contract, #1009)")]
fn test_emergency_stop_blocks_batch_charge() {
    let (env, client, token, admin) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &DEPOSIT);

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&sub_id, &subscriber, &10_000_000i128);
    env.ledger().set_timestamp(T0 + INTERVAL + 1);

    client.enable_emergency_stop(&admin);
    let ids = Vec::from_array(&env, [sub_id]);
    client.batch_charge(&ids);
}

#[test]
fn test_batch_charge_resumes_normally_after_emergency_stop_disabled() {
    let (env, client, token, admin) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &DEPOSIT);

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&sub_id, &subscriber, &10_000_000i128);
    env.ledger().set_timestamp(T0 + INTERVAL + 1);

    client.enable_emergency_stop(&admin);
    client.disable_emergency_stop(&admin);

    let ids = Vec::from_array(&env, [sub_id]);
    let results = client.batch_charge(&ids);
    assert_eq!(results.len(), 1);
    assert!(results.get(0).unwrap().success);
}

#[test]
fn test_lifetime_cap_interval_overrun_cancels_without_debiting_or_crediting() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &DEPOSIT);

    let amount = 10_000_000i128;
    let cap = (2 * amount) - 1;
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &INTERVAL,
        &false,
        &Some(cap),
        &None::<u64>,
    );
    client.deposit_funds(&sub_id, &subscriber, &(3 * amount));

    env.ledger().set_timestamp(T0 + INTERVAL + 1);
    assert_eq!(
        client.try_charge_subscription(&sub_id),
        Ok(Ok(ChargeExecutionResult::Charged))
    );
    let after_first = client.get_subscription(&sub_id);
    let merchant_after_first = client.get_merchant_balance(&merchant);

    env.ledger().set_timestamp(T0 + (2 * INTERVAL) + 1);
    assert_eq!(
        client.try_charge_subscription(&sub_id),
        Ok(Ok(ChargeExecutionResult::Charged))
    );

    let after_second = client.get_subscription(&sub_id);
    assert_eq!(after_second.status, SubscriptionStatus::Cancelled);
    assert_eq!(after_second.prepaid_balance, after_first.prepaid_balance);
    assert_eq!(after_second.lifetime_charged, after_first.lifetime_charged);
    assert_eq!(client.get_merchant_balance(&merchant), merchant_after_first);
}

#[test]
fn test_lifetime_cap_usage_exact_hit_charges_then_auto_cancels() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &DEPOSIT);

    let cap = 50_000_000i128;
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &1i128,
        &INTERVAL,
        &true,
        &Some(cap),
        &None::<u64>,
    );
    client.deposit_funds(&sub_id, &subscriber, &DEPOSIT);
    client.charge_usage_with_reference(
        &sub_id,
        &cap,
        &String::from_str(&env, "cap-exact-usage"),
    );

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, DEPOSIT - cap);
    assert_eq!(sub.lifetime_charged, cap);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(client.get_merchant_balance(&merchant), cap);
}

#[test]
fn test_lifetime_cap_usage_overrun_cancels_without_financial_side_effects() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &DEPOSIT);

    let cap = 50_000_000i128;
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &1i128,
        &INTERVAL,
        &true,
        &Some(cap),
        &None::<u64>,
    );
    client.deposit_funds(&sub_id, &subscriber, &DEPOSIT);

    // Simulate a nearly exhausted cap while still active.
    let mut sub = client.get_subscription(&sub_id);
    sub.lifetime_charged = cap - 1;
    env.as_contract(&client.address, || {
        env.storage().instance().set(&sub_id, &sub);
    });

    client.charge_usage_with_reference(
        &sub_id,
        &2i128,
        &String::from_str(&env, "cap-overrun-usage"),
    );

    let updated = client.get_subscription(&sub_id);
    assert_eq!(updated.status, SubscriptionStatus::Cancelled);
    assert_eq!(updated.prepaid_balance, DEPOSIT);
    assert_eq!(updated.lifetime_charged, cap - 1);
    assert_eq!(client.get_merchant_balance(&merchant), 0);
}

#[test]
fn test_lifetime_cap_oneoff_exact_hit_auto_cancels_and_emits_single_cap_event() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &DEPOSIT);

    let cap = 5_000_000i128;
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &Some(cap),
        &None::<u64>,
    );
    client.deposit_funds(&sub_id, &subscriber, &20_000_000i128);
    client.charge_one_off(&sub_id, &merchant, &cap);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(sub.lifetime_charged, cap);
    assert_eq!(sub.prepaid_balance, 15_000_000i128);
    assert_eq!(client.get_merchant_balance(&merchant), cap);

    let events = env.events().all();
    let mut cap_events = 0u32;
    for event in events.iter() {
        if event.0 != client.address { continue; }
        let topic: Symbol = Symbol::from_val(&env, &event.1.get(0).unwrap());
        if topic == Symbol::new(&env, "lifetime_cap_reached") {
            cap_events += 1;
        }
    }
    assert_eq!(cap_events, 1);
}
