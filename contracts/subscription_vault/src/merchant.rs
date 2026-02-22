//! Merchant payout and accumulated USDC tracking entrypoints.

use crate::types::Error;
use soroban_sdk::{token, Address, Env, Symbol};

fn merchant_balance_key(env: &Env, merchant: &Address) -> (Symbol, Address) {
    (Symbol::new(env, "merchant_balance"), merchant.clone())
}

pub fn get_merchant_balance(env: &Env, merchant: &Address) -> i128 {
    let key = merchant_balance_key(env, merchant);
    env.storage().instance().get(&key).unwrap_or(0i128)
}

fn set_merchant_balance(env: &Env, merchant: &Address, balance: &i128) {
    let key = merchant_balance_key(env, merchant);
    env.storage().instance().set(&key, balance);
}

/// Credit merchant balance (used when subscription charges process).
pub fn credit_merchant_balance(env: &Env, merchant: &Address, amount: i128) -> Result<(), Error> {
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }
    let current = get_merchant_balance(env, merchant);
    let new_balance = current.checked_add(amount).ok_or(Error::Overflow)?;
    set_merchant_balance(env, merchant, &new_balance);
    Ok(())
}

/// Withdraw accumulated USDC from prior subscription charges to the merchant address.
pub fn withdraw_merchant_funds(env: &Env, merchant: Address, amount: i128) -> Result<(), Error> {
    merchant.require_auth();

    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    let current = get_merchant_balance(env, &merchant);
    if current == 0 {
        return Err(Error::NotFound);
    }
    if amount > current {
        return Err(Error::InsufficientBalance);
    }

    let new_balance = current.checked_sub(amount).ok_or(Error::Overflow)?;

    let token_addr = crate::admin::get_token(env)?;

    let token_client = token::Client::new(env, &token_addr);
    token_client.transfer(&env.current_contract_address(), &merchant, &amount);

    set_merchant_balance(env, &merchant, &new_balance);

    Ok(())
}
