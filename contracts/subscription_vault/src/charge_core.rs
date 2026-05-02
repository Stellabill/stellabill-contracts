//! Single charge logic (no auth). Used by charge_subscription and batch_charge.
//!
//! Charge runs only when status is Active or GracePeriod. On insufficient balance the
//! subscription is moved to a recoverable non-active state and an explicit failure
//! event is emitted without mutating financial accounting state.
//! On lifetime cap exhaustion the subscription is cancelled (terminal state).
//!
//! See `docs/subscription_lifecycle.md` for lifecycle details.
//! See `docs/lifetime_caps.md` for cap enforcement semantics.
//!
//! **PRs that only change how one subscription is charged should edit this file only.**
//!
//! # Reentrancy Safety
//!
//! This module does **not make external token transfers**. All state mutations happen
//! before any external interactions:
//!
//! 1. **Checks**: Validate expiration, status, interval, balance, replay protection, caps
//! 2. **Effects**: Update subscription state AND merchant earnings in storage
//! 3. **Interactions**: None (no external calls in this module)
//!
//! The public entry-point `lib.rs::charge_subscription` acquires a `ReentrancyGuard`
//! before calling `charge_one`, providing defense-in-depth protection even though
//! this function is naturally safe from external reentrancy.
//!
//! Merchant crediting happens through internal calls to `merchant::credit_merchant_balance_for_token`,
//! which only updates storage and does not call external contracts.
//!
//! See `docs/reentrancy_hardening.md` for complete charge path analysis.

#![allow(dead_code)]

use crate::queries::get_subscription;
use crate::safe_math::{safe_add, safe_sub, safe_sub_balance};
use crate::state_machine::transition_to;
use crate::subscription::next_charge_time;
use crate::statements::append_statement;
use crate::types::{
    BillingChargeKind, BillingPeriodSnapshot, ChargeExecutionResult, DataKey, Error,
    LifetimeCapReachedEvent, SubscriptionChargeFailedEvent, SubscriptionChargedEvent,
    SubscriptionStatus, UsageChargeRejectedEvent, UsageChargeResult, UsageLimits, UsageState,
    UsageStatementEvent, SNAPSHOT_FLAG_CLOSED, SNAPSHOT_FLAG_INTERVAL_CHARGED,
};
use soroban_sdk::{symbol_short, Env, String, Symbol};

