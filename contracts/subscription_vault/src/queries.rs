//! Read-only entrypoints and helpers: get_subscription, estimate_topup, list_subscriptions_by_subscriber.
//!
//! **PRs that only add or change read-only/query behavior should edit this file only.**
//!
//! ## Pagination invariants (off-chain / indexers)
//!
//! - **`list_subscriptions_by_subscriber`**: Results are ordered by subscription id ascending.
//!   `start_from_id` is inclusive. Continue with `next_start_id` when present (next id to scan).
//!   Each call scans at most `MAX_SCAN_DEPTH` IDs — if the scan budget is exhausted before the
//!   page is full, `next_start_id` is set to the resume point so callers can chain pages.
//! - **`get_subscriptions_by_merchant`** / **`get_subscriptions_by_token`**: Results follow the
//!   order of ids in the on-chain index (`MerchantSubs` / `token_subs`), which is insertion
//!   order (ascending id order for subscriptions created through this contract). `start` is a
//!   0-based offset into that id list. Missing subscription records are skipped; callers should
//!   use [`get_merchant_subscription_count`] or [`get_token_subscription_count`] for the index
//!   length, not `result.len()`, when paginating.
//!
//! ## Read complexity per endpoint
//!
//! | Endpoint | Storage reads | Notes |
//! |---|---|---|
//! | `get_subscription` | 1 | Direct key lookup |
//! | `estimate_topup_for_intervals` | 1 | Calls `get_subscription` |
//! | `get_subscriptions_by_merchant` | 1 + limit | 1 index read + up to `limit` sub reads |
//! | `get_merchant_subscription_count` | 1 | Index length only |
//! | `get_subscriptions_by_token` | 1 + limit | 1 index read + up to `limit` sub reads |
//! | `get_token_subscription_count` | 1 | Index length only |
//! | `compute_next_charge_info` | 0 | Pure computation |
//! | `get_cap_info` | 1 | Calls `get_subscription` |
//! | `get_plan_max_active_subs` | 1 | Direct key lookup |
//! | `list_subscriptions_by_subscriber` | up to MAX_SCAN_DEPTH | Linear scan; capped per call |
//!
//! **Index deserialization note**: `get_subscriptions_by_merchant` and
//! `get_subscriptions_by_token` read the entire index `Vec<u32>` from a single storage
//! key before slicing it. For merchants or tokens with very large index lists the
//! deserialization cost grows with the list length, even when only `limit` entries
//! are needed. Use [`get_merchant_subscription_count`] / [`get_token_subscription_count`]
//! to estimate index size before paginating.
//!
//! ## Key registry: DataKey variants used here
//! - `DataKey::Sub(u32)`
//! - `DataKey::MerchantSubs(Address)`
//! - `DataKey::TokenSubs(Address)`
//! - `DataKey::PlanMaxActive(u32)`
//! - `DataKey::NextId`

use crate::safe_math::{safe_mul, safe_sub};
use crate::subscription::next_charge_time;
use crate::types::{CapInfo, DataKey, Error, NextChargeInfo, Subscription, SubscriptionStatus};
use soroban_sdk::{contracttype, Address, Env, Symbol, Vec};

/// Maximum `limit` for [`get_subscriptions_by_merchant`] and [`get_subscriptions_by_token`]
/// (aligned with [`list_subscriptions_by_subscriber`]).
pub const MAX_SUBSCRIPTION_LIST_PAGE: u32 = 100;

/// Maximum number of subscription IDs scanned in a single
/// [`list_subscriptions_by_subscriber`] call.
///
/// Because `list_subscriptions_by_subscriber` performs a sequential ID scan (rather
/// than using a secondary index), an unbounded scan over a large contract is
/// proportionally expensive in storage reads. This constant caps the scan window
/// per call so that any single invocation reads at most `MAX_SCAN_DEPTH` IDs.
///
/// When the budget is exhausted before the requested page is full, the returned
/// `next_start_id` points to the first unscanned ID so callers can issue a
/// follow-up call to resume. Subscribers whose subscriptions are spread sparsely
/// over a large ID range may therefore need more round-trips to collect all IDs.
///
/// **Rationale**: 1 000 reads per call is the practical upper bound for a
/// sensible contract interaction; it keeps per-call cost predictable while still
/// allowing full enumeration via chaining.
pub const MAX_SCAN_DEPTH: u32 = 1_000;

