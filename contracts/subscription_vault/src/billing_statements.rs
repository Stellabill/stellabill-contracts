//! Period-end billing statement storage, indexing, and queries.

use soroban_sdk::{symbol_short, Address, Env, Vec};

use crate::types::{
    BillingStatementFinalization, BillingStatementPersistedEvent, BillingStatementRef, DataKey,
    Error, PeriodBillingStatement, SubscriptionStatus,
};

fn to_ref(stmt: &PeriodBillingStatement) -> BillingStatementRef {
    BillingStatementRef {
        subscription_id: stmt.subscription_id,
        period_index: stmt.period_index,
        period_end_timestamp: stmt.period_end_timestamp,
    }
}

fn contains_ref(items: &Vec<BillingStatementRef>, target: &BillingStatementRef) -> bool {
    let mut i = 0;
    while i < items.len() {
        let item = items.get(i).unwrap();
        if item.subscription_id == target.subscription_id
            && item.period_index == target.period_index
        {
            return true;
        }
        i += 1;
    }
    false
}

/// Persist `stmt` and maintain both secondary indices. Idempotent.
pub fn upsert_statement(env: &Env, stmt: PeriodBillingStatement) {
    let key = DataKey::BillingStatement(stmt.subscription_id, stmt.period_index);
    env.storage().instance().set(&key, &stmt);

    let stmt_ref = to_ref(&stmt);

    let sub_key = DataKey::BillingStatementsBySubscription(stmt.subscription_id);
    let mut sub_refs: Vec<BillingStatementRef> = env
        .storage().instance().get(&sub_key).unwrap_or(Vec::new(env));
    if !contains_ref(&sub_refs, &stmt_ref) {
        sub_refs.push_back(stmt_ref.clone());
        env.storage().instance().set(&sub_key, &sub_refs);
    }

    let merch_key = DataKey::BillingStatementsByMerchant(stmt.merchant.clone());
    let mut merch_refs: Vec<BillingStatementRef> = env
        .storage().instance().get(&merch_key).unwrap_or(Vec::new(env));
    if !contains_ref(&merch_refs, &stmt_ref) {
        merch_refs.push_back(stmt_ref);
        env.storage().instance().set(&merch_key, &merch_refs);
    }

    env.events().publish(
        (symbol_short!("bill_stmt"),),
        BillingStatementPersistedEvent {
            subscription_id: stmt.subscription_id,
            period_index: stmt.period_index,
            merchant: stmt.merchant,
            finalized_by: stmt.finalized_by,
        },
    );
}

pub fn get_statement(env: &Env, subscription_id: u32, period_index: u32) -> Result<PeriodBillingStatement, Error> {
    env.storage()
        .instance()
        .get(&DataKey::BillingStatement(subscription_id, period_index))
        .ok_or(Error::NotFound)
}

/// Return up to `limit` period statements for `subscription_id`, newest first.
///
/// `start` is a zero-based offset into the index vector (which is append-order,
/// i.e. oldest first), so `start = 0` with a large `limit` returns all records
/// in chronological order. Callers that want newest-first should reverse or use
/// the offset from the end of the index.
///
/// Returns an empty vec when `start` is out of range or `limit` is 0.
pub fn list_statements_by_subscription(
    env: &Env, subscription_id: u32, start: u32, limit: u32,
) -> Vec<PeriodBillingStatement> {
    if limit == 0 { return Vec::new(env); }
    let refs: Vec<BillingStatementRef> = env
        .storage().instance()
        .get(&DataKey::BillingStatementsBySubscription(subscription_id))
        .unwrap_or(Vec::new(env));
    if start >= refs.len() { return Vec::new(env); }
    let end = (start + limit).min(refs.len());
    let mut out = Vec::new(env);
    let mut i = start;
    while i < end {
        let r = refs.get(i).unwrap();
        if let Some(s) = env.storage().instance()
            .get::<_, PeriodBillingStatement>(&DataKey::BillingStatement(r.subscription_id, r.period_index))
        {
            out.push_back(s);
        }
        i += 1;
    }
    out
}

/// Return up to `limit` period statements for `merchant` whose `period_end_timestamp`
/// falls in `[start_timestamp, end_timestamp]` (both inclusive).
///
/// `start` is an offset into the filtered result set (for pagination across
/// multiple calls with the same time range).
pub fn list_statements_by_merchant_time_range(
    env: &Env, merchant: Address, start_timestamp: u64, end_timestamp: u64, start: u32, limit: u32,
) -> Vec<PeriodBillingStatement> {
    if limit == 0 { return Vec::new(env); }
    let refs: Vec<BillingStatementRef> = env
        .storage().instance()
        .get(&DataKey::BillingStatementsByMerchant(merchant))
        .unwrap_or(Vec::new(env));

    let mut filtered: Vec<BillingStatementRef> = Vec::new(env);
    let mut i = 0;
    while i < refs.len() {
        let r = refs.get(i).unwrap();
        if r.period_end_timestamp >= start_timestamp && r.period_end_timestamp <= end_timestamp {
            filtered.push_back(r);
        }
        i += 1;
    }
    if start >= filtered.len() { return Vec::new(env); }
    let end = (start + limit).min(filtered.len());
    let mut out = Vec::new(env);
    let mut j = start;
    while j < end {
        let r = filtered.get(j).unwrap();
        if let Some(s) = env.storage().instance()
            .get::<_, PeriodBillingStatement>(&DataKey::BillingStatement(r.subscription_id, r.period_index))
        {
            out.push_back(s);
        }
        j += 1;
    }
    out
}

pub struct PeriodStatementInput {
    pub subscription_id: u32,
    pub period_index: u32,
    pub merchant: Address,
    pub subscriber: Address,
    pub period_start_timestamp: u64,
    pub period_end_timestamp: u64,
    pub total_amount_charged: i128,
    pub total_usage_units: i128,
    pub protocol_fee_amount: i128,
    pub net_amount_to_merchant: i128,
    pub refund_amount: i128,
    pub status_flags: u32,
    pub subscription_status: SubscriptionStatus,
    pub finalized_by: BillingStatementFinalization,
    pub finalized_at: u64,
}

pub fn build_period_statement(env: &Env, input: PeriodStatementInput) -> Result<PeriodBillingStatement, Error> {
    let token: Address = env
        .storage().instance()
        .get(&soroban_sdk::Symbol::new(env, "token"))
        .ok_or(Error::NotInitialized)?;

    Ok(PeriodBillingStatement {
        subscription_id: input.subscription_id,
        period_index: input.period_index,
        snapshot_period_index: input.period_index,
        merchant: input.merchant,
        subscriber: input.subscriber,
        token,
        period_start_timestamp: input.period_start_timestamp,
        period_end_timestamp: input.period_end_timestamp,
        total_amount_charged: input.total_amount_charged,
        total_usage_units: input.total_usage_units,
        protocol_fee_amount: input.protocol_fee_amount,
        net_amount_to_merchant: input.net_amount_to_merchant,
        refund_amount: input.refund_amount,
        status_flags: input.status_flags,
        subscription_status: input.subscription_status,
        finalized_by: input.finalized_by,
        finalized_at: input.finalized_at,
    })
}
