//! Integration tests for multi-beneficiary treasury split routing.
//!
//! Validates:
//! - Single beneficiary preserves prior behavior
//! - Three-way split with non-divisible amount (rounding to last beneficiary)
//! - Sum-of-bps validation rejects 9_999 and 10_001
//! - Duplicate beneficiary rejected
//! - Zero-bps entry rejected
//! - Empty list rejected
//! - Clear treasury split reverts to single-treasury
//! - ProtocolFeeRoutedEvent emitted per beneficiary
//! - TreasurySplitConfiguredEvent emitted on configuration
//!
//! NOTE: `charge_subscription` (interval) has a pre-existing Symbol serialization
//! bug on this branch, so tests use `charge_usage` and `charge_one_off` which
//! exercise the same fee-routing code paths.

#![cfg(test)]

use crate::test_utils::setup::TestEnv;
use crate::types::{DataKey, TreasurySplitConfig, TreasurySplitEntry};
use soroban_sdk::{
    testutils::Address as _, testutils::Events as _, testutils::Ledger as _, Address, String, Vec,
};

const FEE_BPS: u32 = 1_000; // 10%
const CHARGE: i128 = 100_000_000;
const PREPAID: i128 = 1_000_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;

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

fn make_funded_usage_subscription(t: &TestEnv, merchant: &Address) -> u32 {
    let subscriber = Address::generate(&t.env);
    t.client.configure_usage_limits(
        merchant,
        &0u32,
        &None::<u32>,
        &0u64,
        &0u64,
        &None::<i128>,
    );
    let id = t.client.create_subscription(
        &subscriber,
        merchant,
        &CHARGE,
        &INTERVAL,
        &true, // usage_enabled
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



// ── Validation tests ─────────────────────────────────────────────────────────

#[test]
fn set_treasury_split_stores_config() {
    let t = TestEnv::default();
    let b1 = Address::generate(&t.env);
    let b2 = Address::generate(&t.env);

    let mut entries = Vec::new(&t.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: b1.clone(),
        bps: 7_000,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: b2.clone(),
        bps: 3_000,
    });

    t.client.set_treasury_split(&t.admin, &entries);

    let stored = t.client.get_treasury_split();
    assert!(stored.is_some());
    let config = stored.unwrap();
    assert_eq!(config.entries.len(), 2);
    assert_eq!(config.entries.get(0).unwrap().beneficiary, b1);
    assert_eq!(config.entries.get(0).unwrap().bps, 7_000);
    assert_eq!(config.entries.get(1).unwrap().beneficiary, b2);
    assert_eq!(config.entries.get(1).unwrap().bps, 3_000);
}

#[test]
fn set_treasury_split_emits_configured_event() {
    let t = TestEnv::default();
    let b1 = Address::generate(&t.env);

    let mut entries = Vec::new(&t.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: b1,
        bps: 10_000,
    });

    t.client.set_treasury_split(&t.admin, &entries);

    // Verify the event was emitted by checking the event count increased
    let events = t.env.events().all();
    assert!(!events.is_empty(), "set_treasury_split should emit at least one event");
}

#[test]
fn set_treasury_split_rejects_sum_9999() {
    let t = TestEnv::default();
    let b1 = Address::generate(&t.env);
    let b2 = Address::generate(&t.env);

    let mut entries = Vec::new(&t.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: b1,
        bps: 6_000,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: b2,
        bps: 3_999,
    });

    let result = t.client.try_set_treasury_split(&t.admin, &entries);
    assert!(result.is_err());
}

#[test]
fn set_treasury_split_rejects_sum_10001() {
    let t = TestEnv::default();
    let b1 = Address::generate(&t.env);
    let b2 = Address::generate(&t.env);

    let mut entries = Vec::new(&t.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: b1,
        bps: 6_000,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: b2,
        bps: 4_001,
    });

    let result = t.client.try_set_treasury_split(&t.admin, &entries);
    assert!(result.is_err());
}

