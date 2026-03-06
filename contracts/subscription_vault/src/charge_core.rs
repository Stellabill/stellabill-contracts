//! Single charge logic (no auth). Used by charge_subscription and batch_charge.
//!
//! Charge runs only when status is Active or GracePeriod; on insufficient balance the
//! subscription transitions to InsufficientBalance. On lifetime cap exhaustion the
//! subscription is cancelled (terminal state).
//!
//! See `docs/subscription_lifecycle.md` for lifecycle details.
//! See `docs/lifetime_caps.md` for cap enforcement semantics.
//!
//! **PRs that only change how one subscription is charged should edit this file only.**
//!
//! # Replay protection and idempotency
//!
//! Charges are protected against replay by:
//! - **Period-based key**: We record the last charged billing period index per subscription.
//!   A charge for the same period is rejected with [`Error::Replay`].
//! - **Optional idempotency key**: If the caller supplies an idempotency key (e.g. for retries),
//!   we store one key per subscription. A second call with the same key returns `Ok(())` without
//!   debiting again (idempotent success). Storage stays bounded (one key and one period per sub).
//!
//! # Reentrancy Protection
//!
//! This module does **NOT** make external calls to the token contract. All balance updates
//! are internal:
//! - Subscriber prepaid balance is debited locally
//! - Merchant balance is credited locally via [`crate::merchant::credit_merchant_balance`]
//!
//! Because there are no external calls, there is **no reentrancy risk** in this module.
//! See `docs/reentrancy.md` for the full reentrancy threat model and mitigation strategy.

#![allow(dead_code)]

use crate::queries::get_subscription;
use crate::safe_math::safe_sub_balance;
use crate::state_machine::validate_status_transition;
use crate::types::LifetimeCapReachedEvent;
use crate::types::{
    BillingPeriodSnapshot, Error, ProtocolFeeSkimmedEvent, SubscriptionChargedEvent,
    SubscriptionStatus, UsageCapReachedEvent, BILLING_SNAPSHOT_FLAG_CLOSED,
    BILLING_SNAPSHOT_FLAG_EMPTY_PERIOD, BILLING_SNAPSHOT_FLAG_INTERVAL_CHARGED,
    BILLING_SNAPSHOT_FLAG_USAGE_CHARGED,
};
use soroban_sdk::{symbol_short, Env, Symbol};

const KEY_CHARGED_PERIOD: Symbol = symbol_short!("cp");
const KEY_IDEM: Symbol = symbol_short!("idem");
const KEY_PERIOD_AMT: Symbol = symbol_short!("pamt");
const KEY_PERIOD_USE: Symbol = symbol_short!("puse");
const KEY_PERIOD_FLG: Symbol = symbol_short!("pflg");
const KEY_RATE_START: Symbol = symbol_short!("rstr");
const KEY_RATE_CALLS: Symbol = symbol_short!("rcll");

fn charged_period_key(subscription_id: u32) -> (Symbol, u32) {
    (KEY_CHARGED_PERIOD, subscription_id)
}

fn idem_key(subscription_id: u32) -> (Symbol, u32) {
    (KEY_IDEM, subscription_id)
}

fn period_amount_key(subscription_id: u32) -> (Symbol, u32) {
    (KEY_PERIOD_AMT, subscription_id)
}

fn period_usage_key(subscription_id: u32) -> (Symbol, u32) {
    (KEY_PERIOD_USE, subscription_id)
}

fn period_flags_key(subscription_id: u32) -> (Symbol, u32) {
    (KEY_PERIOD_FLG, subscription_id)
}

fn rate_window_start_key(subscription_id: u32) -> (Symbol, u32) {
    (KEY_RATE_START, subscription_id)
}

fn rate_window_calls_key(subscription_id: u32) -> (Symbol, u32) {
    (KEY_RATE_CALLS, subscription_id)
}

fn period_index_for(sub: &crate::types::Subscription, now: u64) -> u32 {
    if now <= sub.billing_anchor_timestamp {
        return 0;
    }
    ((now - sub.billing_anchor_timestamp) / sub.interval_seconds) as u32
}

