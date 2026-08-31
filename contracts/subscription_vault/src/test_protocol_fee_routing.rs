//! Integration coverage for protocol-fee routing on every charge path.
//!
//! Closes the #932 acceptance criteria:
//! - interval (`charge_one` / `charge_subscription`)
//! - usage (`charge_usage_one`)
//! - one-off (`do_charge_one_off`)
//! - batch (`execute_batch_charge` via cached admin config)
//!
//! Invariant checked on every successful charge:
//! `gross == merchant_net + treasury_fee`

#![cfg(test)]

use crate::test_utils::setup::TestEnv;
use crate::types::DataKey;
use soroban_sdk::{
    testutils::Address as _, testutils::Events as _, testutils::Ledger as _, Address, String, Vec,
};

const FEE_BPS: u32 = 500; // 5 %
const CHARGE: i128 = 100_000_000;
const PREPAID: i128 = 1_000_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;

fn setup_with_fee(bps: u32) -> (TestEnv, Address) {
    let t = TestEnv::default();
    let treasury = Address::generate(&t.env);
    t.client.set_protocol_fee(&t.admin, &treasury, &bps);
    (t, treasury)
}

fn make_merchant(t: &TestEnv) -> Address {
    let merchant = Address::generate(&t.env);
    t.client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0i32,
        &0x1Fi32,
        &None,
        &String::from_str(&t.env, "https://example.com"),
    );
    merchant
}

fn make_funded_subscription(t: &TestEnv, merchant: &Address, usage_enabled: bool) -> u32 {
    let subscriber = Address::generate(&t.env);
    if usage_enabled {
        t.client.configure_usage_limits(
            merchant,
            &0u32,
            &None::<u32>,
            &0u64,
            &0u64,
            &None::<i128>,
        );
    }
    let id = t.client.create_subscription(
        &subscriber,
        merchant,
        &CHARGE,
        &INTERVAL,
        &usage_enabled,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    let mut sub = t.client.get_subscription(&id);
    sub.prepaid_balance = PREPAID;
    t.env.as_contract(&t.client.address, || {
        t.env.storage().persistent().set(&DataKey::Sub(id), &sub);
    });
    id
}

fn advance_interval(t: &TestEnv) {
    t.env.ledger().with_mut(|l| l.timestamp += INTERVAL + 1);
}

fn expected_split(gross: i128, bps: u32) -> (i128, i128) {
    let fee = gross * (bps as i128) / 10_000i128;
    (gross - fee, fee)
}

fn protocol_fee_event_count(t: &TestEnv) -> usize {
    t.env
        .events()
        .all()
        .iter()
        .filter(|(_addr, topics, _data)| {
            if topics.len() == 0 {
                return false;
            }
            format!("{:?}", topics.get(0).unwrap()).contains("protocol_fee_charged")
        })
        .count()
}

fn set_override_bps(t: &TestEnv, merchant: &Address, bps: u32) {
    t.env.as_contract(&t.client.address, || {
        t.env.storage()
            .instance()
            .set(&DataKey::MerchantFeeBps(merchant.clone()), &bps);
    });
}

#[test]
fn set_protocol_fee_writes_fee_bps_and_treasury() {
    let (t, treasury) = setup_with_fee(FEE_BPS);
    assert_eq!(t.client.get_protocol_fee_bps(), FEE_BPS);
    t.env.as_contract(&t.client.address, || {
        let stored: Option<Address> = t
            .env
            .storage()
            .persistent()
            .get(&DataKey::Treasury)
            .or_else(|| t.env.storage().instance().get(&DataKey::Treasury));
        assert_eq!(stored, Some(treasury));
        let stored_bps: Option<u32> = t
            .env
            .storage()
            .persistent()
            .get(&DataKey::FeeBps)
            .or_else(|| t.env.storage().instance().get(&DataKey::FeeBps));
        assert_eq!(stored_bps, Some(FEE_BPS));
    });
}

#[test]
fn interval_charge_routes_fee_to_treasury() {
    let (t, treasury) = setup_with_fee(FEE_BPS);
    let merchant = make_merchant(&t);
    let id = make_funded_subscription(&t, &merchant, false);
    advance_interval(&t);

    t.client
        .charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);

    let (net, fee) = expected_split(CHARGE, FEE_BPS);
    assert_eq!(
        t.client.get_merchant_balance_by_token(&merchant, &t.token),
        net
    );
    assert_eq!(
        t.client.get_merchant_balance_by_token(&treasury, &t.token),
        fee
    );
    assert_eq!(net + fee, CHARGE);
    assert!(protocol_fee_event_count(&t) >= 1);
}

#[test]
fn interval_charge_zero_fee_sends_full_amount_to_merchant() {
    let (t, treasury) = setup_with_fee(0);
    let merchant = make_merchant(&t);
    let id = make_funded_subscription(&t, &merchant, false);
    advance_interval(&t);

    let before = protocol_fee_event_count(&t);
    t.client
        .charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);

    assert_eq!(
        t.client.get_merchant_balance_by_token(&merchant, &t.token),
        CHARGE
    );
    assert_eq!(
        t.client.get_merchant_balance_by_token(&treasury, &t.token),
        0
    );
    assert_eq!(protocol_fee_event_count(&t), before);
}

