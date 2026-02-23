//! Read-only entrypoints and helpers: get_subscription, estimate_topup.
//!
//! **PRs that only add or change read-only/query behavior should edit this file only.**

use crate::types::{DataKey, Error, NextChargeInfo, Subscription, SubscriptionStatus};
use soroban_sdk::{Address, Env, Vec};

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
        SubscriptionStatus::Paused => false,
        SubscriptionStatus::Cancelled => false,
    };

    NextChargeInfo {
        next_charge_timestamp,
        is_charge_expected,
    }
}