fn close_elapsed_periods_on_success(
    env: &Env,
    subscription_id: u32,
    sub: &mut crate::types::Subscription,
    now: u64,
) -> Result<(), Error> {
    let now_period = period_index_for(sub, now);
    if now_period <= sub.current_period_index {
        return Ok(());
    }
    let storage = env.storage().instance();
    let amount = storage
        .get::<_, i128>(&period_amount_key(subscription_id))
        .unwrap_or(0);
    let usage = storage
        .get::<_, i128>(&period_usage_key(subscription_id))
        .unwrap_or(0);
    let mut flags = storage
        .get::<_, u32>(&period_flags_key(subscription_id))
        .unwrap_or(0);
    flags |= BILLING_SNAPSHOT_FLAG_CLOSED;
    if amount == 0 && usage == 0 {
        flags |= BILLING_SNAPSHOT_FLAG_EMPTY_PERIOD;
    }

    let start = sub
        .billing_anchor_timestamp
        .saturating_add((sub.current_period_index as u64).saturating_mul(sub.interval_seconds));
    let end = start.saturating_add(sub.interval_seconds);
    storage.set(
        &(
            Symbol::new(env, "bps"),
            subscription_id,
            sub.current_period_index,
        ),
        &BillingPeriodSnapshot {
            subscription_id,
            period_index: sub.current_period_index,
            period_start_timestamp: start,
            period_end_timestamp: end,
            total_amount_charged: amount,
            total_usage_units: usage,
            status_flags: flags,
        },
    );

    storage.set(&period_amount_key(subscription_id), &0i128);
    storage.set(&period_usage_key(subscription_id), &0i128);
    storage.set(&period_flags_key(subscription_id), &0u32);

    let mut idx = sub.current_period_index + 1;
    while idx < now_period {
        let s = sub
            .billing_anchor_timestamp
            .saturating_add((idx as u64).saturating_mul(sub.interval_seconds));
        let e = s.saturating_add(sub.interval_seconds);
        storage.set(
            &(Symbol::new(env, "bps"), subscription_id, idx),
            &BillingPeriodSnapshot {
                subscription_id,
                period_index: idx,
                period_start_timestamp: s,
                period_end_timestamp: e,
                total_amount_charged: 0,
                total_usage_units: 0,
                status_flags: BILLING_SNAPSHOT_FLAG_CLOSED | BILLING_SNAPSHOT_FLAG_EMPTY_PERIOD,
            },
        );
        idx += 1;
    }
    sub.current_period_index = now_period;
    sub.current_period_usage_units = 0;
    Ok(())
}

fn add_period_totals(
    env: &Env,
    subscription_id: u32,
    amount_delta: i128,
    usage_delta: i128,
    flags: u32,
) -> Result<(), Error> {
    let storage = env.storage().instance();
    let amount = storage
        .get::<_, i128>(&period_amount_key(subscription_id))
        .unwrap_or(0)
        .checked_add(amount_delta)
        .ok_or(Error::Overflow)?;
    let usage = storage
        .get::<_, i128>(&period_usage_key(subscription_id))
        .unwrap_or(0)
        .checked_add(usage_delta)
        .ok_or(Error::Overflow)?;
    let f = storage
        .get::<_, u32>(&period_flags_key(subscription_id))
        .unwrap_or(0)
        | flags;
    storage.set(&period_amount_key(subscription_id), &amount);
    storage.set(&period_usage_key(subscription_id), &usage);
    storage.set(&period_flags_key(subscription_id), &f);
    Ok(())
}

fn enforce_usage_rate_limit(
    env: &Env,
    subscription_id: u32,
    sub: &crate::types::Subscription,
    now: u64,
) -> Result<(), Error> {
    let Some(max_calls) = sub.usage_rate_limit_max_calls else {
        return Ok(());
    };
    if max_calls == 0 || sub.usage_rate_window_secs == 0 {
        return Err(Error::InvalidInput);
    }
    let storage = env.storage().instance();
    let start = storage
        .get::<_, u64>(&rate_window_start_key(subscription_id))
        .unwrap_or(now);
    let mut calls = storage
        .get::<_, u32>(&rate_window_calls_key(subscription_id))
        .unwrap_or(0);
    let (new_start, new_calls) = if now.saturating_sub(start) >= sub.usage_rate_window_secs {
        (now, 1)
    } else {
        if calls >= max_calls {
            return Err(Error::RateLimitExceeded);
        }
        calls += 1;
        (start, calls)
    };
    storage.set(&rate_window_start_key(subscription_id), &new_start);
    storage.set(&rate_window_calls_key(subscription_id), &new_calls);
    Ok(())
}