#[test]
fn interval_charge_max_fee_sends_full_amount_to_treasury() {
    let (t, treasury) = setup_with_fee(10_000);
    let merchant = make_merchant(&t);
    let id = make_funded_subscription(&t, &merchant, false);
    advance_interval(&t);

    t.client
        .charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);

    assert_eq!(
        t.client.get_merchant_balance_by_token(&merchant, &t.token),
        0
    );
    assert_eq!(
        t.client.get_merchant_balance_by_token(&treasury, &t.token),
        CHARGE
    );
}

#[test]
fn interval_charge_rounding_remainder_stays_with_merchant() {
    let (t, treasury) = setup_with_fee(333);
    let merchant = make_merchant(&t);
    let id = make_funded_subscription(&t, &merchant, false);
    advance_interval(&t);

    t.client
        .charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);

    let (net, fee) = expected_split(CHARGE, 333);
    assert_eq!(
        t.client.get_merchant_balance_by_token(&merchant, &t.token),
        net
    );
    assert_eq!(
        t.client.get_merchant_balance_by_token(&treasury, &t.token),
        fee
    );
    assert_eq!(net + fee, CHARGE);
}

#[test]
fn usage_charge_routes_fee_to_treasury() {
    let (t, treasury) = setup_with_fee(FEE_BPS);
    let merchant = make_merchant(&t);
    let id = make_funded_subscription(&t, &merchant, true);
    let usage: i128 = 40_000_000;

    t.client.charge_usage(&id, &usage);

    let (net, fee) = expected_split(usage, FEE_BPS);
    assert_eq!(
        t.client.get_merchant_balance_by_token(&merchant, &t.token),
        net
    );
    assert_eq!(
        t.client.get_merchant_balance_by_token(&treasury, &t.token),
        fee
    );
    assert_eq!(net + fee, usage);
}

#[test]
fn one_off_charge_routes_fee_to_treasury() {
    let (t, treasury) = setup_with_fee(FEE_BPS);
    let merchant = make_merchant(&t);
    let id = make_funded_subscription(&t, &merchant, false);
    let amount: i128 = 25_000_000;

    t.client
        .charge_one_off(&id, &merchant, &amount, &None::<soroban_sdk::BytesN<32>>);

    let (net, fee) = expected_split(amount, FEE_BPS);
    assert_eq!(
        t.client.get_merchant_balance_by_token(&merchant, &t.token),
        net
    );
    assert_eq!(
        t.client.get_merchant_balance_by_token(&treasury, &t.token),
        fee
    );
    assert_eq!(net + fee, amount);
}

#[test]
fn one_off_zero_fee_sends_full_amount_to_merchant() {
    let (t, treasury) = setup_with_fee(0);
    let merchant = make_merchant(&t);
    let id = make_funded_subscription(&t, &merchant, false);

    t.client
        .charge_one_off(&id, &merchant, &CHARGE, &None::<soroban_sdk::BytesN<32>>);

    assert_eq!(
        t.client.get_merchant_balance_by_token(&merchant, &t.token),
        CHARGE
    );
    assert_eq!(
        t.client.get_merchant_balance_by_token(&treasury, &t.token),
        0
    );
}

#[test]
fn batch_charge_respects_merchant_fee_override() {
    let (t, treasury) = setup_with_fee(FEE_BPS);
    let merchant = make_merchant(&t);
    let override_bps: u32 = 200;
    set_override_bps(&t, &merchant, override_bps);

    let id = make_funded_subscription(&t, &merchant, false);
    advance_interval(&t);

    let mut ids = Vec::new(&t.env);
    ids.push_back(id);
    t.client.batch_charge(&ids, &0u64);

    let (net, fee) = expected_split(CHARGE, override_bps);
    assert_eq!(
        t.client.get_merchant_balance_by_token(&merchant, &t.token),
        net,
        "batch charge must use merchant override, not cached global FeeBps"
    );
    assert_eq!(
        t.client.get_merchant_balance_by_token(&treasury, &t.token),
        fee
    );
    assert_eq!(net + fee, CHARGE);
}

#[test]
fn one_off_wrong_merchant_is_rejected() {
    let (t, treasury) = setup_with_fee(FEE_BPS);
    let merchant = make_merchant(&t);
    let id = make_funded_subscription(&t, &merchant, false);
    let impostor = Address::generate(&t.env);

    let result = t.client.try_charge_one_off(
        &id,
        &impostor,
        &CHARGE,
        &None::<soroban_sdk::BytesN<32>>,
    );
    assert!(result.is_err());
    assert_eq!(
        t.client.get_merchant_balance_by_token(&merchant, &t.token),
        0
    );
    assert_eq!(
        t.client.get_merchant_balance_by_token(&treasury, &t.token),
        0
    );
}

#[test]
fn one_off_zero_amount_is_rejected() {
    let (t, treasury) = setup_with_fee(FEE_BPS);
    let merchant = make_merchant(&t);
    let id = make_funded_subscription(&t, &merchant, false);

    let result = t
        .client
        .try_charge_one_off(&id, &merchant, &0i128, &None::<soroban_sdk::BytesN<32>>);
    assert!(result.is_err());
    assert_eq!(
        t.client.get_merchant_balance_by_token(&treasury, &t.token),
        0
    );
}
