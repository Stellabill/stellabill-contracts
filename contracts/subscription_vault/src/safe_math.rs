// src/safe_math.rs
use crate::types::Error;

/// Checked addition. Returns `Error::Overflow` if the operation would overflow.
pub fn safe_add(a: i128, b: i128) -> Result<i128, Error> {
    a.checked_add(b).ok_or(Error::Overflow)
}

/// Checked subtraction. Returns `Error::Underflow` if the operation would underflow.
pub fn safe_sub(a: i128, b: i128) -> Result<i128, Error> {
    a.checked_sub(b).ok_or(Error::Underflow)
}

/// Checked multiplication. Returns `Error::Overflow` if the operation would overflow.
pub fn safe_mul(a: i128, b: i128) -> Result<i128, Error> {
    a.checked_mul(b).ok_or(Error::Overflow)
}

/// Checked addition for balances. Guarantees that `amount` is non‑negative and that the
/// resulting balance does not overflow.
pub fn safe_add_balance(balance: i128, amount: i128) -> Result<i128, Error> {
    if amount < 0 {
        // Negative deposits are logically underflows.
        return Err(Error::Underflow);
    }
    safe_add(balance, amount)
}

/// Checked subtraction for balances. Guarantees that `amount` is non‑negative and that the
/// balance stays non‑negative after subtraction.
pub fn safe_sub_balance(balance: i128, amount: i128) -> Result<i128, Error> {
    if amount < 0 {
        return Err(Error::Underflow);
    }
    // Ensure we never go below zero.
    if balance < amount {
        return Err(Error::Underflow);
    }
    safe_sub(balance, amount)
}
