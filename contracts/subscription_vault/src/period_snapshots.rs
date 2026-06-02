use soroban_sdk::{Env, Vec};
use crate::types::{
    BillingPeriodSnapshot, DataKey, Error, BILLING_PERIOD_SNAPSHOT_TTL_EXTEND_TO,
    BILLING_PERIOD_SNAPSHOT_TTL_THRESHOLD, SNAPSHOT_FLAG_CLOSED, SNAPSHOT_FLAG_EMPTY,
    SNAPSHOT_FLAG_INTERVAL_CHARGED, SNAPSHOT_FLAG_USAGE_CHARGED,
};

pub(crate) fn extend_snapshot_ttl(env: &Env, key: &DataKey) {
    env.storage().persistent().extend_ttl(
        key,
        BILLING_PERIOD_SNAPSHOT_TTL_THRESHOLD,
        BILLING_PERIOD_SNAPSHOT_TTL_EXTEND_TO,
    );
}

pub(crate) fn extend_index_ttl(env: &Env, key: &DataKey) {
    env.storage().persistent().extend_ttl(
        key,
        BILLING_PERIOD_SNAPSHOT_TTL_THRESHOLD,
        BILLING_PERIOD_SNAPSHOT_TTL_EXTEND_TO,
    );
}

pub fn write_period_snapshot(env: &Env, mut snapshot: BillingPeriodSnapshot) -> Result<(), Error> {
    if snapshot.period_start > snapshot.period_end {
        return Err(Error::InvalidInput);
    }
    
    if (snapshot.status_flags & SNAPSHOT_FLAG_INTERVAL_CHARGED) != 0 && snapshot.period_start >= snapshot.period_end {
        return Err(Error::InvalidInput);
    }

    let key = DataKey::BillingPeriodSnapshot(snapshot.subscription_id, snapshot.period_index);
    let mut is_new = true;

    if let Some(existing) = env.storage().persistent().get::<_, BillingPeriodSnapshot>(&key) {
        if (existing.status_flags & SNAPSHOT_FLAG_CLOSED) != 0 {
            return Err(Error::InvalidStatusTransition); // Reject overwriting a closed snapshot
        }
        is_new = false;
        
        snapshot.total_charged = existing
            .total_charged
            .checked_add(snapshot.total_charged)
            .ok_or(Error::Overflow)?;
        snapshot.total_usage_units = existing
            .total_usage_units
            .checked_add(snapshot.total_usage_units)
            .ok_or(Error::Overflow)?;
        
        snapshot.status_flags |= existing.status_flags;
        snapshot.period_start = existing.period_start;
        snapshot.period_end = snapshot.period_end.max(existing.period_end);
        snapshot.finalized_at = snapshot.finalized_at.max(existing.finalized_at);
    }

    if (snapshot.status_flags & SNAPSHOT_FLAG_CLOSED) != 0 {
        if (snapshot.status_flags & (SNAPSHOT_FLAG_INTERVAL_CHARGED | SNAPSHOT_FLAG_USAGE_CHARGED)) == 0 {
            snapshot.status_flags |= SNAPSHOT_FLAG_EMPTY;
        }
    }

    if (snapshot.status_flags & SNAPSHOT_FLAG_EMPTY) == 0 && snapshot.total_charged <= 0 {
        return Err(Error::InvalidInput);
    }

    env.storage().persistent().set(&key, &snapshot);
    extend_snapshot_ttl(env, &key);

    if is_new {
        let idx_key = DataKey::BillingPeriodSnapshotIndex(snapshot.subscription_id);
        let mut idx: Vec<u64> = env.storage().persistent().get(&idx_key).unwrap_or_else(|| Vec::new(env));
        idx.push_back(snapshot.period_index);
        env.storage().persistent().set(&idx_key, &idx);
        extend_index_ttl(env, &idx_key);
    }

    Ok(())
}

pub fn get_period_snapshot(
    env: &Env,
    subscription_id: u32,
    period_index: u64,
) -> Option<BillingPeriodSnapshot> {
    let key = DataKey::BillingPeriodSnapshot(subscription_id, period_index);
    let snapshot = env.storage().persistent().get(&key)?;
    extend_snapshot_ttl(env, &key);
    Some(snapshot)
}

pub fn list_period_snapshots(
    env: &Env,
    subscription_id: u32,
    limit: u32,
) -> Vec<BillingPeriodSnapshot> {
    let idx_key = DataKey::BillingPeriodSnapshotIndex(subscription_id);
    let idx: Vec<u64> = env.storage().persistent().get(&idx_key).unwrap_or_else(|| Vec::new(env));
    
    if idx.is_empty() {
        return Vec::new(env);
    }
    
    extend_index_ttl(env, &idx_key);

    let mut result = Vec::new(env);
    let len = idx.len();
    let count = limit.min(len);
    
    for i in 0..count {
        let period_index = idx.get(len - 1 - i).unwrap();
        if let Some(snapshot) = get_period_snapshot(env, subscription_id, period_index) {
            result.push_back(snapshot);
        }
    }
    result
}