#![no_std]

//! Subscription Vault contract with overflow-safe ID allocation.
//!
//! This contract manages subscription IDs with guaranteed uniqueness and
//! monotonic increment. The ID allocation is protected against overflow
//! by checking the limit before incrementing.

use soroban_sdk::{contract, contracterror, contractimpl, Env, Symbol, Vec};

/// Maximum subscription ID that can be allocated.
pub const MAX_SUBSCRIPTION_ID: u32 = u32::MAX;

/// Contract error codes.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    /// Subscription limit reached (code 429).
    SubscriptionLimitReached = 429,
    /// Subscription not found (code 404).
    NotFound = 404,
}

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    /// Returns the schema version of this contract.
    pub fn version(_env: Env) -> u32 {
        1
    }

    /// Returns the current subscription count.
    ///
    /// This equals the total number of subscriptions ever created,
    /// including cancelled and expired ones.
    pub fn get_subscription_count(env: Env) -> u32 {
        let key = Symbol::new(&env, "next_id");
        env.storage()
            .instance()
            .get(&key)
            .unwrap_or(0u32)
    }

    /// Creates a new subscription and returns its ID.
    ///
    /// # Errors
    ///
    /// Returns `Error::SubscriptionLimitReached` if the ID space is exhausted.
    pub fn create_subscription(env: Env) -> Result<u32, Error> {
        Self::_next_id(&env)
    }

    /// Internal helper to allocate the next subscription ID.
    ///
    /// This function implements overflow-safe ID allocation by checking
    /// the limit before incrementing the counter.
    fn _next_id(env: &Env) -> Result<u32, Error> {
        let key = Symbol::new(env, "next_id");
        let current: u32 = env.storage().instance().get(&key).unwrap_or(0u32);

        if current == MAX_SUBSCRIPTION_ID {
            return Err(Error::SubscriptionLimitReached);
        }

        env.storage().instance().set(&key, &(current + 1));
        Ok(current)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::SubscriptionVaultClient;

    #[test]
    fn version_is_one() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        assert_eq!(client.version(), 1);
    }

    #[test]
    fn test_id_starts_at_zero() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let id = client.create_subscription().unwrap();
        assert_eq!(id, 0);
    }

    #[test]
    fn test_ids_are_monotonically_increasing() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        for expected in 0..10u32 {
            let id = client.create_subscription().unwrap();
            assert_eq!(id, expected);
        }
    }

    #[test]
    fn test_ids_are_unique() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let mut ids = Vec::new(&env);
        for _ in 0..100 {
            let id = client.create_subscription().unwrap();
            ids.push_back(id);
        }

        // Check all IDs are unique
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids.get(i), ids.get(j));
            }
        }
    }

    #[test]
    fn test_get_subscription_count() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        assert_eq!(client.get_subscription_count(), 0);

        for i in 1..=10 {
            client.create_subscription().unwrap();
            assert_eq!(client.get_subscription_count(), i);
        }
    }

    #[test]
    fn test_id_at_max_minus_one_succeeds() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        // Manually set counter to MAX_SUBSCRIPTION_ID - 1
        let key = Symbol::new(&env, symbol_short!("next_id"));
        env.storage()
            .instance()
            .set(&key, &(MAX_SUBSCRIPTION_ID - 1));

        let id = client.create_subscription().unwrap();
        assert_eq!(id, MAX_SUBSCRIPTION_ID - 1);
    }

    #[test]
    fn test_id_at_max_returns_limit_reached() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        // Manually set counter to MAX_SUBSCRIPTION_ID
        let key = Symbol::new(&env, symbol_short!("next_id"));
        env.storage()
            .instance()
            .set(&key, &MAX_SUBSCRIPTION_ID);

        let result = client.create_subscription();
        assert_eq!(result, Err(Error::SubscriptionLimitReached));
    }

    #[test]
    fn test_no_id_reuse_after_limit() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        // Manually set counter to MAX_SUBSCRIPTION_ID
        let key = Symbol::new(&env, symbol_short!("next_id"));
        env.storage()
            .instance()
            .set(&key, &MAX_SUBSCRIPTION_ID);

        // Multiple calls should all fail
        for _ in 0..10 {
            let result = client.create_subscription();
            assert_eq!(result, Err(Error::SubscriptionLimitReached));
        }

        // Counter should remain at MAX_SUBSCRIPTION_ID
        assert_eq!(client.get_subscription_count(), MAX_SUBSCRIPTION_ID);
    }

    #[test]
    fn test_counter_persisted_across_calls() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        // Create some subscriptions
        for _ in 0..5 {
            client.create_subscription().unwrap();
        }

        // Count should be 5
        assert_eq!(client.get_subscription_count(), 5);

        // Create more subscriptions
        for _ in 0..3 {
            client.create_subscription().unwrap();
        }

        // Count should be 8
        assert_eq!(client.get_subscription_count(), 8);

        // Next ID should be 8
        let id = client.create_subscription().unwrap();
        assert_eq!(id, 8);
    }
}
