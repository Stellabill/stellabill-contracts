//! Merchant payout and accumulated USDC tracking entrypoints.
//!
//! # Reentrancy Protection
//!
//! This module contains critical external calls for fund transfers:
//! - `withdraw_merchant_funds`: transfers USDC to merchant via `token.transfer()`
//! - `withdraw_merchant_funds_for_token`: transfers custom tokens to merchant
//! - `merchant_refund`: transfers tokens from merchant to subscriber
//!
//! All functions follow the **Checks-Effects-Interactions (CEI)** pattern:
//!
//! 1. **Checks**: Validate merchant authorization and sufficient balance
//! 2. **Effects**: Update internal state (merchant balance, earnings) in storage
//! 3. **Interactions**: Call token.transfer() AFTER state is consistent and persisted
//!
//! **Guard layer**: Public entry-points in `lib.rs` acquire a `ReentrancyGuard` before
//! calling these internal helpers, providing defense-in-depth protection against
//! potential callbacks during token transfers.
//!
//! See `docs/reentrancy.md` and `docs/reentrancy_hardening.md` for full details on
//! the reentrancy threat model and mitigation strategy.

use crate::safe_math::{safe_add, safe_sub};
use crate::types::{
    AccruedTotals, BillingChargeKind, DataKey, Error, MerchantConfig, MerchantConfigInitializedEvent,
    MerchantConfigUpdatedEvent, MerchantPausedEvent, MerchantUnpausedEvent, MerchantWithdrawalEvent,
    PayoutSchedule, ScheduledPayoutEvent, TokenEarnings, TokenReconciliationSnapshot, MAX_FEE_BIPS,
    is_valid_allowed_operations, OP_CHARGE,
};
use soroban_sdk::{token, Address, Env, String, Symbol, Vec};

pub fn get_merchant_paused(env: &Env, merchant: Address) -> bool {
    // Check both legacy Pause state and new Config state if they overlap
    if let Some(config) = get_merchant_config(env, merchant.clone()) {
        if config.is_paused {
            return true;
        }
    }
    let key = DataKey::MerchantPaused(merchant);
    env.storage().instance().get(&key).unwrap_or(false)
}

pub fn set_merchant_paused(env: &Env, merchant: Address, paused: bool) {
    let key = DataKey::MerchantPaused(merchant);
    env.storage().instance().set(&key, &paused);
}