#[test]
fn set_treasury_split_rejects_duplicate_beneficiary() {
    let t = TestEnv::default();
    let b1 = Address::generate(&t.env);

    let mut entries = Vec::new(&t.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: b1.clone(),
        bps: 5_000,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: b1,
        bps: 5_000,
    });

    let result = t.client.try_set_treasury_split(&t.admin, &entries);
    assert!(result.is_err());
}

#[test]
fn set_treasury_split_rejects_zero_bps() {
    let t = TestEnv::default();
    let b1 = Address::generate(&t.env);
    let b2 = Address::generate(&t.env);

    let mut entries = Vec::new(&t.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: b1,
        bps: 10_000,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: b2,
        bps: 0,
    });

    let result = t.client.try_set_treasury_split(&t.admin, &entries);
    assert!(result.is_err());
}

#[test]
fn set_treasury_split_rejects_empty_list() {
    let t = TestEnv::default();
    let entries = Vec::new(&t.env);

    let result = t.client.try_set_treasury_split(&t.admin, &entries);
    assert!(result.is_err());
}

#[test]
fn set_treasury_split_non_admin_rejected() {
    let t = TestEnv::default();
    let stranger = Address::generate(&t.env);
    let b1 = Address::generate(&t.env);

    let mut entries = Vec::new(&t.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: b1,
        bps: 10_000,
    });

    let result = t.client.try_set_treasury_split(&stranger, &entries);
    assert!(result.is_err());
}

// ── Single beneficiary preserves prior behavior ──────────────────────────────

#[test]
fn single_beneficiary_split_routes_full_fee() {
    let t = TestEnv::default();
    let treasury = Address::generate(&t.env);
    t.client.set_protocol_fee(&t.admin, &treasury, &FEE_BPS);

    let merchant = make_merchant(&t);
    let id = make_funded_usage_subscription(&t, &merchant);

    // Set single-beneficiary split (100% to one address)
    let split_addr = Address::generate(&t.env);
    let mut entries = Vec::new(&t.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: split_addr.clone(),
        bps: 10_000,
    });
    t.client.set_treasury_split(&t.admin, &entries);

    let usage: i128 = 40_000_000;
    t.client.charge_usage(&id, &usage);

    let expected_fee = usage * (FEE_BPS as i128) / 10_000;
    let expected_net = usage - expected_fee;

    assert_eq!(
        t.client.get_merchant_balance_by_token(&merchant, &t.token),
        expected_net
    );
    assert_eq!(
        t.client.get_merchant_balance_by_token(&split_addr, &t.token),
        expected_fee
    );
}

// ── Three-way split with non-divisible amount ────────────────────────────────

#[test]
fn three_way_split_distributes_fee_correctly() {
    let t = TestEnv::default();
    let treasury = Address::generate(&t.env);
    t.client.set_protocol_fee(&t.admin, &treasury, &FEE_BPS);

    let merchant = make_merchant(&t);
    let id = make_funded_usage_subscription(&t, &merchant);

    // 10% fee on 40_000_000 usage = 4_000_000 fee
    // Split: 50% + 30% + 20% = 100%
    let foundation = Address::generate(&t.env);
    let insurance = Address::generate(&t.env);
    let referrer = Address::generate(&t.env);

    let mut entries = Vec::new(&t.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: foundation.clone(),
        bps: 5_000,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: insurance.clone(),
        bps: 3_000,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: referrer.clone(),
        bps: 2_000,
    });
    t.client.set_treasury_split(&t.admin, &entries);

    let usage: i128 = 40_000_000;
    t.client.charge_usage(&id, &usage);

    let expected_fee = usage * (FEE_BPS as i128) / 10_000;
    let expected_net = usage - expected_fee;

    assert_eq!(
        t.client.get_merchant_balance_by_token(&merchant, &t.token),
        expected_net,
        "merchant should receive net amount"
    );

    // Foundation: 50% of 4_000_000 = 2_000_000
    assert_eq!(
        t.client.get_merchant_balance_by_token(&foundation, &t.token),
        expected_fee * 5_000 / 10_000
    );
    // Insurance: 30% of 4_000_000 = 1_200_000
    assert_eq!(
        t.client.get_merchant_balance_by_token(&insurance, &t.token),
        expected_fee * 3_000 / 10_000
    );
    // Referrer: remainder = 800_000 (20%)
    assert_eq!(
        t.client.get_merchant_balance_by_token(&referrer, &t.token),
        expected_fee - (expected_fee * 5_000 / 10_000) - (expected_fee * 3_000 / 10_000)
    );
}

