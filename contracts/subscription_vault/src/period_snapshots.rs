//! Billing period snapshot storage — immutable per-period charge summaries.

use crate::types::{BillingPeriodSnapshot, DataKey, Error, SNAPSHOT_FLAG_CLOSED};
use soroban_sdk::{Env, Vec};

pub fn write_period_snapshot(env: &Env, snapshot: BillingPeriodSnapshot) -> Result<(), Error> {
    let key = DataKey::BillingPeriodSnapshot(snapshot.subscription_id, snapshot.period_index);

    if let Some(existing) = env
        .storage()
        .instance()
        .get::<_, BillingPeriodSnapshot>(&key)
    {
        // Closed snapshots are immutable.
        if existing.status_flags & SNAPSHOT_FLAG_CLOSED != 0 {
            return Err(Error::InvalidInput);
        }
    } else {
        // First write: register this period_index in the subscription's index list.
        let idx_key = DataKey::BillingPeriodSnapshotIndex(snapshot.subscription_id);
        let mut indices: Vec<u64> = env
            .storage()
            .instance()
            .get(&idx_key)
            .unwrap_or_else(|| Vec::new(env));
        indices.push_back(snapshot.period_index);
        env.storage().instance().set(&idx_key, &indices);
    }

    env.storage().instance().set(&key, &snapshot);
    Ok(())
}

pub fn get_period_snapshot(
    env: &Env,
    subscription_id: u32,
    period_index: u64,
) -> Option<BillingPeriodSnapshot> {
    env.storage()
        .instance()
        .get(&DataKey::BillingPeriodSnapshot(subscription_id, period_index))
}

/// Return up to `limit` most-recent snapshots for a subscription, newest first.
pub fn list_period_snapshots(
    env: &Env,
    subscription_id: u32,
    limit: u32,
) -> Vec<BillingPeriodSnapshot> {
    let idx_key = DataKey::BillingPeriodSnapshotIndex(subscription_id);
    let indices: Vec<u64> = env
        .storage()
        .instance()
        .get(&idx_key)
        .unwrap_or_else(|| Vec::new(env));

    let total = indices.len();
    let count = total.min(limit);
    let mut results = Vec::new(env);

    if count == 0 {
        return results;
    }

    // Newest-first: walk from the tail of the index list.
    let start = total - count;
    for i in (start..total).rev() {
        if let Some(period_idx) = indices.get(i) {
            if let Some(snapshot) = get_period_snapshot(env, subscription_id, period_idx) {
                results.push_back(snapshot);
            }
        }
    }
    results
}