pub fn get_subscription(env: &Env, subscription_id: u32) -> Result<Subscription, Error> {
    env.storage()
        .instance()
        .get(&DataKey::Sub(subscription_id))
        .ok_or(Error::NotFound)
}

pub fn estimate_topup_for_intervals(
    env: &Env,
    subscription_id: u32,
    num_intervals: u32,
) -> Result<i128, Error> {
    let sub = get_subscription(env, subscription_id)?;

    if num_intervals == 0 {
        return Ok(0);
    }

    let intervals_i128: i128 = num_intervals.into();
    let required = safe_mul(sub.amount, intervals_i128)?;

    let topup = if required <= sub.prepaid_balance {
        0
    } else {
        safe_sub(required, sub.prepaid_balance)?
    };
    Ok(topup)
}

/// Returns subscriptions for a merchant, paginated by offset into the merchant id index.
///
/// `limit` must be in `1..=MAX_SUBSCRIPTION_LIST_PAGE`. Ordering is stable index order (insertion order).
pub fn get_subscriptions_by_merchant(
    env: &Env,
    merchant: Address,
    start: u32,
    limit: u32,
) -> Result<Vec<Subscription>, Error> {
    if limit == 0 || limit > MAX_SUBSCRIPTION_LIST_PAGE {
        return Err(Error::InvalidInput);
    }
    let key = DataKey::MerchantSubs(merchant);
    let ids: Vec<u32> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));

    let len = ids.len();
    if start >= len {
        return Ok(Vec::new(env));
    }

    let end = if start + limit > len {
        len
    } else {
        start + limit
    };

    let mut result = Vec::new(env);
    let mut i = start;
    while i < end {
        let sub_id = ids.get(i).unwrap();
        if let Some(sub) = env.storage().instance().get::<_, Subscription>(&DataKey::Sub(sub_id)) {
            result.push_back(sub);
        }
        i += 1;
    }
    Ok(result)
}

/// Returns the number of subscriptions for a given merchant.
pub fn get_merchant_subscription_count(env: &Env, merchant: Address) -> u32 {
    let key = DataKey::MerchantSubs(merchant);
    let ids: Vec<u32> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));
    ids.len()
}

/// Number of subscription ids indexed for this token (length of the `token_subs` list).
pub fn get_token_subscription_count(env: &Env, token: Address) -> u32 {
    let key = DataKey::TokenSubs(token);
    let ids: Vec<u32> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));
    ids.len()
}

/// Returns subscriptions for a settlement token, paginated by offset into the token id index.
///
/// `limit` must be in `1..=MAX_SUBSCRIPTION_LIST_PAGE`. Ordering is stable index order (insertion order).
pub fn get_subscriptions_by_token(
    env: &Env,
    token: Address,
    start: u32,
    limit: u32,
) -> Result<Vec<Subscription>, Error> {
    if limit == 0 || limit > MAX_SUBSCRIPTION_LIST_PAGE {
        return Err(Error::InvalidInput);
    }
    let key = DataKey::TokenSubs(token);
    let ids: Vec<u32> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));
    let len = ids.len();
    if start >= len {
        return Ok(Vec::new(env));
    }
    let end = if start + limit > len {
        len
    } else {
        start + limit
    };
    let mut out = Vec::new(env);
    let mut i = start;
    while i < end {
        let id = ids.get(i).unwrap();
        if let Some(sub) = env.storage().instance().get::<_, Subscription>(&DataKey::Sub(id)) {
            out.push_back(sub);
        }
        i += 1;
    }
    Ok(out)
}

/// Returns full next charge information for a subscription.
pub fn get_next_charge_info(env: &Env, subscription_id: u32) -> Result<NextChargeInfo, Error> {
    let sub = get_subscription(env, subscription_id)?;
    Ok(compute_next_charge_info(env, &sub))
}

