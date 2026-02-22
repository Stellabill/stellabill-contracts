#![no_std]

mod admin;
mod charge_core;
mod merchant;
mod queries;
mod state_machine;
mod subscription;
mod types;

// Re-export types so that `crate::Error`, `crate::Subscription`, etc. work
// for tests and for the Soroban contract ABI.
pub use types::{
    BatchChargeResult, DataKey, Error, FundsDepositedEvent, MerchantWithdrawalEvent,
    OneOffChargedEvent, Subscription, SubscriptionCancelledEvent, SubscriptionChargedEvent,
    SubscriptionCreatedEvent, SubscriptionPausedEvent, SubscriptionResumedEvent,
    SubscriptionStatus,
};

// Re-export state machine helpers so tests (and external callers) can use them.
pub use state_machine::{can_transition, get_allowed_transitions, validate_status_transition};

use soroban_sdk::{contract, contractimpl, Address, Env, Symbol, Vec};

/// Result of computing next charge information for a subscription.
///
/// Contains the estimated next charge timestamp and a flag indicating
/// whether the charge is expected to occur based on the subscription status.
#[soroban_sdk::contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NextChargeInfo {
    /// Estimated timestamp for the next charge attempt.
    pub next_charge_timestamp: u64,
    /// Whether a charge is actually expected based on the subscription status.
    pub is_charge_expected: bool,
}

/// Computes the estimated next charge timestamp for a subscription.
///
/// This is a readonly helper that does not mutate contract state.
pub fn compute_next_charge_info(subscription: &Subscription) -> NextChargeInfo {
    let next_charge_timestamp = subscription
        .last_payment_timestamp
        .saturating_add(subscription.interval_seconds);

    let is_charge_expected = match subscription.status {
        SubscriptionStatus::Active => true,
        SubscriptionStatus::InsufficientBalance => true,
        SubscriptionStatus::GracePeriod => true,
        SubscriptionStatus::Paused => false,
        SubscriptionStatus::Cancelled => false,
    };

    NextChargeInfo {
        next_charge_timestamp,
        is_charge_expected,
    }
}

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    pub fn init(
        env: Env,
        token: Address,
        admin: Address,
        min_topup: i128,
        grace_period: u64,
    ) -> Result<(), Error> {
        admin::do_init(&env, token, admin, min_topup, grace_period)
    }

    /// Update the minimum top-up threshold. Only callable by admin.
    pub fn set_min_topup(env: Env, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin::do_set_min_topup(&env, admin, min_topup)
    }

    /// Rotate admin to a new address. Only callable by current admin.
    pub fn rotate_admin(env: Env, current_admin: Address, new_admin: Address) -> Result<(), Error> {
        current_admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "admin"))
            .ok_or(Error::NotFound)?;

        if current_admin != stored_admin {
            return Err(Error::Unauthorized);
        }

        env.storage()
            .instance()
            .set(&Symbol::new(&env, "admin"), &new_admin);

        env.events().publish(
            (Symbol::new(&env, "admin_rotation"), current_admin.clone()),
            (current_admin, new_admin, env.ledger().timestamp()),
        );

        Ok(())
    }

    /// Get the current admin address.
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "admin"))
            .ok_or(Error::NotFound)
    }

    /// Get the current minimum top-up threshold.
    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        admin::get_min_topup(&env)
    }

    pub fn set_grace_period(env: Env, admin: Address, grace_period: u64) -> Result<(), Error> {
        admin::do_set_grace_period(&env, admin, grace_period)
    }

    pub fn get_grace_period(env: Env) -> Result<u64, Error> {
        admin::get_grace_period(&env)
    }

    /// Create a new subscription.
    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
    ) -> Result<u32, Error> {
        subscription::do_create_subscription(
            &env,
            subscriber,
            merchant,
            amount,
            interval_seconds,
            usage_enabled,
        )
    }

    /// Subscriber deposits more funds into their vault for this subscription.
    pub fn deposit_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
    ) -> Result<(), Error> {
        subscription::do_deposit_funds(&env, subscription_id, subscriber, amount)
    }

    /// Billing engine calls this to charge one interval.
    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        subscription::do_charge_subscription(&env, subscription_id, None)
    }

    /// Cancel subscription. Remaining balance can be withdrawn by subscriber.
    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();

        let mut sub = queries::get_subscription(&env, subscription_id)?;
        validate_status_transition(&sub.status, &SubscriptionStatus::Cancelled)?;
        sub.status = SubscriptionStatus::Cancelled;
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    /// Pause subscription (no charges until resumed).
    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();

        let mut sub = queries::get_subscription(&env, subscription_id)?;
        validate_status_transition(&sub.status, &SubscriptionStatus::Paused)?;
        sub.status = SubscriptionStatus::Paused;
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    /// Resume a subscription to Active status.
    pub fn resume_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();

        let mut sub = queries::get_subscription(&env, subscription_id)?;
        validate_status_transition(&sub.status, &SubscriptionStatus::Active)?;
        sub.status = SubscriptionStatus::Active;
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    pub fn withdraw_merchant_funds(env: Env, merchant: Address, amount: i128) -> Result<(), Error> {
        merchant::withdraw_merchant_funds(&env, merchant, amount)
    }

    /// Read subscription by id (for indexing and UI).
    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        queries::get_subscription(&env, subscription_id)
    }

    /// Estimate how much the subscriber needs to top up to cover `num_intervals` billing cycles.
    pub fn estimate_topup_for_intervals(
        env: Env,
        subscription_id: u32,
        num_intervals: u32,
    ) -> Result<i128, Error> {
        queries::estimate_topup_for_intervals(&env, subscription_id, num_intervals)
    }

    /// Batch charge multiple subscriptions in one call. Admin-only.
    pub fn batch_charge(env: Env, subscription_ids: Vec<u32>) -> Result<Vec<BatchChargeResult>, Error> {
        admin::do_batch_charge(&env, &subscription_ids)
    }

    /// Get estimated next charge information for a subscription.
    pub fn get_next_charge_info(env: Env, subscription_id: u32) -> Result<NextChargeInfo, Error> {
        let subscription = queries::get_subscription(&env, subscription_id)?;
        Ok(compute_next_charge_info(&subscription))
    }

    /// Get subscriptions by merchant (paginated).
    pub fn get_subscriptions_by_merchant(
        env: Env,
        merchant: Address,
        start: u32,
        limit: u32,
    ) -> Vec<Subscription> {
        queries::get_subscriptions_by_merchant(&env, merchant, start, limit)
    }

    /// Get the number of subscriptions for a given merchant.
    pub fn get_merchant_subscription_count(env: Env, merchant: Address) -> u32 {
        queries::get_merchant_subscription_count(&env, merchant)
    }

    /// Merchant-initiated one-off charge.
    pub fn charge_one_off(
        env: Env,
        subscription_id: u32,
        merchant: Address,
        amount: i128,
    ) -> Result<(), Error> {
        subscription::do_charge_one_off(&env, subscription_id, merchant, amount)
    }
}

#[cfg(test)]
mod test;
