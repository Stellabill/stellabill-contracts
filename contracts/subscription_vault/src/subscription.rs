//! Subscription lifecycle: create, deposit, charge, cancel, pause, resume.
//!
//! **PRs that only change subscription lifecycle or billing should edit this file only.**

use crate::admin::require_admin;
use crate::charge_core::charge_one;
use crate::queries::get_subscription;
use crate::types::{Error, OneOffChargedEvent, Subscription, SubscriptionStatus};
use soroban_sdk::{symbol_short, Address, Env, Symbol};

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
    let sub = Subscription {
        subscriber: subscriber.clone(),
        merchant,
        amount,
        interval_seconds,
        last_payment_timestamp: env.ledger().timestamp(),
        status: SubscriptionStatus::Active,
        prepaid_balance: 0i128,
        usage_enabled,
    };
    let id = next_id(env);
    env.storage().instance().set(&id, &sub);
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
    Ok(())
}

/// Charges one subscription for the current billing interval.
///
/// # Idempotency
///
/// Pass `idempotency_key` (e.g. from your billing engine) to make retries safe: the first call
/// with a given key performs the charge; repeated calls with the same key return `Ok(())` without
/// double-debiting. If `None`, only period-based replay protection applies (one charge per
/// billing period per subscription).
pub fn do_charge_subscription(
    env: &Env,
    subscription_id: u32,
    idempotency_key: Option<soroban_sdk::BytesN<32>>,
) -> Result<(), Error> {
    let admin = require_admin(env)?;
    admin.require_auth();
    charge_one(env, subscription_id, idempotency_key)
}

/// Merchant-initiated one-off charge: debits `amount` from the subscription's prepaid balance.
/// Requires merchant auth; the subscription's merchant must match the caller. Subscription must be
/// Active or Paused. Amount must be positive and not exceed prepaid_balance.
pub fn do_charge_one_off(
    env: &Env,
    subscription_id: u32,
    merchant: Address,
    amount: i128,
) -> Result<(), Error> {
    merchant.require_auth();

    let mut sub = get_subscription(env, subscription_id)?;
    if sub.merchant != merchant {
        return Err(Error::Unauthorized);
    }
    match sub.status {
        SubscriptionStatus::Active | SubscriptionStatus::Paused => {}
        _ => return Err(Error::NotActive),
    }
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }
    if sub.prepaid_balance < amount {
        return Err(Error::InsufficientBalance);
    }

    sub.prepaid_balance = sub
        .prepaid_balance
        .checked_sub(amount)
        .ok_or(Error::Overflow)?;
    env.storage().instance().set(&subscription_id, &sub);

    env.events().publish(
        (symbol_short!("oneoff_ch"),),
        OneOffChargedEvent {
            subscription_id,
            merchant,
            amount,
        },
    );

    Ok(())
}
