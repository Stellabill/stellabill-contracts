use crate::types::Error;

/// Safely adds two i128 values, preventing overflow.
///
/// Uses Rust's `checked_add()` to detect overflow conditions. If the addition
/// would exceed `i128::MAX`, returns `Error::Overflow` instead of panicking.
///
/// # Arguments
///
/// * `a` - First value to add
/// * `b` - Second value to add
///
/// # Returns
///
/// * `Ok(i128)` - The sum of `a` and `b` if no overflow occurs
/// * `Err(Error::Overflow)` - If the result would exceed `i128::MAX`
///
/// # Examples
///
/// ```
/// use subscription_vault::safe_math::safe_add;
/// use subscription_vault::Error;
///
/// assert_eq!(safe_add(100, 200), Ok(300));
/// assert_eq!(safe_add(i128::MAX, 1), Err(Error::Overflow));
/// ```
///
/// # Compatibility
///
/// Compatible with USDC-style fixed decimals (6 decimals). For example,
/// 1 USDC = 1_000_000 smallest units, 1000 USDC = 1_000_000_000.
pub fn safe_add(a: i128, b: i128) -> Result<i128, Error> {
    a.checked_add(b).ok_or_else(|| {
        if a > 0 {
            Error::Overflow
        } else {
            Error::Underflow
        }
    })
}

/// Safely subtracts two i128 values, preventing underflow.
///
/// Uses Rust's `checked_sub()` to detect underflow conditions. If the subtraction
/// would go below `i128::MIN`, returns `Error::Underflow` instead of panicking.
///
/// # Arguments
///
/// * `a` - Value to subtract from
/// * `b` - Value to subtract
///
/// # Returns
///
/// * `Ok(i128)` - The difference of `a` and `b` if no underflow occurs
/// * `Err(Error::Underflow)` - If the result would go below `i128::MIN`
///
/// # Examples
///
/// ```
/// use subscription_vault::safe_math::safe_sub;
/// use subscription_vault::Error;
///
/// assert_eq!(safe_sub(200, 100), Ok(100));
/// assert_eq!(safe_sub(i128::MIN, 1), Err(Error::Underflow));
/// ```
///
/// # Compatibility
///
/// Compatible with USDC-style fixed decimals (6 decimals). For example,
/// 1 USDC = 1_000_000 smallest units, 1000 USDC = 1_000_000_000.
pub fn safe_sub(a: i128, b: i128) -> Result<i128, Error> {
    a.checked_sub(b).ok_or_else(|| {
        if a >= 0 {
            Error::Overflow
        } else {
            Error::Underflow
        }
    })
}

/// Validates that an amount is non-negative.
///
/// Used for input validation to ensure amounts passed to balance operations
/// are non-negative. This prevents negative amounts from being added or
/// subtracted from balances.
///
/// # Arguments
///
/// * `amount` - The amount to validate
///
/// # Returns
///
/// * `Ok(())` - If the amount is non-negative (>= 0)
/// * `Err(Error::Underflow)` - If the amount is negative (< 0)
///
/// # Examples
///
/// ```
/// use subscription_vault::safe_math::validate_non_negative;
/// use subscription_vault::Error;
///
/// assert_eq!(validate_non_negative(100), Ok(()));
/// assert_eq!(validate_non_negative(0), Ok(()));
/// assert_eq!(validate_non_negative(-1), Err(Error::Underflow));
/// ```
pub fn validate_non_negative(amount: i128) -> Result<(), Error> {
    if amount < 0 {
        Err(Error::Underflow)
    } else {
        Ok(())
    }
}

/// Safely adds an amount to a balance, preventing overflow and negative amounts.
///
/// This is a specialized wrapper around `safe_add()` for balance operations.
/// It ensures that:
/// 1. The amount being added is non-negative (prevents adding negative amounts)
/// 2. The addition doesn't overflow `i128::MAX`
/// 3. The result is always >= 0 (guaranteed by non-negative amount)
///
/// # Arguments
///
/// * `balance` - Current balance value
/// * `amount` - Amount to add to the balance (must be non-negative)
///
/// # Returns
///
/// * `Ok(i128)` - The new balance after adding the amount
/// * `Err(Error::Underflow)` - If `amount` is negative
/// * `Err(Error::Overflow)` - If the result would exceed `i128::MAX`
///
/// # Guarantees
///
/// The result is always >= 0 when successful, as negative amounts are rejected.
///
/// # Examples
///
/// ```
/// use subscription_vault::safe_math::safe_add_balance;
/// use subscription_vault::Error;
///
/// assert_eq!(safe_add_balance(1000, 500), Ok(1500));
/// assert_eq!(safe_add_balance(1000, -100), Err(Error::Underflow));
/// assert_eq!(safe_add_balance(i128::MAX, 1), Err(Error::Overflow));
/// ```
///
/// # Compatibility
///
/// Compatible with USDC-style fixed decimals (6 decimals). For example,
/// 1 USDC = 1_000_000 smallest units, 1000 USDC = 1_000_000_000.
pub fn safe_add_balance(balance: i128, amount: i128) -> Result<i128, Error> {
    validate_non_negative(amount)?;
    safe_add(balance, amount)
}

