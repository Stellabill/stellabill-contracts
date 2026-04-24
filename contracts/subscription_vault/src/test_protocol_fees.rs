use crate::test_utils::{setup::TestEnv, fixtures, assertions};
use crate::types::{SubscriptionStatus, ProtocolFeeChargedEvent, ProtocolFeeConfiguredEvent, SubscriptionChargedEvent, OneOffChargedEvent, UsageStatementEvent};
use soroban_sdk::{Address, testutils::{Events, Ledger}, IntoVal, FromVal, Symbol, symbol_short, vec};

#[test]
fn test_protocol_fee_configuration() {
    let test_env = TestEnv::default();
    let treasury = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);
    let fee_bps: u32 = 500; // 5%

    test_env.client.set_protocol_fee(&test_env.admin, &treasury, &fee_bps);

    // Verify event immediately after the call
    let events = test_env.env.events().all();
    let expected_topics = (Symbol::new(&test_env.env, "protocol_fee_configured"),).into_val(&test_env.env);
    let config_event = events.iter().rev().find(|e| {
        e.0 == test_env.client.address && e.1 == expected_topics
    }).expect("protocol_fee_configured event not found");
    
    let event_data = ProtocolFeeConfiguredEvent::from_val(&test_env.env, &config_event.2);
    assert_eq!(event_data.admin, test_env.admin);
    assert_eq!(event_data.treasury, treasury);
    assert_eq!(event_data.fee_bps, fee_bps);

    let snapshot = test_env.client.export_contract_snapshot(&test_env.admin);
    assert_eq!(snapshot.protocol_fee_treasury.unwrap(), treasury);
    assert_eq!(snapshot.protocol_fee_bps, fee_bps);
}

