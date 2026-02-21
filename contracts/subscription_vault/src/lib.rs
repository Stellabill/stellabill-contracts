#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol};

#[contracterror]
#[repr(u32)]
pub enum Error {
    NotFound = 404,
    Unauthorized = 401,
    BelowMinimumTopup = 402,
    /// Charge attempt was made after the subscription's expiration timestamp.
    SubscriptionExpired = 410,
    /// The contract has allocated [`MAX_SUBSCRIPTION_ID`] subscriptions and
    /// cannot issue any more IDs. This prevents `u32` counter overflow.
    SubscriptionLimitReached = 429,
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
    /// `None` means the subscription has no fixed end date and runs indefinitely.
    pub expiration: Option<u64>,
}

/// Maximum subscription ID this contract will ever allocate.
///
/// The internal counter is a `u32`. When the counter reaches this value
/// [`SubscriptionVault::create_subscription`] returns
/// [`Error::SubscriptionLimitReached`] instead of wrapping or panicking.
/// This equals `u32::MAX` (4 294 967 295), providing a practical lifetime
/// limit that no real deployment will ever approach.
pub const MAX_SUBSCRIPTION_ID: u32 = u32::MAX;

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
    ///
    /// # Arguments
    /// * `expiration` - Optional Unix timestamp (seconds). If `Some(ts)`, charges are blocked
    ///                  at or after `ts`. Pass `None` for an open-ended subscription.
    ///
    /// # Errors
    /// Returns [`Error::SubscriptionLimitReached`] if the contract has already allocated
    /// [`MAX_SUBSCRIPTION_ID`] subscriptions and can issue no more unique IDs.
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
        // Allocate a unique ID before touching any other state to fail fast.
        let id = Self::_next_id(&env)?;
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
            expiration,
        };
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

        let min_topup: i128 = env.storage().instance().get(&Symbol::new(&env, "min_topup")).ok_or(Error::NotFound)?;
        if amount < min_topup {
            return Err(Error::BelowMinimumTopup);
        }

        // TODO: transfer USDC from subscriber, increase prepaid_balance for subscription_id
        let _ = (env, subscription_id, amount);
        Ok(())
    }

    /// Billing engine (backend) calls this to charge one interval. Deducts from vault, pays merchant.
    ///
    /// # Expiration enforcement
    /// If the subscription has an `expiration` timestamp and the current ledger timestamp is
    /// greater than or equal to that value, this function returns `Error::SubscriptionExpired`
    /// and no funds are moved. When `expiration` is `None` there is no time limit.
    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        // Load the subscription from storage.
        let sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;

        // Expiration guard: reject charges at or after the expiration timestamp.
        if let Some(exp_ts) = sub.expiration {
            if env.ledger().timestamp() >= exp_ts {
                return Err(Error::SubscriptionExpired);
            }
        }

        // TODO: require_caller admin or authorized billing service
        // TODO: check interval and balance, transfer to merchant, update last_payment_timestamp and prepaid_balance
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

    /// Return the total number of subscriptions ever created (i.e. the next ID that
    /// would be allocated). This is a free storage read useful for off-chain indexers
    /// and monitoring.
    ///
    /// Returns `0` before any subscription has been created.
    pub fn get_subscription_count(env: Env) -> u32 {
        let key = Symbol::new(&env, "next_id");
        env.storage().instance().get(&key).unwrap_or(0u32)
    }

    /// Allocate the next unique subscription ID.
    ///
    /// # Guarantees
    /// - IDs start at `0` and increment by exactly `1` on each successful call.
    /// - IDs are **never reused**: the counter only moves forward.
    /// - IDs are **bounded**: when the counter reaches [`MAX_SUBSCRIPTION_ID`]
    ///   this function returns [`Error::SubscriptionLimitReached`] instead of
    ///   wrapping or panicking.
    ///
    /// # Errors
    /// [`Error::SubscriptionLimitReached`] — counter is at [`MAX_SUBSCRIPTION_ID`].
    fn _next_id(env: &Env) -> Result<u32, Error> {
        let key = Symbol::new(env, "next_id");
        let current: u32 = env.storage().instance().get(&key).unwrap_or(0u32);

        // Guard: refuse to allocate when we are already at the ceiling.
        // This makes the subsequent +1 infallible (current < u32::MAX).
        if current == MAX_SUBSCRIPTION_ID {
            return Err(Error::SubscriptionLimitReached);
        }

        // Safe: current < MAX_SUBSCRIPTION_ID == u32::MAX, so current + 1 cannot overflow.
        env.storage().instance().set(&key, &(current + 1));
        Ok(current)
    }
}

#[cfg(test)]
mod test;
