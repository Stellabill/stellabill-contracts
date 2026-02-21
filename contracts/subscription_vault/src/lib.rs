#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol};

#[contracterror]
#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
#[derive(Debug, PartialEq)]
pub enum Error {
    NotFound = 404,
    Unauthorized = 401,
    InsufficientBalance = 402,
    InvalidStatusTransition = 400,
    BelowMinimumTopup = 402,
}

/// Represents the lifecycle state of a subscription.
///
/// # State Machine
///
/// The subscription status follows a defined state machine with specific allowed transitions:
///
/// - **Active**: Subscription is active and charges can be processed.
///   - Can transition to: `Paused`, `Cancelled`, `InsufficientBalance`
///
/// - **Paused**: Subscription is temporarily suspended, no charges are processed.
///   - Can transition to: `Active`, `Cancelled`
///
/// - **Cancelled**: Subscription is permanently terminated, no further changes allowed.
///   - No outgoing transitions (terminal state)
///
/// - **InsufficientBalance**: Subscription failed due to insufficient funds.
///   - Can transition to: `Active` (after deposit), `Cancelled`
///
/// Invalid transitions (e.g., `Cancelled` -> `Active`) are rejected with
/// [`Error::InvalidStatusTransition`].
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubscriptionStatus {
    /// Subscription is active and ready for charging.
    Active = 0,
    /// Subscription is temporarily paused, no charges processed.
    Paused = 1,
    /// Subscription is permanently cancelled (terminal state).
    Cancelled = 2,
    /// Subscription failed due to insufficient balance for charging.
    InsufficientBalance = 3,
}

/// Stores subscription details and current state.
///
/// The `status` field is managed by the state machine. Use the provided
/// transition helpers to modify status, never set it directly.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Subscription {
    pub subscriber: Address,
    pub merchant: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub last_payment_timestamp: u64,
    /// Current lifecycle state. Modified only through state machine transitions.
    pub status: SubscriptionStatus,
    pub prepaid_balance: i128,
    pub usage_enabled: bool,
}

/// Validates if a status transition is allowed by the state machine.
///
/// # State Transition Rules
///
/// | From              | To                  | Allowed |
/// |-------------------|---------------------|---------|
/// | Active            | Paused              | Yes     |
/// | Active            | Cancelled           | Yes     |
/// | Active            | InsufficientBalance | Yes     |
/// | Paused            | Active              | Yes     |
/// | Paused            | Cancelled           | Yes     |
/// | InsufficientBalance | Active            | Yes     |
/// | InsufficientBalance | Cancelled         | Yes     |
/// | Cancelled         | *any*               | No      |
/// | *any*             | Same status         | Yes (idempotent) |
///
/// # Arguments
/// * `from` - Current status
/// * `to` - Target status
///
/// # Returns
/// * `Ok(())` if transition is valid
/// * `Err(Error::InvalidStatusTransition)` if transition is invalid
pub fn validate_status_transition(
    from: &SubscriptionStatus,
    to: &SubscriptionStatus,
) -> Result<(), Error> {
    // Same status is always allowed (idempotent)
    if from == to {
        return Ok(());
    }

    let valid = match from {
        SubscriptionStatus::Active => matches!(
            to,
            SubscriptionStatus::Paused
                | SubscriptionStatus::Cancelled
                | SubscriptionStatus::InsufficientBalance
        ),
        SubscriptionStatus::Paused => {
            matches!(to, SubscriptionStatus::Active | SubscriptionStatus::Cancelled)
        }
        SubscriptionStatus::Cancelled => false,
        SubscriptionStatus::InsufficientBalance => {
            matches!(to, SubscriptionStatus::Active | SubscriptionStatus::Cancelled)
        }
    };

    if valid {
        Ok(())
    } else {
        Err(Error::InvalidStatusTransition)
    }
}

