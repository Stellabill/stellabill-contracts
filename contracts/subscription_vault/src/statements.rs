//! Billing statement append-only storage, pagination, and compaction.

use crate::safe_math::{safe_add, safe_sub};
use crate::types::{
    AccruedTotals, BillingChargeKind, BillingCompactionSummary, BillingRetentionConfig,
    BillingStatement, BillingStatementAggregate, BillingStatementsPage, Error,
};
use soroban_sdk::{symbol_short, Address, Env, Symbol, Vec};

const KEY_STATEMENT_NEXT: Symbol = symbol_short!("snext");
const KEY_STATEMENT_LIVE: Symbol = symbol_short!("slive");
const KEY_STATEMENT_ROW: Symbol = symbol_short!("srow");
const KEY_RETENTION: Symbol = symbol_short!("srtn");
const KEY_AGGREGATE: Symbol = symbol_short!("sagg");

fn next_statement_key(subscription_id: u32) -> (Symbol, u32) {
    (KEY_STATEMENT_NEXT, subscription_id)
}

fn live_statement_key(subscription_id: u32) -> (Symbol, u32) {
    (KEY_STATEMENT_LIVE, subscription_id)
}

fn statement_row_key(subscription_id: u32, sequence: u32) -> (Symbol, u32, u32) {
    (KEY_STATEMENT_ROW, subscription_id, sequence)
}

fn aggregate_key(subscription_id: u32) -> (Symbol, u32) {
    (KEY_AGGREGATE, subscription_id)
}

/// Persist default retention (`keep_recent` detailed rows). Caller must enforce admin auth.
///
/// `u32::MAX` means no automatic pruning threshold (keep all detail until overridden at compaction).
pub fn set_retention_config(env: &Env, keep_recent: u32) {
    env.storage()
        .instance()
        .set(&KEY_RETENTION, &BillingRetentionConfig { keep_recent });
}

pub fn get_retention_config(env: &Env) -> BillingRetentionConfig {
    env.storage()
        .instance()
        .get(&KEY_RETENTION)
        .unwrap_or(BillingRetentionConfig {
            keep_recent: u32::MAX,
        })
}

pub fn get_compacted_aggregate(env: &Env, subscription_id: u32) -> BillingStatementAggregate {
    env.storage()
        .instance()
        .get(&aggregate_key(subscription_id))
        .unwrap_or(BillingStatementAggregate {
            pruned_count: 0,
            total_amount: 0,
            totals: AccruedTotals {
                interval: 0,
                usage: 0,
                one_off: 0,
            },
            oldest_period_start: None,
            newest_period_end: None,
        })
}

pub fn append_statement(
    env: &Env,
    subscription_id: u32,
    amount: i128,
    merchant: Address,
    kind: BillingChargeKind,
    period_start: u64,
    period_end: u64,
) -> Result<(), Error> {
    // Invariants for period start/end boundaries
    if period_start > period_end {
        return Err(Error::InvalidInput);
    }
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }
    // For interval charges, ensure period spans time
    if kind == BillingChargeKind::Interval && period_start == period_end {
        return Err(Error::InvalidInput);
    }

    let storage = env.storage().instance();
    let next: u32 = storage
        .get(&next_statement_key(subscription_id))
        .unwrap_or(0);
    let live: u32 = storage
        .get(&live_statement_key(subscription_id))
        .unwrap_or(0);
    let statement = BillingStatement {
        subscription_id,
        sequence: next,
        charged_at: env.ledger().timestamp(),
        period_start,
        period_end,
        amount,
        merchant,
        kind,
    };
    storage.set(&statement_row_key(subscription_id, next), &statement);
    let next_sequence = next.checked_add(1).ok_or(Error::Overflow)?;
    let next_live = live.checked_add(1).ok_or(Error::Overflow)?;
    storage.set(
        &next_statement_key(subscription_id),
        &next_sequence,
    );
    storage.set(
        &live_statement_key(subscription_id),
        &next_live,
    );
    Ok(())
}