/// Computes the estimated next charge timestamp and status for a subscription.
pub fn compute_next_charge_info(env: &Env, subscription: &Subscription) -> NextChargeInfo {
    let next_charge_timestamp = subscription
        .last_payment_timestamp
        .saturating_add(subscription.interval_seconds);

    let is_charge_expected = match subscription.status {
        SubscriptionStatus::Active => true,
        SubscriptionStatus::GracePeriod => true,
        SubscriptionStatus::InsufficientBalance => false,
        SubscriptionStatus::Paused => false,
        SubscriptionStatus::Cancelled => false,
        SubscriptionStatus::Expired => false,
        SubscriptionStatus::Archived => false,
    };

    let reason = match subscription.status {
        SubscriptionStatus::Active => soroban_sdk::symbol_short!("active"),
        SubscriptionStatus::GracePeriod => soroban_sdk::symbol_short!("grace"),
        SubscriptionStatus::InsufficientBalance => soroban_sdk::symbol_short!("insuf_bal"),
        SubscriptionStatus::Paused => soroban_sdk::symbol_short!("paused"),
        SubscriptionStatus::Cancelled => soroban_sdk::symbol_short!("cancelled"),
        SubscriptionStatus::Expired => soroban_sdk::symbol_short!("expired"),
        SubscriptionStatus::Archived => soroban_sdk::symbol_short!("archived"),
        SubscriptionStatus::Active => Symbol::new(env, "active"),
        SubscriptionStatus::GracePeriod => Symbol::new(env, "grace"),
        SubscriptionStatus::InsufficientBalance => Symbol::new(env, "insuf_bal"),
        SubscriptionStatus::Paused => Symbol::new(env, "paused"),
        SubscriptionStatus::Cancelled => Symbol::new(env, "cancelled"),
        SubscriptionStatus::Expired => Symbol::new(env, "expired"),
        SubscriptionStatus::Archived => Symbol::new(env, "archived"),
    };

    NextChargeInfo {
        next_charge_timestamp,
        is_charge_expected,
        status: subscription.status,
        reason,
        amount: subscription.amount,
        token: subscription.token.clone(),
    }
}

/// Returns lifetime cap information for a subscription.
pub fn get_cap_info(env: &Env, subscription_id: u32) -> Result<CapInfo, Error> {
    let sub = get_subscription(env, subscription_id)?;

    let (remaining_cap, cap_reached) = match sub.lifetime_cap {
        Some(cap) => {
            let remaining = cap.saturating_sub(sub.lifetime_charged).max(0);
            (Some(remaining), sub.lifetime_charged >= cap)
        }
        None => (None, false),
    };

    Ok(CapInfo {
        lifetime_cap: sub.lifetime_cap,
        lifetime_charged: sub.lifetime_charged,
        remaining_cap,
        cap_reached,
    })
}

/// Returns the configured max-active-subscriptions limit for a plan template.
///
/// A return value of `0` means no limit is enforced for that plan.
/// The plan must exist; returns `0` if no limit has been explicitly set.
pub fn get_plan_max_active_subs(env: &Env, plan_template_id: u32) -> u32 {
    let key = DataKey::PlanMaxActive(plan_template_id);
    env.storage().instance().get(&key).unwrap_or(0)
}

/// Result of a paginated query for subscriptions by subscriber.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubscriptionsPage {
    pub subscription_ids: Vec<u32>,
    pub next_start_id: Option<u32>,
}

