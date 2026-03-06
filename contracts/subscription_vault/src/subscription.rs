//! Subscription lifecycle: create, deposit, withdraw, cancel.
//!
//! See `docs/subscription_lifecycle.md` for the full lifecycle and state machine.
//!
//! **PRs that only change subscription lifecycle or billing should edit this file only.**
//!
//! # Reentrancy Protection
//!
//! This module contains two critical external calls to the token contract:
//! - `do_deposit_funds`: transfers tokens FROM subscriber TO contract
//! - `do_withdraw_subscriber_funds`: transfers tokens FROM contract TO subscriber
//!
//! Both functions follow the **Checks-Effects-Interactions (CEI)** pattern:
//! 1. **Checks**: Validate inputs and authorization
//! 2. **Effects**: Update internal contract state (prepaid_balance) in storage
//! 3. **Interactions**: Call token.transfer() AFTER state is persisted
//!
//! This ordering ensures that even if the token contract calls back into our contract,
//! the contract state will already be consistent and the attacker cannot exploit the
//! temporal inconsistency.
//!
//! See `docs/reentrancy.md` for full details on reentrancy threats and mitigations.

use crate::queries::get_subscription;
use crate::safe_math::{safe_add_balance, validate_non_negative};
use crate::state_machine::validate_status_transition;
use crate::types::{DataKey, Error, PlanTemplate, Subscription, SubscriptionStatus};
use soroban_sdk::{symbol_short, Address, Env, Symbol, Vec};

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
    lifetime_cap: Option<i128>,
) -> Result<u32, Error> {
    subscriber.require_auth();

    // Blocklist check: prevent blocklisted subscribers from creating subscriptions
    if crate::blocklist::is_blocklisted(env, &subscriber) {
        return Err(Error::SubscriberBlocklisted);
    }

    validate_non_negative(amount)?;
    if interval_seconds == 0 {
        return Err(Error::InvalidInput);
    }

    // Validate lifetime_cap if provided
    if let Some(cap) = lifetime_cap {
        if cap <= 0 {
            return Err(Error::InvalidAmount);
        }
    }

    let now = env.ledger().timestamp();
    let sub = Subscription {
        subscriber: subscriber.clone(),
        merchant: merchant.clone(),
        amount,
        interval_seconds,
        last_payment_timestamp: now,
        status: SubscriptionStatus::Active,
        prepaid_balance: 0i128,
        usage_enabled,
        expiration: None,
        billing_anchor_timestamp: now,
        current_period_index: 0,
        current_period_usage_units: 0,
        usage_cap_units: None,
        usage_rate_limit_max_calls: None,
        usage_rate_window_secs: 0,
        lifetime_cap,
        lifetime_charged: 0i128,
    };

    // Allocate ID with overflow / limit guard.
    let key = Symbol::new(env, "next_id");
    let id: u32 = env.storage().instance().get(&key).unwrap_or(0);
    if id == crate::MAX_SUBSCRIPTION_ID {
        return Err(Error::SubscriptionLimitReached);
    }
    env.storage().instance().set(&key, &(id + 1));

    env.storage().instance().set(&id, &sub);

    // Maintain merchant → subscription-ID index
    let merchant_key = DataKey::MerchantSubs(sub.merchant.clone());
    let mut ids: Vec<u32> = env
        .storage()
        .instance()
        .get(&merchant_key)
        .unwrap_or(Vec::new(env));
    ids.push_back(id);
    env.storage().instance().set(&merchant_key, &ids);

    env.events().publish(
        (symbol_short!("created"), id),
        (
            subscriber.clone(),
            merchant.clone(),
            amount,
            interval_seconds,
            lifetime_cap,
        ),
    );

    Ok(id)
}

