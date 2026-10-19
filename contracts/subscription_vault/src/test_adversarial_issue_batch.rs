#![cfg(test)]

use crate::test_utils::setup::TestEnv;
use crate::types::TreasurySplitEntry;
use crate::SubscriptionStatus;
use soroban_sdk::{testutils::Address as _, Address, Vec};

const CHARGE_AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const PREPAID: i128 = 25_000_000;

fn treasury_entries(
    env: &soroban_sdk::Env,
    beneficiary: Address,
    bps: u32,
) -> Vec<TreasurySplitEntry> {
    let mut entries = Vec::new(env);
    entries.push_back(TreasurySplitEntry { beneficiary, bps });
    entries
}

#[test]
fn set_treasury_split_preserves_previous_config_after_rejection() {
    let t = TestEnv::default();
    let beneficiary = Address::generate(&t.env);
    let entries = treasury_entries(&t.env, beneficiary.clone(), 10_000);
    t.client.set_treasury_split(&t.admin, &entries);

    let invalid = treasury_entries(&t.env, Address::generate(&t.env), 9_999);
    let result = t.client.try_set_treasury_split(&t.admin, &invalid);
    assert!(result.is_err());

    let stored = t.client.get_treasury_split().expect("config should remain");
    assert_eq!(stored.entries.len(), 1);
    assert_eq!(stored.entries.get(0).unwrap().beneficiary, beneficiary);
    assert_eq!(stored.entries.get(0).unwrap().bps, 10_000);
}

#[test]
fn set_auto_pause_threshold_covers_zero_max_and_unauthorized_calls() {
    let t = TestEnv::default();
    let stranger = Address::generate(&t.env);

    t.client.set_auto_pause_threshold(&t.admin, &0);
    let zero = t.env.as_contract(&t.client.address, || {
        crate::admin::get_auto_pause_threshold(&t.env)
    });
    assert_eq!(zero, 0);

    t.client.set_auto_pause_threshold(&t.admin, &u32::MAX);
    let maximum = t.env.as_contract(&t.client.address, || {
        crate::admin::get_auto_pause_threshold(&t.env)
    });
    assert_eq!(maximum, u32::MAX);

    let result = t.client.try_set_auto_pause_threshold(&stranger, &1);
    assert!(result.is_err());
    let unchanged = t.env.as_contract(&t.client.address, || {
        crate::admin::get_auto_pause_threshold(&t.env)
    });
    assert_eq!(unchanged, u32::MAX);
}

#[test]
fn add_accepted_token_rejects_unauthorized_without_state_change() {
    let t = TestEnv::default();
    let stranger = Address::generate(&t.env);
    let token = Address::generate(&t.env);
    let before = t.client.list_accepted_tokens();

    let result = t
        .client
        .try_add_accepted_token(&stranger, &token, &u32::MAX);
    assert!(result.is_err());
    assert_eq!(t.client.list_accepted_tokens(), before);

    t.client.add_accepted_token(&t.admin, &token, &u32::MAX);
    let tokens = t.client.list_accepted_tokens();
    let added = tokens
        .iter()
        .find(|entry| entry.token == token)
        .expect("accepted token should be stored");
    assert_eq!(added.decimals, u32::MAX);
}

fn create_usage_subscription(t: &TestEnv, operator: &Address) -> u32 {
    let subscriber = Address::generate(&t.env);
    let merchant = Address::generate(&t.env);
    let id = t.client.create_subscription(
        &subscriber,
        &merchant,
        &CHARGE_AMOUNT,
        &INTERVAL,
        &true,
        &None,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    t.stellar_token_client().mint(&subscriber, &PREPAID);
    t.client
        .deposit_funds(&id, &subscriber, &PREPAID, &None::<soroban_sdk::BytesN<32>>);
    t.client.set_operator(&t.admin, operator);
    id
}

#[test]
fn do_operator_charge_usage_rejects_invalid_and_unauthorized_without_mutation() {
    let t = TestEnv::default();
    let operator = Address::generate(&t.env);
    let stranger = Address::generate(&t.env);
    let id = create_usage_subscription(&t, &operator);
    let before = t.client.get_subscription(&id);

    let invalid = t.client.try_operator_charge_usage(&operator, &id, &0);
    assert!(invalid.is_err());
    assert_eq!(
        t.client.get_subscription(&id).prepaid_balance,
        before.prepaid_balance
    );

    let unauthorized = t
        .client
        .try_operator_charge_usage(&stranger, &id, &1_000_000);
    assert!(unauthorized.is_err());
    assert_eq!(
        t.client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );

    let charged = t.client.operator_charge_usage(&operator, &id, &1_000_000);
    assert_eq!(charged, crate::types::UsageChargeResult::Charged);
    assert_eq!(
        t.client.get_subscription(&id).prepaid_balance,
        PREPAID - 1_000_000
    );
}
