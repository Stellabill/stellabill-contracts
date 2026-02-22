#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, token, Address, Env, Symbol};

#[contracterror]
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    NotFound = 404,
    Unauthorized = 401,
    BelowMinimumTopup = 402,
    InvalidAmount = 403,
    InsufficientAllowance = 405,
    TransferFailed = 406,
    InsufficientBalance = 407,
    InvalidStatus = 408,
    ArithmeticOverflow = 409,
    BelowMerchantMinimum = 410,
}

#[contracttype]
#[derive(Clone, Debug)]
pub enum DataKey {
    MerchantBalance(Address),
    MerchantConfig(Address),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MerchantConfig {
    /// Schema version for future compatible config expansion.
    pub version: u32,
    /// Minimum subscription amount accepted for this merchant (0 = no merchant-specific minimum).
    pub min_subscription_amount: i128,
    /// Default interval used when `create_subscription` is called with interval `0`.
    pub default_interval_seconds: u64,
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
    /// Wallet that owns and funds this subscription.
    pub subscriber: Address,
    /// Wallet that receives periodic charges.
    pub merchant: Address,
    /// Billing amount charged per interval in token base units.
    pub amount: i128,
    /// Length of each billing interval in seconds.
    pub interval_seconds: u64,
    /// Ledger timestamp of the last successful payment lifecycle event.
    pub last_payment_timestamp: u64,
    /// Current subscription status.
    pub status: SubscriptionStatus,
    /// Subscriber funds currently held in the vault for this subscription.
    pub prepaid_balance: i128,
    /// If true, usage-based add-ons may be charged by downstream logic.
    pub usage_enabled: bool,
}

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    /// Initialize the vault with token/admin config and minimum top-up threshold.
    pub fn init(env: Env, token: Address, admin: Address, min_topup: i128) -> Result<(), Error> {
        if min_topup <= 0 {
            return Err(Error::InvalidAmount);
        }
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
        if min_topup <= 0 {
            return Err(Error::InvalidAmount);
        }
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

    /// Set full merchant configuration. Callable by the merchant or contract admin.
    pub fn set_merchant_config(
        env: Env,
        actor: Address,
        merchant: Address,
        min_subscription_amount: i128,
        default_interval_seconds: u64,
    ) -> Result<(), Error> {
        if min_subscription_amount < 0 {
            return Err(Error::InvalidAmount);
        }
        Self::require_admin_or_merchant(&env, &actor, &merchant)?;
        let config = MerchantConfig {
            version: 1,
            min_subscription_amount,
            default_interval_seconds,
        };
        Self::write_merchant_config(&env, &merchant, &config);
        Ok(())
    }

    /// Update merchant configuration fields. Any `None` field keeps its current value.
    pub fn update_merchant_config(
        env: Env,
        actor: Address,
        merchant: Address,
        min_subscription_amount: Option<i128>,
        default_interval_seconds: Option<u64>,
    ) -> Result<(), Error> {
        Self::require_admin_or_merchant(&env, &actor, &merchant)?;
        let mut current = Self::read_merchant_config(&env, &merchant);
        if let Some(min) = min_subscription_amount {
            if min < 0 {
                return Err(Error::InvalidAmount);
            }
            current.min_subscription_amount = min;
        }
        if let Some(default_interval) = default_interval_seconds {
            current.default_interval_seconds = default_interval;
        }
        Self::write_merchant_config(&env, &merchant, &current);
        Ok(())
    }

    /// Return merchant configuration. If unset, returns default config values.
    pub fn get_merchant_config(env: Env, merchant: Address) -> Result<MerchantConfig, Error> {
        Ok(Self::read_merchant_config(&env, &merchant))
    }

    /// Create a new subscription and pull initial prepaid funds into the vault.
    ///
    /// `amount` is both the recurring charge amount and the required initial prepaid deposit.
    /// The subscriber must approve this contract as spender on the token contract before calling.
    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
    ) -> Result<u32, Error> {
        subscriber.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let merchant_config = Self::read_merchant_config(&env, &merchant);
        if merchant_config.min_subscription_amount > 0
            && amount < merchant_config.min_subscription_amount
        {
            return Err(Error::BelowMerchantMinimum);
        }
        let effective_interval_seconds = if interval_seconds == 0 {
            if merchant_config.default_interval_seconds == 0 {
                return Err(Error::InvalidAmount);
            }
            merchant_config.default_interval_seconds
        } else {
            interval_seconds
        };

        let token_address: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "token"))
            .ok_or(Error::NotFound)?;
        let token_client = token::Client::new(&env, &token_address);
        let contract_address = env.current_contract_address();

        let allowance = token_client.allowance(&subscriber, &contract_address);
        if allowance < amount {
            return Err(Error::InsufficientAllowance);
        }

        let balance = token_client.balance(&subscriber);
        if balance < amount {
            return Err(Error::TransferFailed);
        }

        token_client.transfer_from(&contract_address, &subscriber, &contract_address, &amount);
        let now = env.ledger().timestamp();
        let sub = Subscription {
            subscriber: subscriber.clone(),
            merchant,
            amount,
            interval_seconds: effective_interval_seconds,
            last_payment_timestamp: now,
            status: SubscriptionStatus::Active,
            prepaid_balance: amount,
            usage_enabled,
        };
        let id = Self::_next_id(&env);
        env.storage().instance().set(&id, &sub);
        Ok(id)
    }

    /// Subscriber deposits more USDC into their vault for this subscription.
    /// 
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
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        
        let min_topup: i128 = env.storage().instance().get(&Symbol::new(&env, "min_topup")).ok_or(Error::NotFound)?;
        if amount < min_topup {
            return Err(Error::BelowMinimumTopup);
        }
        
        // TODO: transfer USDC from subscriber, increase prepaid_balance for subscription_id
        let _ = (env, subscription_id, amount);
        Ok(())
    }

    /// Charge one billing interval and accrue earnings to the merchant's internal balance.
    ///
    /// On success this atomically:
    /// 1. debits `subscription.prepaid_balance` by `subscription.amount`
    /// 2. credits the merchant's aggregate balance ledger by the same amount
    /// 3. updates `last_payment_timestamp`
    ///
    /// Tokens are not transferred to the merchant here. They remain in the vault until
    /// `withdraw_merchant_funds` is called.
    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        let mut subscription: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;

        if subscription.status != SubscriptionStatus::Active {
            return Err(Error::InvalidStatus);
        }

        if subscription.prepaid_balance < subscription.amount {
            return Err(Error::InsufficientBalance);
        }

        let updated_prepaid = subscription
            .prepaid_balance
            .checked_sub(subscription.amount)
            .ok_or(Error::ArithmeticOverflow)?;
        let current_merchant_balance = Self::read_merchant_balance(&env, &subscription.merchant);
        let updated_merchant_balance = current_merchant_balance
            .checked_add(subscription.amount)
            .ok_or(Error::ArithmeticOverflow)?;

        subscription.prepaid_balance = updated_prepaid;
        subscription.last_payment_timestamp = env.ledger().timestamp();
        env.storage().instance().set(&subscription_id, &subscription);
        Self::write_merchant_balance(&env, &subscription.merchant, updated_merchant_balance);
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

    /// Merchant withdraws accumulated USDC from their internal earned balance.
    ///
    /// This debits internal merchant earnings first and then transfers the same amount of
    /// tokens from vault custody to the merchant wallet. This prevents double spending across
    /// repeated withdraw calls.
    pub fn withdraw_merchant_funds(
        env: Env,
        merchant: Address,
        amount: i128,
    ) -> Result<(), Error> {
        merchant.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let current_balance = Self::read_merchant_balance(&env, &merchant);
        if current_balance < amount {
            return Err(Error::InsufficientBalance);
        }

        let updated_balance = current_balance
            .checked_sub(amount)
            .ok_or(Error::ArithmeticOverflow)?;
        Self::write_merchant_balance(&env, &merchant, updated_balance);

        let token_address: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "token"))
            .ok_or(Error::NotFound)?;
        let token_client = token::Client::new(&env, &token_address);
        let contract_address = env.current_contract_address();
        token_client.transfer(&contract_address, &merchant, &amount);
        Ok(())
    }

    /// Returns the internal earned balance currently available for a merchant to withdraw.
    pub fn get_merchant_balance(env: Env, merchant: Address) -> Result<i128, Error> {
        Ok(Self::read_merchant_balance(&env, &merchant))
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

    fn read_merchant_balance(env: &Env, merchant: &Address) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::MerchantBalance(merchant.clone()))
            .unwrap_or(0i128)
    }

    fn write_merchant_balance(env: &Env, merchant: &Address, balance: i128) {
        env.storage()
            .instance()
            .set(&DataKey::MerchantBalance(merchant.clone()), &balance);
    }

    fn read_merchant_config(env: &Env, merchant: &Address) -> MerchantConfig {
        env.storage()
            .instance()
            .get(&DataKey::MerchantConfig(merchant.clone()))
            .unwrap_or(MerchantConfig {
                version: 1,
                min_subscription_amount: 0,
                default_interval_seconds: 0,
            })
    }

    fn write_merchant_config(env: &Env, merchant: &Address, config: &MerchantConfig) {
        env.storage()
            .instance()
            .set(&DataKey::MerchantConfig(merchant.clone()), config);
    }

    fn require_admin_or_merchant(env: &Env, actor: &Address, merchant: &Address) -> Result<(), Error> {
        actor.require_auth();
        let admin: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(env, "admin"))
            .ok_or(Error::NotFound)?;
        if actor != merchant && actor != &admin {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }

    /// Returns subscriptions for a merchant, paginated by offset.
    ///
    /// * `merchant` – the merchant address to query.
    /// * `start`    – 0-based offset into the merchant's subscription list.
    /// * `limit`    – maximum number of subscriptions to return.
    ///
    /// Results are ordered chronologically (insertion order).
    /// Returns an empty `Vec` when the merchant has no subscriptions or
    /// `start` is beyond the end of the list.
    pub fn get_subscriptions_by_merchant(
        env: Env,
        merchant: Address,
        start: u32,
        limit: u32,
    ) -> Vec<Subscription> {
        queries::get_subscriptions_by_merchant(&env, merchant, start, limit)
    }

    /// Returns the number of subscriptions for a given merchant.
    ///
    /// Useful for dashboards and pagination metadata.
    pub fn get_merchant_subscription_count(env: Env, merchant: Address) -> u32 {
        queries::get_merchant_subscription_count(&env, merchant)
    }
}

#[cfg(test)]
mod test;