/// Get all subscription IDs for a given subscriber with pagination support.
///
/// ## Complexity
///
/// O(min(`MAX_SCAN_DEPTH`, `next_id - start_from_id`)) storage reads per call.
/// At most [`MAX_SCAN_DEPTH`] IDs are inspected; if the scan budget is exhausted
/// before `limit` matching IDs are found, `next_start_id` is set to the first
/// unscanned position so the caller can resume with another call.
///
/// ## Pagination
///
/// - `start_from_id`: inclusive lower bound on IDs to scan (use `0` for first page).
/// - `limit`: how many matching IDs to return; must be in `1..=MAX_SUBSCRIPTION_LIST_PAGE`.
/// - `next_start_id`: when `Some(id)`, pass it as `start_from_id` to fetch the next
///   page.  `None` means there are no more IDs to scan in the current budget window.
///
/// ## Security note
///
/// The scan cap prevents a single transaction from performing an unbounded number
/// of storage reads under adversarial conditions (e.g. an account with millions of
/// subscriptions).  The cap does **not** affect correctness — it only splits work
/// across more calls.
pub fn list_subscriptions_by_subscriber(
    env: &Env,
    subscriber: Address,
    start_from_id: u32,
    limit: u32,
) -> Result<SubscriptionsPage, Error> {
    if limit == 0 || limit > MAX_SUBSCRIPTION_LIST_PAGE {
        return Err(Error::InvalidInput);
    }

    let next_id: u32 = env.storage().instance().get(&DataKey::NextId).unwrap_or(0);

    // Cap the scan window to MAX_SCAN_DEPTH IDs per call.
    // If the budget is exhausted before `limit` matches are found, `next_start_id`
    // is set to `scan_end` so the caller can resume from exactly where we stopped.
    let scan_end: u32 = start_from_id.saturating_add(MAX_SCAN_DEPTH).min(next_id);

    let mut subscription_ids = Vec::new(env);
    let mut next_start_id: Option<u32> = None;

    let mut id = start_from_id;
    while id < scan_end {
        if let Some(sub) = env.storage().instance().get::<_, Subscription>(&DataKey::Sub(id)) {
            if sub.subscriber == subscriber {
                if subscription_ids.len() < limit {
                    subscription_ids.push_back(id);
                } else {
                    // Page is full; resume from this ID on the next call.
                    next_start_id = Some(id);
                    return Ok(SubscriptionsPage {
                        subscription_ids,
                        next_start_id,
                    });
                }
            }
        }
        id += 1;
    }

    // Scan budget exhausted.  If more IDs remain beyond the window, signal the
    // caller to resume from `scan_end` (even if the current page is not full).
    if scan_end < next_id {
        next_start_id = Some(scan_end);
    }

    Ok(SubscriptionsPage {
        subscription_ids,
        next_start_id,
    })
}

// ── Reconciliation Queries ───────────────────────────────────────────────────

use crate::types::{
    PrepaidQueryRequest, PrepaidQueryResult, ReconciliationProof, ReconciliationSummaryPage,
    TokenEarnings, TokenLiabilities,
};

/// Maximum number of subscriptions to scan in a single prepaid balance query.
///
/// This bounds compute to prevent excessive gas usage when aggregating across
/// many subscriptions. Callers should chain paginated calls to build complete totals.
pub const MAX_PREPAID_SCAN_DEPTH: u32 = 500;

/// Maximum number of token summaries to return in a single reconciliation summary call.
pub const MAX_TOKEN_SUMMARIES_PER_PAGE: u32 = 50;

/// Returns complete reconciliation data for a single token.
///
/// This computes the accounting equation:
/// `contract_token_balance = total_prepaid + total_merchant_liabilities + recoverable`
///
/// # Arguments
///
/// * `token` — The settlement token to audit.
///
/// # Returns
///
/// A [`TokenLiabilities`] struct containing all computed values and a balance check.
///
/// # Complexity
///
/// This function scans all subscriptions and all merchants, which can be expensive
/// for large contracts. For bounded compute, use [`query_prepaid_balances_paginated`]
/// and aggregate off-chain.
pub fn get_token_reconciliation(env: &Env, token: Address) -> TokenLiabilities {
    let token_client = soroban_sdk::token::Client::new(env, &token);
    let contract_balance = token_client.balance(&env.current_contract_address());

    // Compute total prepaid across all subscriptions
    let total_prepaid = compute_total_prepaid(env, &token);

    // Compute total merchant liabilities
    let total_merchant_liabilities = compute_total_merchant_liabilities(env, &token);

    // Recoverable is the difference between contract balance and accounted funds
    let accounted = total_prepaid
        .checked_add(total_merchant_liabilities)
        .unwrap_or(0i128);
    let recoverable_amount = contract_balance.saturating_sub(accounted).max(0);

    let computed_total = total_prepaid
        .checked_add(total_merchant_liabilities)
        .unwrap_or(0i128)
        .checked_add(recoverable_amount)
        .unwrap_or(0i128);

    let is_balanced = contract_balance == computed_total;

    TokenLiabilities {
        token,
        total_prepaid,
        total_merchant_liabilities,
        recoverable_amount,
        contract_balance,
        computed_total,
        is_balanced,
    }
}

