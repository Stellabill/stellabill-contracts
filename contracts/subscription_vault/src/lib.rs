#![no_std]

mod types;
mod admin;
mod queries;
mod subscription;
mod state_machine;
mod charge_core;
mod merchant;

pub use types::*;
pub use admin::*;
pub use queries::*;
pub use subscription::*;
pub use state_machine::*;
pub use charge_core::*;
pub use merchant::*;

use soroban_sdk::{contract, contractimpl, Address, Env, Vec};

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    pub fn init(env: Env, token: Address, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin::do_init(&env, token, admin, min_topup)
    }

    pub fn set_min_topup(env: Env, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin::do_set_min_topup(&env, admin, min_topup)
    }

    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        admin::get_min_topup(&env)
    }

    pub fn rotate_admin(env: Env, current_admin: Address, new_admin: Address) -> Result<(), Error> {
        admin::do_rotate_admin(&env, current_admin, new_admin)
    }

    pub fn get_admin(env: Env) -> Result<Address, Error> {
        admin::get_admin(&env)
    }

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

    pub fn deposit_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
    ) -> Result<(), Error> {
        subscription::do_deposit_funds(&env, subscription_id, subscriber, amount)
    }

    pub fn charge_subscription(
        env: Env,
        subscription_id: u32,
    ) -> Result<(), Error> {
        charge_core::charge_one(&env, subscription_id, None)
    }

    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_cancel_subscription(&env, subscription_id, authorizer)
    }

    /// Subscriber withdraws their remaining prepaid_balance after cancellation.
    pub fn withdraw_subscriber_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
    ) -> Result<(), Error> {
        subscription::do_withdraw_subscriber_funds(&env, subscription_id, subscriber)
    }

    /// Pause subscription (no charges until resumed). Allowed from Active.
    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_pause_subscription(&env, subscription_id, authorizer)
    }

    pub fn resume_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_resume_subscription(&env, subscription_id, authorizer)
    }


    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        queries::get_subscription(&env, subscription_id)
    }

    pub fn estimate_topup_for_intervals(
        env: Env,
        subscription_id: u32,
        intervals: u32,
    ) -> Result<i128, Error> {
        queries::estimate_topup_for_intervals(&env, subscription_id, intervals)
    }

    pub fn batch_charge(env: Env, ids: Vec<u32>) -> Result<Vec<BatchChargeResult>, Error> {
        admin::do_batch_charge(&env, &ids)
    }

    pub fn recover_stranded_funds(
        env: Env,
        admin: Address,
        recipient: Address,
        amount: i128,
        reason: RecoveryReason,
    ) -> Result<(), Error> {
        admin::do_recover_stranded_funds(&env, admin, recipient, amount, reason)
    }

    pub fn get_subscriptions_by_merchant(
        env: Env,
        merchant: Address,
        offset: u32,
        limit: u32,
    ) -> Vec<Subscription> {
        queries::get_subscriptions_by_merchant(&env, merchant, offset, limit)
    }

    pub fn get_merchant_subscription_count(env: Env, merchant: Address) -> u32 {
        queries::get_merchant_subscription_count(&env, merchant)
    }

    pub fn get_next_charge_info(env: Env, subscription_id: u32) -> Result<NextChargeInfo, Error> {

        let subscription = Self::get_subscription(env, subscription_id)?;
        Ok(compute_next_charge_info(&subscription))
    }
}

pub fn compute_next_charge_info(subscription: &Subscription) -> NextChargeInfo {
    let next_charge_timestamp = subscription
        .last_payment_timestamp
        .saturating_add(subscription.interval_seconds);

    let is_charge_expected = match subscription.status {
        SubscriptionStatus::Active => true,
        SubscriptionStatus::InsufficientBalance => true,
        SubscriptionStatus::Paused => false,
        SubscriptionStatus::Cancelled => false,
    };

    NextChargeInfo {
        next_charge_timestamp,
        is_charge_expected,
    }
}

#[cfg(test)]
mod test;
