//! Merchant payout and accumulated USDC tracking entrypoints.
//!
//! # Reentrancy Protection
//!
//! This module contains a critical external call: `withdraw_merchant_funds` transfers
//! USDC tokens to the merchant via `token.transfer()`. The implementation follows the
//! **Checks-Effects-Interactions (CEI)** pattern to prevent reentrancy attacks:
//!
//! 1. **Checks**: Validate merchant authorization and sufficient balance
//! 2. **Effects**: Update internal merchant balance in contract storage
//! 3. **Interactions**: Call token.transfer() AFTER state is consistent
//!
//! See `docs/reentrancy.md` for details on the reentrancy threat model and mitigation.

use crate::safe_math::validate_non_negative;
use crate::types::Error;
use soroban_sdk::{token, Address, Env, Symbol};

fn merchant_balance_key(env: &Env, merchant: &Address) -> (Symbol, Address) {
    (Symbol::new(env, "merchant_balance"), merchant.clone())
}

fn treasury_balance_key(env: &Env) -> Symbol {
    Symbol::new(env, "treasury_balance")
}

pub fn get_merchant_balance(env: &Env, merchant: &Address) -> i128 {
    let key = merchant_balance_key(env, merchant);
    env.storage().instance().get(&key).unwrap_or(0i128)
}

fn set_merchant_balance(env: &Env, merchant: &Address, balance: &i128) {
    let key = merchant_balance_key(env, merchant);
    env.storage().instance().set(&key, balance);
}

pub fn get_treasury_balance(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&treasury_balance_key(env))
        .unwrap_or(0i128)
}

pub fn credit_treasury_balance(env: &Env, amount: i128) -> Result<(), Error> {
    validate_non_negative(amount)?;
    let current = get_treasury_balance(env);
    let new_balance = current.checked_add(amount).ok_or(Error::Overflow)?;
    env.storage()
        .instance()
        .set(&treasury_balance_key(env), &new_balance);
    Ok(())
}

/// Credit merchant balance (used when subscription charges process).
pub fn credit_merchant_balance(env: &Env, merchant: &Address, amount: i128) -> Result<(), Error> {
    validate_non_negative(amount)?;
    let current = get_merchant_balance(env, merchant);
    let new_balance = current.checked_add(amount).ok_or(Error::Overflow)?;
    set_merchant_balance(env, merchant, &new_balance);
    Ok(())
}

pub fn credit_merchant_and_treasury(
    env: &Env,
    merchant: &Address,
    gross_amount: i128,
) -> Result<(i128, i128), Error> {
    validate_non_negative(gross_amount)?;
    let fee_bps: u32 = env
        .storage()
        .instance()
        .get(&Symbol::new(env, "protocol_fee_bps"))
        .unwrap_or(0);
    if fee_bps > 10_000 {
        return Err(Error::InvalidFeeBps);
    }

    let fee_amount = gross_amount
        .checked_mul(fee_bps as i128)
        .ok_or(Error::Overflow)?
        / 10_000;
    let net_amount = gross_amount
        .checked_sub(fee_amount)
        .ok_or(Error::Underflow)?;

    credit_merchant_balance(env, merchant, net_amount)?;
    if fee_amount > 0 {
        credit_treasury_balance(env, fee_amount)?;
    }
    Ok((net_amount, fee_amount))
}

/// Withdraw accumulated USDC from prior subscription charges to the merchant address.
///
/// **Reentrancy Protection**: This function follows the Checks-Effects-Interactions (CEI) pattern:
/// 1. All validation happens first (checks)
/// 2. Internal state is updated before any external calls (effects)
/// 3. External token transfer happens last (interactions)
///
/// This ordering ensures that if the token contract attempts a callback into our contract,
/// our internal state will already be consistent and the merchant balance will be correct.
pub fn withdraw_merchant_funds(env: &Env, merchant: Address, amount: i128) -> Result<(), Error> {
    merchant.require_auth();
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    // ──────────────────────────────────────────────────────────────────────────
    // CHECKS: Validate all preconditions before any state mutations
    // ──────────────────────────────────────────────────────────────────────────
    let current = get_merchant_balance(env, &merchant);
    if current == 0 {
        return Err(Error::NotFound);
    }
    if amount > current {
        return Err(Error::InsufficientBalance);
    }

    let new_balance = current.checked_sub(amount).ok_or(Error::Overflow)?;
    let token_addr = crate::admin::get_token(env)?;

    // ──────────────────────────────────────────────────────────────────────────
    // EFFECTS: Update internal state before external interactions (CEI pattern)
    // ──────────────────────────────────────────────────────────────────────────
    set_merchant_balance(env, &merchant, &new_balance);
    env.events()
        .publish((Symbol::new(env, "withdrawn"), merchant.clone()), amount);

    // ──────────────────────────────────────────────────────────────────────────
    // INTERACTIONS: Only after internal state is consistent, call token contract
    // This ensures that even if token contract calls back, our state is correct
    // ──────────────────────────────────────────────────────────────────────────
    let token_client = token::Client::new(env, &token_addr);
    token_client.transfer(&env.current_contract_address(), &merchant, &amount);

    Ok(())
}

pub fn withdraw_treasury_funds(env: &Env, admin: Address, amount: i128) -> Result<(), Error> {
    admin.require_auth();
    let stored_admin = crate::admin::require_admin(env)?;
    if admin != stored_admin {
        return Err(Error::Forbidden);
    }
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }
    let current = get_treasury_balance(env);
    if amount > current {
        return Err(Error::InsufficientBalance);
    }
    let new_balance = current.checked_sub(amount).ok_or(Error::Underflow)?;
    env.storage()
        .instance()
        .set(&treasury_balance_key(env), &new_balance);

    let treasury: Address = env
        .storage()
        .instance()
        .get(&Symbol::new(env, "treasury"))
        .ok_or(Error::TreasuryNotConfigured)?;
    let token_addr = crate::admin::get_token(env)?;
    let token_client = token::Client::new(env, &token_addr);
    token_client.transfer(&env.current_contract_address(), &treasury, &amount);
    Ok(())
}