/// Returns paginated reconciliation summaries for all accepted tokens.
///
/// # Arguments
///
/// * `start_token_index` — Index into the accepted tokens list to start from.
/// * `limit` — Maximum number of token summaries to return (max 50).
///
/// # Returns
///
/// A [`ReconciliationSummaryPage`] with token summaries and pagination cursor.
pub fn get_contract_reconciliation_summary(
    env: &Env,
    start_token_index: u32,
    limit: u32,
) -> ReconciliationSummaryPage {
    let limit = limit.min(MAX_TOKEN_SUMMARIES_PER_PAGE);

    // Get all accepted tokens
    let accepted_tokens: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::AcceptedTokens)
        .unwrap_or(Vec::new(env));

    let total_tokens = accepted_tokens.len();

    if start_token_index >= total_tokens {
        return ReconciliationSummaryPage {
            token_summaries: Vec::new(env),
            next_token_index: None,
        };
    }

    let end_index = (start_token_index + limit).min(total_tokens);
    let mut token_summaries = Vec::new(env);

    for i in start_token_index..end_index {
        if let Some(token) = accepted_tokens.get(i) {
            let summary = get_token_reconciliation(env, token);
            token_summaries.push_back(summary);
        }
    }

    let next_token_index = if end_index < total_tokens {
        Some(end_index)
    } else {
        None
    };

    ReconciliationSummaryPage {
        token_summaries,
        next_token_index,
    }
}

/// Generates an auditable proof for off-chain reconciliation verification.
///
/// This function creates a snapshot with all necessary data for auditors to
/// independently validate the accounting equation without needing full
/// contract state access.
///
/// # Arguments
///
/// * `token` — The settlement token to generate the proof for.
///
/// # Returns
///
/// A [`ReconciliationProof`] containing all validation data.
///
/// # Security
///
/// This is a read-only function that cannot modify state. The proof is generated
/// at the current ledger state and includes the ledger sequence for temporal
/// anchoring.
pub fn generate_reconciliation_proof(env: &Env, token: Address) -> ReconciliationProof {
    let token_client = soroban_sdk::token::Client::new(env, &token);
    let contract_balance = token_client.balance(&env.current_contract_address());

    // Get prepaid total with count
    let (total_prepaid, sub_count) = compute_total_prepaid_with_count(env, &token);

    // Get merchant liabilities with count
    let (total_merchant_liabilities, merchant_count) =
        compute_total_merchant_liabilities_with_count(env, &token);

    // Compute recoverable
    let accounted = total_prepaid
        .checked_add(total_merchant_liabilities)
        .unwrap_or(0i128);
    let computed_recoverable = contract_balance.saturating_sub(accounted).max(0);

    // Validate accounting equation
    let computed_total = accounted.checked_add(computed_recoverable).unwrap_or(0i128);
    let is_valid = contract_balance == computed_total;

    ReconciliationProof {
        timestamp: env.ledger().timestamp(),
        ledger_sequence: env.ledger().sequence(),
        token,
        contract_balance,
        total_prepaid,
        total_merchant_liabilities,
        computed_recoverable,
        subscription_count: sub_count,
        merchant_count,
        is_valid,
    }
}

