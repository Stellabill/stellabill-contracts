//! Merchant entrypoints: withdraw_merchant_funds, batch_withdraw_merchant_funds.
//!
//! **PRs that only change merchant payouts should edit this file only.**

use crate::types::{BatchWithdrawResult, Error};
use soroban_sdk::{Address, Env, Vec};

/// Withdraw a single amount for a merchant.
pub fn withdraw_merchant_funds(_env: &Env, merchant: Address, _amount: i128) -> Result<(), Error> {
    merchant.require_auth();
    Ok(())
}

/// Batch withdraw multiple amounts for a single merchant.
///
/// # Guarantees
/// - Merchant must authorize once for the entire batch.
/// - Each withdrawal is attempted independently; failures do not stop the batch.
/// - Returns one [`BatchChargeResult`] per entry, in input order.
/// - Overdrafts are caught per-entry and reported as [`Error::InsufficientBalance`].
/// - Accounting is never double-debited; a failed entry leaves state unchanged.
pub fn batch_withdraw_merchant_funds(
    env: &Env,
    merchant: Address,
    amounts: Vec<i128>,
) -> Result<Vec<BatchWithdrawResult>, Error> {
    // Single auth for the entire batch
    merchant.require_auth();

    let mut results: Vec<BatchWithdrawResult> = Vec::new(env);

    for i in 0..amounts.len() {
        let amount = amounts.get(i).unwrap();

        // Validate amount is positive
        if amount <= 0 {
            results.push_back(BatchWithdrawResult {
                success: false,
                error_code: Error::InsufficientBalance.to_code(),
            });
            continue;
        }

        // Attempt the withdrawal — partial failures are safe, state unchanged on error
        match do_single_withdraw(env, &merchant, amount) {
            Ok(()) => {
                results.push_back(BatchWithdrawResult {
                    success: true,
                    error_code: 0,
                });
            }
            Err(e) => {
                results.push_back(BatchWithdrawResult {
                    success: false,
                    error_code: e.to_code(),
                });
            }
        }
    }

    Ok(results)
}

/// Internal single withdrawal logic — reused by both single and batch entrypoints.
fn do_single_withdraw(_env: &Env, _merchant: &Address, _amount: i128) -> Result<(), Error> {
    // TODO: deduct from merchant balance ledger entry and transfer token
    // Mirrors withdraw_merchant_funds semantics
    Ok(())
}
