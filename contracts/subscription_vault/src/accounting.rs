use soroban_sdk::{Env, Address};
use crate::types::{DataKey, Error};

/// Increases the globally tracked accounted amount for a token
pub fn add_total_accounted(env: &Env, token: &Address, amount: i128) -> Result<(), Error> {
    if amount < 0 {
        return Err(Error::InvalidAmount);
    }

    let key = DataKey::TotalAccounted(token.clone());
    let current: i128 = env.storage().instance().get(&key).unwrap_or(0i128);

    let new_value = current
        .checked_add(amount)
        .ok_or(Error::Overflow)?;

    env.storage().instance().set(&key, &new_value);
    Ok(())
}

/// Decreases the globally tracked accounted amount for a token
pub fn sub_total_accounted(env: &Env, token: &Address, amount: i128) -> Result<(), Error> {
    if amount < 0 {
        return Err(Error::InvalidAmount);
    }

    let key = DataKey::TotalAccounted(token.clone());
    let current: i128 = env.storage().instance().get(&key).unwrap_or(0i128);

    let new_value = current
        .checked_sub(amount)
        .ok_or(Error::Underflow)?;

    env.storage().instance().set(&key, &new_value);
    Ok(())
}

/// Reads total accounted value
pub fn get_total_accounted(env: &Env, token: &Address) -> i128 {
    let key = DataKey::TotalAccounted(token.clone());
    env.storage().instance().get(&key).unwrap_or(0i128)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn get_total_accounted_returns_zero_for_untracked_token() {
        let env = Env::default();
        let token = Address::generate(&env);
        assert_eq!(get_total_accounted(&env, &token), 0);
    }

    #[test]
    fn get_total_accounted_reflects_single_add() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, 1_000_000).unwrap();
        assert_eq!(get_total_accounted(&env, &token), 1_000_000);
    }

    #[test]
    fn get_total_accounted_reflects_cumulative_adds() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, 100).unwrap();
        add_total_accounted(&env, &token, 200).unwrap();
        add_total_accounted(&env, &token, 300).unwrap();
        assert_eq!(get_total_accounted(&env, &token), 600);
    }

    #[test]
    fn get_total_accounted_reflects_net_of_add_and_sub() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, 1_000).unwrap();
        sub_total_accounted(&env, &token, 400).unwrap();
        assert_eq!(get_total_accounted(&env, &token), 600);
    }

    #[test]
    fn get_total_accounted_zero_amount_add_leaves_total_unchanged() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, 500).unwrap();
        add_total_accounted(&env, &token, 0).unwrap();
        assert_eq!(get_total_accounted(&env, &token), 500);
    }

    #[test]
    fn get_total_accounted_isolated_per_token() {
        let env = Env::default();
        let token_a = Address::generate(&env);
        let token_b = Address::generate(&env);
        add_total_accounted(&env, &token_a, 700).unwrap();
        add_total_accounted(&env, &token_b, 300).unwrap();
        assert_eq!(get_total_accounted(&env, &token_a), 700);
        assert_eq!(get_total_accounted(&env, &token_b), 300);
        sub_total_accounted(&env, &token_a, 100).unwrap();
        assert_eq!(get_total_accounted(&env, &token_a), 600);
        assert_eq!(get_total_accounted(&env, &token_b), 300);
    }

    #[test]
    fn get_total_accounted_unchanged_after_rejected_sub_underflow() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, 250).unwrap();
        let result = sub_total_accounted(&env, &token, 251);
        assert_eq!(result, Err(Error::Underflow));
        assert_eq!(get_total_accounted(&env, &token), 250);
    }

    #[test]
    fn get_total_accounted_unchanged_after_rejected_sub_on_zero_balance() {
        let env = Env::default();
        let token = Address::generate(&env);
        let result = sub_total_accounted(&env, &token, 1);
        assert_eq!(result, Err(Error::Underflow));
        assert_eq!(get_total_accounted(&env, &token), 0);
    }

    #[test]
    fn get_total_accounted_unchanged_after_rejected_add_negative_amount() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, 100).unwrap();
        let result = add_total_accounted(&env, &token, -1);
        assert_eq!(result, Err(Error::InvalidAmount));
        assert_eq!(get_total_accounted(&env, &token), 100);
    }

    #[test]
    fn get_total_accounted_unchanged_after_rejected_add_overflow() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, i128::MAX).unwrap();
        assert_eq!(get_total_accounted(&env, &token), i128::MAX);
        let result = add_total_accounted(&env, &token, 1);
        assert_eq!(result, Err(Error::Overflow));
        assert_eq!(get_total_accounted(&env, &token), i128::MAX);
    }

    #[test]
    fn get_total_accounted_tracks_i128_max_boundary() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, i128::MAX).unwrap();
        sub_total_accounted(&env, &token, i128::MAX).unwrap();
        assert_eq!(get_total_accounted(&env, &token), 0);
    }

    #[test]
    fn sub_total_accounted_reduces_balance_by_valid_amount() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, 1_000).unwrap();
        sub_total_accounted(&env, &token, 400).unwrap();
        assert_eq!(get_total_accounted(&env, &token), 600);
    }

    #[test]
    fn sub_total_accounted_exact_balance_succeeds_and_zeroes() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, 750).unwrap();
        sub_total_accounted(&env, &token, 750).unwrap();
        assert_eq!(get_total_accounted(&env, &token), 0);
    }

    #[test]
    fn sub_total_accounted_zero_amount_succeeds_without_change() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, 750).unwrap();
        sub_total_accounted(&env, &token, 0).unwrap();
        assert_eq!(get_total_accounted(&env, &token), 750);
    }

    #[test]
    fn sub_total_accounted_negative_amount_rejected_and_state_unchanged() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, 750).unwrap();
        let result = sub_total_accounted(&env, &token, -1);
        assert_eq!(result, Err(Error::InvalidAmount));
        assert_eq!(get_total_accounted(&env, &token), 750);
    }

    #[test]
    fn sub_total_accounted_from_zero_balance_rejected_and_state_unchanged() {
        let env = Env::default();
        let token = Address::generate(&env);
        let result = sub_total_accounted(&env, &token, 1);
        assert_eq!(result, Err(Error::Underflow));
        assert_eq!(get_total_accounted(&env, &token), 0);
    }

    #[test]
    fn sub_total_accounted_above_balance_rejected_and_state_unchanged() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, 100).unwrap();
        let result = sub_total_accounted(&env, &token, 101);
        assert_eq!(result, Err(Error::Underflow));
        assert_eq!(get_total_accounted(&env, &token), 100);
    }

    #[test]
    fn sub_total_accounted_after_rejection_uses_original_balance() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, 500).unwrap();
        assert_eq!(sub_total_accounted(&env, &token, 900), Err(Error::Underflow));
        sub_total_accounted(&env, &token, 500).unwrap();
        assert_eq!(get_total_accounted(&env, &token), 0);
    }

    #[test]
    fn sub_total_accounted_isolated_per_token() {
        let env = Env::default();
        let token_a = Address::generate(&env);
        let token_b = Address::generate(&env);
        add_total_accounted(&env, &token_a, 800).unwrap();
        add_total_accounted(&env, &token_b, 800).unwrap();
        sub_total_accounted(&env, &token_a, 300).unwrap();
        assert_eq!(get_total_accounted(&env, &token_a), 500);
        assert_eq!(get_total_accounted(&env, &token_b), 800);
    }

    #[test]
    fn sub_total_accounted_i128_max_boundary() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, i128::MAX).unwrap();
        sub_total_accounted(&env, &token, i128::MAX - 1).unwrap();
        assert_eq!(get_total_accounted(&env, &token), 1);
        let result = sub_total_accounted(&env, &token, 2);
        assert_eq!(result, Err(Error::Underflow));
        assert_eq!(get_total_accounted(&env, &token), 1);
    }

    #[test]
    fn sub_total_accounted_repeated_interleaved_with_adds() {
        let env = Env::default();
        let token = Address::generate(&env);
        add_total_accounted(&env, &token, 1_000).unwrap();
        sub_total_accounted(&env, &token, 250).unwrap();
        add_total_accounted(&env, &token, 500).unwrap();
        sub_total_accounted(&env, &token, 1_250).unwrap();
        assert_eq!(get_total_accounted(&env, &token), 0);
    }
}