use crate::test_utils::{setup::TestEnv, fixtures, assertions};
use crate::types::{SubscriptionStatus, ProtocolFeeChargedEvent, ProtocolFeeConfiguredEvent, SubscriptionChargedEvent, OneOffChargedEvent, UsageStatementEvent};
use soroban_sdk::{Address, testutils::{Events, Ledger}, IntoVal, FromVal, Symbol, symbol_short};

#[test]
fn test_protocol_fee_configuration() {
    let test_env = TestEnv::default();
    let treasury = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);
    let fee_bps = 500; // 5%

    test_env.client.set_protocol_fee(&test_env.admin, &treasury, &fee_bps);

    let snapshot = test_env.client.export_contract_snapshot(&test_env.admin);
    assert_eq!(snapshot.protocol_fee_treasury.unwrap(), treasury);
    assert_eq!(snapshot.protocol_fee_bps, fee_bps);

    // Verify event
    let events = test_env.env.events().all();
    let config_event = events.iter().find(|e| {
        if e.0 != test_env.client.address { return false; }
        let topic: Symbol = Symbol::from_val(&test_env.env, &e.1.get(0).unwrap());
        topic == Symbol::new(&test_env.env, "protocol_fee_configured")
    }).expect("protocol_fee_configured event not found");
    
    let event_data: ProtocolFeeConfiguredEvent = ProtocolFeeConfiguredEvent::from_val(&test_env.env, &config_event.2);
    assert_eq!(event_data.admin, test_env.admin);
    assert_eq!(event_data.treasury, treasury);
    assert_eq!(event_data.fee_bps, fee_bps);
}

#[test]
#[should_panic(expected = "Error(Contract, #1015)")]
fn test_protocol_fee_max_bps_limit() {
    let test_env = TestEnv::default();
    let treasury = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);
    let fee_bps = 5001; // Over 50% limit

    test_env.client.set_protocol_fee(&test_env.admin, &treasury, &fee_bps);
}

#[test]
fn test_protocol_fee_interval_charge_distribution() {
    let test_env = TestEnv::default();
    let treasury = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);
    let fee_bps = 1000; // 10%

    test_env.client.set_protocol_fee(&test_env.admin, &treasury, &fee_bps);

    let amount = 10_000_000;
    let (id, subscriber, merchant) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        amount,
        30 * 24 * 60 * 60,
    );

    fixtures::seed_balance(&test_env.env, &test_env.client, id, 50_000_000);

    test_env.env.ledger().set_timestamp(test_env.env.ledger().timestamp() + 30 * 24 * 60 * 60 + 1);

    test_env.client.charge_subscription(&id).unwrap();

    let merchant_bal = test_env.client.get_merchant_balance_by_token(&merchant, &test_env.token);
    let treasury_bal = test_env.client.get_merchant_balance_by_token(&treasury, &test_env.token);

    assert_eq!(treasury_bal, 1_000_000);
    assert_eq!(merchant_bal, 9_000_000);

    // Verify events
    let events = test_env.env.events().all();
    
    // Find ProtocolFeeChargedEvent
    let fee_event = events.iter().find(|e| {
        if e.0 != test_env.client.address { return false; }
        let topic: Symbol = Symbol::from_val(&test_env.env, &e.1.get(0).unwrap());
        topic == Symbol::new(&test_env.env, "protocol_fee_charged") && u32::from_val(&test_env.env, &e.1.get(1).unwrap()) == id
    }).unwrap();
    let fee_data: ProtocolFeeChargedEvent = ProtocolFeeChargedEvent::from_val(&test_env.env, &fee_event.2);
    assert_eq!(fee_data.fee_amount, 1_000_000);
    assert_eq!(fee_data.net_amount, 9_000_000);
    assert_eq!(fee_data.gross_amount, 10_000_000);

    // Find SubscriptionChargedEvent
    let charge_event = events.iter().find(|e| {
        if e.0 != test_env.client.address { return false; }
        let topic: Symbol = Symbol::from_val(&test_env.env, &e.1.get(0).unwrap());
        topic == Symbol::new(&test_env.env, "charged")
    }).unwrap();
    let charge_data: SubscriptionChargedEvent = SubscriptionChargedEvent::from_val(&test_env.env, &charge_event.2);
    assert_eq!(charge_data.amount, 10_000_000);
    assert_eq!(charge_data.fee, 1_000_000);
    assert_eq!(charge_data.net_amount, 9_000_000);
}

