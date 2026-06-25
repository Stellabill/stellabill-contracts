#![cfg(test)]

use crate::types::{normalize_amount, denormalize_amount, DataKey, Error};
use soroban_sdk::{Address, Env};
use soroban_sdk::testutils::Address as _;

#[test]
fn test_6_decimal_token_normalization() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &6u32);

        // 1.234567 in 6 decimals = 1_234_567
        let raw = 1_234_567i128;
        let normalized = normalize_amount(&env, &token, raw).unwrap();
        // Should scale up by 10^(9-6) = 1000
        assert_eq!(normalized, 1_234_567_000);

        let denormalized = denormalize_amount(&env, &token, normalized).unwrap();
        assert_eq!(denormalized, raw);
    });
}

#[test]
fn test_7_decimal_token_normalization() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &7u32);

        // 12.345678 in 7 decimals = 12_345_678
        let raw = 12_345_678i128;
        let normalized = normalize_amount(&env, &token, raw).unwrap();
        // Should scale up by 10^(9-7) = 100
        assert_eq!(normalized, 1_234_567_800);

        let denormalized = denormalize_amount(&env, &token, normalized).unwrap();
        assert_eq!(denormalized, raw);
    });
}

#[test]
fn test_zero_decimal_token_rejected() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &0u32);

        let res = normalize_amount(&env, &token, 100);
        assert_eq!(res, Err(Error::InvalidTokenDecimals));

        let res_denorm = denormalize_amount(&env, &token, 100);
        assert_eq!(res_denorm, Err(Error::InvalidTokenDecimals));
    });
}

#[test]
fn test_unregistered_token_rejected() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        let res = normalize_amount(&env, &token, 100);
        assert_eq!(res, Err(Error::InvalidToken));
    });
}

#[test]
fn test_normalization_overflow() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &6u32);

        // raw = i128::MAX, which will overflow when multiplying by 1000
        let res = normalize_amount(&env, &token, i128::MAX);
        assert_eq!(res, Err(Error::Overflow));
    });
}

#[test]
fn test_denormalization_overflow() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &12u32);

        // normalized = i128::MAX, which will overflow when multiplying by 1000
        let res = denormalize_amount(&env, &token, i128::MAX);
        assert_eq!(res, Err(Error::Overflow));
    });
}

#[test]
fn test_greater_than_9_decimals() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &12u32); // 12 decimals

        // 1.234567890123 in 12 decimals = 1_234_567_890_123
        // This cannot be represented in 9 decimals without precision loss
        let raw_loss = 1_234_567_890_123i128;
        let res_loss = normalize_amount(&env, &token, raw_loss);
        assert_eq!(res_loss, Err(Error::InvalidInput));

        // 1.234567890000 in 12 decimals = 1_234_567_890_000
        // This can be represented exactly in 9 decimals as 1_234_567_890
        let raw_exact = 1_234_567_890_000i128;
        let normalized = normalize_amount(&env, &token, raw_exact).unwrap();
        assert_eq!(normalized, 1_234_567_890);

        let denormalized = denormalize_amount(&env, &token, normalized).unwrap();
        assert_eq!(denormalized, raw_exact);
    });
}

#[test]
fn test_denormalization_precision_loss() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &6u32);

        // 1.0005 in 9 decimals = 1_000_500_000
        // Try to denormalize to 6 decimals, which cannot represent 1.0005 exactly
        let normalized = 1_000_500_500i128;
        let res = denormalize_amount(&env, &token, normalized);
        assert_eq!(res, Err(Error::InvalidInput));
    });
}
