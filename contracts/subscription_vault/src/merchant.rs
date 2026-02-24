//! Merchant entrypoints: withdraw_merchant_funds.
//!
//! **PRs that only change merchant payouts should edit this file only.**

use crate::safe_math::validate_non_negative;
use crate::types::Error;
use soroban_sdk::{Address, Env, Symbol};

pub fn withdraw_merchant_funds(env: &Env, merchant: Address, amount: i128) -> Result<(), Error> {
    merchant.require_auth();
    validate_non_negative(amount)?;
    env.events()
        .publish((Symbol::new(env, "withdrawn"), merchant.clone()), amount);
    Ok(())
}
