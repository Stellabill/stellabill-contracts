//! Focused adversarial coverage for the public `add_total_accounted` helper.
//!
//! The currently exposed helper is a no-op: it has no caller parameter,
//! validation, or storage write. These tests characterize that public behavior
//! without introducing authorization or arithmetic rules that the function does
//! not currently define.

use crate::accounting::{add_total_accounted, get_total_accounted};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

#[test]
fn add_total_accounted_accepts_a_positive_amount_without_changing_state() {
    let env = Env::default();
    let token = Address::generate(&env);

    assert_eq!(get_total_accounted(&env, &token), 0);
    assert_eq!(add_total_accounted(&env, &token, 100), Ok(()));
    assert_eq!(get_total_accounted(&env, &token), 0);
}

#[test]
fn add_total_accounted_accepts_zero_without_changing_state() {
    let env = Env::default();
    let token = Address::generate(&env);

    assert_eq!(add_total_accounted(&env, &token, 0), Ok(()));
    assert_eq!(get_total_accounted(&env, &token), 0);
}

#[test]
fn add_total_accounted_accepts_negative_and_maximum_amounts_without_panicking() {
    let env = Env::default();
    let token = Address::generate(&env);

    assert_eq!(add_total_accounted(&env, &token, i128::MIN), Ok(()));
    assert_eq!(add_total_accounted(&env, &token, i128::MAX), Ok(()));
    assert_eq!(get_total_accounted(&env, &token), 0);
}

#[test]
fn add_total_accounted_has_no_authorization_or_token_address_failure_path() {
    let env = Env::default();
    let first_token = Address::generate(&env);
    let second_token = Address::generate(&env);

    assert_eq!(add_total_accounted(&env, &first_token, 1), Ok(()));
    assert_eq!(add_total_accounted(&env, &second_token, i128::MAX), Ok(()));
    assert_eq!(get_total_accounted(&env, &first_token), 0);
    assert_eq!(get_total_accounted(&env, &second_token), 0);
}