pub fn get_total_statements(env: &Env, subscription_id: u32) -> u32 {
    env.storage()
        .instance()
        .get(&live_statement_key(subscription_id))
        .unwrap_or(0)
}

pub fn compact_subscription_statements(
    env: &Env,
    subscription_id: u32,
    keep_recent_override: Option<u32>,
) -> Result<BillingCompactionSummary, Error> {
    let keep_recent = keep_recent_override.unwrap_or(get_retention_config(env).keep_recent);
    let storage = env.storage().instance();
    let next: u32 = storage
        .get(&next_statement_key(subscription_id))
        .unwrap_or(0);
    let live: u32 = storage
        .get(&live_statement_key(subscription_id))
        .unwrap_or(0);

    if live <= keep_recent || live == 0 {
        return Ok(BillingCompactionSummary {
            subscription_id,
            pruned_count: 0,
            kept_count: live,
            total_pruned_amount: 0,
        });
    }

    let target_pruned = (safe_sub(live as i128, keep_recent as i128).unwrap_or(0)) as u32;
    let mut removed = 0u32;
    let mut amount = 0i128;
    let mut oldest: Option<u64> = None;
    let mut newest: Option<u64> = None;

    let mut seq = next.saturating_sub(live);
    let mut interval_amt = 0i128;
    let mut usage_amt = 0i128;
    let mut one_off_amt = 0i128;

    while seq < next && removed < target_pruned {
        let key = statement_row_key(subscription_id, seq);
        if let Some(row) = storage.get::<_, BillingStatement>(&key) {
            amount = safe_add(amount, row.amount)?;
            match row.kind {
                BillingChargeKind::Interval => interval_amt = safe_add(interval_amt, row.amount)?,
                BillingChargeKind::Usage => usage_amt = safe_add(usage_amt, row.amount)?,
                BillingChargeKind::OneOff => one_off_amt = safe_add(one_off_amt, row.amount)?,
            }
            oldest = match oldest {
                Some(v) => Some(v.min(row.period_start)),
                None => Some(row.period_start),
            };
            newest = match newest {
                Some(v) => Some(v.max(row.period_end)),
                None => Some(row.period_end),
            };
            storage.remove(&key);
            removed += 1;
        }
        seq += 1;
    }

    // Consistency check
    if amount != (safe_add(safe_add(interval_amt, usage_amt)?, one_off_amt)?) {
        return Err(Error::Underflow); // Or some other appropriate error
    }

    let mut aggregate = get_compacted_aggregate(env, subscription_id);
    aggregate.pruned_count = aggregate
        .pruned_count
        .checked_add(removed)
        .ok_or(Error::Overflow)?;
    aggregate.total_amount = safe_add(aggregate.total_amount, amount)?;
    aggregate.totals.interval = safe_add(aggregate.totals.interval, interval_amt)?;
    aggregate.totals.usage = safe_add(aggregate.totals.usage, usage_amt)?;
    aggregate.totals.one_off = safe_add(aggregate.totals.one_off, one_off_amt)?;

    aggregate.oldest_period_start = match (aggregate.oldest_period_start, oldest) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (None, Some(b)) => Some(b),
        (a, None) => a,
    };
    aggregate.newest_period_end = match (aggregate.newest_period_end, newest) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (None, Some(b)) => Some(b),
        (a, None) => a,
    };
    storage.set(&aggregate_key(subscription_id), &aggregate);

    let kept_count = live.saturating_sub(removed);
    storage.set(&live_statement_key(subscription_id), &kept_count);

    Ok(BillingCompactionSummary {
        subscription_id,
        pruned_count: removed,
        kept_count,
        total_pruned_amount: amount,
    })
}