#[test]
fn test_protocol_fee_interval_charge_distribution() {
    let test_env = TestEnv::default();
    let treasury = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);
    let fee_bps = 1000; // 10%
    test_env.client.set_protocol_fee(&test_env.admin, &treasury, &fee_bps);

    let (id, _subscriber, merchant) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        crate::types::SubscriptionStatus::Active,
        10_000_000,
        30 * 24 * 60 * 60,
    );
    
    // Deposit enough for 1 charge (10,000,000)
    fixtures::seed_balance(&test_env.env, &test_env.client, id, 10_000_000);

    // Charge
    test_env.jump(30 * 24 * 60 * 60 + 1);
    test_env.client.charge_subscription(&id);
    let events = test_env.env.events().all();

    // Verify balances.
    // Treasury should have 10% of 10M = 1M
    assert_eq!(test_env.client.get_merchant_balance_by_token(&treasury, &test_env.token), 1_000_000);
    // Merchant should have 90% of 10M = 9M
    assert_eq!(test_env.client.get_merchant_balance_by_token(&merchant, &test_env.token), 9_000_000);
    
    // Find ProtocolFeeChargedEvent
    let expected_topics = (Symbol::new(&test_env.env, "protocol_fee_charged"), id).into_val(&test_env.env);
    let fee_event = if let Some(e) = events.iter().rev().find(|e| {
        e.0 == test_env.client.address && e.1 == expected_topics
    }) {
        e
    } else {
        panic!("ProtocolFeeChargedEvent not found. EVENTS: {:#?}", events);
    };
    
    let fee_data = ProtocolFeeChargedEvent::from_val(&test_env.env, &fee_event.2);
    assert_eq!(fee_data.fee_amount, 1_000_000);
    assert_eq!(fee_data.net_amount, 9_000_000);
    assert_eq!(fee_data.gross_amount, 10_000_000);

    // Find SubscriptionChargedEvent
    let expected_topics = (Symbol::new(&test_env.env, "charged"),).into_val(&test_env.env);
    let expected_topics_short = (symbol_short!("charged"),).into_val(&test_env.env);
    let charge_event = events.iter().rev().find(|e| {
        e.0 == test_env.client.address && (e.1 == expected_topics || e.1 == expected_topics_short)
    }).expect("SubscriptionChargedEvent not found");
    
    let charge_data = SubscriptionChargedEvent::from_val(&test_env.env, &charge_event.2);
    assert_eq!(charge_data.amount, 10_000_000);
    assert_eq!(charge_data.fee, 1_000_000);
    assert_eq!(charge_data.net_amount, 9_000_000);
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
        let mut sub: crate::types::Subscription = test_env.env.storage().instance().get(&id).unwrap();
        sub.usage_enabled = true;
        test_env.env.storage().instance().set(&id, &sub);
    });

    fixtures::seed_balance(&test_env.env, &test_env.client, id, 50_000_000);

    let usage_amount = 5_000_000;
    test_env.client.charge_usage_with_reference(&id, &usage_amount, &soroban_sdk::String::from_str(&test_env.env, "ref1"));
    let events = test_env.env.events().all();

    let merchant_bal = test_env.client.get_merchant_balance_by_token(&merchant, &test_env.token);
    let treasury_bal = test_env.client.get_merchant_balance_by_token(&treasury, &test_env.token);

    // 5_000_000 * 20% = 1_000_000
    assert_eq!(treasury_bal, 1_000_000);
    assert_eq!(merchant_bal, 4_000_000);

    // Verify UsageStatementEvent
    let expected_topics = (Symbol::new(&test_env.env, "usage_charged"), id).into_val(&test_env.env);
    let usage_event = if let Some(e) = events.iter().rev().find(|e| {
        e.0 == test_env.client.address && e.1 == expected_topics
    }) {
        e
    } else {
        panic!("usage_charged event not found. EVENTS: {:#?}", events);
    };
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
    test_env.client.charge_one_off(&id, &merchant, &one_off_amount);
    let events = test_env.env.events().all();

    let merchant_bal = test_env.client.get_merchant_balance_by_token(&merchant, &test_env.token);
    let treasury_bal = test_env.client.get_merchant_balance_by_token(&treasury, &test_env.token);

    // 4_000_000 * 15% = 600_000
    assert_eq!(treasury_bal, 600_000);
    assert_eq!(merchant_bal, 3_400_000);

    let expected_topics = (Symbol::new(&test_env.env, "oneoff_ch"), id).into_val(&test_env.env);
    let oneoff_event = events.iter().rev().find(|e| {
        e.0 == test_env.client.address && e.1 == expected_topics
    }).expect("oneoff_ch event not found");
    
    let oneoff_data = OneOffChargedEvent::from_val(&test_env.env, &oneoff_event.2);
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

    let (id, _subscriber, merchant) = fixtures::create_test_subscription(
        &test_env.env,
        &test_env.client,
        crate::types::SubscriptionStatus::Active,
    );
    
    // Deposit enough for 1 charge (10,000,000)
    fixtures::seed_balance(&test_env.env, &test_env.client, id, 10_000_000);

    // Charge
    test_env.jump(30 * 24 * 60 * 60 + 1);
    test_env.client.charge_subscription(&id);

    // 10,000,000 * 0.01% = 1,000
    assert_eq!(test_env.client.get_merchant_balance_by_token(&treasury, &test_env.token), 1_000);
    assert_eq!(test_env.client.get_merchant_balance_by_token(&merchant, &test_env.token), 9_999_000);
}

#[test]
fn test_protocol_fee_zero_bps_disabled() {
    let test_env = TestEnv::default();
    let treasury = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);
    let fee_bps = 0;

    test_env.client.set_protocol_fee(&test_env.admin, &treasury, &fee_bps);

    let (id, _subscriber, merchant) = fixtures::create_test_subscription(
        &test_env.env,
        &test_env.client,
        crate::types::SubscriptionStatus::Active,
    );
    
    // Deposit enough for 1 charge (10,000,000)
    fixtures::seed_balance(&test_env.env, &test_env.client, id, 10_000_000);

    // Charge
    test_env.jump(30 * 24 * 60 * 60 + 1);
    test_env.client.charge_subscription(&id);

    assert_eq!(test_env.client.get_merchant_balance_by_token(&treasury, &test_env.token), 0);
    assert_eq!(test_env.client.get_merchant_balance_by_token(&merchant, &test_env.token), 10_000_000);
}