/// Returns all valid target statuses for a given current status.
///
/// This is useful for UI/documentation to show available actions.
///
/// # Examples
///
/// ```
/// let targets = get_allowed_transitions(&SubscriptionStatus::Active);
/// assert!(targets.contains(&SubscriptionStatus::Paused));
/// ```
pub fn get_allowed_transitions(status: &SubscriptionStatus) -> &'static [SubscriptionStatus] {
    match status {
        SubscriptionStatus::Active => &[
            SubscriptionStatus::Paused,
            SubscriptionStatus::Cancelled,
            SubscriptionStatus::InsufficientBalance,
        ],
        SubscriptionStatus::Paused => &[
            SubscriptionStatus::Active,
            SubscriptionStatus::Cancelled,
        ],
        SubscriptionStatus::Cancelled => &[],
        SubscriptionStatus::InsufficientBalance => &[
            SubscriptionStatus::Active,
            SubscriptionStatus::Cancelled,
        ],
    }
}

/// Checks if a transition is valid without returning an error.
///
/// Convenience wrapper around [`validate_status_transition`] for boolean checks.
pub fn can_transition(from: &SubscriptionStatus, to: &SubscriptionStatus) -> bool {
    validate_status_transition(from, to).is_ok()
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
        
    ///
    /// # State Transitions
    /// - On success: `Active` -> `Active` (no change)
    /// - On insufficient balance: `Active` -> `InsufficientBalance`
    ///
    /// Subscriptions that are `Paused` or `Cancelled` cannot be charged.
    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        // TODO: require_caller admin or authorized billing service
        // TODO: load subscription, check interval and balance, transfer to merchant

        // Placeholder for actual charge logic
        let maybe_sub: Option<Subscription> = env.storage().instance().get(&subscription_id);
        if let Some(mut sub) = maybe_sub {
            // Check current status allows charging
            if sub.status == SubscriptionStatus::Cancelled || sub.status == SubscriptionStatus::Paused {
                // Cannot charge cancelled or paused subscriptions
                return Err(Error::InvalidStatusTransition);
            }

            // Simulate charge logic - on insufficient balance, transition to InsufficientBalance
            let insufficient_balance = false; // TODO: actual balance check
            if insufficient_balance {
                validate_status_transition(&sub.status, &SubscriptionStatus::InsufficientBalance)?;
                sub.status = SubscriptionStatus::InsufficientBalance;
                env.storage().instance().set(&subscription_id, &sub);
            }
            // TODO: update last_payment_timestamp and prepaid_balance on successful charge
        }
        Ok(())
    }

    /// Subscriber or merchant cancels the subscription. Remaining balance can be withdrawn by subscriber.
    ///
    /// # State Transitions
    /// Allowed from: `Active`, `Paused`, `InsufficientBalance`
    /// - Transitions to: `Cancelled` (terminal state)
    ///
    /// Once cancelled, no further transitions are possible.
    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();

        let mut sub = Self::get_subscription(env.clone(), subscription_id)?;

        // Validate and apply status transition
        validate_status_transition(&sub.status, &SubscriptionStatus::Cancelled)?;
        sub.status = SubscriptionStatus::Cancelled;

        // TODO: allow withdraw of prepaid_balance

        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    /// Pause subscription (no charges until resumed).
    ///
    /// # State Transitions
    /// Allowed from: `Active`
    /// - Transitions to: `Paused`
    ///
    /// Cannot pause a subscription that is already `Paused`, `Cancelled`, or in `InsufficientBalance`.
    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();

        let mut sub = Self::get_subscription(env.clone(), subscription_id)?;

        // Validate and apply status transition
        validate_status_transition(&sub.status, &SubscriptionStatus::Paused)?;
        sub.status = SubscriptionStatus::Paused;

        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    /// Resume a subscription to Active status.
    ///
    /// # State Transitions
    /// Allowed from: `Paused`, `InsufficientBalance`
    /// - Transitions to: `Active`
    ///
    /// Cannot resume a `Cancelled` subscription.
    pub fn resume_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();

        let mut sub = Self::get_subscription(env.clone(), subscription_id)?;

        // Validate and apply status transition
        validate_status_transition(&sub.status, &SubscriptionStatus::Active)?;
        sub.status = SubscriptionStatus::Active;

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