/// Safely subtracts an amount from a balance, preventing underflow and negative balances.
///
/// This is a specialized wrapper around `safe_sub()` for balance operations.
/// It ensures that:
/// 1. The amount being subtracted is non-negative
/// 2. The subtraction doesn't underflow `i128::MIN`
/// 3. The result is non-negative (prevents negative balances)
///
/// # Arguments
///
/// * `balance` - Current balance value
/// * `amount` - Amount to subtract from the balance (must be non-negative)
///
/// # Returns
///
/// * `Ok(i128)` - The new balance after subtracting the amount (always >= 0)
/// * `Err(Error::Underflow)` - If `amount` is negative, or if the result would be negative
/// * `Err(Error::Underflow)` - If the subtraction would go below `i128::MIN`
///
/// # Guarantees
///
/// The result is always >= 0 when successful, as negative balances are prevented.
///
/// # Examples
///
/// ```
/// use subscription_vault::safe_math::safe_sub_balance;
/// use subscription_vault::Error;
///
/// assert_eq!(safe_sub_balance(1000, 500), Ok(500));
/// assert_eq!(safe_sub_balance(1000, 1000), Ok(0));
/// assert_eq!(safe_sub_balance(1000, 1500), Err(Error::Underflow));
/// assert_eq!(safe_sub_balance(1000, -100), Err(Error::Underflow));
/// ```
///
/// # Compatibility
///
/// Compatible with USDC-style fixed decimals (6 decimals). For example,
/// 1 USDC = 1_000_000 smallest units, 1000 USDC = 1_000_000_000.
pub fn safe_sub_balance(balance: i128, amount: i128) -> Result<i128, Error> {
    validate_non_negative(amount)?;
    let result = safe_sub(balance, amount)?;
    if result < 0 {
        Err(Error::Underflow)
    } else {
        Ok(result)
    }
}

/// Safely multiplies two i128 values, preventing overflow.
pub fn safe_mul(a: i128, b: i128) -> Result<i128, Error> {
    a.checked_mul(b).ok_or_else(|| {
        if (a > 0 && b > 0) || (a < 0 && b < 0) {
            Error::Overflow
        } else {
            Error::Underflow
        }
    })
}

/// Safely divides two i128 values, preventing division by zero and underflow.
pub fn safe_div(a: i128, b: i128) -> Result<i128, Error> {
    if b == 0 {
        return Err(Error::InvalidInput);
    }
    // checked_div only fails for MIN / -1 (Overflow)
    a.checked_div(b).ok_or(Error::Overflow)
}

/// Safely calculates power of an i128 value, preventing overflow.
pub fn safe_pow(base: i128, exp: u32) -> Result<i128, Error> {
    base.checked_pow(exp).ok_or_else(|| {
        if base > 0 || exp % 2 == 0 {
            Error::Overflow
        } else {
            Error::Underflow
        }
    })
}

/// Calculates the protocol fee and net amount using floor rounding for positive numbers.
/// Returns `(net_amount, fee_amount)`.
pub fn calculate_fee(gross: i128, bps: u32) -> Result<(i128, i128), Error> {
    validate_non_negative(gross)?;
    if bps == 0 {
        return Ok((gross, 0));
    }
    let fee_amount = safe_div(safe_mul(gross, bps as i128)?, 10_000)?;
    let net_amount = safe_sub(gross, fee_amount)?;
    Ok((net_amount, fee_amount))
}

/// Safely prorates an amount based on elapsed time over a total period.
/// Uses floor rounding towards zero.
pub fn safe_prorate(amount: i128, elapsed: u64, total: u64) -> Result<i128, Error> {
    validate_non_negative(amount)?;
    if total == 0 {
        return Err(Error::InvalidInput);
    }
    if elapsed >= total {
        return Ok(amount);
    }
    let prorated = safe_div(safe_mul(amount, elapsed as i128)?, total as i128)?;
    Ok(prorated)
}

/// Converts token base units to internal accounting units.
/// Currently a 1:1 mapping, provided for boundary conversion validation and future scaling.
#[inline(always)]
pub const fn to_internal_units(amount: i128) -> i128 {
    amount
}

/// Converts internal accounting units back to token base units.
/// Currently a 1:1 mapping, provided for boundary conversion validation.
#[inline(always)]
pub const fn from_internal_units(amount: i128) -> i128 {
    amount
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_add() {
        assert_eq!(safe_add(10, 20).unwrap(), 30);
        assert_eq!(safe_add(i128::MAX, 1), Err(Error::Overflow));
    }

    #[test]
    fn test_safe_sub() {
        assert_eq!(safe_sub(30, 10).unwrap(), 20);
        assert_eq!(safe_sub(i128::MIN, 1), Err(Error::Underflow));
    }

    #[test]
    fn test_safe_mul() {
        assert_eq!(safe_mul(10, 20).unwrap(), 200);
        assert_eq!(safe_mul(i128::MAX, 2), Err(Error::Overflow));
    }

    #[test]
    fn test_safe_div() {
        assert_eq!(safe_div(40, 2).unwrap(), 20);
        assert_eq!(safe_div(10, 0), Err(Error::InvalidInput));
    }

    #[test]
    fn test_safe_pow() {
        assert_eq!(safe_pow(10, 3).unwrap(), 1000);
        assert_eq!(safe_pow(10, 40), Err(Error::Overflow));
    }
}