/// Offset/limit pagination over active statements.
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

    let total = get_total_statements(env, subscription_id);
    if total == 0 || offset >= total {
        return Ok(BillingStatementsPage {
            statements: Vec::new(env),
            next_cursor: None,
            total,
        });
    }

    let storage = env.storage().instance();
    let next: u32 = storage
        .get(&next_statement_key(subscription_id))
        .unwrap_or(0);
    let mut out = Vec::new(env);
    let mut skipped = 0u32;
    let mut taken = 0u32;
    let mut cursor: Option<u32> = None;

    if newest_first {
        let mut seq = next;
        while seq > 0 {
            seq -= 1;
            if let Some(row) =
                storage.get::<_, BillingStatement>(&statement_row_key(subscription_id, seq))
            {
                if skipped < offset {
                    skipped += 1;
                    continue;
                }
                out.push_back(row);
                taken += 1;
                if taken >= limit {
                    cursor = if seq > 0 { Some(seq - 1) } else { None };
                    break;
                }
            }
        }
    } else {
        let live: u32 = storage
            .get(&live_statement_key(subscription_id))
            .unwrap_or(0);
        let mut seq = next.saturating_sub(live);
        while seq < next {
            if let Some(row) =
                storage.get::<_, BillingStatement>(&statement_row_key(subscription_id, seq))
            {
                if skipped < offset {
                    skipped += 1;
                    seq += 1;
                    continue;
                }
                out.push_back(row);
                taken += 1;
                if taken >= limit {
                    cursor = if seq + 1 < next { Some(seq + 1) } else { None };
                    break;
                }
            }
            seq += 1;
        }
    }

    Ok(BillingStatementsPage {
        statements: out,
        next_cursor: cursor,
        total,
    })
}

/// Cursor pagination over active statements.
pub fn get_statements_by_subscription_cursor(
    env: &Env,
    subscription_id: u32,
    cursor: Option<u32>,
    limit: u32,
    newest_first: bool,
) -> Result<BillingStatementsPage, Error> {
    if limit == 0 {
        return Err(Error::InvalidInput);
    }

    let total = get_total_statements(env, subscription_id);
    if total == 0 {
        return Ok(BillingStatementsPage {
            statements: Vec::new(env),
            next_cursor: None,
            total,
        });
    }

    let storage = env.storage().instance();
    let next: u32 = storage
        .get(&next_statement_key(subscription_id))
        .unwrap_or(0);
    if next == 0 {
        return Ok(BillingStatementsPage {
            statements: Vec::new(env),
            next_cursor: None,
            total,
        });
    }
    let max_seq = next - 1;
    let live: u32 = storage
        .get(&live_statement_key(subscription_id))
        .unwrap_or(0);
    let min_seq = next.saturating_sub(live);
    let start = cursor.unwrap_or(if newest_first { max_seq } else { min_seq });
    if start > max_seq || start < min_seq {
        return Ok(BillingStatementsPage {
            statements: Vec::new(env),
            next_cursor: None,
            total,
        });
    }

    let mut out = Vec::new(env);
    let mut taken = 0u32;
    let mut next_cursor = None;

    if newest_first {
        let mut seq = start;
        loop {
            if let Some(row) =
                storage.get::<_, BillingStatement>(&statement_row_key(subscription_id, seq))
            {
                out.push_back(row);
                taken += 1;
                if taken >= limit {
                    next_cursor = if seq > min_seq { Some(seq - 1) } else { None };
                    break;
                }
            }
            if seq == min_seq {
                break;
            }
            seq -= 1;
        }
    } else {
        let mut seq = start;
        while seq <= max_seq {
            if let Some(row) =
                storage.get::<_, BillingStatement>(&statement_row_key(subscription_id, seq))
            {
                out.push_back(row);
                taken += 1;
                if taken >= limit {
                    next_cursor = if seq < max_seq { Some(seq + 1) } else { None };
                    break;
                }
            }
            if seq == max_seq {
                break;
            }
            seq += 1;
        }
    }

    Ok(BillingStatementsPage {
        statements: out,
        next_cursor,
        total,
    })
}