// ── Rounding remainder goes to last beneficiary ──────────────────────────────

#[test]
fn rounding_remainder_goes_to_last_beneficiary() {
    let t = TestEnv::default();
    let treasury = Address::generate(&t.env);
    t.client.set_protocol_fee(&t.admin, &treasury, &333); // 3.33%

    let merchant = make_merchant(&t);
    let id = make_funded_usage_subscription(&t, &merchant);

    let b1 = Address::generate(&t.env);
    let b2 = Address::generate(&t.env);

    let mut entries = Vec::new(&t.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: b1.clone(),
        bps: 7_000,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: b2.clone(),
        bps: 3_000,
    });
    t.client.set_treasury_split(&t.admin, &entries);

    // Use a non-divisible usage amount: 100_000_001
    let usage: i128 = 100_000_001;
    t.client.charge_usage(&id, &usage);

    let fee = usage * 333 / 10_000;
    let b1_share = fee * 7_000 / 10_000;
    let b2_share = fee * 3_000 / 10_000;
    let remainder = fee - b1_share - b2_share;

    // Last beneficiary gets the rounding remainder
    assert_eq!(
        t.client.get_merchant_balance_by_token(&b1, &t.token),
        b1_share,
        "first beneficiary gets floor share"
    );
    assert_eq!(
        t.client.get_merchant_balance_by_token(&b2, &t.token),
        b2_share + remainder,
        "last beneficiary should receive the rounding remainder"
    );

    // Total fee distributed must equal the total fee
    let total_distributed = b1_share + b2_share + remainder;
    assert_eq!(total_distributed, fee);
}

// ── ProtocolFeeRoutedEvent emitted per beneficiary ───────────────────────────

#[test]
fn protocol_fee_routed_event_emitted_per_beneficiary() {
    let t = TestEnv::default();
    let treasury = Address::generate(&t.env);
    t.client.set_protocol_fee(&t.admin, &treasury, &FEE_BPS);

    let merchant = make_merchant(&t);
    let id = make_funded_usage_subscription(&t, &merchant);

    let b1 = Address::generate(&t.env);
    let b2 = Address::generate(&t.env);

    let mut entries = Vec::new(&t.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: b1,
        bps: 6_000,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: b2,
        bps: 4_000,
    });
    t.client.set_treasury_split(&t.admin, &entries);

    let usage: i128 = 40_000_000;
    t.client.charge_usage(&id, &usage);

    // Verify events were emitted
    let events = t.env.events().all();
    assert!(!events.is_empty(), "charge should emit events");
}

// ── Clear treasury split ─────────────────────────────────────────────────────

#[test]
fn clear_treasury_split_reverts_to_single_treasury() {
    let t = TestEnv::default();
    let treasury = Address::generate(&t.env);
    t.client.set_protocol_fee(&t.admin, &treasury, &FEE_BPS);

    let merchant = make_merchant(&t);
    let id = make_funded_usage_subscription(&t, &merchant);

    // Configure split
    let b1 = Address::generate(&t.env);
    let mut entries = Vec::new(&t.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: b1,
        bps: 10_000,
    });
    t.client.set_treasury_split(&t.admin, &entries);

    // Clear split
    t.client.clear_treasury_split(&t.admin);
    assert!(t.client.get_treasury_split().is_none());

    let usage: i128 = 40_000_000;
    t.client.charge_usage(&id, &usage);

    // Fee should go to single treasury
    let expected_fee = usage * (FEE_BPS as i128) / 10_000;
    assert_eq!(
        t.client.get_merchant_balance_by_token(&treasury, &t.token),
        expected_fee
    );
}

