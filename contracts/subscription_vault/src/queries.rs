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

use crate::safe_math::{safe_mul, safe_sub};
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
        .get(&subscription_id)
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

    let topup = safe_sub(required, sub.prepaid_balance).unwrap_or(0).max(0);
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
        if let Some(sub) = env.storage().instance().get::<u32, Subscription>(&sub_id) {
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
    let key = (Symbol::new(env, "token_subs"), token);
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
    let key = (Symbol::new(env, "token_subs"), token);
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
        if let Some(sub) = env.storage().instance().get::<u32, Subscription>(&id) {
            out.push_back(sub);
        }
        i += 1;
    }
    Ok(out)
}

/// Computes the estimated next charge timestamp for a subscription.
pub fn compute_next_charge_info(subscription: &Subscription) -> NextChargeInfo {
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

    NextChargeInfo {
        next_charge_timestamp,
        is_charge_expected,
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
    let key = (Symbol::new(env, "plan_max_active"), plan_template_id);
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

    let next_id_key = Symbol::new(env, "next_id");
    let next_id: u32 = env.storage().instance().get(&next_id_key).unwrap_or(0);

    // Cap the scan window to MAX_SCAN_DEPTH IDs per call.
    // If the budget is exhausted before `limit` matches are found, `next_start_id`
    // is set to `scan_end` so the caller can resume from exactly where we stopped.
    let scan_end: u32 = start_from_id
        .saturating_add(MAX_SCAN_DEPTH)
        .min(next_id);

    let mut subscription_ids = Vec::new(env);
    let mut next_start_id: Option<u32> = None;

    let mut id = start_from_id;
    while id < scan_end {
        if let Some(sub) = env.storage().instance().get::<u32, Subscription>(&id) {
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
