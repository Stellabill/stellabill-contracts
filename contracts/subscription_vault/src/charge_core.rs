//! Single charge logic (no auth). Used by charge_subscription and batch_charge.
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

use crate::queries::get_subscription;
use crate::state_machine::validate_status_transition;
use crate::types::{Error, SubscriptionChargedEvent, SubscriptionStatus};
use soroban_sdk::{symbol_short, Env, Symbol};

const KEY_CHARGED_PERIOD: Symbol = symbol_short!("cp");
const KEY_IDEM: Symbol = symbol_short!("idem");

fn charged_period_key(subscription_id: u32) -> (Symbol, u32) {
    (KEY_CHARGED_PERIOD, subscription_id)
}

fn idem_key(subscription_id: u32) -> (Symbol, u32) {
    (KEY_IDEM, subscription_id)
}

/// Performs a single interval-based charge with optional replay protection.
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
    idempotency_key: Option<soroban_sdk::BytesN<32>>,
) -> Result<(), Error> {
    let mut sub = get_subscription(env, subscription_id)?;

    if sub.status != SubscriptionStatus::Active {
        return Err(Error::NotActive);
    }

    let now = env.ledger().timestamp();
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

    if sub.prepaid_balance < sub.amount {
        validate_status_transition(&sub.status, &SubscriptionStatus::InsufficientBalance)?;
        sub.status = SubscriptionStatus::InsufficientBalance;
        env.storage().instance().set(&subscription_id, &sub);
        return Err(Error::InsufficientBalance);
    }

    sub.prepaid_balance = sub
        .prepaid_balance
        .checked_sub(sub.amount)
        .ok_or(Error::Overflow)?;
    sub.last_payment_timestamp = now;
    env.storage().instance().set(&subscription_id, &sub);

    // Record charged period and optional idempotency key (bounded storage)
    env.storage()
        .instance()
        .set(&charged_period_key(subscription_id), &period_index);
    if let Some(k) = idempotency_key {
        env.storage().instance().set(&idem_key(subscription_id), &k);
    }

    env.events().publish(
        (symbol_short!("charged"),),
        SubscriptionChargedEvent {
            subscription_id,
            merchant: sub.merchant.clone(),
            amount: sub.amount,
        },
    );

    Ok(())
}