pub fn do_deposit_funds(
    env: &Env,
    subscription_id: u32,
    subscriber: Address,
    amount: i128,
) -> Result<(), Error> {
    subscriber.require_auth();

    // Blocklist check: prevent blocklisted subscribers from depositing funds
    if crate::blocklist::is_blocklisted(env, &subscriber) {
        return Err(Error::SubscriberBlocklisted);
    }

    // ──────────────────────────────────────────────────────────────────────────
    // CHECKS: Validate all preconditions before any state mutations
    // ──────────────────────────────────────────────────────────────────────────
    let min_topup: i128 = crate::admin::get_min_topup(env)?;
    if amount < min_topup {
        return Err(Error::BelowMinimumTopup);
    }
    validate_non_negative(amount)?;

    let mut sub = get_subscription(env, subscription_id)?;
    let token_addr: Address = env
        .storage()
        .instance()
        .get(&Symbol::new(env, "token"))
        .ok_or(Error::NotInitialized)?;

    // ──────────────────────────────────────────────────────────────────────────
    // EFFECTS: Update internal state before external interactions (CEI pattern)
    // ──────────────────────────────────────────────────────────────────────────
    sub.prepaid_balance = safe_add_balance(sub.prepaid_balance, amount)?;
    env.storage().instance().set(&subscription_id, &sub);

    // ──────────────────────────────────────────────────────────────────────────
    // INTERACTIONS: Only after internal state is consistent, call token contract
    // ──────────────────────────────────────────────────────────────────────────
    let token_client = soroban_sdk::token::Client::new(env, &token_addr);
    token_client.transfer(&subscriber, &env.current_contract_address(), &amount);

    // Emit event after successful transfer
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
///
/// Requires merchant auth; the subscription's merchant must match the caller.
/// Subscription must be Active or Paused. Amount must be positive and not exceed prepaid_balance.
///
/// One-off charges also count toward the lifetime cap when one is configured.
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

    // Enforce lifetime cap for one-off charges
    if let Some(cap) = sub.lifetime_cap {
        let new_charged = sub
            .lifetime_charged
            .checked_add(amount)
            .ok_or(Error::Overflow)?;
        if new_charged > cap {
            return Err(Error::LifetimeCapReached);
        }
        sub.lifetime_charged = new_charged;
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
        return Err(Error::InvalidStatusTransition);
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
    lifetime_cap: Option<i128>,
) -> Result<u32, Error> {
    merchant.require_auth();

    // Validate lifetime_cap if provided
    if let Some(cap) = lifetime_cap {
        if cap <= 0 {
            return Err(Error::InvalidAmount);
        }
    }

    let plan = PlanTemplate {
        merchant,
        amount,
        interval_seconds,
        usage_enabled,
        lifetime_cap,
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

    // Blocklist check: prevent blocklisted subscribers from creating subscriptions
    if crate::blocklist::is_blocklisted(env, &subscriber) {
        return Err(Error::SubscriberBlocklisted);
    }

    let plan = get_plan_template(env, plan_template_id)?;

    let now = env.ledger().timestamp();
    let key = Symbol::new(env, "next_id");
    let id: u32 = env.storage().instance().get(&key).unwrap_or(0);
    env.storage().instance().set(&key, &(id + 1));

    let sub = Subscription {
        subscriber: subscriber.clone(),
        merchant: plan.merchant.clone(),
        amount: plan.amount,
        interval_seconds: plan.interval_seconds,
        last_payment_timestamp: now,
        status: SubscriptionStatus::Active,
        prepaid_balance: 0i128,
        usage_enabled: plan.usage_enabled,
        expiration: None,
        billing_anchor_timestamp: now,
        current_period_index: 0,
        current_period_usage_units: 0,
        usage_cap_units: None,
        usage_rate_limit_max_calls: None,
        usage_rate_window_secs: 0,
        lifetime_cap: plan.lifetime_cap,
        lifetime_charged: 0i128,
    };

    env.storage().instance().set(&id, &sub);

    // Maintain merchant → subscription-ID index
    let merchant_key = DataKey::MerchantSubs(plan.merchant.clone());
    let mut ids: Vec<u32> = env
        .storage()
        .instance()
        .get(&merchant_key)
        .unwrap_or(Vec::new(env));
    ids.push_back(id);
    env.storage().instance().set(&merchant_key, &ids);

    Ok(id)
}

pub fn do_set_usage_cap(
    env: &Env,
    subscription_id: u32,
    authorizer: Address,
    usage_cap_units: Option<i128>,
) -> Result<(), Error> {
    authorizer.require_auth();
    let mut sub = get_subscription(env, subscription_id)?;
    if authorizer != sub.merchant {
        return Err(Error::Forbidden);
    }
    if let Some(cap) = usage_cap_units {
        if cap <= 0 {
            return Err(Error::InvalidAmount);
        }
    }
    sub.usage_cap_units = usage_cap_units;
    env.storage().instance().set(&subscription_id, &sub);
    Ok(())
}

pub fn do_set_usage_rate_limit(
    env: &Env,
    subscription_id: u32,
    authorizer: Address,
    max_calls: Option<u32>,
    window_seconds: u64,
) -> Result<(), Error> {
    authorizer.require_auth();
    let mut sub = get_subscription(env, subscription_id)?;
    if authorizer != sub.merchant {
        return Err(Error::Forbidden);
    }
    if let Some(max) = max_calls {
        if max == 0 || window_seconds == 0 {
            return Err(Error::InvalidInput);
        }
    }
    sub.usage_rate_limit_max_calls = max_calls;
    sub.usage_rate_window_secs = window_seconds;
    env.storage().instance().set(&subscription_id, &sub);
    Ok(())
}
