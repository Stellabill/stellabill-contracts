#![no_std]

//! Subscription Vault contract.
//!
//! This contract manages prepaid subscriptions with recurring USDC billing on Stellar.
//! The contract includes initialization protection to prevent re-initialization attacks.

use soroban_sdk::{
    contract, contracterror, contractimpl, symbol_short, Address, Env, Symbol,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
}

/// Storage keys for instance data.
#[derive(Clone)]
pub enum DataKey {
    Admin = 0,
    Token = 1,
    MinTopup = 2,
}

impl DataKey {
    pub fn to_symbol(&self) -> Symbol {
        match self {
            DataKey::Admin => symbol_short!("admin"),
            DataKey::Token => symbol_short!("token"),
            DataKey::MinTopup => symbol_short!("min_topup"),
        }
    }
}

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    /// Returns the schema version of this contract.
    pub fn version(_env: Env) -> u32 {
        0
    }

    /// Initialize the contract with admin, token, and minimum topup amount.
    ///
    /// # Security
    /// This function can only be called once. The admin key serves as a sentinel
    /// to detect whether initialization has already occurred. Any attempt to
    /// re-initialize will return `Error::AlreadyInitialized` and leave the existing
    /// configuration unchanged.
    ///
    /// # Arguments
    /// * `admin` - The admin address that will control the contract
    /// * `token` - The token address used for payments
    /// * `min_topup` - The minimum topup amount in token units
    ///
    /// # Errors
    /// * `Error::AlreadyInitialized` - If the contract has already been initialized
    pub fn init(env: Env, admin: Address, token: Address, min_topup: i128) -> Result<(), Error> {
        // Check if already initialized by verifying the admin key exists
        if env.storage().instance().has(&DataKey::Admin.to_symbol()) {
            return Err(Error::AlreadyInitialized);
        }

        // Store initial configuration
        env.storage()
            .instance()
            .set(&DataKey::Admin.to_symbol(), &admin);
        env.storage()
            .instance()
            .set(&DataKey::Token.to_symbol(), &token);
        env.storage()
            .instance()
            .set(&DataKey::MinTopup.to_symbol(), &min_topup);

        Ok(())
    }

    /// Get the current admin address.
    pub fn get_admin(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::Admin.to_symbol())
    }

    /// Get the token address.
    pub fn get_token(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::Token.to_symbol())
    }

    /// Get the minimum topup amount.
    pub fn get_min_topup(env: Env) -> Option<i128> {
        env.storage()
            .instance()
            .get(&DataKey::MinTopup.to_symbol())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{Address, Env};
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn version_is_zero() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        assert_eq!(client.version(), 0);
    }

    #[test]
    fn init_succeeds_on_first_call() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        let min_topup = 1000;

        // First init should succeed
        client.init(&admin, &token, &min_topup);

        // Verify values were stored
        assert_eq!(client.get_admin(), Some(admin));
        assert_eq!(client.get_token(), Some(token));
        assert_eq!(client.get_min_topup(), Some(min_topup));
    }

    #[test]
    fn init_fails_on_second_call() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        let min_topup = 1000;

        // First init should succeed
        client.init(&admin, &token, &min_topup);

        // Second init should fail with AlreadyInitialized error
        let result = client.try_init(&admin, &token, &min_topup);
        assert!(result.is_err());
        assert_eq!(result.err(), Some(Ok(Error::AlreadyInitialized)));
    }

    #[test]
    fn re_init_does_not_modify_existing_values() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let admin1 = Address::generate(&env);
        let token1 = Address::generate(&env);
        let min_topup1 = 1000;

        // First init
        client.init(&admin1, &token1, &min_topup1);

        // Attempt re-init with different values
        let admin2 = Address::generate(&env);
        let token2 = Address::generate(&env);
        let min_topup2 = 2000;

        let result = client.try_init(&admin2, &token2, &min_topup2);
        assert!(result.is_err());

        // Verify original values are unchanged
        assert_eq!(client.get_admin(), Some(admin1));
        assert_eq!(client.get_token(), Some(token1));
        assert_eq!(client.get_min_topup(), Some(min_topup1));
    }

    #[test]
    fn get_functions_return_none_before_init() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        // Before init, all get functions should return None
        assert_eq!(client.get_admin(), None);
        assert_eq!(client.get_token(), None);
        assert_eq!(client.get_min_topup(), None);
    }
}
