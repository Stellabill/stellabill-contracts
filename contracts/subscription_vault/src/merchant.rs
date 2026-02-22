//! Merchant entrypoints: withdraw_merchant_funds.
//!
//! **PRs that only change merchant payouts should edit this file only.**

use crate::types::Error;
use soroban_sdk::{Address, Env};

pub fn withdraw_merchant_funds(_env: &Env, merchant: Address, _amount: i128) -> Result<(), Error> {
    merchant.require_auth();
    Ok(())
}
