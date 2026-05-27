#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol};

#[contracterror]
#[repr(u32)]
pub enum Error {
    NotFound = 404,
    Unauthorized = 401,
    BelowMinimumTopup = 402,
    SubscriptionExpired = 410,
    NotActive = 1002,
    UsageNotEnabled = 1004,
    InsufficientPrepaidBalance = 1005,
    InvalidAmount = 1006,
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
    pub expiration: Option<u64>,
}

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    /// Initialize the contract with token, admin, and minimum top-up requirements.
    pub fn init(env: Env, token: Address, admin: Address, min_topup: i128) -> Result<(), Error> {
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "token"), &token);
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "admin"), &admin);
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "min_topup"), &min_topup);
        Ok(())
    }

    /// Update the minimum top-up threshold. Only callable by admin.
    pub fn set_min_topup(env: Env, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "admin"))
            .ok_or(Error::NotFound)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "min_topup"), &min_topup);
        Ok(())
    }

    /// Get the current minimum top-up threshold.
    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "min_topup"))
            .ok_or(Error::NotFound)
    }

    /// Create a new subscription agreement.
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
            prepaid_balance: 0i128,
            usage_enabled,
            expiration,
        };
        let id = Self::_next_id(&env);
        env.storage().instance().set(&id, &sub);
        Ok(id)
    }

    /// Subscriber deposits funds to increase prepaid balance.
    pub fn deposit_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
    ) -> Result<(), Error> {
        subscriber.require_auth();
        let min_topup: i128 = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "min_topup"))
            .ok_or(Error::NotFound)?;
        if amount < min_topup {
            return Err(Error::BelowMinimumTopup);
        }
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;
        sub.prepaid_balance += amount;
        if sub.status == SubscriptionStatus::InsufficientBalance && sub.prepaid_balance > 0 {
            sub.status = SubscriptionStatus::Active;
        }
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    /// Record metered usage units for the subscription (Admin Authorized).
    pub fn report_usage(
        env: Env,
        admin: Address,
        subscription_id: u32,
        units: i128,
    ) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "admin"))
            .ok_or(Error::NotFound)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        if units < 0 {
            return Err(Error::InvalidAmount);
        }
        let sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;
        if sub.status != SubscriptionStatus::Active {
            return Err(Error::NotActive);
        }
        if !sub.usage_enabled {
            return Err(Error::UsageNotEnabled);
        }

        let key = (Symbol::new(&env, "usage"), subscription_id);
        let current_usage: i128 = env.storage().instance().get(&key).unwrap_or(0);
        env.storage().instance().set(&key, &(current_usage + units));
        Ok(())
    }

    /// Billing execution entry point.
    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;
        if sub.status != SubscriptionStatus::Active {
            return Err(Error::NotActive);
        }
        if let Some(exp_ts) = sub.expiration {
            if env.ledger().timestamp() >= exp_ts {
                return Err(Error::SubscriptionExpired);
            }
        }

        let charge_amount = if sub.usage_enabled {
            let key = (Symbol::new(&env, "usage"), subscription_id);
            let units: i128 = env.storage().instance().get(&key).unwrap_or(0);
            let total_charge = units * sub.amount;
            env.storage().instance().set(&key, &0i128); // Reset usage accumulated
            total_charge
        } else {
            sub.amount
        };

        if charge_amount > 0 {
            if sub.prepaid_balance < charge_amount {
                return Err(Error::InsufficientPrepaidBalance);
            }
            sub.prepaid_balance -= charge_amount;
            if sub.prepaid_balance == 0 {
                sub.status = SubscriptionStatus::InsufficientBalance;
            }
        }

        sub.last_payment_timestamp = env.ledger().timestamp();
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    /// Standalone usage charge endpoint for direct off-chain sync updates.
    pub fn charge_usage(env: Env, subscription_id: u32, usage_amount: i128) -> Result<(), Error> {
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;
        if sub.status != SubscriptionStatus::Active {
            return Err(Error::NotActive);
        }
        if !sub.usage_enabled {
            return Err(Error::UsageNotEnabled);
        }
        if usage_amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        if sub.prepaid_balance < usage_amount {
            return Err(Error::InsufficientPrepaidBalance);
        }

        sub.prepaid_balance -= usage_amount;
        if sub.prepaid_balance == 0 {
            sub.status = SubscriptionStatus::InsufficientBalance;
        }
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;
        sub.status = SubscriptionStatus::Cancelled;
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;
        sub.status = SubscriptionStatus::Paused;
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    pub fn withdraw_merchant_funds(
        _env: Env,
        merchant: Address,
        _amount: i128,
    ) -> Result<(), Error> {
        merchant.require_auth();
        Ok(())
    }

    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        env.storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)
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
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Address, Env}; // Brings the .generate() method into scope

    fn setup_test_env() -> (
        Env,
        SubscriptionVaultClient<'static>,
        Address,
        Address,
        Address,
    ) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);

        client.init(&token, &admin, &10i128);
        (env, client, admin, subscriber, merchant)
    }

    #[test]
    fn test_fixed_interval_charging() {
        let (_env, client, _admin, subscriber, merchant) = setup_test_env();
        let sub_id =
            client.create_subscription(&subscriber, &merchant, &100i128, &3600, &false, &None);
        client.deposit_funds(&sub_id, &subscriber, &500i128);

        client.charge_subscription(&sub_id);
        let sub = client.get_subscription(&sub_id);
        assert_eq!(sub.prepaid_balance, 400);
        assert_eq!(sub.status, SubscriptionStatus::Active);
    }

    #[test]
    fn test_report_usage_and_charging_path() {
        let (_env, client, admin, subscriber, merchant) = setup_test_env();
        let sub_id =
            client.create_subscription(&subscriber, &merchant, &5i128, &3600, &true, &None);
        client.deposit_funds(&sub_id, &subscriber, &500i128);

        client.report_usage(&admin, &sub_id, &10i128);
        client.charge_subscription(&sub_id);

        let sub = client.get_subscription(&sub_id);
        assert_eq!(sub.prepaid_balance, 450);
    }

    #[test]
    fn test_zero_usage_charge() {
        let (_env, client, _admin, subscriber, merchant) = setup_test_env();
        let sub_id =
            client.create_subscription(&subscriber, &merchant, &5i128, &3600, &true, &None);
        client.deposit_funds(&sub_id, &subscriber, &100i128);

        client.charge_subscription(&sub_id);
        let sub = client.get_subscription(&sub_id);
        assert_eq!(sub.prepaid_balance, 100);
    }

    #[test]
    fn test_usage_charge_exceeding_prepaid_balance() {
        let (_env, client, admin, subscriber, merchant) = setup_test_env();
        let sub_id =
            client.create_subscription(&subscriber, &merchant, &10i128, &3600, &true, &None);
        client.deposit_funds(&sub_id, &subscriber, &50i128);

        client.report_usage(&admin, &sub_id, &10i128);
        let res = client.try_charge_subscription(&sub_id);
        assert!(res.is_err());
    }

    #[test]
    fn test_charge_usage_direct() {
        let (_env, client, _admin, subscriber, merchant) = setup_test_env();
        let sub_id =
            client.create_subscription(&subscriber, &merchant, &5i128, &3600, &true, &None);
        client.deposit_funds(&sub_id, &subscriber, &100i128);

        client.charge_usage(&sub_id, &40i128);
        let sub = client.get_subscription(&sub_id);
        assert_eq!(sub.prepaid_balance, 60);
    }

    #[test]
    fn test_min_topup_and_management() {
        let (_env, client, admin, subscriber, merchant) = setup_test_env();
        let sub_id =
            client.create_subscription(&subscriber, &merchant, &100i128, &3600, &false, &None);

        let low_deposit = client.try_deposit_funds(&sub_id, &subscriber, &5i128);
        assert!(low_deposit.is_err());

        client.set_min_topup(&admin, &50i128);
        assert_eq!(client.get_min_topup(), 50);
    }

    #[test]
    fn test_pause_and_cancel() {
        let (_env, client, _admin, subscriber, merchant) = setup_test_env();
        let sub_id =
            client.create_subscription(&subscriber, &merchant, &100i128, &3600, &false, &None);

        client.pause_subscription(&sub_id, &subscriber);
        let sub_paused = client.get_subscription(&sub_id);
        assert_eq!(sub_paused.status, SubscriptionStatus::Paused);

        client.cancel_subscription(&sub_id, &subscriber);
        let sub_cancelled = client.get_subscription(&sub_id);
        assert_eq!(sub_cancelled.status, SubscriptionStatus::Cancelled);
    }
}
