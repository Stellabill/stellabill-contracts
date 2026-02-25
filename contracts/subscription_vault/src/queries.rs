//! Read-only entrypoints and helpers: get_subscription, estimate_topup, list_subscriptions_by_subscriber.
//!
//! **PRs that only add or change read-only/query behavior should edit this file only.**

use crate::types::{DataKey, Error, NextChargeInfo, Subscription, SubscriptionStatus};
use soroban_sdk::{contracttype, Address, Env, Symbol, Vec};

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
    let required = sub
        .amount
        .checked_mul(intervals_i128)
        .ok_or(Error::Overflow)?;

    let topup = required
        .checked_sub(sub.prepaid_balance)
        .unwrap_or(0)
        .max(0);
    Ok(topup)
}

/// Returns subscriptions for a merchant, paginated by offset.
///
/// * `merchant` – the merchant address to query.
/// * `start`    – 0-based offset into the merchant's subscription list.
/// * `limit`    – maximum number of subscriptions to return.
///
/// Results are ordered chronologically (insertion order).
/// Returns an empty `Vec` when the merchant has no subscriptions or
/// `start` is beyond the end of the list.
pub fn get_subscriptions_by_merchant(
    env: &Env,
    merchant: Address,
    start: u32,
    limit: u32,
) -> Vec<Subscription> {
    let key = DataKey::MerchantSubs(merchant);
    let ids: Vec<u32> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));

    let len = ids.len();
    if start >= len || limit == 0 {
        return Vec::new(env);
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
    result
}

/// Returns the number of subscriptions for a given merchant.
///
/// Useful for dashboards and pagination metadata.
pub fn get_merchant_subscription_count(env: &Env, merchant: Address) -> u32 {
    let key = DataKey::MerchantSubs(merchant);
    let ids: Vec<u32> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));
    ids.len()
}

/// Computes the estimated next charge timestamp for a subscription.
///
/// This is a readonly helper that does not mutate contract state. It provides
/// information for off-chain scheduling systems and UX displays.
pub fn compute_next_charge_info(subscription: &Subscription) -> NextChargeInfo {
    let next_charge_timestamp = subscription
        .last_payment_timestamp
        .saturating_add(subscription.interval_seconds);

    let is_charge_expected = match subscription.status {
        SubscriptionStatus::Active => true,
        SubscriptionStatus::InsufficientBalance => true,
        SubscriptionStatus::GracePeriod => true,
        SubscriptionStatus::Paused => false,
        SubscriptionStatus::Cancelled => false,
    };

    NextChargeInfo {
        next_charge_timestamp,
        is_charge_expected,
    }
}

/// Result of a paginated query for subscriptions by subscriber.
/// Contains the subscription IDs and metadata for pagination.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionsPage {
    /// List of subscription IDs owned by the subscriber (ordered by ID).
    pub subscription_ids: Vec<u32>,
    /// Whether there are more subscriptions beyond this page.
    pub has_next: bool,
}

/// Get all subscription IDs for a given subscriber with pagination support.
///
/// This function retrieves subscription IDs owned by a subscriber in a paginated manner.
/// Subscriptions are returned in order by ID (ascending) for predictable iteration.
///
/// # Arguments
/// - `env`: The Soroban environment
/// - `subscriber`: The address of the subscriber to query
/// - `start_from_id`: Inclusive lower bound for pagination (use 0 for the first page).
///   Only subscription IDs >= this value will be returned.
/// - `limit`: Maximum number of subscription IDs to return (recommended: 10-100 for efficiency).
///   Must be greater than 0.
///
/// # Returns
/// A `SubscriptionsPage` containing:
/// - `subscription_ids`: Vec of subscription IDs (sorted ascending)
/// - `has_next`: True if there are more subscriptions after the last returned ID
///
/// # Performance Notes
/// - Time complexity: O(n) where n = total number of subscriptions in the contract
/// - Space complexity: O(limit)
/// - On-chain storage usage is minimal (only subscription objects are stored)
/// - Suitable for off-chain indexers and UI pagination
///
/// # Pagination Example
/// ```ignore
/// // Get first page (subscriptions with ID >= 0)
/// let page1 = list_subscriptions_by_subscriber(env, subscriber, 0, 10)?;
///
/// // Get next page: pass last_returned_id + 1 as start_from_id
/// if page1.has_next {
///     let last_id = page1.subscription_ids.last().unwrap();
///     let page2 = list_subscriptions_by_subscriber(env, subscriber, last_id + 1, 10)?;
/// }
/// ```
pub fn list_subscriptions_by_subscriber(
    env: &Env,
    subscriber: Address,
    start_from_id: u32,
    limit: u32,
) -> Result<SubscriptionsPage, Error> {
    if limit == 0 {
        return Err(Error::NotFound);
    }

    // Get the next_id counter to determine the range of valid subscription IDs
    let next_id_key = Symbol::new(env, "next_id");
    let next_id: u32 = env.storage().instance().get(&next_id_key).unwrap_or(0);

    let mut subscription_ids = Vec::new(env);
    let mut count = 0u32;
    let mut last_found_id = start_from_id;

    // Iterate through all subscription IDs from start_from_id (inclusive) and filter by subscriber
    for id in start_from_id..next_id {
        match env.storage().instance().get::<u32, Subscription>(&id) {
            Some(sub) => {
                if sub.subscriber == subscriber {
                    subscription_ids.push_back(id);
                    count += 1;
                    last_found_id = id;
                    if count >= limit {
                        break;
                    }
                }
            }
            None => {
                // Subscription was deleted or ID skipped; continue to next
            }
        }
    }

    // Determine if there are more subscriptions by checking beyond the last found
    let has_next = if count >= limit {
        // We hit the limit; check if there is at least one more subscriber match
        let mut found_next = false;
        for id in (last_found_id + 1)..next_id {
            if let Some(sub) = env.storage().instance().get::<u32, Subscription>(&id) {
                if sub.subscriber == subscriber {
                    found_next = true;
                    break;
                }
            }
        }
        found_next
    } else {
        false
    };

    Ok(SubscriptionsPage {
        subscription_ids,
        has_next,
    })
}
