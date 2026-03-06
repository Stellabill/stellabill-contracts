//! On-chain referral rewards ledger.
//!
//! Tracks referral relationships established at subscription creation time and
//! credits one-time rewards to referrers upon the first successful interval charge.
//!
//! # Design overview
//!
//! - **Config** (`DataKey::ReferralCfg`): global reward rate (basis points) and
//!   optional per-referral ceiling, managed by admin via `configure_referral_program`.
//! - **Record** (`DataKey::Referral(sub_id)`): per-subscription record linking a
//!   referrer address and tracking whether the reward has been credited.
//! - **Balance** (`DataKey::ReferralBalance(referrer)`): per-referrer accumulated
//!   reward balance, separate from merchant earnings and subscriber prepaid vaults.
//!
//! # Reward computation
//!
//! ```text
//! raw_reward  = charge_amount × reward_bps / 10_000
//! final_reward = min(raw_reward, max_reward)   // if max_reward is Some
//! ```
//!
//! # Qualifying event
//!
//! The reward is credited exactly once: on the first successful subscription
//! interval charge after a referral is registered. If the referral program is
//! disabled at that moment, the reward is skipped and the record is still marked
//! as processed so no future charge can re-trigger it.
//!
//! # Fund separation
//!
//! Referral balances live under `DataKey::ReferralBalance(address)` and are
//! entirely independent of:
//! - Merchant earnings: `("merchant_balance", address)` keys
//! - Subscriber prepaid vaults: fields within `Subscription.prepaid_balance`
//!
//! # Reentrancy protection
//!
//! `do_withdraw_referral_rewards` follows the Checks-Effects-Interactions (CEI)
//! pattern: balance is zeroed in storage before the token transfer is made.

use crate::types::{
    DataKey, Error, ReferralConfig, ReferralRecord, ReferralRegisteredEvent,
    ReferralRewardCreditedEvent, ReferralRewardWithdrawnEvent,
};
use soroban_sdk::{Address, Env, Symbol};

// ── Storage helpers ───────────────────────────────────────────────────────────

pub fn get_referral_config(env: &Env) -> Option<ReferralConfig> {
    env.storage().instance().get(&DataKey::ReferralCfg)
}

fn set_referral_config(env: &Env, config: &ReferralConfig) {
    env.storage().instance().set(&DataKey::ReferralCfg, config);
}

pub fn get_referral_record(env: &Env, subscription_id: u32) -> Option<ReferralRecord> {
    env.storage()
        .instance()
        .get(&DataKey::Referral(subscription_id))
}

fn set_referral_record(env: &Env, subscription_id: u32, record: &ReferralRecord) {
    env.storage()
        .instance()
        .set(&DataKey::Referral(subscription_id), record);
}

pub fn get_referral_balance(env: &Env, referrer: &Address) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::ReferralBalance(referrer.clone()))
        .unwrap_or(0i128)
}

fn set_referral_balance(env: &Env, referrer: &Address, balance: i128) {
    env.storage()
        .instance()
        .set(&DataKey::ReferralBalance(referrer.clone()), &balance);
}

// ── Admin: configure program ──────────────────────────────────────────────────

/// Set the referral program parameters. Admin only.
///
/// # Arguments
/// * `reward_bps`  – Reward rate in basis points (0–10 000). 500 = 5 %.
/// * `max_reward`  – Optional ceiling per referral in token base units.
/// * `enabled`     – Activate or deactivate the program.
///
/// # Errors
/// - [`Error::NotInitialized`] if the contract has not been initialised.
/// - [`Error::Unauthorized`] if `admin` does not match the stored admin.
/// - [`Error::InvalidInput`] if `reward_bps` exceeds 10 000.
/// - [`Error::InvalidAmount`] if `max_reward` is `Some(≤0)`.
pub fn do_configure_referral_program(
    env: &Env,
    admin: Address,
    reward_bps: u32,
    max_reward: Option<i128>,
    enabled: bool,
) -> Result<(), Error> {
    admin.require_auth();

    let stored_admin: Address = env
        .storage()
        .instance()
        .get(&Symbol::new(env, "admin"))
        .ok_or(Error::NotInitialized)?;

    if admin != stored_admin {
        return Err(Error::Unauthorized);
    }

    if reward_bps > 10_000 {
        return Err(Error::InvalidInput);
    }

    if let Some(cap) = max_reward {
        if cap <= 0 {
            return Err(Error::InvalidAmount);
        }
    }

    let config = ReferralConfig {
        reward_bps,
        max_reward,
        enabled,
    };
    set_referral_config(env, &config);

    env.events().publish(
        (Symbol::new(env, "ref_cfg_set"),),
        (reward_bps, max_reward, enabled),
    );

    Ok(())
}

// ── Subscriber: register a referrer ──────────────────────────────────────────

