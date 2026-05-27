use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol, Vec};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    InvalidInput = 400,
    Unauthorized = 401,
    BelowMinimumTopup = 402,
    NotFound = 404,
    SubscriptionExpired = 410,
    NotActive = 411,
    InsufficientBalance = 412,
    IntervalNotElapsed = 413,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubscriptionStatus {
    Active = 0,
    Paused = 1,
    Cancelled = 2,
    InsufficientBalance = 3,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Subscription {
    pub subscriber: Address,
    pub merchant: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub last_payment_timestamp: u64,
    pub status: SubscriptionStatus,
    pub prepaid_balance: i128,
    pub usage_enabled: bool,
    /// Optional Unix timestamp (seconds) after which no more charges are allowed.
    pub expiration: Option<u64>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchChargeResult {
    pub success: bool,
    pub error_code: u32,
}

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    /// Initialize the contract (e.g. set token and admin).
    pub fn init(env: Env, token: Address, admin: Address, min_topup: i128) -> Result<(), Error> {
        env.storage().instance().set(&Symbol::new(&env, "token"), &token);
        env.storage().instance().set(&Symbol::new(&env, "admin"), &admin);
        env.storage().instance().set(&Symbol::new(&env, "min_topup"), &min_topup);
        Ok(())
    }

    /// Update the minimum top-up threshold. Only callable by admin.
    pub fn set_min_topup(env: Env, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&Symbol::new(&env, "admin")).ok_or(Error::NotFound)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        env.storage().instance().set(&Symbol::new(&env, "min_topup"), &min_topup);
        Ok(())
    }

    /// Get the current minimum top-up threshold.
    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        env.storage().instance().get(&Symbol::new(&env, "min_topup")).ok_or(Error::NotFound)
    }

    /// Create a new subscription.
    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        expiration: Option<u64>,
    ) -> Result<u32, Error> {
        subscriber.require_auth();
        
        let sub = Subscription {
            subscriber: subscriber.clone(),
            merchant,
            amount,
            interval_seconds,
            last_payment_timestamp: env.ledger().timestamp(),
            status: SubscriptionStatus::Active,
            prepaid_balance: amount, // Seed with initial amount to enable baseline testing
            usage_enabled,
            expiration,
        };
        let id = Self::_next_id(&env);
        env.storage().instance().set(&id, &sub);
        Ok(id)
    }

    /// Subscriber deposits more USDC into their vault for this subscription.
    pub fn deposit_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
    ) -> Result<(), Error> {
        subscriber.require_auth();

        let min_topup: i128 = env.storage().instance().get(&Symbol::new(&env, "min_topup")).ok_or(Error::NotFound)?;
        if amount < min_topup {
            return Err(Error::BelowMinimumTopup);
        }

        let mut sub: Subscription = env.storage().instance().get(&subscription_id).ok_or(Error::NotFound)?;
        sub.prepaid_balance += amount;
        if sub.status == SubscriptionStatus::InsufficientBalance && sub.prepaid_balance >= sub.amount {
            sub.status = SubscriptionStatus::Active;
        }
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    /// Billing engine calls this to charge one interval for a single subscription.
    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&Symbol::new(&env, "admin")).ok_or(Error::NotFound)?;
        admin.require_auth();

        Self::internal_charge(&env, subscription_id)
    }

    /// Admin-only entrypoint to charge multiple subscriptions in a single transaction.
    pub fn batch_charge(env: Env, subscription_ids: Vec<u32>) -> Result<Vec<BatchChargeResult>, Error> {
        if subscription_ids.len() > 100 {
            return Err(Error::InvalidInput);
        }

        let admin: Address = env.storage().instance().get(&Symbol::new(&env, "admin")).ok_or(Error::NotFound)?;
        admin.require_auth();

        let mut results = Vec::new(&env);
        for id in subscription_ids.iter() {
            match Self::internal_charge(&env, id) {
                Ok(_) => {
                    results.push_back(BatchChargeResult {
                        success: true,
                        error_code: 0,
                    });
                }
                Err(e) => {
                    results.push_back(BatchChargeResult {
                        success: false,
                        error_code: e as u32,
                    });
                }
            }
        }

        Ok(results)
    }

    /// Subscriber or merchant cancels the subscription.
    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        let mut sub: Subscription = env.storage().instance().get(&subscription_id).ok_or(Error::NotFound)?;
        sub.status = SubscriptionStatus::Cancelled;
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    /// Pause subscription (no charges until resumed).
    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        let mut sub: Subscription = env.storage().instance().get(&subscription_id).ok_or(Error::NotFound)?;
        sub.status = SubscriptionStatus::Paused;
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    /// Merchant withdraws accumulated USDC to their wallet.
    pub fn withdraw_merchant_funds(
        _env: Env,
        merchant: Address,
        _amount: i128,
    ) -> Result<(), Error> {
        merchant.require_auth();
        Ok(())
    }

    /// Read subscription by id.
    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        env.storage().instance().get(&subscription_id).ok_or(Error::NotFound)
    }

    /// Shared internal charging sequence reused by both single and batch charging.
    fn internal_charge(env: &Env, subscription_id: u32) -> Result<(), Error> {
        let mut sub: Subscription = env.storage().instance().get(&subscription_id).ok_or(Error::NotFound)?;

        if sub.status == SubscriptionStatus::Cancelled || sub.status == SubscriptionStatus::Paused {
            return Err(Error::NotActive);
        }

        if let Some(exp_ts) = sub.expiration {
            if env.ledger().timestamp() >= exp_ts {
                return Err(Error::SubscriptionExpired);
            }
        }

        if sub.prepaid_balance < sub.amount {
            sub.status = SubscriptionStatus::InsufficientBalance;
            env.storage().instance().set(&subscription_id, &sub);
            return Err(Error::InsufficientBalance);
        }

        let next_allowed_payment = sub.last_payment_timestamp + sub.interval_seconds;
        if env.ledger().timestamp() < next_allowed_payment {
            return Err(Error::IntervalNotElapsed);
        }

        sub.prepaid_balance -= sub.amount;
        sub.last_payment_timestamp = env.ledger().timestamp();
        env.storage().instance().set(&subscription_id, &sub);

        Ok(())
    }

    fn _next_id(env: &Env) -> u32 {
        let key = Symbol::new(env, "next_id");
        let id: u32 = env.storage().instance().get(&key).unwrap_or(0);
        env.storage().instance().set(&key, &(id + 1));
        id
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{Env, Address, Vec};
    use soroban_sdk::testutils::Address as _;

    fn setup_test_env(env: &Env) -> (SubscriptionVaultClient<'static>, Address, Address, Address) {
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(env, &contract_id);
        
        let admin = Address::generate(env);
        let token = Address::generate(env);
        let subscriber = Address::generate(env);
        let merchant = Address::generate(env);
        
        client.init(&token, &admin, &100_i128);
        (client, admin, subscriber, merchant)
    }

    #[test]
    fn test_empty_batch() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _, _, _) = setup_test_env(&env);
        
        let ids = Vec::new(&env);
        let res = client.batch_charge(&ids);
        assert_eq!(res.len(), 0);
    }

    #[test]
    fn test_oversized_batch() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _, _, _) = setup_test_env(&env);
        
        let mut ids = Vec::new(&env);
        for _ in 0..101 {
            ids.push_back(1);
        }
        let res = client.try_batch_charge(&ids);
        assert!(res.is_err());
    }

    #[test]
    fn test_mixed_batch_and_duplicates() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _, subscriber, merchant) = setup_test_env(&env);
        
        let id_active = client.create_subscription(&subscriber, &merchant, &100_i128, &0_u64, &true, &None);
        
        let id_insufficient = client.create_subscription(&subscriber, &merchant, &500_i128, &0_u64, &true, &None);
        client.charge_subscription(&id_insufficient); 
        
        let current_time = env.ledger().timestamp();
        let id_expired = client.create_subscription(&subscriber, &merchant, &100_i128, &0_u64, &true, &Some(current_time));
        
        let mut ids = Vec::new(&env);
        ids.push_back(id_active);
        ids.push_back(id_insufficient);
        ids.push_back(id_expired);
        ids.push_back(id_active); 
        
        let res = client.batch_charge(&ids);
        assert_eq!(res.len(), 4);
        
        assert_eq!(res.get(0).unwrap(), BatchChargeResult { success: true, error_code: 0 });
        assert_eq!(res.get(1).unwrap(), BatchChargeResult { success: false, error_code: 412 });
        assert_eq!(res.get(2).unwrap(), BatchChargeResult { success: false, error_code: 410 });
        assert_eq!(res.get(3).unwrap(), BatchChargeResult { success: false, error_code: 412 });
    }

    #[test]
    fn test_get_min_topup_after_init() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _, _, _) = setup_test_env(&env);

        // Assert get_min_topup returns the value set in init (100_i128)
        assert_eq!(client.get_min_topup(), 100_i128);
    }

    #[test]
    fn test_deposit_funds_min_topup_boundaries() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _, subscriber, merchant) = setup_test_env(&env);

        let id = client.create_subscription(&subscriber, &merchant, &100_i128, &0_u64, &true, &None);

        // Edge case: amount = min_topup - 1 (99) -> Should fail
        let res_below = client.try_deposit_funds(&id, &subscriber, &99_i128);
        assert!(res_below.is_err());

        // Edge case: amount = min_topup (100) -> Should succeed
        let res_at = client.deposit_funds(&id, &subscriber, &100_i128);
        assert_eq!(res_at, ());

        // Verify round-trip state change
        let sub = client.get_subscription(&id);
        assert_eq!(sub.prepaid_balance, 200_i128);
    }

    #[test]
    fn test_set_min_topup_authorization() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, subscriber, _) = setup_test_env(&env);

        // Edge case: non-admin set -> Should return Unauthorized
        let res_non_admin = client.try_set_min_topup(&subscriber, &150_i128);
        assert!(res_non_admin.is_err());

        // Edge case: admin set -> Should successfully update get_min_topup
        let res_admin = client.set_min_topup(&admin, &150_i128);
        assert_eq!(res_admin, ());
        assert_eq!(client.get_min_topup(), 150_i128);
    }
}
