#![no_std]

mod admin;
mod charge_core;
mod merchant;
mod queries;
mod state_machine;
mod subscription;
mod types;

use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env, Vec};

pub use state_machine::{can_transition, get_allowed_transitions, validate_status_transition};
pub use types::{
    BatchChargeResult, Error, FundsDepositedEvent, MerchantWithdrawalEvent, Subscription,
    SubscriptionCancelledEvent, SubscriptionChargedEvent, SubscriptionCreatedEvent,
    SubscriptionPausedEvent, SubscriptionResumedEvent, SubscriptionStatus,
};

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

    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        subscription::do_charge_subscription(&env, subscription_id)
    }

    pub fn estimate_topup_for_intervals(
        env: Env,
        subscription_id: u32,
        num_intervals: u32,
    ) -> Result<i128, Error> {
        queries::estimate_topup_for_intervals(&env, subscription_id, num_intervals)
    }

    pub fn batch_charge(
        env: Env,
        subscription_ids: Vec<u32>,
    ) -> Result<Vec<BatchChargeResult>, Error> {
        admin::do_batch_charge(&env, &subscription_ids)
    }

    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;

        validate_status_transition(&sub.status, &SubscriptionStatus::Cancelled)?;

        let refund = sub.prepaid_balance;
        sub.status = SubscriptionStatus::Cancelled;
        env.storage().instance().set(&subscription_id, &sub);

        env.events().publish(
            (symbol_short!("cancelled"),),
            SubscriptionCancelledEvent {
                subscription_id,
                authorizer,
                refund_amount: refund,
            },
        );

        Ok(())
    }

    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;

        validate_status_transition(&sub.status, &SubscriptionStatus::Paused)?;

        sub.status = SubscriptionStatus::Paused;
        env.storage().instance().set(&subscription_id, &sub);

        env.events().publish(
            (symbol_short!("paused"),),
            SubscriptionPausedEvent {
                subscription_id,
                authorizer,
            },
        );

        Ok(())
    }

    pub fn resume_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;

        validate_status_transition(&sub.status, &SubscriptionStatus::Active)?;

        sub.status = SubscriptionStatus::Active;
        env.storage().instance().set(&subscription_id, &sub);

        env.events().publish(
            (symbol_short!("resumed"),),
            SubscriptionResumedEvent {
                subscription_id,
                authorizer,
            },
        );

        Ok(())
    }

    pub fn withdraw_merchant_funds(env: Env, merchant: Address, amount: i128) -> Result<(), Error> {
        merchant::withdraw_merchant_funds(&env, merchant, amount)
    }

    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        queries::get_subscription(&env, subscription_id)
    }
}

#[cfg(test)]
mod test;