/// Performs a single interval-based charge with optional replay protection.
///
/// # Lifetime Cap Enforcement
///
/// When a subscription has a `lifetime_cap` configured:
/// 1. Before charging, the remaining cap (`cap - lifetime_charged`) is checked.
/// 2. If the remaining cap is already zero, returns [`Error::LifetimeCapReached`]
///    and transitions the subscription to `Cancelled`.
/// 3. If `amount > remaining_cap`, returns [`Error::LifetimeCapReached`] and
///    cancels the subscription — partial charges are not issued.
/// 4. On a successful charge, `lifetime_charged` is incremented by `amount`.
/// 5. After the charge, if `lifetime_charged == lifetime_cap`, the subscription is
///    cancelled and a [`LifetimeCapReachedEvent`] is emitted.
///
/// # Idempotency
///
/// - If `idempotency_key` is `Some(k)` and we already processed this subscription with key `k`,
///   returns `Ok(())` without changing state (idempotent success).
/// - Otherwise we derive a period from `now / interval_seconds`. If this period was already
///   charged, returns `Err(Error::Replay)`.
///
/// # Storage
///
/// Bounded: one `u64` (last charged period) and optionally one idempotency key per subscription.
pub fn charge_one(
    env: &Env,
    subscription_id: u32,
    now: u64,
    idempotency_key: Option<soroban_sdk::BytesN<32>>,
) -> Result<(), Error> {
    let mut sub = get_subscription(env, subscription_id)?;
    if let Some(exp_ts) = sub.expiration {
        if now >= exp_ts {
            return Err(Error::SubscriptionExpired);
        }
    }

    if sub.status != SubscriptionStatus::Active && sub.status != SubscriptionStatus::GracePeriod {
        return Err(Error::NotActive);
    }

    let period_index = now / sub.interval_seconds;

    // Idempotent return: same idempotency key already processed for this subscription
    if let Some(ref k) = idempotency_key {
        if let Some(stored) = env
            .storage()
            .instance()
            .get::<_, soroban_sdk::BytesN<32>>(&idem_key(subscription_id))
        {
            if stored == *k {
                return Ok(());
            }
        }
    }

    // Replay: already charged for this billing period (derived key)
    if let Some(stored_period) = env
        .storage()
        .instance()
        .get::<_, u64>(&charged_period_key(subscription_id))
    {
        if period_index <= stored_period {
            return Err(Error::Replay);
        }
    }

    let next_allowed = sub
        .last_payment_timestamp
        .checked_add(sub.interval_seconds)
        .ok_or(Error::Overflow)?;
    if now < next_allowed {
        return Err(Error::IntervalNotElapsed);
    }

    // ── Lifetime cap pre-check ────────────────────────────────────────────────
    // NOTE: In Soroban, state changes are rolled back when a function returns
    // an error. Therefore, when the cap pre-check fires (charge would exceed cap),
    // we cancel the subscription and return Ok(()) so the cancellation persists.
    // The billing engine detects this via the LifetimeCapReachedEvent and by
    // checking the subscription status (Cancelled).
    if let Some(cap) = sub.lifetime_cap {
        let remaining = cap.checked_sub(sub.lifetime_charged).unwrap_or(0).max(0);

        if remaining == 0 || sub.amount > remaining {
            // Cap already exhausted or this charge would exceed it — cancel.
            validate_status_transition(&sub.status, &SubscriptionStatus::Cancelled)?;
            sub.status = SubscriptionStatus::Cancelled;
            env.storage().instance().set(&subscription_id, &sub);

            env.events().publish(
                (Symbol::new(env, "lifetime_cap_reached"), subscription_id),
                LifetimeCapReachedEvent {
                    subscription_id,
                    lifetime_cap: cap,
                    lifetime_charged: sub.lifetime_charged,
                    timestamp: now,
                },
            );

            // Return Ok so Soroban persists the Cancelled state.
            // The caller detects cap-cancellation via the event and subscription status.
            return Ok(());
        }
    }
    // ─────────────────────────────────────────────────────────────────────────

    let storage = env.storage().instance();

    match safe_sub_balance(sub.prepaid_balance, sub.amount) {
        Ok(new_balance) => {
            close_elapsed_periods_on_success(env, subscription_id, &mut sub, now)?;
            sub.prepaid_balance = new_balance;
            let (net_amount, fee_amount) =
                crate::merchant::credit_merchant_and_treasury(env, &sub.merchant, sub.amount)?;
            sub.last_payment_timestamp = now;

            // Accumulate lifetime charged amount
            sub.lifetime_charged = sub
                .lifetime_charged
                .checked_add(sub.amount)
                .ok_or(Error::Overflow)?;

            // Recover from grace period on successful charge
            if sub.status == SubscriptionStatus::GracePeriod {
                validate_status_transition(&sub.status, &SubscriptionStatus::Active)?;
                sub.status = SubscriptionStatus::Active;
            }
            add_period_totals(
                env,
                subscription_id,
                sub.amount,
                0,
                BILLING_SNAPSHOT_FLAG_INTERVAL_CHARGED,
            )?;

            // Check if cap is now exactly reached — auto-cancel
            let cap_reached = sub
                .lifetime_cap
                .map(|cap| sub.lifetime_charged >= cap)
                .unwrap_or(false);

            if cap_reached {
                validate_status_transition(&sub.status, &SubscriptionStatus::Cancelled)?;
                sub.status = SubscriptionStatus::Cancelled;
            }

            storage.set(&subscription_id, &sub);

            // Record charged period and optional idempotency key (bounded storage)
            storage.set(&charged_period_key(subscription_id), &period_index);
            if let Some(k) = idempotency_key {
                storage.set(&idem_key(subscription_id), &k);
            }

            // Emit charge event
            env.events().publish(
                (symbol_short!("charged"),),
                SubscriptionChargedEvent {
                    subscription_id,
                    merchant: sub.merchant.clone(),
                    amount: sub.amount,
                    lifetime_charged: sub.lifetime_charged,
                },
            );
            if fee_amount > 0 {
                let treasury: soroban_sdk::Address = env
                    .storage()
                    .instance()
                    .get(&Symbol::new(env, "treasury"))
                    .ok_or(Error::TreasuryNotConfigured)?;
                env.events().publish(
                    (symbol_short!("fee"),),
                    ProtocolFeeSkimmedEvent {
                        subscription_id,
                        merchant: sub.merchant.clone(),
                        treasury,
                        gross_amount: sub.amount,
                        fee_amount,
                        net_amount,
                    },
                );
            }

            // Emit cap-reached event if applicable
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

            Ok(())
        }
        Err(_) => {
            // Insufficient balance — check if grace period applies
            let grace_duration = crate::admin::get_grace_period(env).unwrap_or(0);
            let grace_expires = next_allowed
                .checked_add(grace_duration)
                .ok_or(Error::Overflow)?;

            if grace_duration > 0 && now < grace_expires {
                if sub.status != SubscriptionStatus::GracePeriod {
                    validate_status_transition(&sub.status, &SubscriptionStatus::GracePeriod)?;
                    sub.status = SubscriptionStatus::GracePeriod;
                    storage.set(&subscription_id, &sub);
                }
                Err(Error::InsufficientBalance)
            } else {
                validate_status_transition(&sub.status, &SubscriptionStatus::InsufficientBalance)?;
                sub.status = SubscriptionStatus::InsufficientBalance;
                storage.set(&subscription_id, &sub);
                Err(Error::InsufficientBalance)
            }
        }
    }
}

