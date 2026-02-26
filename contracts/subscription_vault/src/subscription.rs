//! Subscription lifecycle: create, deposit.
//!
//! See `docs/subscription_lifecycle.md` for the full lifecycle and state machine.
//!
//! **PRs that only change subscription lifecycle or billing should edit this file only.**

#![allow(dead_code)]

use crate::queries::get_subscription;
use crate::safe_math::{safe_add_balance, validate_non_negative};
use crate::state_machine::validate_status_transition;
use crate::types::{DataKey, Error, PlanTemplate, Subscription, SubscriptionStatus};
use soroban_sdk::{Address, Env, Symbol, Vec};

pub fn next_id(env: &Env) -> u32 {
    let key = Symbol::new(env, "next_id");
    let storage = env.storage().instance();
    let id: u32 = storage.get(&key).unwrap_or(0);
    storage.set(&key, &(id + 1));
    id
}

pub fn next_plan_id(env: &Env) -> u32 {
    let key = Symbol::new(env, "next_plan_id");
    let id: u32 = env.storage().instance().get(&key).unwrap_or(0);
    env.storage().instance().set(&key, &(id + 1));
    id
}

pub fn get_plan_template(env: &Env, plan_template_id: u32) -> Result<PlanTemplate, Error> {
    let key = (Symbol::new(env, "plan"), plan_template_id);
    env.storage().instance().get(&key).ok_or(Error::NotFound)
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
    validate_non_negative(amount)?;
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
    validate_non_negative(amount)?;

    let mut sub = get_subscription(env, subscription_id)?;
    sub.prepaid_balance = safe_add_balance(sub.prepaid_balance, amount)?;
    let token_addr: Address = env
        .storage()
        .instance()
        .get(&Symbol::new(env, "token"))
        .ok_or(Error::NotInitialized)?;
    let token_client = soroban_sdk::token::Client::new(env, &token_addr);

    token_client.transfer(&subscriber, &env.current_contract_address(), &amount);
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

    if authorizer != sub.subscriber && authorizer != sub.merchant {
        return Err(Error::Forbidden);
    }

    validate_status_transition(&sub.status, &SubscriptionStatus::Cancelled)?;
    sub.status = SubscriptionStatus::Cancelled;

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
    if sub.status != SubscriptionStatus::Active && sub.status != SubscriptionStatus::Paused {
        return Err(Error::NotActive);
    }
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }
    if sub.prepaid_balance < amount {
        return Err(Error::InsufficientPrepaidBalance);
    }

    sub.prepaid_balance = sub
        .prepaid_balance
        .checked_sub(amount)
        .ok_or(Error::Overflow)?;

    env.storage().instance().set(&subscription_id, &sub);

    Ok(())
}

pub fn do_withdraw_subscriber_funds(
    env: &Env,
    subscription_id: u32,
    subscriber: Address,
) -> Result<(), Error> {
    subscriber.require_auth();

    let mut sub = get_subscription(env, subscription_id)?;

    if subscriber != sub.subscriber {
        return Err(Error::Forbidden);
    }

    if sub.status != SubscriptionStatus::Cancelled {
        return Err(Error::InvalidStatusTransition); // Or Unauthorized/InvalidState
    }

    let amount_to_refund = sub.prepaid_balance;
    if amount_to_refund > 0 {
        sub.prepaid_balance = 0;
        env.storage().instance().set(&subscription_id, &sub);

        let token_addr: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(env, "token"))
            .ok_or(Error::NotInitialized)?;
        let token_client = soroban_sdk::token::Client::new(env, &token_addr);

        token_client.transfer(
            &env.current_contract_address(),
            &subscriber,
            &amount_to_refund,
        );
    }

    Ok(())
}

pub fn do_create_plan_template(
    env: &Env,
    merchant: Address,
    amount: i128,
    interval_seconds: u64,
    usage_enabled: bool,
) -> Result<u32, Error> {
    merchant.require_auth();

    let plan = PlanTemplate {
        merchant,
        amount,
        interval_seconds,
        usage_enabled,
    };

    let plan_id = next_plan_id(env);
    let key = (Symbol::new(env, "plan"), plan_id);
    env.storage().instance().set(&key, &plan);

    Ok(plan_id)
}

pub fn do_create_subscription_from_plan(
    env: &Env,
    subscriber: Address,
    plan_template_id: u32,
) -> Result<u32, Error> {
    subscriber.require_auth();

    let plan = get_plan_template(env, plan_template_id)?;

    let sub = Subscription {
        subscriber: subscriber.clone(),
        merchant: plan.merchant,
        amount: plan.amount,
        interval_seconds: plan.interval_seconds,
        last_payment_timestamp: env.ledger().timestamp(),
        status: SubscriptionStatus::Active,
        prepaid_balance: 0i128,
        usage_enabled: plan.usage_enabled,
    };

    let id = next_id(env);
    env.storage().instance().set(&id, &sub);
    Ok(id)
}
