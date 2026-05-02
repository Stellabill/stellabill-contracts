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
//! - `do_partial_refund`: transfers tokens FROM contract TO subscriber
//!
//! All functions follow the **Checks-Effects-Interactions (CEI)** pattern:
//! 1. **Checks**: Validate inputs and authorization
//! 2. **Effects**: Update internal contract state (prepaid_balance) in storage
//! 3. **Interactions**: Call token.transfer() AFTER state is persisted
//!
//! **Guard layer**: Public entry-points in `lib.rs` acquire a `ReentrancyGuard` before
//! calling these internal helpers. This prevents the same function from being re-entered
//! during an external call (defense in depth).
//!
//! See `docs/reentrancy.md` and `docs/reentrancy_hardening.md` for full details
//! on reentrancy threats and mitigations.
//!
//! # Write-path scan complexity
//!
//! Two internal helpers perform O(n) sequential scans over all subscription IDs:
//!
//! | Helper | Called from | Guard |
//! |---|---|---|
//! | `count_active_subscriptions_for_plan` | `enforce_plan_concurrency_limit` | Fast-path skip when max_active == 0 |
//! | `compute_subscriber_exposure` | `enforce_credit_limit_for_delta` | Fast-path skip when limit == 0 |
//!
//! Both helpers are bounded by [`MAX_WRITE_PATH_SCAN_DEPTH`]. Because their
//! callers short-circuit when the relevant limit is not configured (the common
//! case), the scan is only reached in contracts that actively enforce plan
//! concurrency or subscriber credit limits.

use crate::queries::get_subscription;
use crate::safe_math::{safe_add, safe_add_balance, safe_sub, validate_non_negative};
use crate::state_machine::transition_to;
use crate::statements::append_statement;
use crate::types::{
    BillingChargeKind, DataKey, Error, FundsDepositedEvent,
    GlobalCapDefaultUpdatedEvent, LifetimeCapReachedEvent, LifetimeCapUpdatedEvent,
    MerchantCapDefaultUpdatedEvent, PartialRefundEvent, PlanMaxActiveUpdatedEvent,
    PlanTemplate, PlanTemplateUpdatedEvent, SubscriberWithdrawalEvent,
    Subscription, SubscriptionCancelledEvent, SubscriptionCreatedEvent, SubscriptionMigratedEvent,
    SubscriptionRecoveryReadyEvent, SubscriptionResumedEvent, SubscriptionPausedEvent,
    Subscription, SubscriptionCancelledEvent, SubscriptionCreatedEvent,
    SubscriptionMigratedEvent, SubscriptionRecoveryReadyEvent,
    SubscriptionStatus, UsageLimits, UsageLimitsConfiguredEvent,
};
use soroban_sdk::{symbol_short, Address, Env, Symbol, Vec};

const MIN_SUBSCRIPTION_INTERVAL_SECONDS: u64 = 60;
/// Hard upper bound on billing interval: 365 days (31 536 000 s).
///
/// Prevents absurdly large intervals from making `last_payment_timestamp +
/// interval_seconds` overflow `u64` in practice, and keeps subscriptions
/// semantically reasonable.
pub const MAX_SUBSCRIPTION_INTERVAL_SECONDS: u64 = 31_536_000;

/// Validates that `interval_seconds` is within the allowed `[MIN, MAX]` range.
///
/// Returns `Err(Error::InvalidInput)` when the value is below the minimum (60 s)
/// or above the maximum (365 days).  Zero is implicitly rejected because
/// `MIN_SUBSCRIPTION_INTERVAL_SECONDS` is non-zero.
///
/// This is the single authoritative validation gate: every code path that
/// persists an interval (subscription creation, plan-template creation) must
/// call this function rather than performing ad-hoc comparisons.
pub fn validate_interval(interval_seconds: u64) -> Result<(), Error> {
    if interval_seconds < MIN_SUBSCRIPTION_INTERVAL_SECONDS
        || interval_seconds > MAX_SUBSCRIPTION_INTERVAL_SECONDS
    {
        return Err(Error::InvalidInput);
    }
    Ok(())
}

/// Returns the earliest timestamp at which the *next* charge is allowed.
///
/// `next_charge_time(last, interval) == last + interval`
///
/// This is the canonical time-math helper.  Both the charge path
/// (`charge_core.rs`) and the query path (`queries.rs`) must use this
/// function instead of computing `last + interval` inline, so that the
/// semantics are identical across all call sites.
///
/// Returns `Err(Error::Overflow)` if the addition would wrap past `u64::MAX`.
/// In practice this cannot happen for validated intervals (≤ 365 days) and
/// real ledger timestamps, but the checked arithmetic is retained as a
/// belt-and-suspenders guard.
pub fn next_charge_time(last_payment: u64, interval: u64) -> Result<u64, Error> {
    last_payment.checked_add(interval).ok_or(Error::Overflow)
}

/// Hard upper bound on the number of subscription IDs that may be scanned in a
/// single write-path helper invocation.
///
/// Two internal helpers — `count_active_subscriptions_for_plan` and
/// `compute_subscriber_exposure` — perform a full sequential scan over all
/// subscription IDs when their respective feature (plan concurrency limit /
/// subscriber credit limit) is configured.  This constant prevents a single
/// transaction from reading an unbounded number of storage entries.
///
/// **Fast-path note**: when no plan concurrency limit or credit limit is
/// configured (the common case), the O(n) scan is *never* reached — the
/// caller returns early before calling these helpers.  The guard is therefore
/// only triggered in contracts that actively use both features *and* have
/// accumulated more than `MAX_WRITE_PATH_SCAN_DEPTH` subscriptions.
///
/// **Upgrade note**: if a live contract exceeds this threshold while a limit
/// is configured, the affected operations (`create_subscription`,
/// `deposit_funds`) will return `Error::InvalidInput`.  Raise this constant
/// or disable the relevant limits before upgrading in that scenario.
pub(crate) const MAX_WRITE_PATH_SCAN_DEPTH: u32 = 5_000;

#[allow(dead_code)]
pub fn next_id(env: &Env) -> u32 {
    let storage = env.storage().instance();
    let id: u32 = storage.get(&DataKey::NextId).unwrap_or(0);
    let next = id.checked_add(1).unwrap_or(id);
    storage.set(&DataKey::NextId, &next);
    id
}