/// Debit a metered `usage_amount` from a subscription's prepaid balance.
///
/// # Lifetime Cap Enforcement
///
/// If a lifetime cap is configured, `usage_amount` must not cause `lifetime_charged`
/// to exceed `lifetime_cap`. If it would, the subscription is cancelled and
/// [`Error::LifetimeCapReached`] is returned.
///
/// # Shared safety checks
///
/// * Subscription must exist (`NotFound`).
/// * Subscription must be `Active` (`NotActive`).
/// * `usage_enabled` must be `true` (`UsageNotEnabled`).
/// * `usage_amount` must be positive (`InvalidAmount`).
/// * `prepaid_balance >= usage_amount` (`InsufficientPrepaidBalance`).
///
/// On success the prepaid balance is reduced. If the balance reaches zero the
/// subscription transitions to `InsufficientBalance`, blocking further charges
/// until the subscriber tops up.
pub fn charge_usage_one(env: &Env, subscription_id: u32, usage_amount: i128) -> Result<(), Error> {
    let mut sub = get_subscription(env, subscription_id)?;
    let now = env.ledger().timestamp();

    if sub.status != SubscriptionStatus::Active {
        return Err(Error::NotActive);
    }

    if !sub.usage_enabled {
        return Err(Error::UsageNotEnabled);
    }

    if usage_amount <= 0 {
        return Err(Error::InvalidAmount);
    }
    enforce_usage_rate_limit(env, subscription_id, &sub, now)?;
    close_elapsed_periods_on_success(env, subscription_id, &mut sub, now)?;

    let next_usage = sub
        .current_period_usage_units
        .checked_add(usage_amount)
        .ok_or(Error::Overflow)?;
    if let Some(cap) = sub.usage_cap_units {
        if next_usage > cap {
            env.events().publish(
                (symbol_short!("usage_cap"),),
                UsageCapReachedEvent {
                    subscription_id,
                    period_index: sub.current_period_index,
                    cap_units: cap,
                    attempted_units: next_usage,
                },
            );
            return Err(Error::UsageCapExceeded);
        }
    }

    if sub.prepaid_balance < usage_amount {
        return Err(Error::InsufficientPrepaidBalance);
    }

    // ── Lifetime cap pre-check ────────────────────────────────────────────────
    // See charge_one for rationale on returning Ok() here.
    if let Some(cap) = sub.lifetime_cap {
        let new_charged = sub
            .lifetime_charged
            .checked_add(usage_amount)
            .ok_or(Error::Overflow)?;
        if new_charged > cap {
            validate_status_transition(&sub.status, &SubscriptionStatus::Cancelled)?;
            sub.status = SubscriptionStatus::Cancelled;
            env.storage().instance().set(&subscription_id, &sub);

            let now = env.ledger().timestamp();
            env.events().publish(
                (Symbol::new(env, "lifetime_cap_reached"), subscription_id),
                LifetimeCapReachedEvent {
                    subscription_id,
                    lifetime_cap: cap,
                    lifetime_charged: sub.lifetime_charged,
                    timestamp: now,
                },
            );

            return Ok(());
        }
        sub.lifetime_charged = new_charged;
    }
    // ─────────────────────────────────────────────────────────────────────────

    sub.prepaid_balance = sub
        .prepaid_balance
        .checked_sub(usage_amount)
        .ok_or(Error::Overflow)?;
    sub.current_period_usage_units = next_usage;
    add_period_totals(
        env,
        subscription_id,
        usage_amount,
        usage_amount,
        BILLING_SNAPSHOT_FLAG_USAGE_CHARGED,
    )?;
    let (net_amount, fee_amount) =
        crate::merchant::credit_merchant_and_treasury(env, &sub.merchant, usage_amount)?;
    if fee_amount > 0 {
        let treasury: soroban_sdk::Address = env
            .storage()
            .instance()
            .get(&Symbol::new(env, "treasury"))
            .ok_or(Error::TreasuryNotConfigured)?;
        env.events().publish(
            (symbol_short!("fee"),),
            ProtocolFeeSkimmedEvent {
                subscription_id,
                merchant: sub.merchant.clone(),
                treasury,
                gross_amount: usage_amount,
                fee_amount,
                net_amount,
            },
        );
    }

    // If the vault is now empty, transition to InsufficientBalance so no
    // further charges (interval or usage) can proceed until top-up.
    if sub.prepaid_balance == 0 {
        validate_status_transition(&sub.status, &SubscriptionStatus::InsufficientBalance)?;
        sub.status = SubscriptionStatus::InsufficientBalance;
    }

    // Check if cap is exactly reached after usage charge
    let cap_reached = sub
        .lifetime_cap
        .map(|cap| sub.lifetime_charged >= cap)
        .unwrap_or(false);

    if cap_reached {
        // Cancel even if balance is > 0 (cap overrides balance)
        validate_status_transition(&sub.status, &SubscriptionStatus::Cancelled)?;
        sub.status = SubscriptionStatus::Cancelled;

        if let Some(cap) = sub.lifetime_cap {
            let now = env.ledger().timestamp();
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

    env.storage().instance().set(&subscription_id, &sub);
    Ok(())
}
