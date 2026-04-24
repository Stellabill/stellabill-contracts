#![cfg(test)]

use crate::safe_math::{
    calculate_fee, from_internal_units, safe_add, safe_div, safe_mul, safe_pow, safe_prorate,
    safe_sub, to_internal_units,
};
use crate::types::Error;
use soroban_sdk::Env;

struct BinaryTestCase {
    a: i128,
    b: i128,
    expected: Result<i128, Error>,
}

#[test]
fn test_safe_add_regression() {
    let _env = Env::default();
    let cases = [
        BinaryTestCase { a: 10, b: 20, expected: Ok(30) },
        BinaryTestCase { a: -10, b: 20, expected: Ok(10) },
        BinaryTestCase { a: 0, b: 0, expected: Ok(0) },
        BinaryTestCase { a: i128::MAX, b: 0, expected: Ok(i128::MAX) },
        BinaryTestCase { a: i128::MAX, b: -1, expected: Ok(i128::MAX - 1) },
        BinaryTestCase { a: i128::MAX, b: 1, expected: Err(Error::Overflow) },
        BinaryTestCase { a: i128::MAX, b: i128::MAX, expected: Err(Error::Overflow) },
        BinaryTestCase { a: i128::MIN, b: -1, expected: Err(Error::Underflow) },
        BinaryTestCase { a: i128::MIN, b: 0, expected: Ok(i128::MIN) },
        BinaryTestCase { a: i128::MIN, b: 1, expected: Ok(i128::MIN + 1) },
        // Large balances
        BinaryTestCase { a: 1_000_000_000_000_000, b: 1_000_000_000_000_000, expected: Ok(2_000_000_000_000_000) },
    ];

    for case in cases {
        assert_eq!(safe_add(case.a, case.b), case.expected, "Failed for a: {}, b: {}", case.a, case.b);
    }
}

#[test]
fn test_safe_sub_regression() {
    let _env = Env::default();
    let cases = [
        BinaryTestCase { a: 30, b: 10, expected: Ok(20) },
        BinaryTestCase { a: 10, b: 20, expected: Ok(-10) },
        BinaryTestCase { a: i128::MIN, b: 0, expected: Ok(i128::MIN) },
        BinaryTestCase { a: i128::MIN, b: -1, expected: Ok(i128::MIN + 1) },
        BinaryTestCase { a: i128::MIN, b: 1, expected: Err(Error::Underflow) },
        BinaryTestCase { a: 0, b: i128::MAX, expected: Ok(-i128::MAX) },
        BinaryTestCase { a: i128::MIN, b: i128::MAX, expected: Err(Error::Underflow) },
        BinaryTestCase { a: i128::MAX, b: -1, expected: Err(Error::Overflow) },
    ];

    for case in cases {
        assert_eq!(safe_sub(case.a, case.b), case.expected, "Failed for a: {}, b: {}", case.a, case.b);
    }
}

#[test]
fn test_safe_mul_regression() {
    let _env = Env::default();
    let cases = [
        BinaryTestCase { a: 10, b: 20, expected: Ok(200) },
        BinaryTestCase { a: -10, b: 20, expected: Ok(-200) },
        BinaryTestCase { a: 0, b: 100, expected: Ok(0) },
        BinaryTestCase { a: i128::MAX, b: 1, expected: Ok(i128::MAX) },
        BinaryTestCase { a: i128::MIN, b: 1, expected: Ok(i128::MIN) },
        BinaryTestCase { a: i128::MAX, b: 2, expected: Err(Error::Overflow) },
        BinaryTestCase { a: i128::MIN, b: 2, expected: Err(Error::Underflow) },
        BinaryTestCase { a: i128::MIN, b: -1, expected: Err(Error::Overflow) },
        BinaryTestCase { a: 1_000_000_000, b: 1_000_000_000, expected: Ok(1_000_000_000_000_000_000) },
    ];

    for case in cases {
        assert_eq!(safe_mul(case.a, case.b), case.expected, "Failed for a: {}, b: {}", case.a, case.b);
    }
}

