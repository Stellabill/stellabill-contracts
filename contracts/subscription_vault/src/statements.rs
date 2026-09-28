//! Billing statements: persistent, append-only ledger of charges per subscription.
//!
//! Maintains per-charge audit rows keyed by `DataKey::BillingStatement(subscription_id, seq)`,
//! indexed by `DataKey::BillingStatementsBySubscription(subscription_id)` as a vector of sequence IDs,
//! with sequential numbering tracked via `DataKey::BillingStatementSequence(subscription_id)`.
//! Supports configurable retention, inline pruning, explicit compaction into aggregated totals
//! (`DataKey::BillingStatementAggregate`), and offset/cursor pagination.

#![allow(dead_code)]

use crate::types::{
    AccruedTotals, BillingChargeKind, BillingCompactionSummary, BillingRetentionConfig,
    BillingStatement, BillingStatementAggregate, BillingStatementsPage, DataKey, Error,
    BILLING_STATEMENT_TTL_EXTEND_TO, BILLING_STATEMENT_TTL_THRESHOLD,
};
use soroban_sdk::{Address, Env, Vec};

/// Extends the TTL of a persistent billing-statement storage entry.
///
/// Only extends when the remaining TTL is below `BILLING_STATEMENT_TTL_THRESHOLD`.
/// This is a no-op when the key does not exist (the host ignores the call
/// for absent keys). Callers must not treat a missing return as an error.
pub(crate) fn extend_statement_ttl(env: &Env, key: &DataKey) {
    env.storage().persistent().extend_ttl(
        key,
        BILLING_STATEMENT_TTL_THRESHOLD,
        BILLING_STATEMENT_TTL_EXTEND_TO,
    );
}

/// Appends a new, immutable statement to the subscription's ledger under a
/// fresh, monotonically-increasing sequence number.
///
/// TTL is extended on every write for:
/// - `DataKey::BillingStatementSequence(subscription_id)` — the sequence counter.
/// - `DataKey::BillingStatement(subscription_id, seq)` — the statement body.
/// - `DataKey::BillingStatementsBySubscription(subscription_id)` — the secondary index.
///
/// If a global retention policy `keep_recent > 0` is set and the active statement
/// count exceeds `keep_recent`, inline pruning is triggered to keep storage bounded.
pub fn append_statement(
    env: &Env,
    subscription_id: u32,
    amount: i128,
    merchant: Address,
    kind: BillingChargeKind,
    period_start: u64,
    timestamp: u64,
) -> Result<(), Error> {
    let seq_key = DataKey::BillingStatementSequence(subscription_id);
    let seq: u32 = env.storage().persistent().get(&seq_key).unwrap_or(0);
    let next_seq = seq.checked_add(1).ok_or(Error::Overflow)?;
    env.storage().persistent().set(&seq_key, &next_seq);
    // Extend TTL on the sequence counter so it survives between charges.
    extend_statement_ttl(env, &seq_key);

    let stmt = BillingStatement {
        subscription_id,
        sequence: next_seq,
        charged_at: timestamp,
        period_start,
        period_end: timestamp,
        amount,
        merchant,
        kind,
    };
    let stmt_key = DataKey::BillingStatement(subscription_id, next_seq);
    env.storage().persistent().set(&stmt_key, &stmt);
    // Extend TTL on the statement body itself.
    extend_statement_ttl(env, &stmt_key);

    let idx_key = DataKey::BillingStatementsBySubscription(subscription_id);
    let mut ids: Vec<u32> = env.storage().persistent().get(&idx_key).unwrap_or(Vec::new(env));
    ids.push_back(next_seq);
    env.storage().persistent().set(&idx_key, &ids);
    // Extend TTL on the secondary index so queries remain available.
    extend_statement_ttl(env, &idx_key);

    // If retention config is enabled (keep_recent > 0), perform inline compaction
    let retention = get_retention_config(env);
    if retention.keep_recent > 0 && ids.len() > retention.keep_recent {
        compact_subscription_statements(env, subscription_id, Some(retention.keep_recent))?;
    }

    Ok(())
}