/// Performs a single interval-based charge with optional replay protection.
pub fn charge_one(
    env: &Env,
    subscription_id: u32,
    now: u64,
    idempotency_key: Option<soroban_sdk::BytesN<32>>,
) -> Result<ChargeExecutionResult, Error> {
    let mut sub = get_subscription(env, subscription_id)?;

    // Merchant pause guard — mirrors charge_usage_one enforcement
    if crate::merchant::get_merchant_paused(env, sub.merchant.clone()) {
        return Err(Error::MerchantPaused);
    }

    // Blocklist guard — do not charge subscriptions belonging to blocklisted subscribers
    if crate::blocklist::is_blocklisted(env, &sub.subscriber) {
        return Err(Error::SubscriberBlocklisted);
    }

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

    let charge_amount = crate::oracle::resolve_charge_amount(env, subscription_id, &sub)?;

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
            return Ok(ChargeExecutionResult::LifetimeCapReached);
        }
    }

    if sub.status != SubscriptionStatus::Active && sub.status != SubscriptionStatus::GracePeriod {
        return Err(Error::NotActive);
    }

    let period_index = now.saturating_sub(sub.start_time) / sub.interval_seconds;
    let period_start = sub.start_time
        .checked_add(period_index.checked_mul(sub.interval_seconds).ok_or(Error::Overflow)?)
        .ok_or(Error::Overflow)?;
    let period_end = period_start
        .checked_add(sub.interval_seconds)
        .ok_or(Error::Overflow)?;

    // Idempotent return: same idempotency key already processed
    if let Some(ref k) = idempotency_key {
        if let Some(stored) = env
            .storage()
            .instance()
            .get::<_, soroban_sdk::BytesN<32>>(&DataKey::IdemKey(subscription_id))
        {
            if stored == *k {
                return Ok(ChargeExecutionResult::Charged);
            }
        }
    }

    // Replay: already charged for this billing period
    if let Some(stored_period) = env
        .storage()
        .instance()
        .get::<_, u64>(&DataKey::ChargedPeriod(subscription_id))
    {
        if period_index <= stored_period {
            return Err(Error::Replay);
        }
    }

    let next_allowed = next_charge_time(sub.last_payment_timestamp, sub.interval_seconds)?;
    if now < next_allowed {
        return Err(Error::IntervalNotElapsed);
    }

    // -- Lifetime cap pre-check -----------------------------------------------
    if let Some(cap) = sub.lifetime_cap {
        let remaining = if sub.lifetime_charged >= cap {
            0
        } else {
            safe_sub(cap, sub.lifetime_charged)?
        };

        if remaining == 0 || charge_amount > remaining {
            // Cap already exhausted or this charge would exceed it: cancel without
            // moving funds and return an explicit terminal error.
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

            return Ok(ChargeExecutionResult::LifetimeCapReached);
        }
    }

    let storage = env.storage().instance();

    match safe_sub_balance(sub.prepaid_balance, charge_amount) {
        Ok(new_balance) => {
            sub.prepaid_balance = new_balance;
            let fee_bps = crate::admin::get_protocol_fee_bps(env);
            let treasury_opt = crate::admin::get_treasury(env);
            let (merchant_amount, fee_amount) = if fee_bps > 0 {
                if let Some(ref _t) = treasury_opt {
                    let fee = charge_amount * fee_bps as i128 / 10_000i128;
                    let net = charge_amount - fee;
                    (net, fee)
                } else {
                    (charge_amount, 0i128)
                }
            } else {
                (charge_amount, 0i128)
            };
            crate::merchant::credit_merchant_balance_for_token(
                env,
                &sub.merchant,
                &sub.token,
                merchant_amount,
                BillingChargeKind::Interval,
            )?;
            if fee_amount > 0 {
                if let Some(ref treasury) = treasury_opt {
                    crate::merchant::credit_merchant_balance_for_token(
                        env,
                        treasury,
                        &sub.token,
                        fee_amount,
                        BillingChargeKind::Interval,
                    )?;
                    env.events().publish(
                        (Symbol::new(env, "protocol_fee_charged"), subscription_id),
                        crate::types::ProtocolFeeChargedEvent {
                            subscription_id,
                            merchant: sub.merchant.clone(),
                            token: sub.token.clone(),
                            fee_amount,
                            treasury: treasury.clone(),
                            timestamp: now,
                        },
                    );
                }
            }
            sub.last_payment_timestamp = period_start;

            sub.lifetime_charged = safe_add(sub.lifetime_charged, charge_amount)?;

            // Recover from grace period or insufficient balance on successful charge
            if sub.status == SubscriptionStatus::GracePeriod || sub.status == SubscriptionStatus::InsufficientBalance {
                transition_to(&mut sub.status, SubscriptionStatus::Active)?;
            }

            // Check if cap is now exactly reached -- auto-cancel
            let cap_reached = sub
                .lifetime_cap
                .map(|cap| sub.lifetime_charged >= cap)
                .unwrap_or(false);

            if cap_reached {
                transition_to(&mut sub.status, SubscriptionStatus::Cancelled)?;
            }

            storage.set(&DataKey::Sub(subscription_id), &sub);
            append_statement(
                env,
                subscription_id,
                charge_amount,
                sub.merchant.clone(),
                BillingChargeKind::Interval,
                next_allowed.saturating_sub(sub.interval_seconds),
                now,
            )?;

            crate::period_snapshots::write_period_snapshot(
                env,
                BillingPeriodSnapshot {
                    subscription_id,
                    period_index,
                    period_start: next_allowed.saturating_sub(sub.interval_seconds),
                    period_end: now,
                    total_charged: charge_amount,
                    total_usage_units: 0,
                    status_flags: SNAPSHOT_FLAG_CLOSED | SNAPSHOT_FLAG_INTERVAL_CHARGED,
                    finalized_at: now,
                },
            )?;

            // Record charged period and optional idempotency key
            storage.set(&DataKey::ChargedPeriod(subscription_id), &period_index);
            if let Some(k) = idempotency_key {
                storage.set(&DataKey::IdemKey(subscription_id), &k);
            }

            env.events().publish(
                (symbol_short!("charged"),),
                SubscriptionChargedEvent {
                    subscription_id,
                    subscriber: sub.subscriber.clone(),
                    merchant: sub.merchant.clone(),
                    token: sub.token.clone(),
                    amount: charge_amount,
                    lifetime_charged: sub.lifetime_charged,
                    timestamp: now,
                    period_start,
                    period_end,
                },
            );

            if cap_reached {
                if let Some(cap) = sub.lifetime_cap {
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
            }

            Ok(ChargeExecutionResult::Charged)
        }
        Err(_) => {
            let grace_duration = crate::admin::get_grace_period(env)?;
            let due_timestamp = sub
                .last_payment_timestamp
                .checked_add(sub.interval_seconds)
                .ok_or(Error::Overflow)?;

            let grace_expires = due_timestamp
                .checked_add(grace_duration)
                .ok_or(Error::Overflow)?;

            let target_status = if grace_duration > 0 && now < grace_expires {
                SubscriptionStatus::GracePeriod
            } else {
                SubscriptionStatus::InsufficientBalance
            };

            if sub.status != target_status {
                transition_to(&mut sub.status, target_status.clone())?;
            }

            storage.set(&DataKey::Sub(subscription_id), &sub);

            let shortfall = charge_amount.saturating_sub(sub.prepaid_balance).max(0);
            env.events().publish(
                (Symbol::new(env, "charge_failed"), subscription_id),
                SubscriptionChargeFailedEvent {
                    subscription_id,
                    merchant: sub.merchant,
                    required_amount: charge_amount,
                    available_balance: sub.prepaid_balance,
                    shortfall,
                    resulting_status: target_status,
                    timestamp: now,
                },
            );

            Ok(ChargeExecutionResult::InsufficientBalance)
        }
    }
}