pub fn next_plan_id(env: &Env) -> u32 {
    let id: u32 = env.storage().instance().get(&DataKey::NextPlanId).unwrap_or(0);
    let next = id.checked_add(1).unwrap_or(id);
    env.storage().instance().set(&DataKey::NextPlanId, &next);
    id
}

pub fn get_plan_template(env: &Env, plan_template_id: u32) -> Result<PlanTemplate, Error> {
    env.storage().instance().get(&DataKey::Plan(plan_template_id)).ok_or(Error::NotFound)
}

fn sub_plan_key(subscription_id: u32) -> DataKey {
    DataKey::SubPlan(subscription_id)
}

fn plan_max_active_key(plan_template_id: u32) -> DataKey {
    DataKey::PlanMaxActive(plan_template_id)
}

fn get_plan_max_active(env: &Env, plan_template_id: u32) -> u32 {
    env.storage()
        .instance()
        .get(&plan_max_active_key(plan_template_id))
        .unwrap_or(0)
}

/// Count active subscriptions for `subscriber` on `plan_template_id`.
///
/// ## Complexity
///
/// O(min(`MAX_WRITE_PATH_SCAN_DEPTH`, `next_id`)) storage reads.
/// Returns `Err(Error::InvalidInput)` if the total subscription count exceeds
/// [`MAX_WRITE_PATH_SCAN_DEPTH`] to prevent unbounded reads on very large
/// contracts.
///
/// ## Fast path
///
/// This function is only called when `plan_max_active > 0` for the given plan.
/// When no concurrency limit is configured, `enforce_plan_concurrency_limit`
/// returns early and this scan is never triggered.
fn count_active_subscriptions_for_plan(
    env: &Env,
    subscriber: &Address,
    plan_template_id: u32,
) -> Result<u32, Error> {
    let next_id: u32 = env.storage().instance().get(&DataKey::NextId).unwrap_or(0);

    // Guard: refuse to scan more than MAX_WRITE_PATH_SCAN_DEPTH IDs to prevent
    // excessive storage reads in high-volume contracts.
    if next_id > MAX_WRITE_PATH_SCAN_DEPTH {
        return Err(Error::InvalidInput);
    }

    let mut count = 0u32;
    let storage = env.storage().instance();

    for id in 0..next_id {
        let key = sub_plan_key(id);
        let maybe_plan_id: Option<u32> = storage.get(&key);
        if maybe_plan_id != Some(plan_template_id) {
            continue;
        }

        if let Some(sub) = storage.get::<_, Subscription>(&DataKey::Sub(id)) {
            if &sub.subscriber == subscriber && sub.status == SubscriptionStatus::Active {
                count = count.saturating_add(1);
            }
        }
    }

    Ok(count)
}

fn enforce_plan_concurrency_limit(
    env: &Env,
    subscriber: &Address,
    plan_template_id: u32,
) -> Result<(), Error> {
    let max_active = get_plan_max_active(env, plan_template_id);
    // Zero means "no limit" for this plan.
    if max_active == 0 {
        return Ok(());
    }

    let current = count_active_subscriptions_for_plan(env, subscriber, plan_template_id)?;
    if current >= max_active {
        return Err(Error::MaxConcurrentSubscriptionsReached);
    }

    Ok(())
}

fn credit_limit_key(
    subscriber: &Address,
    token: &Address,
) -> DataKey {
    DataKey::CreditLimit(subscriber.clone(), token.clone())
}

fn get_subscriber_credit_limit_internal(env: &Env, subscriber: &Address, token: &Address) -> i128 {
    env.storage()
        .instance()
        .get(&credit_limit_key(subscriber, token))
        .unwrap_or(0)
}

/// Compute the total financial exposure of `subscriber` for a given `token`.
///
/// Exposure = sum of prepaid balances + next-interval amount for every active
/// subscription belonging to this subscriber and token.
///
/// ## Complexity
///
/// O(min(`MAX_WRITE_PATH_SCAN_DEPTH`, `next_id`)) storage reads.
/// Returns `Err(Error::InvalidInput)` when the subscription count exceeds
/// [`MAX_WRITE_PATH_SCAN_DEPTH`].
///
/// ## Fast path
///
/// Only called when `get_subscriber_credit_limit_internal` returns a non-zero
/// value. When no credit limit is configured, `enforce_credit_limit_for_delta`
/// returns early and this scan is never triggered.
fn compute_subscriber_exposure(
    env: &Env,
    subscriber: &Address,
    token: &Address,
) -> Result<i128, Error> {
    let next_id: u32 = env.storage().instance().get(&DataKey::NextId).unwrap_or(0);

    // Guard: refuse to scan more than MAX_WRITE_PATH_SCAN_DEPTH IDs.
    if next_id > MAX_WRITE_PATH_SCAN_DEPTH {
        return Err(Error::InvalidInput);
    }

    let storage = env.storage().instance();

    let mut exposure: i128 = 0;
    for id in 0..next_id {
        if let Some(sub) = storage.get::<_, Subscription>(&DataKey::Sub(id)) {
            if &sub.subscriber != subscriber || &sub.token != token {
                continue;
            }

            // Base exposure: current prepaid balance.
            exposure = safe_add(exposure, sub.prepaid_balance)?;

            // For active subscriptions we also treat the next interval amount as expected liability.
            if sub.status == SubscriptionStatus::Active {
                exposure = safe_add(exposure, sub.amount)?;
            }
        }
    }

    Ok(exposure)
}

fn enforce_credit_limit_for_delta(
    env: &Env,
    subscriber: &Address,
    token: &Address,
    additional_liability: i128,
) -> Result<(), Error> {
    // Zero or negative additions do not increase exposure.
    if additional_liability <= 0 {
        return Ok(());
    }

    let limit = get_subscriber_credit_limit_internal(env, subscriber, token);
    // Zero means "no credit limit" configured.
    if limit == 0 {
        return Ok(());
    }

    let current = compute_subscriber_exposure(env, subscriber, token)?;
    let new_exposure = safe_add(current, additional_liability)?;
    if new_exposure > limit {
        return Err(Error::CreditLimitExceeded);
    }

    Ok(())
}

