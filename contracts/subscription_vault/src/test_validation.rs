//! Unit tests for the ABI boundary validators.
//!
//! These tests cover `reject_empty_string` and `reject_contract_self` in isolation.
//! Integration-level tests (confirming the guards are actually wired into each
//! entrypoint) live in `test_abi_validators_integration.rs`.

use crate::{
    types::Error,
    validation::{reject_contract_self, reject_empty_string},
    SubscriptionVault,
};
use soroban_sdk::{Env, String};

// ── reject_empty_string ────────────────────────────────────────────────────────

#[test]
fn test_reject_empty_string_rejects_zero_len() {
    let env = Env::default();
    let s = String::from_str(&env, "");
    assert_eq!(reject_empty_string(&s), Err(Error::InvalidInput));
}

#[test]
fn test_reject_empty_string_rejects_spaces_only() {
    let env = Env::default();
    let s = String::from_str(&env, "   ");
    assert_eq!(reject_empty_string(&s), Err(Error::InvalidInput));
}

#[test]
fn test_reject_empty_string_rejects_tabs_and_newlines() {
    let env = Env::default();
    let s = String::from_str(&env, "\t\n\r");
    assert_eq!(reject_empty_string(&s), Err(Error::InvalidInput));
}

#[test]
fn test_reject_empty_string_accepts_single_char() {
    let env = Env::default();
    let s = String::from_str(&env, "a");
    assert_eq!(reject_empty_string(&s), Ok(()));
}

#[test]
fn test_reject_empty_string_accepts_normal_string() {
    let env = Env::default();
    let s = String::from_str(&env, "plan_name");
    assert_eq!(reject_empty_string(&s), Ok(()));
}

#[test]
fn test_reject_empty_string_accepts_string_with_leading_space() {
    // A string that starts with whitespace but has non-whitespace content
    // should be accepted; trimming is not our job here.
    let env = Env::default();
    let s = String::from_str(&env, " hello");
    assert_eq!(reject_empty_string(&s), Ok(()));
}

#[test]
fn test_reject_empty_string_accepts_long_string() {
    let env = Env::default();
    // 300 'x' chars — well above the 256-byte inspection cap
    let long = "x".repeat(300);
    let s = String::from_str(&env, &long);
    assert_eq!(reject_empty_string(&s), Ok(()));
}

// ── reject_contract_self ──────────────────────────────────────────────────────

#[test]
fn test_reject_contract_self_rejects_own_address() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    env.as_contract(&contract_id, || {
        let self_addr = env.current_contract_address();
        assert_eq!(
            reject_contract_self(&env, &self_addr),
            Err(Error::InvalidInput)
        );
    });
}

#[test]
fn test_reject_contract_self_accepts_other_address() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    // Register a second contract to obtain a distinct, non-self address.
    let other_id = env.register(SubscriptionVault, ());
    env.as_contract(&contract_id, || {
        assert_eq!(reject_contract_self(&env, &other_id), Ok(()));
    });
}
