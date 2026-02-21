#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol};

#[contracterror]
#[repr(u32)]
#[derive(Debug, PartialEq)]
pub enum Error {
    NotFound = 404,
    Unauthorized = 401,
    InsufficientBalance = 402,
    BelowMinimumTopup = 402,
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
}

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    /// Initialize the contract (e.g. set token and admin). Extend as needed.
    pub fn init(env: Env, token: Address, admin: Address, min_topup: i128) -> Result<(), Error> {
        env.storage().instance().set(&Symbol::new(&env, "token"), &token);
        env.storage().instance().set(&Symbol::new(&env, "admin"), &admin);
        env.storage().instance().set(&Symbol::new(&env, "min_topup"), &min_topup);
        Ok(())
    }

    /// Update the minimum top-up threshold. Only callable by admin.
    /// 
    /// # Arguments
    /// * `min_topup` - Minimum amount (in token base units) required for deposit_funds.
    ///                 Prevents inefficient micro-deposits. Typical range: 1-10 USDC (1_000000 - 10_000000 for 6 decimals).
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

    /// Create a new subscription. Caller deposits initial USDC; contract stores agreement.
    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
    ) -> Result<u32, Error> {
        subscriber.require_auth();
        // TODO: transfer initial deposit from subscriber to contract, then store subscription
        let sub = Subscription {
            subscriber: subscriber.clone(),
            merchant,
            amount,
            interval_seconds,
            last_payment_timestamp: env.ledger().timestamp(),
            status: SubscriptionStatus::Active,
            prepaid_balance: 0i128, // TODO: set from initial deposit
            usage_enabled,
        };
        let id = Self::_next_id(&env);
        env.storage().instance().set(&id, &sub);
        Ok(id)
    }

    /// Subscriber deposits more USDC into their vault for this subscription.
    /// 
    /// # Recovery Flow
    /// If subscription status is InsufficientBalance, deposits will:
    /// 1. Add funds to prepaid_balance
    /// 2. Transition status back to Active
    /// 
    /// This enables the recovery flow: InsufficientBalance → deposit_funds → Active
    /// # Minimum top-up enforcement
    /// Rejects deposits below the configured minimum threshold to prevent inefficient
    /// micro-transactions that waste gas and complicate accounting. The minimum is set
    /// globally at contract initialization and adjustable by admin via `set_min_topup`.
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
        
        // TODO: transfer USDC from subscriber, increase prepaid_balance for subscription_id
        
        // Load subscription
        let mut subscription: Subscription = env.storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;

        // Verify subscriber is authorized
        if subscription.subscriber != subscriber {
            return Err(Error::Unauthorized);
        }

        // Add funds to prepaid balance
        subscription.prepaid_balance += amount;

        // Recovery flow: transition from InsufficientBalance back to Active
        if subscription.status == SubscriptionStatus::InsufficientBalance {
            subscription.status = SubscriptionStatus::Active;
        }

        env.storage().instance().set(&subscription_id, &subscription);
        
        Ok(())
    }

    /// Billing engine (backend) calls this to charge one interval. Deducts from vault, pays merchant.
    /// 
    /// # Arguments
    /// * `subscription_id` - The ID of the subscription to charge
    /// 
    /// # Returns
    /// * `Ok(())` - Charge successful, balance deducted and updated
    /// * `Err(Error::NotFound)` - Subscription does not exist
    /// * `Err(Error::InsufficientBalance)` - Balance too low, status set to InsufficientBalance
    /// 
    /// # Invariants
    /// * If charge fails due to insufficient balance, prepaid_balance remains UNCHANGED
    /// * Status transitions to InsufficientBalance on failed charge
    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        // TODO: require_caller admin or authorized billing service
        
        // Load subscription
        let mut subscription: Subscription = env.storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;

        // Can only charge Active subscriptions
        if subscription.status != SubscriptionStatus::Active {
            return Ok(()); // Skip charging for non-active subscriptions
        }

        // Check if interval has passed (simple time check)
        let current_time = env.ledger().timestamp();
        if current_time < subscription.last_payment_timestamp + subscription.interval_seconds {
            return Ok(()); // Interval not yet passed
        }

        // Check if sufficient balance exists
        if subscription.prepaid_balance < subscription.amount {
            // CRITICAL: Non-destructive failure - do NOT modify balance
            // Set status to InsufficientBalance to signal to frontend/backend
            subscription.status = SubscriptionStatus::InsufficientBalance;
            env.storage().instance().set(&subscription_id, &subscription);
            return Err(Error::InsufficientBalance);
        }

        // Deduct amount from prepaid balance (successful charge)
        subscription.prepaid_balance -= subscription.amount;
        subscription.last_payment_timestamp = current_time;
        
        env.storage().instance().set(&subscription_id, &subscription);
        
        Ok(())
    }

    /// Subscriber or merchant cancels the subscription. Remaining balance can be withdrawn by subscriber.
    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        // TODO: load subscription, set status Cancelled, allow withdraw of prepaid_balance
        let _ = (env, subscription_id);
        Ok(())
    }

    /// Pause subscription (no charges until resumed).
    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        // TODO: load subscription, set status Paused
        let _ = (env, subscription_id);
        Ok(())
    }

    /// Merchant withdraws accumulated USDC to their wallet.
    pub fn withdraw_merchant_funds(
        _env: Env,
        merchant: Address,
        _amount: i128,
    ) -> Result<(), Error> {
        merchant.require_auth();
        // TODO: deduct from merchant's balance in contract, transfer token to merchant
        Ok(())
    }

    /// Read subscription by id (for indexing and UI).
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
mod test;