pub fn set_retention_config(env: &Env, keep_recent: u32) {
    env.storage()
        .instance()
        .set(&DataKey::BillingRetentionConfig, &BillingRetentionConfig { keep_recent });
}

pub fn get_retention_config(env: &Env) -> BillingRetentionConfig {
    env.storage()
        .instance()
        .get(&DataKey::BillingRetentionConfig)
        .unwrap_or(BillingRetentionConfig { keep_recent: 0 })
}

/// Cumulative totals across every statement ever pruned for this
/// subscription (accumulates across multiple `compact_subscription_statements`
/// calls; does not include statements still retained).
pub fn get_compacted_aggregate(env: &Env, subscription_id: u32) -> BillingStatementAggregate {
    env.storage()
        .persistent()
        .get(&DataKey::BillingStatementAggregate(subscription_id))
        .unwrap_or(BillingStatementAggregate {
            pruned_count: 0,
            total_amount: 0,
            totals: AccruedTotals { interval: 0, usage: 0, one_off: 0 },
            oldest_period_start: None,
            newest_period_end: None,
        })
}

/// Prunes all but the `keep_recent` most-recently-appended statements for
/// `subscription_id` (or `keep_recent_override`, if given), folding each
/// pruned statement's amount into the persistent [`BillingStatementAggregate`]
/// before deleting it. A no-op (zero-valued summary) when there are no more
/// than `keep_recent` statements to begin with — including an empty history.
pub fn compact_subscription_statements(
    env: &Env,
    subscription_id: u32,
    keep_recent_override: Option<u32>,
) -> Result<BillingCompactionSummary, Error> {
    let keep_recent = keep_recent_override.unwrap_or_else(|| get_retention_config(env).keep_recent);

    let idx_key = DataKey::BillingStatementsBySubscription(subscription_id);
    let ids: Vec<u32> = env.storage().persistent().get(&idx_key).unwrap_or(Vec::new(env));
    let total = ids.len();

    if total <= keep_recent {
        return Ok(BillingCompactionSummary {
            subscription_id,
            pruned_count: 0,
            kept_count: total,
            total_pruned_amount: 0,
        });
    }

    let prune_count = total - keep_recent;
    let mut pruned_amount_total: i128 = 0;
    let mut pruned_interval: i128 = 0;
    let mut pruned_usage: i128 = 0;
    let mut pruned_one_off: i128 = 0;
    let mut batch_oldest: Option<u64> = None;
    let mut batch_newest: Option<u64> = None;
    let mut kept_ids: Vec<u32> = Vec::new(env);

    for i in 0..total {
        let seq = ids.get(i).unwrap();
        let stmt_key = DataKey::BillingStatement(subscription_id, seq);
        if i < prune_count {
            if let Some(stmt) = env.storage().persistent().get::<_, BillingStatement>(&stmt_key) {
                pruned_amount_total = pruned_amount_total.checked_add(stmt.amount).ok_or(Error::Overflow)?;
                match stmt.kind {
                    BillingChargeKind::Interval => {
                        pruned_interval = pruned_interval.checked_add(stmt.amount).ok_or(Error::Overflow)?;
                    }
                    BillingChargeKind::Usage => {
                        pruned_usage = pruned_usage.checked_add(stmt.amount).ok_or(Error::Overflow)?;
                    }
                    BillingChargeKind::OneOff => {
                        pruned_one_off = pruned_one_off.checked_add(stmt.amount).ok_or(Error::Overflow)?;
                    }
                }
                batch_oldest = Some(batch_oldest.map_or(stmt.period_start, |o| o.min(stmt.period_start)));
                batch_newest = Some(batch_newest.map_or(stmt.period_end, |n| n.max(stmt.period_end)));
            }
            env.storage().persistent().remove(&stmt_key);
        } else {
            kept_ids.push_back(seq);
        }
    }

    env.storage().persistent().set(&idx_key, &kept_ids);

    let mut agg = get_compacted_aggregate(env, subscription_id);
    agg.pruned_count = agg.pruned_count.checked_add(prune_count).ok_or(Error::Overflow)?;
    agg.total_amount = agg.total_amount.checked_add(pruned_amount_total).ok_or(Error::Overflow)?;
    agg.totals.interval = agg.totals.interval.checked_add(pruned_interval).ok_or(Error::Overflow)?;
    agg.totals.usage = agg.totals.usage.checked_add(pruned_usage).ok_or(Error::Overflow)?;
    agg.totals.one_off = agg.totals.one_off.checked_add(pruned_one_off).ok_or(Error::Overflow)?;
    agg.oldest_period_start = match (agg.oldest_period_start, batch_oldest) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (None, x) => x,
        (x, None) => x,
    };
    agg.newest_period_end = match (agg.newest_period_end, batch_newest) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (None, x) => x,
        (x, None) => x,
    };
    env.storage()
        .persistent()
        .set(&DataKey::BillingStatementAggregate(subscription_id), &agg);

    Ok(BillingCompactionSummary {
        subscription_id,
        pruned_count: prune_count,
        kept_count: kept_ids.len(),
        total_pruned_amount: pruned_amount_total,
    })
}