/// Register a referrer address for a subscription.
///
/// Must be called by the subscription's own subscriber. Only one referrer may
/// be registered per subscription, and only before the first charge is credited.
/// Self-referral (referrer == subscriber) is rejected.
///
/// Registration succeeds regardless of whether the referral program is currently
/// enabled; the program state is evaluated at reward-credit time.
///
/// # Errors
/// - [`Error::NotFound`] if the subscription does not exist.
/// - [`Error::Forbidden`] if the caller is not the subscription's subscriber.
/// - [`Error::ReferralAlreadyRegistered`] if a referrer was already registered.
/// - [`Error::InvalidInput`] if `referrer == subscriber` (self-referral).
pub fn do_register_referral(
    env: &Env,
    subscription_id: u32,
    subscriber: Address,
    referrer: Address,
) -> Result<(), Error> {
    subscriber.require_auth();

    let sub = crate::queries::get_subscription(env, subscription_id)?;

    if sub.subscriber != subscriber {
        return Err(Error::Forbidden);
    }

    if referrer == subscriber {
        return Err(Error::InvalidInput);
    }

    if get_referral_record(env, subscription_id).is_some() {
        return Err(Error::ReferralAlreadyRegistered);
    }

    let record = ReferralRecord {
        referrer: referrer.clone(),
        rewarded: false,
        reward_amount: 0,
    };
    set_referral_record(env, subscription_id, &record);

    env.events().publish(
        (Symbol::new(env, "ref_registered"), subscription_id),
        ReferralRegisteredEvent {
            subscription_id,
            subscriber,
            referrer,
        },
    );

    Ok(())
}

// ── Internal: credit reward on first charge ───────────────────────────────────

/// Attempt to credit a referral reward after a successful charge.
///
/// Called by [`crate::charge_core::charge_one`] immediately after every
/// successful interval charge. Silently returns `Ok(())` when:
/// - No referral record exists for the subscription.
/// - The record has already been rewarded (`record.rewarded == true`).
/// - No referral program is configured.
/// - The program is disabled.
/// - The computed reward rounds to zero.
///
/// When the program IS active and the reward is positive, the referrer's balance
/// is credited and the record is marked `rewarded = true`, preventing any future
/// charge from triggering a second credit.
///
/// # Errors
/// - [`Error::Overflow`] if reward arithmetic overflows `i128`.
pub fn try_credit_referral_reward(
    env: &Env,
    subscription_id: u32,
    charge_amount: i128,
) -> Result<(), Error> {
    let Some(mut record) = get_referral_record(env, subscription_id) else {
        return Ok(());
    };

    // Mark as processed on any first-charge path (even if reward is zero or
    // program is disabled) so subsequent charges never re-evaluate.
    if record.rewarded {
        return Ok(());
    }

    // Retrieve config; if absent, mark rewarded and exit.
    let Some(config) = get_referral_config(env) else {
        record.rewarded = true;
        set_referral_record(env, subscription_id, &record);
        return Ok(());
    };

    // If the program is disabled, still mark rewarded to close the window.
    if !config.enabled || config.reward_bps == 0 {
        record.rewarded = true;
        set_referral_record(env, subscription_id, &record);
        return Ok(());
    }

    // reward = charge_amount × reward_bps / 10_000, capped by max_reward.
    let raw_reward = charge_amount
        .checked_mul(config.reward_bps as i128)
        .ok_or(Error::Overflow)?
        / 10_000i128;

    let reward = match config.max_reward {
        Some(cap) => raw_reward.min(cap),
        None => raw_reward,
    };

    // Mark the record as rewarded regardless of reward amount.
    record.rewarded = true;
    record.reward_amount = reward;
    set_referral_record(env, subscription_id, &record);

    if reward <= 0 {
        return Ok(());
    }

    // Credit the referrer's isolated balance.
    let current = get_referral_balance(env, &record.referrer);
    let new_balance = current.checked_add(reward).ok_or(Error::Overflow)?;
    set_referral_balance(env, &record.referrer, new_balance);

    env.events().publish(
        (Symbol::new(env, "ref_rewarded"), subscription_id),
        ReferralRewardCreditedEvent {
            subscription_id,
            referrer: record.referrer,
            amount: reward,
        },
    );

    Ok(())
}

// ── Referrer: withdraw earned rewards ─────────────────────────────────────────

/// Withdraw accumulated referral rewards to the referrer's wallet.
///
/// Follows the Checks-Effects-Interactions (CEI) pattern:
/// the on-chain balance is reduced before the token transfer is initiated.
///
/// # Errors
/// - [`Error::InvalidAmount`] if `amount <= 0`.
/// - [`Error::NotFound`] if the referrer has no accumulated balance.
/// - [`Error::InsufficientBalance`] if `amount > current balance`.
/// - [`Error::NotInitialized`] if the contract token is not set.
/// - [`Error::Overflow`] if balance arithmetic overflows.
pub fn do_withdraw_referral_rewards(
    env: &Env,
    referrer: Address,
    amount: i128,
) -> Result<(), Error> {
    referrer.require_auth();

    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    // CHECKS
    let current = get_referral_balance(env, &referrer);
    if current == 0 {
        return Err(Error::NotFound);
    }
    if amount > current {
        return Err(Error::InsufficientBalance);
    }

    let new_balance = current.checked_sub(amount).ok_or(Error::Overflow)?;
    let token_addr = crate::admin::get_token(env)?;

    // EFFECTS: update balance before external call
    set_referral_balance(env, &referrer, new_balance);

    env.events().publish(
        (Symbol::new(env, "ref_withdrawn"), referrer.clone()),
        ReferralRewardWithdrawnEvent {
            referrer: referrer.clone(),
            amount,
        },
    );

    // INTERACTIONS: token transfer last
    let token_client = soroban_sdk::token::Client::new(env, &token_addr);
    token_client.transfer(&env.current_contract_address(), &referrer, &amount);

    Ok(())
}