/// Returns paginated prepaid balance aggregation for a token.
///
/// This provides bounded compute for auditors to incrementally build the total
/// prepaid balance without iterating unbounded subscription sets in a single call.
///
/// # Arguments
///
/// * `request` — A [`PrepaidQueryRequest`] specifying token, start ID, and scan limit.
///
/// # Returns
///
/// A [`PrepaidQueryResult`] with partial totals and pagination info.
///
/// # Example
///
/// To compute the full prepaid total off-chain:
/// 1. Call with `start_subscription_id = 0`
/// 2. Sum `partial_total` from each response
/// 3. Use `next_start_id` for subsequent calls until `has_more` is false
pub fn query_prepaid_balances_paginated(
    env: &Env,
    request: PrepaidQueryRequest,
) -> PrepaidQueryResult {
    let scan_limit = request.scan_limit.min(MAX_PREPAID_SCAN_DEPTH);
    let next_id: u32 = env
        .storage()
        .instance()
        .get(&DataKey::NextId)
        .unwrap_or(0u32);

    if next_id == 0 || request.start_subscription_id >= next_id {
        return PrepaidQueryResult {
            token: request.token.clone(),
            partial_total: 0,
            subscriptions_count: 0,
            next_start_id: None,
            has_more: false,
        };
    }

    let scan_end = (request.start_subscription_id + scan_limit).min(next_id);
    let mut partial_total: i128 = 0;
    let mut subscriptions_count: u32 = 0;

    for id in request.start_subscription_id..scan_end {
        if let Some(sub) = env
            .storage()
            .instance()
            .get::<_, Subscription>(&DataKey::Sub(id))
        {
            if sub.token == request.token && sub.prepaid_balance > 0 {
                partial_total = partial_total.saturating_add(sub.prepaid_balance);
                subscriptions_count = subscriptions_count.saturating_add(1);
            }
        }
    }

    let has_more = scan_end < next_id;
    let next_start_id = if has_more { Some(scan_end) } else { None };

    PrepaidQueryResult {
        token: request.token,
        partial_total,
        subscriptions_count,
        next_start_id,
        has_more,
    }
}

// ── Internal helpers for reconciliation ────────────────────────────────────

fn compute_total_prepaid(env: &Env, token: &Address) -> i128 {
    let next_id: u32 = env
        .storage()
        .instance()
        .get(&DataKey::NextId)
        .unwrap_or(0u32);

    let mut total: i128 = 0;
    for id in 0..next_id {
        if let Some(sub) = env
            .storage()
            .instance()
            .get::<_, Subscription>(&DataKey::Sub(id))
        {
            if sub.token == *token {
                total = total.saturating_add(sub.prepaid_balance);
            }
        }
    }
    total
}

fn compute_total_prepaid_with_count(env: &Env, token: &Address) -> (i128, u32) {
    let next_id: u32 = env
        .storage()
        .instance()
        .get(&DataKey::NextId)
        .unwrap_or(0u32);

    let mut total: i128 = 0;
    let mut count: u32 = 0;
    for id in 0..next_id {
        if let Some(sub) = env
            .storage()
            .instance()
            .get::<_, Subscription>(&DataKey::Sub(id))
        {
            if sub.token == *token && sub.prepaid_balance > 0 {
                total = total.saturating_add(sub.prepaid_balance);
                count = count.saturating_add(1);
            }
        }
    }
    (total, count)
}

fn compute_total_merchant_liabilities(env: &Env, token: &Address) -> i128 {
    // Get all merchants by iterating through MerchantTokens index
    // This is a simplified approach - for a complete solution, we'd need
    // a global merchant index. For now, we use the TotalAccounted value
    // as a proxy for total merchant liabilities.
    crate::accounting::get_total_accounted(env, token)
}

fn compute_total_merchant_liabilities_with_count(env: &Env, token: &Address) -> (i128, u32) {
    // Since we don't have a global merchant index, we count merchants
    // that have earnings records for this token
    let total = crate::accounting::get_total_accounted(env, token);

    // Count merchants with earnings for this token by checking MerchantTokens
    // This is an approximation - counting all merchants with token earnings
    let mut merchant_count: u32 = 0;

    // We need to scan for merchants with earnings for this token
    // Since there's no global merchant index, we use the fact that
    // TotalAccounted > 0 implies at least one merchant has earnings
    if total > 0 {
        // At minimum, there's activity for this token
        merchant_count = 1; // Conservative estimate
    }

    (total, merchant_count)
}
