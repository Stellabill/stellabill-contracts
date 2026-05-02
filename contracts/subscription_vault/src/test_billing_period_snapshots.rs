#![cfg(test)]

use crate::test_utils::setup::TestEnv;
use crate::types::{
    BillingPeriodSnapshot, ChargeExecutionResult, Error,
    SNAPSHOT_FLAG_CLOSED, SNAPSHOT_FLAG_INTERVAL_CHARGED,
};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::Address;

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const PREPAID: i128 = 200_000_000;

fn setup_charged_subscription(test_env: &TestEnv) -> (u32, Address, Address) {
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env.client.deposit_funds(&sub_id, &subscriber, &PREPAID);
    (sub_id, subscriber, merchant)
}

#[test]
fn test_snapshot_written_on_interval_charge() {
    let env = TestEnv::default();
    let (sub_id, _, _) = setup_charged_subscription(&env);

    // Advance past first interval
    env.set_timestamp(INTERVAL);
    let result = env.client.charge_subscription(&sub_id);
    assert_eq!(result, ChargeExecutionResult::Charged);

    let period_index = INTERVAL / INTERVAL; // = 1
    let snapshot = env.client.get_period_snapshot(&sub_id, &period_index);
    assert!(snapshot.is_some());

    let snap = snapshot.unwrap();
    assert_eq!(snap.subscription_id, sub_id);
    assert_eq!(snap.period_index, period_index);
    assert_eq!(snap.total_charged, AMOUNT);
    assert_eq!(snap.total_usage_units, 0);
    assert_eq!(snap.period_end, INTERVAL);
    assert!(snap.status_flags & SNAPSHOT_FLAG_CLOSED != 0);
    assert!(snap.status_flags & SNAPSHOT_FLAG_INTERVAL_CHARGED != 0);
    assert_eq!(snap.finalized_at, INTERVAL);
}

#[test]
fn test_no_snapshot_before_charge() {
    let env = TestEnv::default();
    let (sub_id, _, _) = setup_charged_subscription(&env);

    // No charge has happened yet
    let snapshot = env.client.get_period_snapshot(&sub_id, &1u64);
    assert!(snapshot.is_none());
}

#[test]
fn test_snapshot_immutable_after_close() {
    let env = TestEnv::default();
    let (sub_id, _, _) = setup_charged_subscription(&env);

    env.set_timestamp(INTERVAL);
    env.client.charge_subscription(&sub_id);

    let period_index = 1u64;
    let snap_before = env.client.get_period_snapshot(&sub_id, &period_index).unwrap();

    // Second charge for the same period is a Replay — snapshot must not change
    let err = env.client.try_charge_subscription(&sub_id);
    assert!(matches!(err, Err(Ok(Error::Replay))));

    let snap_after = env.client.get_period_snapshot(&sub_id, &period_index).unwrap();
    assert_eq!(snap_before, snap_after);
}

#[test]
fn test_multiple_periods_create_separate_snapshots() {
    let env = TestEnv::default();
    let (sub_id, _, _) = setup_charged_subscription(&env);

    // Period 1
    env.set_timestamp(INTERVAL);
    env.client.charge_subscription(&sub_id);

    // Period 2
    env.set_timestamp(INTERVAL * 2);
    env.client.charge_subscription(&sub_id);

    // Period 3
    env.set_timestamp(INTERVAL * 3);
    env.client.charge_subscription(&sub_id);

    for period in 1u64..=3 {
        let snap = env.client.get_period_snapshot(&sub_id, &period).unwrap();
        assert_eq!(snap.period_index, period);
        assert_eq!(snap.total_charged, AMOUNT);
        assert!(snap.status_flags & SNAPSHOT_FLAG_CLOSED != 0);
    }
}

#[test]
fn test_list_period_snapshots_newest_first() {
    let env = TestEnv::default();
    let (sub_id, _, _) = setup_charged_subscription(&env);

    env.set_timestamp(INTERVAL);
    env.client.charge_subscription(&sub_id);
    env.set_timestamp(INTERVAL * 2);
    env.client.charge_subscription(&sub_id);
    env.set_timestamp(INTERVAL * 3);
    env.client.charge_subscription(&sub_id);

    let snapshots = env.client.list_period_snapshots(&sub_id, &10u32);
    assert_eq!(snapshots.len(), 3);

    // Newest first
    assert_eq!(snapshots.get(0).unwrap().period_index, 3);
    assert_eq!(snapshots.get(1).unwrap().period_index, 2);
    assert_eq!(snapshots.get(2).unwrap().period_index, 1);
}

#[test]
fn test_list_respects_limit() {
    let env = TestEnv::default();
    let (sub_id, _, _) = setup_charged_subscription(&env);

    for i in 1u64..=5 {
        env.set_timestamp(INTERVAL * i);
        env.client.charge_subscription(&sub_id);
    }

    let snapshots = env.client.list_period_snapshots(&sub_id, &3u32);
    assert_eq!(snapshots.len(), 3);
    // Most-recent 3 are periods 5, 4, 3
    assert_eq!(snapshots.get(0).unwrap().period_index, 5);
    assert_eq!(snapshots.get(1).unwrap().period_index, 4);
    assert_eq!(snapshots.get(2).unwrap().period_index, 3);
}

#[test]
fn test_list_empty_for_new_subscription() {
    let env = TestEnv::default();
    let (sub_id, _, _) = setup_charged_subscription(&env);

    let snapshots = env.client.list_period_snapshots(&sub_id, &10u32);
    assert_eq!(snapshots.len(), 0);
}

#[test]
fn test_no_snapshot_on_failed_charge() {
    let env = TestEnv::default();
    let subscriber = Address::generate(&env.env);
    let merchant = Address::generate(&env.env);
    let sub_id = env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );
    // Deposit less than one charge — balance will be insufficient
    env.stellar_token_client().mint(&subscriber, &(AMOUNT / 2));
    env.client.deposit_funds(&sub_id, &subscriber, &(AMOUNT / 2));

    env.set_timestamp(INTERVAL);
    let result = env.client.charge_subscription(&sub_id);
    assert_eq!(result, ChargeExecutionResult::InsufficientBalance);

    // No snapshot should exist for a failed charge
    let snapshot = env.client.get_period_snapshot(&sub_id, &1u64);
    assert!(snapshot.is_none());
}

#[test]
fn test_snapshot_period_start_and_end() {
    let env = TestEnv::default();
    let (sub_id, _, _) = setup_charged_subscription(&env);

    let charge_time = INTERVAL + 100;
    env.set_timestamp(charge_time);
    env.client.charge_subscription(&sub_id);

    let period_index = charge_time / INTERVAL;
    let snap = env.client.get_period_snapshot(&sub_id, &period_index).unwrap();

    // period_end is the charge timestamp
    assert_eq!(snap.period_end, charge_time);
    // period_start is next_allowed - interval = (0 + INTERVAL) - INTERVAL = 0
    assert_eq!(snap.period_start, 0);
}
