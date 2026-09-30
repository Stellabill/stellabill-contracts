// src/safe_math.rs
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

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

/// SECURITY: checked narrowing cast from `i128` to `u32`.
///
/// Use this in place of `value as u32` whenever an `i128` (e.g. a prorated
/// amount, lifetime-charged sum, or division result) is fed into storage,
/// ledger, or indexing APIs that expect a `u32`. `as` silently truncates any
/// value outside `[0, u32::MAX]` and silently flips the sign on negative
/// inputs; on the Stellar ledger an undetected truncation can permanently
/// mis-account funds.
///
/// Failure modes:
/// * Negative input            -> `Error::Underflow`
/// * Input above `u32::MAX`    -> `Error::Overflow`
///
/// This mapping intentionally mirrors the existing `safe_sub` / `safe_add`
/// error vocabulary so callers can reuse the same match arms.
///
/// Scope of lint enforcement:
/// * **Inside `safe_math.rs`**: the `#![deny(clippy::cast_possible_truncation,
///   clippy::cast_sign_loss)]` header makes any inline `as` cast here a
///   compile-error, so future contributors cannot silently bypass this
///   helper. If a *legitimate* narrowing cast is ever needed in this
///   module (e.g. a provably-safe `usize as u32` for a `Vec` index), gate
///   it with `#[allow(clippy::cast_possible_truncation)]` and a one-line
///   justification comment.
/// * **At the crate level** (`Cargo.toml`): the same lints are reduced to
///   `warn`. Code that follows the helper-oriented pattern passes; code
///   that inlines a truncating cast on an `i128` in some other module
///   will still trip CI for review, but does not block the build.
pub fn safe_i128_to_u32(value: i128) -> Result<u32, Error> {
    if value < 0 {
        return Err(Error::Underflow);
    }
    u32::try_from(value).map_err(|_| Error::Overflow)
}

/// SECURITY: checked narrowing cast from `i128` to `u64`.
///
/// Equivalent to [`safe_i128_to_u32`] but for `u64` consumers (e.g. ledger
/// timestamps, interval math). Same error vocabulary: negatives are
/// `Underflow`, values above `u64::MAX` are `Overflow`.
pub fn safe_i128_to_u64(value: i128) -> Result<u64, Error> {
    if value < 0 {
        return Err(Error::Underflow);
    }
    u64::try_from(value).map_err(|_| Error::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- safe_i128_to_u32 ----

    #[test]
    fn safe_i128_to_u32_zero_ok() {
        assert_eq!(safe_i128_to_u32(0), Ok(0u32));
    }

    #[test]
    fn safe_i128_to_u32_positive_ok() {
        assert_eq!(safe_i128_to_u32(1), Ok(1u32));
        assert_eq!(safe_i128_to_u32(123_456), Ok(123_456u32));
    }

    #[test]
    fn safe_i128_to_u32_max_ok() {
        assert_eq!(safe_i128_to_u32(i128::from(u32::MAX)), Ok(u32::MAX));
    }

    #[test]
    fn safe_i128_to_u32_above_max_overflow() {
        let too_big = i128::from(u32::MAX) + 1;
        assert_eq!(safe_i128_to_u32(too_big), Err(Error::Overflow));
    }

    #[test]
    fn safe_i128_to_u32_negative_underflow() {
        assert_eq!(safe_i128_to_u32(-1), Err(Error::Underflow));
        assert_eq!(safe_i128_to_u32(i128::MIN), Err(Error::Underflow));
    }

    #[test]
    fn safe_i128_to_u32_i128_max_overflow() {
        assert_eq!(safe_i128_to_u32(i128::MAX), Err(Error::Overflow));
    }

    // ---- safe_i128_to_u64 ----

    #[test]
    fn safe_i128_to_u64_zero_ok() {
        assert_eq!(safe_i128_to_u64(0), Ok(0u64));
    }

    #[test]
    fn safe_i128_to_u64_positive_ok() {
        assert_eq!(safe_i128_to_u64(1), Ok(1u64));
        assert_eq!(safe_i128_to_u64(86_400), Ok(86_400u64));
        assert_eq!(
            safe_i128_to_u64(i128::from(u32::MAX)),
            Ok(u64::from(u32::MAX))
        );
    }

    #[test]
    fn safe_i128_to_u64_max_ok() {
        assert_eq!(safe_i128_to_u64(i128::from(u64::MAX)), Ok(u64::MAX));
    }

    #[test]
    fn safe_i128_to_u64_above_max_overflow() {
        let too_big = i128::from(u64::MAX) + 1;
        assert_eq!(safe_i128_to_u64(too_big), Err(Error::Overflow));
    }

    #[test]
    fn safe_i128_to_u64_negative_underflow() {
        assert_eq!(safe_i128_to_u64(-1), Err(Error::Underflow));
        assert_eq!(safe_i128_to_u64(i128::MIN), Err(Error::Underflow));
    }

    #[test]
    fn safe_i128_to_u64_i128_max_overflow() {
        assert_eq!(safe_i128_to_u64(i128::MAX), Err(Error::Overflow));
    }

    // ---- regression: existing safe_* helpers stay green ----

    #[test]
    fn existing_safe_add_overflow() {
        assert_eq!(safe_add(i128::MAX, 1), Err(Error::Overflow));
        assert_eq!(safe_add(0, i128::MAX), Ok(i128::MAX));
    }

    #[test]
    fn existing_safe_sub_underflow() {
        assert_eq!(safe_sub(i128::MIN, 1), Err(Error::Underflow));
        assert_eq!(safe_sub(0, 0), Ok(0));
    }

    #[test]
    fn existing_balance_helpers() {
        assert_eq!(safe_add_balance(100, -1), Err(Error::Underflow));
        assert_eq!(safe_sub_balance(0, -1), Err(Error::Underflow));
        assert_eq!(safe_sub_balance(5, 10), Err(Error::Underflow));
        assert_eq!(safe_sub_balance(10, 5), Ok(5));
    }
}
