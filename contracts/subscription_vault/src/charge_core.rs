//! Single charge logic (no auth). Used by charge_subscription and batch_charge.
//!
//! Charge runs only when status is Active; on insufficient balance the subscription
//! transitions to InsufficientBalance. See `docs/subscription_lifecycle.md` for details.
//!
//! **PRs that only change how one subscription is charged should edit this file only.**

use crate::queries::get_subscription;
use crate::safe_math::safe_sub_balance;
use crate::state_machine::validate_status_transition;
use crate::types::{Error, SubscriptionStatus};
use soroban_sdk::Env;

pub fn charge_one(env: &Env, subscription_id: u32) -> Result<(), Error> {
    let mut sub = get_subscription(env, subscription_id)?;

    if sub.status != SubscriptionStatus::Active {
        return Err(Error::NotActive);
    }

    let now = env.ledger().timestamp();
    let next_allowed = sub
        .last_payment_timestamp
        .checked_add(sub.interval_seconds)
        .ok_or(Error::Overflow)?;
    if now < next_allowed {
        return Err(Error::IntervalNotElapsed);
    }

    match safe_sub_balance(sub.prepaid_balance, sub.amount) {
        Ok(new_balance) => {
            sub.prepaid_balance = new_balance;
            sub.last_payment_timestamp = now;
            env.storage().instance().set(&subscription_id, &sub);
            Ok(())
        }
        Err(_) => {
            validate_status_transition(&sub.status, &SubscriptionStatus::InsufficientBalance)?;
            sub.status = SubscriptionStatus::InsufficientBalance;
            env.storage().instance().set(&subscription_id, &sub);
            Err(Error::InsufficientBalance)
        }
    }
}