pub fn do_create_subscription(
    env: &Env,
    subscriber: Address,
    merchant: Address,
    amount: i128,
    interval_seconds: u64,
    usage_enabled: bool,
    lifetime_cap: Option<i128>,
    expires_at: Option<u64>,
) -> Result<u32, Error> {
    let token = crate::admin::get_token(env)?;

    // Enforce subscriber-level credit limit for this token before creating a new
    // subscription with additional interval liability `amount`.
    enforce_credit_limit_for_delta(env, &subscriber, &token, amount)?;
    do_create_subscription_with_token(
        env,
        subscriber,
        merchant,
        token,
        amount,
        interval_seconds,
        usage_enabled,
        lifetime_cap,
        expires_at,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn do_create_subscription_with_token(
    env: &Env,
    subscriber: Address,
    merchant: Address,
    token: Address,
    amount: i128,
    interval_seconds: u64,
    usage_enabled: bool,
    lifetime_cap: Option<i128>,
    expires_at: Option<u64>,
) -> Result<u32, Error> {
    subscriber.require_auth();

    if crate::blocklist::is_blocklisted(env, &subscriber) {
        return Err(Error::SubscriberBlocklisted);
    }

    validate_non_negative(amount)?;
    if amount == 0 {
        return Err(Error::InvalidAmount);
    }

    validate_interval(interval_seconds)?;

    if !crate::admin::is_token_accepted(env, &token) {
        return Err(Error::InvalidInput);
    }

    // Resolve effective cap: explicit > merchant default > global default.
    let resolved_cap = resolve_cap(env, &merchant, lifetime_cap);

    if let Some(cap) = resolved_cap {
        if cap <= 0 {
            return Err(Error::InvalidAmount);
        }
        if cap < amount {
            return Err(Error::InvalidInput);
        }
    }

    // Enforce credit limit for the token-specific subscription.
    enforce_credit_limit_for_delta(env, &subscriber, &token, amount)?;

    let sub = Subscription {
        subscriber: subscriber.clone(),
        merchant: merchant.clone(),
        token: token.clone(),
        amount,
        interval_seconds,
        last_payment_timestamp: env.ledger().timestamp(),
        status: SubscriptionStatus::Active,
        prepaid_balance: 0i128,
        usage_enabled,
        lifetime_cap: resolved_cap,
        lifetime_charged: 0i128,
        start_time: env.ledger().timestamp(),
        expires_at,
        grace_start_timestamp: None,
    };

    // Allocate ID with overflow / limit guard.
    let id: u32 = env.storage().instance().get(&DataKey::NextId).unwrap_or(0);
    if id == crate::MAX_SUBSCRIPTION_ID {
        return Err(Error::SubscriptionLimitReached);
    }
    let next_id = id.checked_add(1).ok_or(Error::SubscriptionLimitReached)?;

    env.storage().instance().set(&DataKey::NextId, &next_id);
    env.storage().instance().set(&DataKey::Sub(id), &sub);

    // Maintain merchant -> subscription-ID index
    let merchant_key = DataKey::MerchantSubs(merchant.clone());
    let mut ids: Vec<u32> = env
        .storage()
        .instance()
        .get(&merchant_key)
        .unwrap_or(Vec::new(env));
    ids.push_back(id);
    env.storage().instance().set(&merchant_key, &ids);

    // Maintain token -> subscription-ID index
    let token_key = DataKey::TokenSubs(token.clone());
    let mut token_ids: Vec<u32> = env
        .storage()
        .instance()
        .get(&token_key)
        .unwrap_or(Vec::new(env));
    token_ids.push_back(id);
    env.storage().instance().set(&token_key, &token_ids);

    env.events().publish(
        (Symbol::new(env, "subscription_created"), id),
        crate::types::SubscriptionCreatedEvent {
            subscription_id: id,
            subscriber,
            merchant,
            token,
            amount,
            interval_seconds,
            lifetime_cap,
            expires_at,
            timestamp: env.ledger().timestamp(),
        },
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
    crate::blocklist::require_not_blocklisted(env, &subscriber)?;

    // CHECKS: Validate all preconditions before any state mutations
    let min_topup: i128 = crate::admin::get_min_topup(env)?;
    if amount < min_topup {
        return Err(Error::BelowMinimumTopup);
    }
    validate_non_negative(amount)?;

    let mut sub = get_subscription(env, subscription_id)?;
    if subscriber != sub.subscriber {
        return Err(Error::Unauthorized);
    }

    // Block deposits to subscriptions whose merchant is paused — paused
    // merchants must not accumulate new subscriber funds.
    if crate::merchant::get_merchant_paused(env, sub.merchant.clone()) {
        return Err(Error::MerchantPaused);
    }

    let now = env.ledger().timestamp();
    // Expiration guard
    if sub.is_expired(now) {
        if sub.status != SubscriptionStatus::Expired {
            transition_to(&mut sub.status, SubscriptionStatus::Expired)?;
            env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);
            env.events().publish(
                (Symbol::new(env, "subscription_expired"), subscription_id),
                crate::types::SubscriptionExpiredEvent {
                    subscription_id,
                    timestamp: now,
                },
            );
        }
        return Err(Error::SubscriptionExpired);
    }


    let token_addr = sub.token.clone();

    // Enforce credit limit for additional prepaid balance being loaded.
    enforce_credit_limit_for_delta(env, &subscriber, &token_addr, amount)?;

    // Enforce lifetime cap: deposit cannot exceed remaining chargeable capacity.
    enforce_deposit_cap(&sub, amount)?;

    // EFFECTS
    sub.prepaid_balance = safe_add_balance(sub.prepaid_balance, amount)?;
    env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);

    // INTERACTIONS
    let token_client = soroban_sdk::token::Client::new(env, &token_addr);
    token_client.transfer(&subscriber, &env.current_contract_address(), &amount);

    crate::accounting::add_total_accounted(env, &token_addr, amount)?;

    env.events().publish(
        (Symbol::new(env, "deposited"), subscription_id),
        FundsDepositedEvent {
            subscription_id,
            subscriber: subscriber.clone(),
            token: token_addr.clone(),
            amount,
            new_balance: sub.prepaid_balance,
            timestamp: env.ledger().timestamp(),
        },
    );

    if (sub.status == SubscriptionStatus::InsufficientBalance
        || sub.status == SubscriptionStatus::GracePeriod)
        && sub.prepaid_balance >= sub.amount
    {
        sub.status = SubscriptionStatus::Active;
        env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);

        env.events().publish(
            (Symbol::new(env, "recovery_ready"), subscription_id),
            SubscriptionRecoveryReadyEvent {
                subscription_id,
                subscriber: sub.subscriber.clone(),
                prepaid_balance: sub.prepaid_balance,
                required_amount: sub.amount,
                timestamp: env.ledger().timestamp(),
            },
        );

        env.events().publish(
            (Symbol::new(env, "sub_resumed"), subscription_id),
            crate::types::SubscriptionResumedEvent {
                subscription_id,
                subscriber: sub.subscriber.clone(),
                merchant: sub.merchant.clone(),
                authorizer: sub.subscriber.clone(),
                previous_status: SubscriptionStatus::Paused,
                timestamp: env.ledger().timestamp(),
            },
        );
    }

    Ok(())
}