/// Debit a metered `usage_amount` from a subscription's prepaid balance.
pub fn charge_usage_one(
    env: &Env,
    subscription_id: u32,
    usage_amount: i128,
    reference: String,
) -> Result<UsageChargeResult, Error> {
    let mut sub = get_subscription(env, subscription_id)?;
    let merchant = sub.merchant.clone();

    if crate::merchant::get_merchant_paused(env, merchant.clone()) {
        return Err(Error::MerchantPaused);
    }

    // Blocklist guard — do not process usage charges for blocklisted subscribers
    if crate::blocklist::is_blocklisted(env, &sub.subscriber) {
        return Err(Error::SubscriberBlocklisted);
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

    if sub.status != SubscriptionStatus::Active {
        return Err(Error::NotActive);
    }

    if !sub.usage_enabled {
        return Err(Error::UsageNotEnabled);
    }

    if usage_amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    if sub.prepaid_balance < usage_amount {
        return Err(Error::InsufficientPrepaidBalance);
    }

    // -- Replay protection (Reference-based) ----------------------------------
    // We use the reference as a unique idempotency key for usage charges.
    // If the reference has been seen before for this subscription, we return Replay.
    let ref_key = (
        Symbol::new(env, "usage_ref"),
        subscription_id,
        reference.clone(),
    );

    if env.storage().instance().has(&ref_key) {
        env.events().publish(
            (Symbol::new(env, "usage_charge_rejected"), subscription_id),
            UsageChargeRejectedEvent {
                subscription_id,
                merchant: sub.merchant.clone(),
                token: sub.token.clone(),
                usage_amount,
                timestamp: now,
                reference,
                result: UsageChargeResult::Replay,
            },
        );
        return Ok(UsageChargeResult::Replay);
    }

    // -- Usage Limits & State -------------------------------------------------
    let now = env.ledger().timestamp();
    let limits_key = DataKey::UsageLimits(subscription_id);
    let maybe_limits: Option<UsageLimits> = env.storage().instance().get(&limits_key);

    if let Some(limits) = maybe_limits {
        let state_key = DataKey::UsageState(subscription_id);
        let mut state = env
            .storage()
            .instance()
            .get(&state_key)
            .unwrap_or(UsageState {
                last_usage_timestamp: 0,
                window_start_timestamp: now,
                window_call_count: 0,
                current_period_usage_units: 0,
                period_index: now.saturating_sub(sub.start_time) / sub.interval_seconds,
            });

        // 1. Burst protection
        if limits.burst_min_interval_secs > 0 {
            let elapsed = now.saturating_sub(state.last_usage_timestamp);
            if elapsed < limits.burst_min_interval_secs {
                env.events().publish(
                    (Symbol::new(env, "usage_charge_rejected"), subscription_id),
                    UsageChargeRejectedEvent {
                        subscription_id,
                        merchant: sub.merchant.clone(),
                        token: sub.token.clone(),
                        usage_amount,
                        timestamp: now,
                        reference,
                        result: UsageChargeResult::BurstLimitExceeded,
                    },
                );
                return Ok(UsageChargeResult::BurstLimitExceeded);
            }
        }

        // 2. Rate limit (sliding window approximate)
        if let Some(max_calls) = limits.rate_limit_max_calls {
            if now
                >= state
                    .window_start_timestamp
                    .saturating_add(limits.rate_window_secs)
            {
                state.window_start_timestamp = now;
                state.window_call_count = 0;
            }
            if state.window_call_count >= max_calls {
                env.events().publish(
                    (Symbol::new(env, "usage_charge_rejected"), subscription_id),
                    UsageChargeRejectedEvent {
                        subscription_id,
                        merchant: sub.merchant.clone(),
                        token: sub.token.clone(),
                        usage_amount,
                        timestamp: now,
                        reference,
                        result: UsageChargeResult::RateLimitExceeded,
                    },
                );
                return Ok(UsageChargeResult::RateLimitExceeded);
            }
        }

        // 3. Usage cap (per-interval)
        if let Some(cap_units) = limits.usage_cap_units {
            let current_period = now.saturating_sub(sub.start_time) / sub.interval_seconds;
            if current_period > state.period_index {
                state.period_index = current_period;
                state.current_period_usage_units = 0;
            }
            if state
                .current_period_usage_units
                .saturating_add(usage_amount)
                > cap_units
            {
                env.events().publish(
                    (Symbol::new(env, "usage_charge_rejected"), subscription_id),
                    UsageChargeRejectedEvent {
                        subscription_id,
                        merchant: sub.merchant.clone(),
                        token: sub.token.clone(),
                        usage_amount,
                        timestamp: now,
                        reference,
                        result: UsageChargeResult::UsageCapExceeded,
                    },
                );
                return Ok(UsageChargeResult::UsageCapExceeded);
            }
        }

        // Update state
        state.last_usage_timestamp = now;
        state.window_call_count = state.window_call_count.saturating_add(1);
        state.current_period_usage_units = state
            .current_period_usage_units
            .saturating_add(usage_amount);
        env.storage().instance().set(&state_key, &state);
    }

    // -- Lifetime cap pre-check -----------------------------------------------
    // Over-cap attempts are blocked and cancel the subscription without debiting funds.
    let pending_lifetime = safe_add(sub.lifetime_charged, usage_amount)?;
    if let Some(cap) = sub.lifetime_cap {
        if pending_lifetime > cap {
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
            return Ok(UsageChargeResult::Charged);
        }
    }

    match crate::safe_math::safe_sub_balance(sub.prepaid_balance, usage_amount) {
        Ok(new_balance) => {
            sub.prepaid_balance = new_balance;
            crate::merchant::credit_merchant_balance_for_token(
                env,
                &sub.merchant,
                &sub.token,
                usage_amount,
                BillingChargeKind::Usage,
            )?;

            sub.lifetime_charged = pending_lifetime;
            let cap_reached = sub
                .lifetime_cap
                .map(|cap| sub.lifetime_charged >= cap)
                .unwrap_or(false);

            if cap_reached {
                transition_to(&mut sub.status, SubscriptionStatus::Cancelled)?;
            } else if new_balance == 0 {
                // Without a cap hit, zero remaining prepaid means underfunded for future usage.
                transition_to(&mut sub.status, SubscriptionStatus::InsufficientBalance)?;
            }

            env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);
            env.storage().instance().set(&ref_key, &true); // Mark reference as used

            append_statement(
                env,
                subscription_id,
                usage_amount,
                sub.merchant.clone(),
                BillingChargeKind::Usage,
                now,
                now,
            )?;

            env.events().publish(
                (Symbol::new(env, "usage_charged"), subscription_id),
                UsageStatementEvent {
                    subscription_id,
                    merchant: sub.merchant.clone(),
                    usage_amount,
                    token: sub.token.clone(),
                    timestamp: now,
                    reference,
                },
            );

            if cap_reached {
                if let Some(cap) = sub.lifetime_cap {
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
            }
            Ok(UsageChargeResult::Charged)
        }
        Err(_) => {
            transition_to(&mut sub.status, SubscriptionStatus::InsufficientBalance)?;
            env.storage().instance().set(&DataKey::Sub(subscription_id), &sub);

            env.events().publish(
                (Symbol::new(env, "charge_failed"), subscription_id),
                SubscriptionChargeFailedEvent {
                    subscription_id,
                    merchant: sub.merchant,
                    required_amount: usage_amount,
                    available_balance: sub.prepaid_balance,
                    shortfall: usage_amount.saturating_sub(sub.prepaid_balance),
                    resulting_status: SubscriptionStatus::InsufficientBalance,
                    timestamp: now,
                },
            );
            Ok(UsageChargeResult::Charged)
        }
    }
}