#[test]
fn test_protocol_fee_usage_charge() {
    let test_env = TestEnv::default();
    let treasury = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);
    let fee_bps = 2000; // 20%

    test_env.client.set_protocol_fee(&test_env.admin, &treasury, &fee_bps);

    let (id, _, merchant) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        1_000_000,
        30 * 24 * 60 * 60,
    );
    // Enable usage
    test_env.env.as_contract(&test_env.client.address, || {
        let mut sub = test_env.client.get_subscription(&id);
        sub.usage_enabled = true;
        test_env.env.storage().instance().set(&id, &sub);
    });

    fixtures::seed_balance(&test_env.env, &test_env.client, id, 50_000_000);

    let usage_amount = 5_000_000;
    test_env.client.charge_usage_with_reference(&id, &usage_amount, &soroban_sdk::String::from_str(&test_env.env, "ref1")).unwrap();

    let merchant_bal = test_env.client.get_merchant_balance_by_token(&merchant, &test_env.token);
    let treasury_bal = test_env.client.get_merchant_balance_by_token(&treasury, &test_env.token);

    // 5_000_000 * 20% = 1_000_000
    assert_eq!(treasury_bal, 1_000_000);
    assert_eq!(merchant_bal, 4_000_000);

    // Verify UsageStatementEvent
    let events = test_env.env.events().all();
    let usage_event = events.iter().find(|e| {
        if e.0 != test_env.client.address { return false; }
        let topic: Symbol = Symbol::from_val(&test_env.env, &e.1.get(0).unwrap());
        topic == Symbol::new(&test_env.env, "usage_stmt")
    }).expect("usage_stmt event not found");
    let usage_data: UsageStatementEvent = UsageStatementEvent::from_val(&test_env.env, &usage_event.2);
    assert_eq!(usage_data.usage_amount, 5_000_000);
    assert_eq!(usage_data.fee, 1_000_000);
    assert_eq!(usage_data.net_amount, 4_000_000);
}

#[test]
fn test_protocol_fee_one_off_charge() {
    let test_env = TestEnv::default();
    let treasury = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);
    let fee_bps = 1500; // 15%

    test_env.client.set_protocol_fee(&test_env.admin, &treasury, &fee_bps);

    let (id, _, merchant) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        1_000_000,
        30 * 24 * 60 * 60,
    );

    fixtures::seed_balance(&test_env.env, &test_env.client, id, 50_000_000);

    let one_off_amount = 4_000_000;
    test_env.client.charge_one_off(&id, &merchant, &one_off_amount).unwrap();

    let merchant_bal = test_env.client.get_merchant_balance_by_token(&merchant, &test_env.token);
    let treasury_bal = test_env.client.get_merchant_balance_by_token(&treasury, &test_env.token);

    // 4_000_000 * 15% = 600_000
    assert_eq!(treasury_bal, 600_000);
    assert_eq!(merchant_bal, 3_400_000);

    // Verify OneOffChargedEvent
    let events = test_env.env.events().all();
    let oneoff_event = events.iter().find(|e| {
        if e.0 != test_env.client.address { return false; }
        let topic: Symbol = Symbol::from_val(&test_env.env, &e.1.get(0).unwrap());
        topic == Symbol::new(&test_env.env, "oneoff_ch")
    }).expect("oneoff_ch event not found");
    let oneoff_data: OneOffChargedEvent = OneOffChargedEvent::from_val(&test_env.env, &oneoff_event.2);
    assert_eq!(oneoff_data.amount, 4_000_000);
    assert_eq!(oneoff_data.fee, 600_000);
    assert_eq!(oneoff_data.net_amount, 3_400_000);
}

#[test]
fn test_protocol_fee_rounding_floor() {
    let test_env = TestEnv::default();
    let treasury = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);
    let fee_bps = 1; // 0.01%

    test_env.client.set_protocol_fee(&test_env.admin, &treasury, &fee_bps);

    let (id, _, merchant) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        1_000_000,
        30 * 24 * 60 * 60,
    );

    fixtures::seed_balance(&test_env.env, &test_env.client, id, 50_000_000);

    // 9,999 * 1 / 10,000 = 0.9999 -> 0 (floor rounding)
    let amount = 9_999;
    test_env.client.charge_one_off(&id, &merchant, &amount);

    let merchant_bal = test_env.client.get_merchant_balance_by_token(&merchant, &test_env.token);
    let treasury_bal = test_env.client.get_merchant_balance_by_token(&treasury, &test_env.token);

    assert_eq!(treasury_bal, 0);
    assert_eq!(merchant_bal, 9_999);

    // 10,000 * 1 / 10,000 = 1
    let amount2 = 10_000;
    test_env.client.charge_one_off(&id, &merchant, &amount2);
    
    let treasury_bal2 = test_env.client.get_merchant_balance_by_token(&treasury, &test_env.token);
    assert_eq!(treasury_bal2, 1);
}

#[test]
fn test_protocol_fee_zero_bps_disabled() {
    let test_env = TestEnv::default();
    let treasury = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);
    
    // Explicitly set to 0
    test_env.client.set_protocol_fee(&test_env.admin, &treasury, &0);

    let (id, _, merchant) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        10_000_000,
        30 * 24 * 60 * 60,
    );

    fixtures::seed_balance(&test_env.env, &test_env.client, id, 50_000_000);

    test_env.client.charge_one_off(&id, &merchant, &5_000_000);

    let merchant_bal = test_env.client.get_merchant_balance_by_token(&merchant, &test_env.token);
    let treasury_bal = test_env.client.get_merchant_balance_by_token(&treasury, &test_env.token);

    assert_eq!(treasury_bal, 0);
    assert_eq!(merchant_bal, 5_000_000);
}