pub fn pause_merchant(env: &Env, merchant: Address) -> Result<(), Error> {
    merchant.require_auth();

    if get_merchant_paused(env, merchant.clone()) {
        return Ok(());
    }

    set_merchant_paused(env, merchant.clone(), true);

    env.events().publish(
        (Symbol::new(env, "merchant_paused"), merchant.clone()),
        MerchantPausedEvent {
            merchant,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

pub fn unpause_merchant(env: &Env, merchant: Address) -> Result<(), Error> {
    merchant.require_auth();

    if !get_merchant_paused(env, merchant.clone()) {
        return Ok(());
    }

    set_merchant_paused(env, merchant.clone(), false);

    env.events().publish(
        (Symbol::new(env, "merchant_unpaused"), merchant.clone()),
        MerchantUnpausedEvent {
            merchant,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

fn validate_merchant_config_input(
    _payout_address: &Address,
    fee_bips: i32,
    allowed_operations: i32,
) -> Result<(), Error> {
    if fee_bips > MAX_FEE_BIPS {
        return Err(Error::InvalidFeeBips);
    }
    if !is_valid_allowed_operations(allowed_operations) {
        return Err(Error::InvalidOperations);
    }
    if allowed_operations & OP_CHARGE == 0 {
        return Err(Error::MustAllowChargeOperation);
    }
    Ok(())
}

pub fn initialize_merchant_config(
    env: &Env,
    merchant: Address,
    payout_address: Address,
    fee_bips: i32,
    allowed_operations: i32,
    fee_address: Option<Address>,
    redirect_url: String,
) -> Result<MerchantConfig, Error> {
    merchant.require_auth();
    validate_merchant_config_input(&payout_address, fee_bips, allowed_operations)?;

    let config = MerchantConfig {
        version: 1,
        payout_address,
        fee_bips,
        allowed_operations,
        is_active: true,
        fee_address,
        redirect_url,
        is_paused: false,
        last_updated: env.ledger().timestamp(),
    };

    let key = DataKey::MerchantConfig(merchant.clone());
    env.storage().instance().set(&key, &config);

    env.events().publish(
        (Symbol::new(env, "merchant_config_initialized"),),
        MerchantConfigInitializedEvent {
            merchant: merchant.clone(),
            payout_address: config.payout_address.clone(),
            fee_bips: config.fee_bips,
            allowed_operations: config.allowed_operations,
            timestamp: config.last_updated,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(config)
}

pub fn set_merchant_config(
    env: &Env,
    merchant: Address,
    config: MerchantConfig,
) -> Result<(), Error> {
    merchant.require_auth();
    validate_merchant_config_input(&config.payout_address, config.fee_bips, config.allowed_operations)?;

    let key = DataKey::MerchantConfig(merchant.clone());
    let timestamp = env.ledger().timestamp();
    let mut updated_config = config;
    updated_config.last_updated = timestamp;
    env.storage().instance().set(&key, &updated_config);

    env.events().publish(
        (Symbol::new(env, "merchant_config_set"),),
        MerchantConfigUpdatedEvent {
            merchant: merchant.clone(),
            payout_address: updated_config.payout_address.clone(),
            fee_bips: updated_config.fee_bips,
            allowed_operations: updated_config.allowed_operations,
            timestamp,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

pub fn get_merchant_config(env: &Env, merchant: Address) -> Option<MerchantConfig> {
    let key = DataKey::MerchantConfig(merchant);
    env.storage().instance().get(&key)
}



fn merchant_balance_key(merchant: &Address, token: &Address) -> DataKey {
    DataKey::MerchantBalance(merchant.clone(), token.clone())
}

pub fn get_merchant_token_earnings(
    env: &Env,
    merchant: &Address,
    token: &Address,
) -> TokenEarnings {
    let key = DataKey::MerchantEarnings(merchant.clone(), token.clone());
    env.storage().instance().get(&key).unwrap_or(TokenEarnings {
        accruals: AccruedTotals {
            interval: 0,
            usage: 0,
            one_off: 0,
        },
        withdrawals: 0,
        refunds: 0,
    })
}

fn set_merchant_token_earnings(
    env: &Env,
    merchant: &Address,
    token: &Address,
    earnings: &TokenEarnings,
) {
    let key = DataKey::MerchantEarnings(merchant.clone(), token.clone());
    env.storage().instance().set(&key, earnings);
}

fn add_merchant_token(env: &Env, merchant: &Address, token: &Address) {
    let key = DataKey::MerchantTokens(merchant.clone());
    let mut tokens: Vec<Address> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));
    if !tokens.contains(token) {
        tokens.push_back(token.clone());
        env.storage().instance().set(&key, &tokens);
    }
}

pub fn get_merchant_total_earnings(env: &Env, merchant: &Address) -> Vec<(Address, TokenEarnings)> {
    let key = DataKey::MerchantTokens(merchant.clone());
    let tokens: Vec<Address> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));
    let mut result = Vec::new(env);
    for token in tokens.iter() {
        let earnings = get_merchant_token_earnings(env, merchant, &token);
        result.push_back((token, earnings));
    }
    result
}

pub fn get_reconciliation_snapshot(
    env: &Env,
    merchant: &Address,
) -> Vec<TokenReconciliationSnapshot> {
    let key = DataKey::MerchantTokens(merchant.clone());
    let tokens: Vec<Address> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));
    let mut result = Vec::new(env);

    for token in tokens.iter() {
        let earnings = get_merchant_token_earnings(env, merchant, &token);
        let total_accruals = earnings
            .accruals
            .interval
            .checked_add(earnings.accruals.usage)
            .unwrap_or(0)
            .checked_add(earnings.accruals.one_off)
            .unwrap_or(0);

        let computed_balance = total_accruals
            .checked_sub(earnings.withdrawals)
            .unwrap_or(0)
            .checked_sub(earnings.refunds)
            .unwrap_or(0);

        result.push_back(TokenReconciliationSnapshot {
            token: token.clone(),
            total_accruals,
            total_withdrawals: earnings.withdrawals,
            total_refunds: earnings.refunds,
            computed_balance,
            stored_balance: 0, // Will be computed by caller
            matches: computed_balance == 0, // Placeholder
        });
    }
    result
}

pub fn get_merchant_balance(env: &Env, merchant: &Address) -> i128 {
    if let Ok(token_addr) = crate::admin::get_token(env) {
        return get_merchant_balance_by_token(env, merchant, &token_addr);
    }
    0
}

pub fn get_merchant_balance_by_token(env: &Env, merchant: &Address, token: &Address) -> i128 {
    let key = merchant_balance_key(merchant, token);
    env.storage().instance().get(&key).unwrap_or(0i128)
}

fn set_merchant_balance(env: &Env, merchant: &Address, token: &Address, balance: &i128) {
    let key = merchant_balance_key(merchant, token);
    env.storage().instance().set(&key, balance);
}

/// Credit merchant balance (used when subscription charges process).
#[allow(dead_code)]
pub fn credit_merchant_balance(
    env: &Env,
    merchant: &Address,
    amount: i128,
    kind: BillingChargeKind,
) -> Result<(), Error> {
    let token_addr = crate::admin::get_token(env)?;
    credit_merchant_balance_for_token(env, merchant, &token_addr, amount, kind)
}

pub fn credit_merchant_balance_for_token(
    env: &Env,
    merchant: &Address,
    token_addr: &Address,
    amount: i128,
    kind: BillingChargeKind,
) -> Result<(), Error> {
    if amount < 0 {
        return Err(Error::InvalidAmount);
    }

    // Update simple balance
    let current = get_merchant_balance_by_token(env, merchant, token_addr);
    let new_balance = safe_add(current, amount)?;
    set_merchant_balance(env, merchant, token_addr, &new_balance);

    // Update earnings struct
    let mut earnings = get_merchant_token_earnings(env, merchant, token_addr);
    match kind {
        BillingChargeKind::Interval => {
            earnings.accruals.interval = earnings
                .accruals
                .interval
                .checked_add(amount)
                .ok_or(Error::Overflow)?
        }
        BillingChargeKind::Usage => {
            earnings.accruals.usage = earnings
                .accruals
                .usage
                .checked_add(amount)
                .ok_or(Error::Overflow)?
        }
        BillingChargeKind::OneOff => {
            earnings.accruals.one_off = earnings
                .accruals
                .one_off
                .checked_add(amount)
                .ok_or(Error::Overflow)?
        }
    }
    set_merchant_token_earnings(env, merchant, token_addr, &earnings);
    add_merchant_token(env, merchant, token_addr);

    Ok(())
}

pub fn withdraw_merchant_funds(env: &Env, merchant: Address, amount: i128) -> Result<(), Error> {
    let token_addr = crate::admin::get_token(env)?;
    withdraw_merchant_funds_for_token(env, merchant, token_addr, amount)
}

pub fn withdraw_merchant_funds_for_token(
    env: &Env,
    merchant: Address,
    token_addr: Address,
    amount: i128,
) -> Result<(), Error> {
    merchant.require_auth();
    crate::blocklist::require_not_blocklisted(env, &merchant)?;
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }
    if !crate::admin::is_token_accepted(env, &token_addr) {
        return Err(Error::InvalidInput);
    }

    let current = get_merchant_balance_by_token(env, &merchant, &token_addr);
    if current == 0 {
        return Err(Error::NotFound);
    }
    if amount > current {
        return Err(Error::InsufficientBalance);
    }

    // Explicitly check vault's actual token balance before attempting transfer
    let token_client = token::Client::new(env, &token_addr);
    if token_client.balance(&env.current_contract_address()) < amount {
        return Err(Error::InsufficientBalance);
    }

    let new_balance = safe_sub(current, amount)?;

    // ──────────────────────────────────────────────────────────────────────────
    // EFFECTS: Update internal state before external interactions (CEI pattern)
    // ──────────────────────────────────────────────────────────────────────────
    set_merchant_balance(env, &merchant, &token_addr, &new_balance);

    // Keep TokenEarnings.withdrawals in sync so the reconciliation invariant holds:
    // balance = accruals - withdrawals - refunds
    let mut earnings = get_merchant_token_earnings(env, &merchant, &token_addr);
    earnings.withdrawals = earnings
        .withdrawals
        .checked_add(amount)
        .ok_or(Error::Overflow)?;
    set_merchant_token_earnings(env, &merchant, &token_addr, &earnings);

    crate::accounting::sub_total_accounted(env, &token_addr, amount)?;
    env.events().publish(
        (Symbol::new(env, "withdrawn"), merchant.clone(), token_addr.clone()),
        MerchantWithdrawalEvent {
            merchant: merchant.clone(),
            token: token_addr.clone(),
            amount,
            remaining_balance: new_balance,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    // ──────────────────────────────────────────────────────────────────────────
    // INTERACTIONS: Only after internal state is consistent, call token contract
    // INTERACTIONS: Only after internal state is consistent, call token contract
    // ──────────────────────────────────────────────────────────────────────────
    let token_client = token::Client::new(env, &token_addr);
    let contract = env.current_contract_address();
    token_client.transfer(&contract, &merchant, &amount);

    Ok(())
}

pub fn merchant_refund(
    env: &Env,
    merchant: Address,
    subscriber: Address,
    token_addr: Address,
    amount: i128,
) -> Result<(), Error> {
    merchant.require_auth();
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    let current = get_merchant_balance_by_token(env, &merchant, &token_addr);
    if current == 0 {
        return Err(Error::NotFound);
    }
    if amount > current {
        return Err(Error::InsufficientBalance);
    }

    let new_balance = current.checked_sub(amount).ok_or(Error::Underflow)?;

    // EFFECTS
    set_merchant_balance(env, &merchant, &token_addr, &new_balance);

    let mut earnings = get_merchant_token_earnings(env, &merchant, &token_addr);
    earnings.refunds = earnings
        .refunds
        .checked_add(amount)
        .ok_or(Error::Overflow)?;
    set_merchant_token_earnings(env, &merchant, &token_addr, &earnings);

    // Funds leave vault custody — keep TotalAccounted consistent.
    crate::accounting::sub_total_accounted(env, &token_addr, amount)?;

    env.events().publish(
        (Symbol::new(env, "merchant_refund"), merchant.clone()),
        crate::types::MerchantRefundEvent {
            merchant,
            subscriber: subscriber.clone(),
            token: token_addr.clone(),
            amount,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    // INTERACTIONS
    let token_client = token::Client::new(env, &token_addr);
    token_client.transfer(&env.current_contract_address(), &subscriber, &amount);

    Ok(())
}

pub fn get_payout_schedule(env: &Env, merchant: &Address) -> PayoutSchedule {
    let key = DataKey::PayoutSchedule(merchant.clone());
    env.storage().instance().get(&key).unwrap_or(PayoutSchedule {
        cadence_seconds: 0,
        min_payout: 0,
        last_payout_at: 0,
    })
}

fn set_payout_schedule(env: &Env, merchant: &Address, schedule: &PayoutSchedule) {
    let key = DataKey::PayoutSchedule(merchant.clone());
    env.storage().instance().set(&key, schedule);
}

/// Set or clear the payout schedule for a merchant.
///
/// When `cadence_seconds` is 0 and `min_payout` is 0 the schedule is cleared
/// (equivalent to "no auto-payout").  The merchant must authorize this call.
///
/// Returns the previous schedule so callers can diff the change off-chain.
pub fn do_set_payout_schedule(
    env: &Env,
    merchant: Address,
    cadence_seconds: u64,
    min_payout: i128,
) -> Result<PayoutSchedule, Error> {
    merchant.require_auth();

    if min_payout < 0 {
        return Err(Error::InvalidAmount);
    }

    let previous = get_payout_schedule(env, &merchant);
    let now = env.ledger().timestamp();

    let schedule = PayoutSchedule {
        cadence_seconds,
        min_payout,
        last_payout_at: if previous.last_payout_at == 0 {
            0
        } else {
            previous.last_payout_at
        },
    };

    set_payout_schedule(env, &merchant, &schedule);

    env.events().publish(
        (Symbol::new(env, "payout_schedule_set"), merchant.clone()),
        (cadence_seconds, min_payout, now),
    );

    Ok(previous)
}

/// Execute a single per-token payout for a merchant during a flush.
///
/// Reads the merchant's balance for `token`.  If the balance is below
/// `min_payout` the function returns 0 (no-op).  Otherwise it transfers
/// the entire balance to the merchant's payout address, updates internal
/// accounting, and returns the amount transferred.
///
/// # CEI
///
/// Effects (balance update, earnings update) are written *before* the
/// external token transfer.
fn flush_merchant_token(
    env: &Env,
    merchant: &Address,
    token: &Address,
    min_payout: i128,
) -> Result<i128, Error> {
    let balance = get_merchant_balance_by_token(env, merchant, token);
    if balance < min_payout || balance <= 0 {
        return Ok(0i128);
    }

    let config = get_merchant_config(env, merchant.clone())
        .ok_or(Error::NotFound)?;
    let payout_address = config.payout_address;

    // EFFECTS — update state before external call
    set_merchant_balance(env, merchant, token, &0i128);

    let mut earnings = get_merchant_token_earnings(env, merchant, token);
    earnings.withdrawals = earnings
        .withdrawals
        .checked_add(balance)
        .ok_or(Error::Overflow)?;
    set_merchant_token_earnings(env, merchant, token, &earnings);

    crate::accounting::sub_total_accounted(env, token, balance)?;

    env.events().publish(
        (Symbol::new(env, "withdrawn"), merchant.clone(), token.clone()),
        MerchantWithdrawalEvent {
            merchant: merchant.clone(),
            token: token.clone(),
            amount: balance,
            remaining_balance: 0,
            timestamp: env.ledger().timestamp(),
        },
    );

    // INTERACTIONS
    let token_client = token::Client::new(env, token);
    token_client.transfer(&env.current_contract_address(), &payout_address, &balance);

    Ok(balance)
}

/// Process all scheduled payouts for a merchant.
///
/// Iterates every token the merchant has earnings in.  For each token that
/// meets the configured `min_payout` threshold, the full balance is
/// transferred to the merchant's payout address.
///
/// Anyone may call this function.  The cadence check is enforced here:
/// if the configured `cadence_seconds` has not elapsed since the last flush,
/// the call is a no-op (`Err(Error::IntervalNotElapsed)`).
///
/// Returns the number of token payouts actually executed.
pub fn do_flush_payouts(
    env: &Env,
    merchant: Address,
    caller: Address,
) -> Result<u32, Error> {
    let schedule = get_payout_schedule(env, &merchant);

    // No schedule configured — nothing to do.
    if schedule.cadence_seconds == 0 && schedule.min_payout == 0 {
        return Ok(0);
    }

    let now = env.ledger().timestamp();

    // Enforce cadence: enough time since last flush?
    if schedule.last_payout_at > 0
        && now.saturating_sub(schedule.last_payout_at) < schedule.cadence_seconds
    {
        return Err(Error::IntervalNotElapsed);
    }

    // Iterate all tokens the merchant has earnings in.
    let token_key = DataKey::MerchantTokens(merchant.clone());
    let tokens: Vec<Address> = env.storage().instance().get(&token_key).unwrap_or(Vec::new(env));

    let mut tokens_paid: u32 = 0;
    for token in tokens.iter() {
        let amount = flush_merchant_token(env, &merchant, &token, schedule.min_payout)?;
        if amount > 0 {
            tokens_paid = tokens_paid.saturating_add(1);
        }
    }

    // Update last_payout_at
    let mut updated_schedule = schedule;
    updated_schedule.last_payout_at = now;
    set_payout_schedule(env, &merchant, &updated_schedule);

    env.events().publish(
        (Symbol::new(env, "scheduled_payout"), merchant.clone()),
        ScheduledPayoutEvent {
            merchant,
            caller,
            tokens_paid,
            timestamp: now,
        },
    );

    Ok(tokens_paid)
}

pub fn update_merchant_config(
    env: &Env,
    merchant: Address,
    new_payout_address: Option<Address>,
    new_fee_bips: Option<i32>,
    new_allowed_operations: Option<i32>,
    new_is_active: Option<bool>,
    new_fee_address: Option<Option<Address>>,
    new_redirect_url: Option<soroban_sdk::String>,
    new_is_paused: Option<bool>,
) -> Result<MerchantConfig, Error> {
    merchant.require_auth();

    let key = DataKey::MerchantConfig(merchant.clone());
    let mut config: MerchantConfig = env
        .storage()
        .instance()
        .get(&key)
        .ok_or(Error::NotFound)?;

    if let Some(payout) = new_payout_address {
        config.payout_address = payout;
    }
    if let Some(bips) = new_fee_bips {
        if bips > MAX_FEE_BIPS {
            return Err(Error::InvalidFeeBips);
        }
        config.fee_bips = bips;
    }
    if let Some(ops) = new_allowed_operations {
        if !is_valid_allowed_operations(ops) {
            return Err(Error::InvalidOperations);
        }
        if ops & OP_CHARGE == 0 {
            return Err(Error::MustAllowChargeOperation);
        }
        config.allowed_operations = ops;
    }
    if let Some(active) = new_is_active {
        config.is_active = active;
    }
    if let Some(fee_addr) = new_fee_address {
        config.fee_address = fee_addr;
    }
    if let Some(url) = new_redirect_url {
        config.redirect_url = url;
    }
    if let Some(paused) = new_is_paused {
        config.is_paused = paused;
    }

    config.last_updated = env.ledger().timestamp();
    env.storage().instance().set(&key, &config);

    env.events().publish(
        (soroban_sdk::Symbol::new(env, "merchant_config_updated"),),
        MerchantConfigUpdatedEvent {
            merchant: merchant.clone(),
            payout_address: config.payout_address.clone(),
            fee_bips: config.fee_bips,
            allowed_operations: config.allowed_operations,
            timestamp: config.last_updated,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(config)
}