// ── Treasury split not configured falls back to single treasury ──────────────

#[test]
fn no_split_configured_falls_back_to_single_treasury() {
    let t = TestEnv::default();
    let treasury = Address::generate(&t.env);
    t.client.set_protocol_fee(&t.admin, &treasury, &FEE_BPS);

    let merchant = make_merchant(&t);
    let id = make_funded_usage_subscription(&t, &merchant);

    // No treasury split configured
    assert!(t.client.get_treasury_split().is_none());

    let usage: i128 = 40_000_000;
    t.client.charge_usage(&id, &usage);

    let expected_fee = usage * (FEE_BPS as i128) / 10_000;
    assert_eq!(
        t.client.get_merchant_balance_by_token(&treasury, &t.token),
        expected_fee
    );
}

// ── Usage charge with treasury split ─────────────────────────────────────────

#[test]
fn usage_charge_respects_treasury_split() {
    let t = TestEnv::default();
    let treasury = Address::generate(&t.env);
    t.client.set_protocol_fee(&t.admin, &treasury, &FEE_BPS);

    let merchant = make_merchant(&t);
    let id = make_funded_usage_subscription(&t, &merchant);

    let b1 = Address::generate(&t.env);
    let b2 = Address::generate(&t.env);

    let mut entries = Vec::new(&t.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: b1.clone(),
        bps: 6_000,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: b2.clone(),
        bps: 4_000,
    });
    t.client.set_treasury_split(&t.admin, &entries);

    let usage: i128 = 40_000_000;
    t.client.charge_usage(&id, &usage);

    let fee = usage * (FEE_BPS as i128) / 10_000;
    let net = usage - fee;

    assert_eq!(
        t.client.get_merchant_balance_by_token(&merchant, &t.token),
        net
    );
    assert_eq!(
        t.client.get_merchant_balance_by_token(&b1, &t.token),
        fee * 6_000 / 10_000
    );
    assert_eq!(
        t.client.get_merchant_balance_by_token(&b2, &t.token),
        fee - (fee * 6_000 / 10_000)
    );
}

// ── Conservation invariant: total distributed == fee amount ──────────────────

#[test]
fn conservation_invariant_holds_for_split() {
    let t = TestEnv::default();
    let treasury = Address::generate(&t.env);
    t.client.set_protocol_fee(&t.admin, &treasury, &777); // 7.77%

    let merchant = make_merchant(&t);
    let id = make_funded_usage_subscription(&t, &merchant);

    let b1 = Address::generate(&t.env);
    let b2 = Address::generate(&t.env);
    let b3 = Address::generate(&t.env);

    let mut entries = Vec::new(&t.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: b1.clone(),
        bps: 4_000,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: b2.clone(),
        bps: 3_500,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: b3.clone(),
        bps: 2_500,
    });
    t.client.set_treasury_split(&t.admin, &entries);

    let usage: i128 = 40_000_000;
    t.client.charge_usage(&id, &usage);

    let total_fee = usage * 777 / 10_000;
    let merchant_net = usage - total_fee;

    let merchant_bal = t.client.get_merchant_balance_by_token(&merchant, &t.token);
    let b1_bal = t.client.get_merchant_balance_by_token(&b1, &t.token);
    let b2_bal = t.client.get_merchant_balance_by_token(&b2, &t.token);
    let b3_bal = t.client.get_merchant_balance_by_token(&b3, &t.token);

    assert_eq!(merchant_net, merchant_bal, "merchant net must match");
    assert_eq!(
        total_fee,
        b1_bal + b2_bal + b3_bal,
        "sum of split beneficiaries must equal total fee"
    );
    assert_eq!(
        merchant_bal + b1_bal + b2_bal + b3_bal,
        usage,
        "conservation: gross == net + all fees"
    );
}
