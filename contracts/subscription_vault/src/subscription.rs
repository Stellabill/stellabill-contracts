//! Subscription lifecycle: create, deposit.
//!
//! **PRs that only change subscription lifecycle or billing should edit this file only.**

use crate::queries::get_subscription;
use crate::state_machine::validate_status_transition;
use crate::types::{DataKey, Error, Subscription, SubscriptionStatus};
use soroban_sdk::{Address, Env, Symbol, Vec};

pub fn next_id(env: &Env) -> u32 {
    let key = Symbol::new(env, "next_id");
    let id: u32 = env.storage().instance().get(&key).unwrap_or(0);
    env.storage().instance().set(&key, &(id + 1));
    id
}

pub fn do_create_subscription(
    env: &Env,
    subscriber: Address,
    merchant: Address,
    amount: i128,
    interval_seconds: u64,
    usage_enabled: bool,
) -> Result<u32, Error> {
    subscriber.require_auth();
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }
    if interval_seconds == 0 && !usage_enabled {
        return Err(Error::InvalidInterval);
    }
    let sub = Subscription {
        subscriber: subscriber.clone(),
        merchant: merchant.clone(),
        amount,
        interval_seconds,
        last_payment_timestamp: env.ledger().timestamp(),
        status: SubscriptionStatus::Active,
        prepaid_balance: 0i128,
        usage_enabled,
    };
    let id = next_id(env);
    env.storage().instance().set(&id, &sub);

    // Maintain merchant → subscription-ID index
    let key = DataKey::MerchantSubs(sub.merchant.clone());
    let mut ids: Vec<u32> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));
    ids.push_back(id);
    env.storage().instance().set(&key, &ids);

    Ok(id)
}

pub fn do_deposit_funds(
    env: &Env,
    subscription_id: u32,
    subscriber: Address,
    amount: i128,
) -> Result<(), Error> {
    subscriber.require_auth();

    let min_topup: i128 = crate::admin::get_min_topup(env)?;
    if amount < min_topup {
        return Err(Error::BelowMinimumTopup);
    }

    let mut sub = get_subscription(env, subscription_id)?;
    sub.prepaid_balance = sub
        .prepaid_balance
        .checked_add(amount)
        .ok_or(Error::Overflow)?;
    env.storage().instance().set(&subscription_id, &sub);
    env.events().publish(
        (Symbol::new(env, "deposited"), subscription_id),
        (subscriber, amount, sub.prepaid_balance),
    );
    Ok(())
}

pub fn do_cancel_subscription(
    env: &Env,
    subscription_id: u32,
    authorizer: Address,
) -> Result<(), Error> {
    authorizer.require_auth();

    let mut sub = get_subscription(env, subscription_id)?;
    validate_status_transition(&sub.status, &SubscriptionStatus::Cancelled)?;
    sub.status = SubscriptionStatus::Cancelled;

    // TODO: allow withdraw of prepaid_balance
    env.storage().instance().set(&subscription_id, &sub);
    Ok(())
}

pub fn do_pause_subscription(
    env: &Env,
    subscription_id: u32,
    authorizer: Address,
) -> Result<(), Error> {
    authorizer.require_auth();

    let mut sub = get_subscription(env, subscription_id)?;
    validate_status_transition(&sub.status, &SubscriptionStatus::Paused)?;
    sub.status = SubscriptionStatus::Paused;

    env.storage().instance().set(&subscription_id, &sub);
    Ok(())
}

pub fn do_resume_subscription(
    env: &Env,
    subscription_id: u32,
    authorizer: Address,
) -> Result<(), Error> {
    authorizer.require_auth();

    let mut sub = get_subscription(env, subscription_id)?;
    validate_status_transition(&sub.status, &SubscriptionStatus::Active)?;
    sub.status = SubscriptionStatus::Active;

    env.storage().instance().set(&subscription_id, &sub);
    Ok(())
}
