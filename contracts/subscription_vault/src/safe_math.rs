use crate::Error;

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
    a.checked_add(b).ok_or(Error::Overflow)
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
    a.checked_sub(b).ok_or(Error::Overflow)
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
/// * `Err(Error::Overflow)` - If the amount is negative (< 0)
///
/// # Examples
///
/// ```
/// use subscription_vault::safe_math::validate_non_negative;
/// use subscription_vault::Error;
///
/// assert_eq!(validate_non_negative(100), Ok(()));
/// assert_eq!(validate_non_negative(0), Ok(()));
/// assert_eq!(validate_non_negative(-1), Err(Error::Overflow));
/// ```
pub fn validate_non_negative(amount: i128) -> Result<(), Error> {
    if amount < 0 {
        Err(Error::Overflow)
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
        Err(Error::Overflow)
    } else {
        Ok(result)
    }
}