#[test]
fn test_safe_div_regression() {
    let _env = Env::default();
    let cases = [
        BinaryTestCase { a: 200, b: 10, expected: Ok(20) },
        BinaryTestCase { a: -200, b: 10, expected: Ok(-20) },
        BinaryTestCase { a: 100, b: 0, expected: Err(Error::InvalidInput) },
        BinaryTestCase { a: i128::MIN, b: -1, expected: Err(Error::Overflow) },
        BinaryTestCase { a: i128::MAX, b: 1, expected: Ok(i128::MAX) },
        // Truncation towards zero
        BinaryTestCase { a: 5, b: 2, expected: Ok(2) },
        BinaryTestCase { a: -5, b: 2, expected: Ok(-2) },
    ];

    for case in cases {
        assert_eq!(safe_div(case.a, case.b), case.expected, "Failed for a: {}, b: {}", case.a, case.b);
    }
}

#[test]
fn test_safe_pow_regression() {
    let _env = Env::default();
    assert_eq!(safe_pow(10, 2).unwrap(), 100);
    assert_eq!(safe_pow(2, 10).unwrap(), 1024);
    assert_eq!(safe_pow(10, 0).unwrap(), 1);
    assert_eq!(safe_pow(0, 10).unwrap(), 0);
    assert_eq!(safe_pow(10, 38).unwrap(), 100_000_000_000_000_000_000_000_000_000_000_000_000);
    assert_eq!(safe_pow(10, 39), Err(Error::Overflow));
    assert_eq!(safe_pow(2, 126).unwrap(), 85070591730234615865843651857942052864);
    assert_eq!(safe_pow(2, 127), Err(Error::Overflow));
}

#[test]
fn test_calculate_fee_rounding_and_limits() {
    let _env = Env::default();

    // Normal case: 10.00 USDC with 10% fee (1000 bps)
    // 10.00 USDC = 10_000_000 stroops
    assert_eq!(calculate_fee(10_000_000, 1000).unwrap(), (9_000_000, 1_000_000));

    // Floor rounding: 15 stroops with 10% fee -> 1.5 stroops -> 1 stroop fee
    assert_eq!(calculate_fee(15, 1000).unwrap(), (14, 1));

    // Edge cases
    assert_eq!(calculate_fee(0, 500).unwrap(), (0, 0));
    assert_eq!(calculate_fee(100, 0).unwrap(), (100, 0));
    
    // Negative gross returns Error
    assert_eq!(calculate_fee(-100, 500), Err(Error::Underflow));

    // Very large token balances
    let large_balance = 1_000_000_000_000_000i128; // 1 billion units with 6 decimals
    let bps = 500; // 5%
    let expected_fee = 50_000_000_000_000i128;
    assert_eq!(calculate_fee(large_balance, bps).unwrap(), (large_balance - expected_fee, expected_fee));
}

#[test]
fn test_safe_prorate_regression() {
    let _env = Env::default();
    
    // Exact half
    assert_eq!(safe_prorate(1000, 15, 30).unwrap(), 500);

    // Rounding down (truncation)
    // 1000 * 10 / 30 = 333.33... -> 333
    assert_eq!(safe_prorate(1000, 10, 30).unwrap(), 333);

    // Edge cases
    assert_eq!(safe_prorate(1000, 0, 30).unwrap(), 0);
    assert_eq!(safe_prorate(1000, 30, 30).unwrap(), 1000);
    assert_eq!(safe_prorate(1000, 40, 30).unwrap(), 1000); // elapsed >= total
    
    // Negative amount
    assert_eq!(safe_prorate(-1000, 15, 30), Err(Error::Underflow));
    
    // Total is 0
    assert_eq!(safe_prorate(1000, 15, 0), Err(Error::InvalidInput));
}

#[test]
fn test_boundary_conversion_identity() {
    let val = 1_234_567_890;
    assert_eq!(to_internal_units(val), val);
    assert_eq!(from_internal_units(val), val);
}
