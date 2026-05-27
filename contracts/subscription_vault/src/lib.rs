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
    pub fn init(env: Env, admin: Address, token: Address, min_topup: i128) -> Result<(), Error> {
        if env
            .storage()
            .instance()
            .has(&Symbol::new(&env, "admin"))
        {
            return Err(Error::AlreadyInitialized);
        }
        env.storage().instance().set(&Symbol::new(&env, "admin"), &admin);
        env.storage().instance().set(&Symbol::new(&env, "token"), &token);
        env.storage().instance().set(&Symbol::new(&env, "min_topup"), &min_topup);
        Ok(())
    }

    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        expires_at: Option<u64>,
    ) -> Result<u32, Error> {
        subscriber.require_auth();

        if amount <= 0 {
            return Err(Error::InvalidArgument);
        }
        if interval_seconds == 0 {
            return Err(Error::InvalidArgument);
        }
        if let Some(ts) = expires_at {
            if ts <= env.ledger().timestamp() {
                return Err(Error::InvalidArgument);
            }
        }

        let token: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "token"))
            .ok_or(Error::NotFound)?;

        let id = Self::_next_id(&env)?;
        let sub = Subscription {
            subscriber,
            token,
            merchant,
            amount,
            interval_seconds,
            last_payment_timestamp: env.ledger().timestamp(),
            status: SubscriptionStatus::Active,
            prepaid_balance: 0,
            usage_enabled,
            expires_at,
        };
        env.storage().instance().set(&id, &sub);
        Ok(id)
    }

    pub fn get_subscription(env: Env, id: u32) -> Result<Subscription, Error> {
        env.storage()
            .instance()
            .get(&id)
            .ok_or(Error::NotFound)
    }

    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "min_topup"))
            .ok_or(Error::NotFound)
    }

    pub fn get_subscription_count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "next_id"))
            .unwrap_or(0)
    }

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