pub fn do_cancel_subscription(
    env: &Env,
    subscription_id: u32,
    authorizer: Address,
) -> Result<(), Error> {
    authorizer.require_auth();

    let mut sub = get_subscription(env, subscription_id)?;

    if sub.is_expired(env.ledger().timestamp()) {
        return Err(Error::SubscriptionExpired);
    }

    if authorizer != sub.subscriber && authorizer != sub.merchant {
        return Err(Error::Forbidden);
    }

    transition_to(&mut sub.status, SubscriptionStatus::Cancelled)?;
    let refund_amount = sub.prepaid_balance;

    env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);

    env.events().publish(
        (Symbol::new(env, "subscription_cancelled"), subscription_id),
        SubscriptionCancelledEvent {
            subscription_id,
            subscriber: sub.subscriber.clone(),
            merchant: sub.merchant.clone(),
            token: sub.token.clone(),
            authorizer,
            refund_amount,
            timestamp: env.ledger().timestamp(),
        },
    );
    Ok(())
}

/// Pause a subscription (no charges until resumed).
///
/// # Authorization
/// Only the subscription's `subscriber` or `merchant` may pause.
/// Any other caller receives [`Error::Forbidden`].
///
/// # Transition guard
/// Only `Active → Paused` is permitted by the state machine.
/// Calling on an already-`Paused` subscription is idempotent (same-state rule).
/// Any other source state returns [`Error::InvalidStatusTransition`].
///
/// # Events
/// Emits [`SubscriptionPausedEvent`] on every state-changing call.
pub fn do_pause_subscription(
    env: &Env,
    subscription_id: u32,
    authorizer: Address,
) -> Result<(), Error> {
    authorizer.require_auth();

    let mut sub = get_subscription(env, subscription_id)?;

    if sub.is_expired(env.ledger().timestamp()) {
        return Err(Error::SubscriptionExpired);
    }

    if authorizer != sub.subscriber && authorizer != sub.merchant {
        return Err(Error::Forbidden);
    }

    // Idempotent: already paused — nothing to do, no event.
    if sub.status == SubscriptionStatus::Paused {
        return Ok(());
    }

    transition_to(&mut sub.status, SubscriptionStatus::Paused)?;

    env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);

    env.events().publish(
        (Symbol::new(env, "sub_paused"), subscription_id),
        crate::types::SubscriptionPausedEvent {
            subscription_id,
            subscriber: sub.subscriber.clone(),
            merchant: sub.merchant.clone(),
            authorizer,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(())
}

/// Resume a paused, grace-period, or insufficient-balance subscription back to `Active`.
///
/// # Authorization
/// Only the subscription's `subscriber` or `merchant` may resume.
/// Any other caller receives [`Error::Forbidden`].
///
/// # Transition guard
/// `Paused → Active`, `GracePeriod → Active`, and `InsufficientBalance → Active` are permitted.
/// Any other source state (including `Cancelled`) returns [`Error::InvalidStatusTransition`].
///
/// # Events
/// Emits [`SubscriptionResumedEvent`] on every state-changing call.
pub fn do_resume_subscription(
    env: &Env,
    subscription_id: u32,
    authorizer: Address,
) -> Result<(), Error> {
    authorizer.require_auth();

    let mut sub = get_subscription(env, subscription_id)?;

    if sub.is_expired(env.ledger().timestamp()) {
        return Err(Error::SubscriptionExpired);
    }
    if authorizer != sub.subscriber && authorizer != sub.merchant {
        return Err(Error::Forbidden);
    }
    if authorizer == sub.subscriber {
        crate::blocklist::require_not_blocklisted(env, &sub.subscriber)?;
    }

    // Idempotent: already active — nothing to do, no event.
    if sub.status == SubscriptionStatus::Active {
        return Ok(());
    }
    if (sub.status == SubscriptionStatus::InsufficientBalance
        || sub.status == SubscriptionStatus::GracePeriod)
        && sub.prepaid_balance < sub.amount
    {
        return Err(Error::InsufficientBalance);
    }

    transition_to(&mut sub.status, SubscriptionStatus::Active)?;

    let previous_status = sub.status;
    sub.status = SubscriptionStatus::Active;
    env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);

     env.events().publish(
        (Symbol::new(env, "sub_resumed"), subscription_id),
        crate::types::SubscriptionResumedEvent {
            subscription_id,
            subscriber: sub.subscriber.clone(),
            merchant: sub.merchant.clone(),
            authorizer,
            previous_status,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(())
}

/// Merchant-initiated one-off charge: debits `amount` from the subscription's prepaid balance.
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

    let now = env.ledger().timestamp();
    // Expiration guard
    if sub.is_expired(now) {
        if sub.status != SubscriptionStatus::Expired {
            transition_to(&mut sub.status, SubscriptionStatus::Expired)?;
            env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);
            env.events().publish(
                (Symbol::new(env, "subscription_expired"), subscription_id),
                crate::types::SubscriptionExpiredEvent {
                    subscription_id,
                    timestamp: now,
                },
            );
        }
        return Err(Error::SubscriptionExpired);
    }

    if sub.merchant != merchant {
        return Err(Error::Unauthorized);
    }
    if let Some(cap) = sub.lifetime_cap {
        if sub.lifetime_charged >= cap {
            if sub.status != SubscriptionStatus::Cancelled {
                transition_to(&mut sub.status, SubscriptionStatus::Cancelled)?;
                env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);
                env.events().publish(
                    (Symbol::new(env, "lifetime_cap_reached"), subscription_id),
                    LifetimeCapReachedEvent {
                        subscription_id,
                        lifetime_cap: cap,
                        lifetime_charged: sub.lifetime_charged,
                        timestamp: now,
                    },
                );
            }
            return Err(Error::LifetimeCapReached);
        }
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
    let new_charged = safe_add(sub.lifetime_charged, amount)?;
    if let Some(cap) = sub.lifetime_cap {
        if new_charged > cap {
            transition_to(&mut sub.status, SubscriptionStatus::Cancelled)?;
            env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);
            env.events().publish(
                (Symbol::new(env, "lifetime_cap_reached"), subscription_id),
                LifetimeCapReachedEvent {
                    subscription_id,
                    lifetime_cap: cap,
                    lifetime_charged: sub.lifetime_charged,
                    timestamp: now,
                },
            );
            return Err(Error::LifetimeCapReached);
        }
    }
    sub.lifetime_charged = new_charged;
    let cap_reached = sub
        .lifetime_cap
        .map(|cap| sub.lifetime_charged >= cap)
        .unwrap_or(false);

    sub.prepaid_balance = safe_sub(sub.prepaid_balance, amount)?;

    crate::merchant::credit_merchant_balance_for_token(
        env,
        &sub.merchant,
        &sub.token,
        amount,
        BillingChargeKind::OneOff,
    )?;

    if cap_reached {
        transition_to(&mut sub.status, SubscriptionStatus::Cancelled)?;
        
        if let Some(cap) = sub.lifetime_cap {
            env.events().publish(
                (symbol_short!("cap_reach"), subscription_id),
                LifetimeCapReachedEvent {
                    subscription_id,
                    lifetime_cap: cap,
                    lifetime_charged: sub.lifetime_charged,
                    timestamp: env.ledger().timestamp(),
                },
            );
        }
    }

    env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);
    append_statement(
        env,
        subscription_id,
        amount,
        sub.merchant.clone(),
        BillingChargeKind::OneOff,
        env.ledger().timestamp(),
        env.ledger().timestamp(),
    )?;

    env.events().publish(
        (symbol_short!("oneoff_ch"), subscription_id),
        crate::types::OneOffChargedEvent {
            subscription_id,
            subscriber: sub.subscriber.clone(),
            merchant: sub.merchant.clone(),
            token: sub.token.clone(),
            amount,
            remaining_balance: sub.prepaid_balance,
            timestamp: now,
        },
    );

    Ok(())
}