/// Returns up to `limit` statements starting at `offset` into the
/// subscription's retained (non-pruned) statement list, ordered
/// newest-first or oldest-first. `next_cursor` (the next `offset` to
/// request) is `Some` iff more statements remain after this page.
///
/// TTL is extended on every read for the secondary index and each
/// statement body that is fetched, keeping them alive as long as the
/// contract is actively queried.
pub fn get_statements_by_subscription_offset(
    env: &Env,
    subscription_id: u32,
    offset: u32,
    limit: u32,
    newest_first: bool,
) -> Result<BillingStatementsPage, Error> {
    if limit == 0 {
        return Err(Error::InvalidInput);
    }

    let idx_key = DataKey::BillingStatementsBySubscription(subscription_id);
    let ids: Vec<u32> = env.storage().persistent().get(&idx_key).unwrap_or(Vec::new(env));
    // Extend TTL on the secondary index entry whenever it is read.
    if !ids.is_empty() {
        extend_statement_ttl(env, &idx_key);
    }
    let total = ids.len();

    let mut ordered: Vec<u32> = Vec::new(env);
    if newest_first {
        let mut i = ids.len();
        while i > 0 {
            i -= 1;
            ordered.push_back(ids.get(i).unwrap());
        }
    } else {
        ordered = ids;
    }

    let mut statements: Vec<BillingStatement> = Vec::new(env);
    let end = offset.saturating_add(limit).min(total);
    let mut i = offset;
    while i < end {
        let seq = ordered.get(i).unwrap();
        let stmt_key = DataKey::BillingStatement(subscription_id, seq);
        if let Some(stmt) = env
            .storage()
            .persistent()
            .get::<_, BillingStatement>(&stmt_key)
        {
            // Extend TTL on each fetched statement body.
            extend_statement_ttl(env, &stmt_key);
            statements.push_back(stmt);
        }
        i += 1;
    }

    let next_cursor = if end < total { Some(end) } else { None };
    Ok(BillingStatementsPage { statements, next_cursor, total })
}

/// Cursor-based pagination over the same ordering as
/// [`get_statements_by_subscription_offset`] — `cursor` is simply the
/// offset to resume from (`None` starts at the beginning).
pub fn get_statements_by_subscription_cursor(
    env: &Env,
    subscription_id: u32,
    cursor: Option<u32>,
    limit: u32,
    newest_first: bool,
) -> Result<BillingStatementsPage, Error> {
    get_statements_by_subscription_offset(
        env,
        subscription_id,
        cursor.unwrap_or(0),
        limit,
        newest_first,
    )
}
