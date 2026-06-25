use crate::{
    period_snapshots::{get_period_snapshot, list_period_snapshots, write_period_snapshot},
    types::{
        BillingPeriodSnapshot, Error, SNAPSHOT_FLAG_CLOSED, SNAPSHOT_FLAG_EMPTY,
        SNAPSHOT_FLAG_INTERVAL_CHARGED, SNAPSHOT_FLAG_USAGE_CHARGED,
    },
    SubscriptionVault,
};
use soroban_sdk::{testutils::Ledger, Env, Address};

fn setup() -> (Env, Address) {
    let env = Env::default();
    env.ledger().set_timestamp(1_000_000);
    let contract_id = env.register(SubscriptionVault, ());
    (env, contract_id) 
}

fn as_contract<F, R>(env: &Env, f: F) -> R
where
    F: FnOnce() -> R,
{
    let contract_id = env.register(crate::SubscriptionVault, ());
    env.as_contract(&contract_id, f)
}

#[test]
fn test_write_and_get_snapshot() {
    let (env, contract_id) = setup();
    let sub_id = 1;
    let period_index = 0;

    let snapshot = BillingPeriodSnapshot {
        subscription_id: sub_id,
        period_index,
        period_start: 100,
        period_end: 200,
        total_charged: 500,
        total_usage_units: 50,
        status_flags: SNAPSHOT_FLAG_USAGE_CHARGED,
        finalized_at: 200,
    };

    as_contract(&env, || {
        assert!(write_period_snapshot(&env, snapshot.clone()).is_ok());

        let fetched = get_period_snapshot(&env, sub_id, period_index).unwrap();
        assert_eq!(fetched, snapshot);
    });
}

#[test]
fn test_list_period_snapshots_returns_latest_n() {
    let (env, contract_id) = setup();
    let sub_id = 2;

    as_contract(&env, || {
        for i in 0..5 {
            let snapshot = BillingPeriodSnapshot {
                subscription_id: sub_id,
                period_index: i,
                period_start: 100 + i * 100,
                period_end: 200 + i * 100,
                total_charged: 500,
                total_usage_units: 0,
                status_flags: SNAPSHOT_FLAG_INTERVAL_CHARGED | SNAPSHOT_FLAG_CLOSED,
                finalized_at: 200 + i * 100,
            };
            assert!(write_period_snapshot(&env, snapshot).is_ok());
        }

        // Get latest 3
        let latest = list_period_snapshots(&env, sub_id, 3);
        assert_eq!(latest.len(), 3);
        
        // Should return newest first
        assert_eq!(latest.get(0).unwrap().period_index, 4);
        assert_eq!(latest.get(1).unwrap().period_index, 3);
        assert_eq!(latest.get(2).unwrap().period_index, 2);
    });
}

#[test]
fn test_overwrite_closed_snapshot_rejected() {
    let (env, contract_id) = setup();
    let sub_id = 3;
    let period_index = 0;

    env.as_contract(&cid, || {
        let mut snapshot = BillingPeriodSnapshot {
            subscription_id: sub_id,
            period_index,
            period_start: 100,
            period_end: 200,
            total_charged: 500,
            total_usage_units: 0,
            status_flags: SNAPSHOT_FLAG_CLOSED | SNAPSHOT_FLAG_INTERVAL_CHARGED,
            finalized_at: 200,
        };

    as_contract(&env, || {
        assert!(write_period_snapshot(&env, snapshot.clone()).is_ok());

        // Try to update closed snapshot
        snapshot.total_charged = 1000;
        let result = write_period_snapshot(&env, snapshot);
        assert_eq!(result, Err(Error::InvalidStatusTransition));
    });
}

#[test]
fn test_empty_period_sets_empty_flag() {
    let (env, contract_id) = setup();
    let sub_id = 4;
    let period_index = 0;

    env.as_contract(&cid, || {
        let snapshot = BillingPeriodSnapshot {
            subscription_id: sub_id,
            period_index,
            period_start: 100,
            period_end: 200,
            total_charged: 0,
            total_usage_units: 0,
            status_flags: SNAPSHOT_FLAG_CLOSED,
            finalized_at: 200,
        };

    as_contract(&env, || {
        assert!(write_period_snapshot(&env, snapshot).is_ok());

        let fetched = get_period_snapshot(&env, sub_id, period_index).unwrap();
        assert_eq!(fetched.status_flags, SNAPSHOT_FLAG_CLOSED | SNAPSHOT_FLAG_EMPTY);
    });
}

#[test]
fn test_mixed_interval_and_usage_sets_both_flags() {
    let (env, contract_id) = setup();
    let sub_id = 5;
    let period_index = 0;

    as_contract(&env, || {
        // 1. Write usage charge
        let usage_snapshot = BillingPeriodSnapshot {
            subscription_id: sub_id,
            period_index,
            period_start: 100,
            period_end: 150,
            total_charged: 200,
            total_usage_units: 20,
            status_flags: SNAPSHOT_FLAG_USAGE_CHARGED,
            finalized_at: 150,
        };
        assert!(write_period_snapshot(&env, usage_snapshot).is_ok());

        // 2. Write interval charge closing the period
        let interval_snapshot = BillingPeriodSnapshot {
            subscription_id: sub_id,
            period_index,
            period_start: 100,
            period_end: 200,
            total_charged: 1000,
            total_usage_units: 0,
            status_flags: SNAPSHOT_FLAG_CLOSED | SNAPSHOT_FLAG_INTERVAL_CHARGED,
            finalized_at: 200,
        };
        assert!(write_period_snapshot(&env, interval_snapshot).is_ok());

        let fetched = get_period_snapshot(&env, sub_id, period_index).unwrap();
        
        // Assert merged state
        assert_eq!(fetched.total_charged, 1200);
        assert_eq!(fetched.total_usage_units, 20);
        assert_eq!(
            fetched.status_flags,
            SNAPSHOT_FLAG_CLOSED | SNAPSHOT_FLAG_INTERVAL_CHARGED | SNAPSHOT_FLAG_USAGE_CHARGED
        );
        assert_eq!(fetched.period_start, 100);
        assert_eq!(fetched.period_end, 200);
        assert_eq!(fetched.finalized_at, 200); // the max finalized_at
    });
}

#[test]
fn test_integrity_checks() {
    let (env, contract_id) = setup();
    
    // Invalid boundaries
    let bad_bounds = BillingPeriodSnapshot {
        subscription_id: 6,
        period_index: 0,
        period_start: 200,
        period_end: 100, // end < start
        total_charged: 100,
        total_usage_units: 0,
        status_flags: SNAPSHOT_FLAG_USAGE_CHARGED,
        finalized_at: 150,
    };
    as_contract(&env, || {
        assert_eq!(write_period_snapshot(&env, bad_bounds), Err(Error::InvalidInput));

        // Interval charge requires period_start < period_end
        let bad_interval = BillingPeriodSnapshot {
            subscription_id: 6,
            period_index: 0,
            period_start: 100,
            period_end: 100, // start == end
            total_charged: 100,
            total_usage_units: 0,
            status_flags: SNAPSHOT_FLAG_INTERVAL_CHARGED,
            finalized_at: 100,
        };
        assert_eq!(write_period_snapshot(&env, bad_interval), Err(Error::InvalidInput));
    });
}