pub fn do_cleanup_subscription(
    env: &Env,
    subscription_id: u32,
    authorizer: Address,
) -> Result<(), Error> {
    authorizer.require_auth();

    let mut sub = get_subscription(env, subscription_id)?;

    // Can only cleanup if it's already expired or cancelled
    let now = env.ledger().timestamp();
    let is_terminal = sub.status == SubscriptionStatus::Cancelled || sub.is_expired(now);

    if !is_terminal {
        return Err(Error::InvalidStatusTransition);
    }

    if sub.status != SubscriptionStatus::Archived {
        // If it's expired but not yet marked as Expired or Cancelled, transition it to Expired first
        if sub.status != SubscriptionStatus::Cancelled
            && sub.status != SubscriptionStatus::Expired
            && sub.is_expired(now)
        {
            transition_to(&mut sub.status, SubscriptionStatus::Expired)?;
        }

        transition_to(&mut sub.status, SubscriptionStatus::Archived)?;
        env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);
        
        env.events().publish(
            (Symbol::new(env, "subscription_archived"), subscription_id),
            crate::types::SubscriptionArchivedEvent {
                subscription_id,
                timestamp: now,
            },
        );
    }

    // We do NOT delete the subscription, we keep it in Archived state
    // We could potentially remove some metadata, but "remain readable" means we should keep core fields.
    // The funds are preserved in `prepaid_balance` and can still be withdrawn because
    // `do_withdraw_subscriber_funds` allows withdrawal in `Archived` state.

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

    if sub.status != SubscriptionStatus::Cancelled
        && sub.status != SubscriptionStatus::Expired
        && sub.status != SubscriptionStatus::Archived
        && !sub.is_expired(env.ledger().timestamp())
    {
        return Err(Error::InvalidStatusTransition);
    }

    let amount_to_refund = sub.prepaid_balance;
    if amount_to_refund <= 0 {
        return Err(Error::InsufficientBalance);
    }

    // EFFECTS: zero the balance before the external token transfer (CEI pattern).
    sub.prepaid_balance = 0;
    let token_addr = sub.token.clone();
    env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);

    // INTERACTIONS: transfer refund from vault to subscriber.
    let token_client = soroban_sdk::token::Client::new(env, &token_addr);
    token_client.transfer(
        &env.current_contract_address(),
        &subscriber,
        &amount_to_refund,
    );
    crate::accounting::sub_total_accounted(env, &token_addr, amount_to_refund)?;

    env.events().publish(
        (Symbol::new(env, "sub_withdrawn"), subscription_id),
        SubscriberWithdrawalEvent {
            subscription_id,
            subscriber,
            token: token_addr,
            amount: amount_to_refund,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(())
}

/// Process a partial refund against a subscription's remaining prepaid balance.
///
/// # Authorization
/// Only the contract admin may authorize partial refunds. The `subscriber`
/// parameter is validated against the subscription record but does **not**
/// require the subscriber's own signature — the admin acts on their behalf.
///
/// # Preconditions
/// - `amount > 0`
/// - `amount <= subscription.prepaid_balance`
/// - `subscriber` matches `subscription.subscriber`
///
/// # CEI pattern
/// State is updated before the token transfer to prevent reentrancy.
pub fn do_partial_refund(
    env: &Env,
    admin: Address,
    subscription_id: u32,
    subscriber: Address,
    amount: i128,
) -> Result<(), Error> {
    // Checks: admin authorization and input validation first.
    super::require_admin_auth(env, &admin)?;

    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    let mut sub = get_subscription(env, subscription_id)?;

    if subscriber != sub.subscriber {
        return Err(Error::Unauthorized);
    }

    if amount > sub.prepaid_balance {
        return Err(Error::InsufficientBalance);
    }

    // Effects: debit balance before external call.
    sub.prepaid_balance = safe_sub(sub.prepaid_balance, amount)?;
    env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);

    // Interactions: transfer refund from vault to subscriber.
    let token_addr = sub.token.clone();
    let token_client = soroban_sdk::token::Client::new(env, &token_addr);
    token_client.transfer(&env.current_contract_address(), &subscriber, &amount);
    crate::accounting::sub_total_accounted(env, &sub.token, amount)?;

    env.events().publish(
        (Symbol::new(env, "partial_refund"), subscription_id),
        PartialRefundEvent {
            subscription_id,
            subscriber,
            token: sub.token.clone(),
            amount,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(())
}

// ── Lifetime cap helpers ──────────────────────────────────────────────────────

pub fn get_global_cap_default(env: &Env) -> Option<i128> {
    env.storage()
        .instance()
        .get::<_, i128>(&Symbol::new(env, "cap_default"))
}

pub fn get_merchant_cap_default_internal(env: &Env, merchant: &Address) -> Option<i128> {
    env.storage()
        .instance()
        .get::<_, i128>(&(Symbol::new(env, "merch_cap"), merchant.clone()))
}

/// Resolve effective cap: explicit > merchant default > global default.
fn resolve_cap(env: &Env, merchant: &Address, explicit_cap: Option<i128>) -> Option<i128> {
    if explicit_cap.is_some() {
        return explicit_cap;
    }
    let merchant_default = get_merchant_cap_default_internal(env, merchant);
    if merchant_default.is_some() {
        return merchant_default;
    }
    get_global_cap_default(env)
}

/// Reject a deposit that would lock funds beyond the remaining chargeable cap.
fn enforce_deposit_cap(sub: &Subscription, deposit: i128) -> Result<(), Error> {
    if let Some(cap) = sub.lifetime_cap {
        let chargeable_remaining = cap.saturating_sub(sub.lifetime_charged);
        let depositable_remaining = chargeable_remaining.saturating_sub(sub.prepaid_balance);
        if deposit > depositable_remaining {
            return Err(Error::LifetimeCapReached);
        }
    }
    Ok(())
}

pub fn do_set_global_cap_default(
    env: &Env,
    admin: Address,
    cap: Option<i128>,
) -> Result<(), Error> {
    super::require_admin_auth(env, &admin)?;

    if let Some(c) = cap {
        if c <= 0 {
            return Err(Error::InvalidAmount);
        }
    }

    let old_default = get_global_cap_default(env);
    let key = Symbol::new(env, "cap_default");
    match cap {
        Some(c) => env.storage().instance().set(&key, &c),
        None => env.storage().instance().remove(&key),
    }

    env.events().publish(
        (Symbol::new(env, "global_cap_set"),),
        GlobalCapDefaultUpdatedEvent {
            admin,
            cap: cap.unwrap_or(0),
            timestamp: env.ledger().timestamp(),
        },
    );
    Ok(())
}

pub fn do_set_merchant_cap_default(
    env: &Env,
    merchant: Address,
    cap: Option<i128>,
) -> Result<(), Error> {
    merchant.require_auth();

    if let Some(c) = cap {
        if c <= 0 {
            return Err(Error::InvalidAmount);
        }
    }

    let old_default = get_merchant_cap_default_internal(env, &merchant);
    let key = (Symbol::new(env, "merch_cap"), merchant.clone());
    match cap {
        Some(c) => env.storage().instance().set(&key, &c),
        None => env.storage().instance().remove(&key),
    }

    env.events().publish(
        (Symbol::new(env, "merchant_cap_set"), merchant.clone()),
        MerchantCapDefaultUpdatedEvent {
            admin: merchant,
            cap: cap.unwrap_or(0),
            timestamp: env.ledger().timestamp(),
        },
    );
    Ok(())
}

pub fn do_update_subscription_cap(
    env: &Env,
    admin: Address,
    subscription_id: u32,
    new_cap: Option<i128>,
) -> Result<(), Error> {
    super::require_admin_auth(env, &admin)?;

    if let Some(c) = new_cap {
        if c <= 0 {
            return Err(Error::InvalidAmount);
        }
    }

    let mut sub = get_subscription(env, subscription_id)?;
    let old_cap = sub.lifetime_cap;

    // Cannot set cap below what has already been charged.
    if let Some(new_c) = new_cap {
        if new_c < sub.lifetime_charged {
            return Err(Error::LifetimeCapReached);
        }
    }

    sub.lifetime_cap = new_cap;
    env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);

    // Get admin address for event, fallback to a zero-address if not set
    let admin_addr = env.storage().instance().get(&DataKey::Admin).unwrap_or(sub.merchant.clone());
    
    env.events().publish(
        (Symbol::new(env, "cap_updated"), subscription_id),
        LifetimeCapUpdatedEvent {
            admin: admin_addr,
            cap: new_cap.unwrap_or(0),
            timestamp: env.ledger().timestamp(),
        },
    );
    Ok(())
}

pub fn get_merchant_cap_default(env: &Env, merchant: Address) -> Option<i128> {
    get_merchant_cap_default_internal(env, &merchant)
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

    validate_interval(interval_seconds)?;

    // Validate lifetime_cap if provided
    if let Some(cap) = lifetime_cap {
        if cap <= 0 {
            return Err(Error::InvalidAmount);
        }
    }

    let token = crate::admin::get_token(env)?;
    let plan_id = next_plan_id(env);
    let plan = PlanTemplate {
        merchant: merchant.clone(),
        token: token.clone(),
        amount,
        interval_seconds,
        usage_enabled,
        lifetime_cap,
        template_key: plan_id,
        version: 1,
        is_disabled: false,
    };

    let key = DataKey::Plan(plan_id);
    env.storage().instance().set(&key, &plan);

    env.events().publish(
        (Symbol::new(env, "plan_created"), plan_id),
        crate::types::PlanTemplateCreatedEvent {
            plan_id,
            admin: merchant.clone(),
            interval: interval_seconds,
            amount,
            usage_enabled,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(plan_id)
}

pub fn do_create_plan_template_with_token(
    env: &Env,
    merchant: Address,
    token: Address,
    amount: i128,
    interval_seconds: u64,
    usage_enabled: bool,
    lifetime_cap: Option<i128>,
) -> Result<u32, Error> {
    merchant.require_auth();
    validate_interval(interval_seconds)?;
    if !crate::admin::is_token_accepted(env, &token) {
        return Err(Error::InvalidInput);
    }
    if let Some(cap) = lifetime_cap {
        if cap <= 0 {
            return Err(Error::InvalidAmount);
        }
    }

    let plan_id = next_plan_id(env);
    let plan = PlanTemplate {
        merchant: merchant.clone(),
        token: token.clone(),
        amount,
        interval_seconds,
        usage_enabled,
        lifetime_cap,
        template_key: plan_id,
        version: 1,
        is_disabled: false,
    };

    let key = DataKey::Plan(plan_id);
    env.storage().instance().set(&key, &plan);

    env.events().publish(
        (Symbol::new(env, "plan_created"), plan_id),
        crate::types::PlanTemplateCreatedEvent {
            plan_id,
            admin: merchant.clone(),
            interval: interval_seconds,
            amount,
            usage_enabled,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(plan_id)
}

pub fn do_create_subscription_from_plan(
    env: &Env,
    subscriber: Address,
    plan_template_id: u32,
) -> Result<u32, Error> {
    subscriber.require_auth();
    crate::blocklist::require_not_blocklisted(env, &subscriber)?;

    let plan = get_plan_template(env, plan_template_id)?;

    if plan.is_disabled {
        return Err(Error::InvalidInput);
    }

    // Enforce subscriber-level credit limit for the plan's token.
    enforce_credit_limit_for_delta(env, &subscriber, &plan.token, plan.amount)?;

    // Enforce per-plan concurrency limit for this subscriber/plan pair.
    enforce_plan_concurrency_limit(env, &subscriber, plan_template_id)?;

    let id: u32 = env.storage().instance().get(&DataKey::NextId).unwrap_or(0);
    let next_id = id.checked_add(1).ok_or(Error::Overflow)?;
    env.storage().instance().set(&DataKey::NextId, &next_id);

    let resolved_cap = resolve_cap(env, &plan.merchant, plan.lifetime_cap);
    let sub = Subscription {
        subscriber: subscriber.clone(),
        merchant: plan.merchant.clone(),
        token: plan.token.clone(),
        amount: plan.amount,
        interval_seconds: plan.interval_seconds,
        last_payment_timestamp: env.ledger().timestamp(),
        status: SubscriptionStatus::Active,
        prepaid_balance: 0i128,
        usage_enabled: plan.usage_enabled,
        lifetime_cap: resolved_cap,
        lifetime_charged: 0i128,
        start_time: env.ledger().timestamp(),
        expires_at: None,
        grace_start_timestamp: None,
    };

    env.storage().instance().set(&DataKey::Sub(id), &sub);

    // Persist linkage between subscription and the plan template
    let sub_plan_storage_key = sub_plan_key(id);
    env.storage()
        .instance()
        .set(&sub_plan_storage_key, &plan_template_id);

    // Maintain merchant -> subscription-ID index
    let merchant_key = DataKey::MerchantSubs(plan.merchant.clone());
    let mut ids: Vec<u32> = env
        .storage()
        .instance()
        .get(&merchant_key)
        .unwrap_or(Vec::new(env));
    ids.push_back(id);
    env.storage().instance().set(&merchant_key, &ids);

    // Maintain token -> subscription-ID index
    let token_key = DataKey::TokenSubs(plan.token.clone());
    let mut token_ids: Vec<u32> = env
        .storage()
        .instance()
        .get(&token_key)
        .unwrap_or(Vec::new(env));
    token_ids.push_back(id);
    env.storage().instance().set(&token_key, &token_ids);

    env.events().publish(
        (symbol_short!("created"), id),
        SubscriptionCreatedEvent {
            subscription_id: id,
            subscriber: subscriber.clone(),
            merchant: plan.merchant.clone(),
            token: plan.token.clone(),
            amount: plan.amount,
            interval_seconds: plan.interval_seconds,
            lifetime_cap: plan.lifetime_cap,
            expires_at: None,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(id)
}

pub fn do_update_plan_template(
    env: &Env,
    merchant: Address,
    plan_template_id: u32,
    amount: i128,
    interval_seconds: u64,
    usage_enabled: bool,
    lifetime_cap: Option<i128>,
) -> Result<u32, Error> {
    merchant.require_auth();

    validate_interval(interval_seconds)?;

    // Validate lifetime_cap if provided
    if let Some(cap) = lifetime_cap {
        if cap <= 0 {
            return Err(Error::InvalidAmount);
        }
    }

    let existing = get_plan_template(env, plan_template_id)?;
    if existing.merchant != merchant {
        return Err(Error::Forbidden);
    }

    // Do not allow changing token through versioning – that would be a different plan family.
    let token = existing.token.clone();

    // Enforce usage flag consistency: usage_enabled cannot be changed through versioning
    // to prevent accidental billing model shifts for downstream subscribers.
    if usage_enabled != existing.usage_enabled {
        return Err(Error::CannotChangeUsageMode);
    }

    let new_plan_id = next_plan_id(env);
    let new_version = existing.version.checked_add(1).ok_or(Error::Overflow)?;
    let updated = PlanTemplate {
        merchant: merchant.clone(),
        token,
        amount,
        interval_seconds,
        usage_enabled,
        lifetime_cap,
        template_key: existing.template_key,
        version: new_version,
        is_disabled: false,
    };

    let key = DataKey::Plan(new_plan_id);
    env.storage().instance().set(&key, &updated);

    env.events().publish(
        (
            Symbol::new(env, "plan_template_updated"),
            existing.template_key,
        ),
        PlanTemplateUpdatedEvent {
            template_key: existing.template_key,
            old_plan_id: plan_template_id,
            new_plan_id,
            version: new_version,
            merchant,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(new_plan_id)
}

pub fn do_disable_plan_template(
    env: &Env,
    merchant: Address,
    plan_template_id: u32,
) -> Result<(), Error> {
    merchant.require_auth();

    let mut plan = get_plan_template(env, plan_template_id)?;
    if plan.merchant != merchant {
        return Err(Error::Forbidden);
    }

    if plan.is_disabled {
        return Ok(());
    }

    plan.is_disabled = true;
    let key = (Symbol::new(env, "plan"), plan_template_id);
    env.storage().instance().set(&key, &plan);

    env.events().publish(
        (Symbol::new(env, "plan_disabled"), plan_template_id),
        crate::types::PlanTemplateDisabledEvent {
            plan_template_id,
            merchant,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(())
}

pub fn do_migrate_subscription_to_plan(
    env: &Env,
    subscriber: Address,
    subscription_id: u32,
    new_plan_template_id: u32,
) -> Result<(), Error> {
    subscriber.require_auth();
    crate::blocklist::require_not_blocklisted(env, &subscriber)?;

    let mut sub = get_subscription(env, subscription_id)?;
    if sub.subscriber != subscriber {
        return Err(Error::Forbidden);
    }

    // Resolve the current plan the subscription is pinned to (if any).
    let sub_plan_storage_key = sub_plan_key(subscription_id);
    let current_plan_id: u32 = match env.storage().instance().get(&sub_plan_storage_key) {
        Some(id) => id,
        None => {
            // Subscription was not created from a plan template – explicit migration required.

            return Err(Error::InvalidInput);
        }
    };

    let current_plan = get_plan_template(env, current_plan_id)?;
    let new_plan = get_plan_template(env, new_plan_template_id)?;

    // Enforce migration within the same logical template family.
    if current_plan.template_key != new_plan.template_key {
        return Err(Error::InvalidInput);
    }

    // Only allow upgrades to newer versions.
    if new_plan.version <= current_plan.version {
        return Err(Error::InvalidInput);
    }

    // For safety, do not allow token switches via migration.
    if new_plan.token != sub.token {
        return Err(Error::InvalidInput);
    }

    // For safety, do not allow billing model switches via migration.
    if new_plan.usage_enabled != sub.usage_enabled {
        return Err(Error::CannotChangeUsageMode);
    }

    // Enforce compatibility of lifetime caps: cannot migrate into a cap that is already exceeded.
    if let Some(cap) = new_plan.lifetime_cap {
        if sub.lifetime_charged > cap {
            return Err(Error::LifetimeCapReached);
        }
        sub.lifetime_cap = Some(cap);
    } else {
        // Removing a cap via migration is allowed; keeps existing lifetime_charged.
        sub.lifetime_cap = None;
    }

    // Apply updated commercial terms from the new plan version.
    sub.amount = new_plan.amount;
    sub.interval_seconds = new_plan.interval_seconds;
    sub.usage_enabled = new_plan.usage_enabled;

    env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);
    env.storage()
        .instance()
        .set(&sub_plan_storage_key, &new_plan_template_id);

    env.events().publish(
        (Symbol::new(env, "subscription_migrated"), subscription_id),
        SubscriptionMigratedEvent {
            subscription_id,
            template_key: new_plan.template_key,
            from_plan_id: current_plan_id,
            to_plan_id: new_plan_template_id,
            merchant: new_plan.merchant,
            subscriber,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(())
}

pub fn do_set_plan_max_active_subs(
    env: &Env,
    merchant: Address,
    plan_template_id: u32,
    max_active: u32,
) -> Result<(), Error> {
    merchant.require_auth();

    let plan = get_plan_template(env, plan_template_id)?;
    if plan.merchant != merchant {
        return Err(Error::Forbidden);
    }

    env.storage()
        .instance()
        .set(&plan_max_active_key(plan_template_id), &max_active);

    env.events().publish(
        (Symbol::new(env, "plan_max_active_set"), plan_template_id),
        PlanMaxActiveUpdatedEvent {
            plan_template_id,
            merchant,
            max_active,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(())
}

/// Returns the configured max-active limit for a plan template.
/// Returns `0` when no limit has been set (meaning unlimited).
#[allow(dead_code)]
pub fn get_plan_max_active_subs(env: &Env, plan_template_id: u32) -> u32 {
    get_plan_max_active(env, plan_template_id)
}

pub fn do_set_subscriber_credit_limit(
    env: &Env,
    admin: Address,
    subscriber: Address,
    token: Address,
    limit: i128,
) -> Result<(), Error> {
    super::require_admin_auth(env, &admin)?;

    if limit < 0 {
        return Err(Error::InvalidAmount);
    }

    env.storage()
        .instance()
        .set(&credit_limit_key(&subscriber, &token), &limit);

    Ok(())
}

pub fn get_subscriber_credit_limit(env: &Env, subscriber: Address, token: Address) -> i128 {
    get_subscriber_credit_limit_internal(env, &subscriber, &token)
}

pub fn get_subscriber_exposure(
    env: &Env,
    subscriber: Address,
    token: Address,
) -> Result<i128, Error> {
    compute_subscriber_exposure(env, &subscriber, &token)
}

pub fn do_configure_usage_limits(
    env: &Env,

    merchant: Address,
    subscription_id: u32,
    rate_limit_max_calls: Option<u32>,
    rate_window_secs: u64,
    burst_min_interval_secs: u64,
    usage_cap_units: Option<i128>,
) -> Result<(), Error> {
    merchant.require_auth();

    let sub = get_subscription(env, subscription_id)?;
    if sub.merchant != merchant {
        return Err(Error::Forbidden);
    }
    if !sub.usage_enabled {
        return Err(Error::UsageNotEnabled);
    }

    if let Some(cap) = usage_cap_units {
        if cap <= 0 {
            return Err(Error::InvalidAmount);
        }
    }

    let limits = UsageLimits {
        rate_limit_max_calls,
        rate_window_secs,
        burst_min_interval_secs,
        usage_cap_units,
    };

    env.storage()
        .instance()
        .set(&DataKey::UsageLimits(subscription_id), &limits);

    env.events().publish(
        (Symbol::new(env, "usage_limits_configured"), subscription_id),
        UsageLimitsConfiguredEvent {
            subscription_id,
            merchant,
            rate_limit_max_calls,
            rate_window_secs,
            burst_min_interval_secs,
            usage_cap_units,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(())
}

