//! Read-only entrypoints and helpers: get_subscription, estimate_topup.
//!
//! **PRs that only add or change read-only/query behavior should edit this file only.**

use crate::types::{Error, Subscription};
use soroban_sdk::Env;

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
