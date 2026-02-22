#![no_std]

mod admin;
mod charge_core;
mod merchant;
mod queries;
mod state_machine;
mod subscription;
mod types;


#[contracterror]
#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    NotFound = 404,
    Unauthorized = 401,
    InvalidStatusTransition = 400,
    BelowMinimumTopup = 402,
    /// Charge attempt was made after the subscription's expiration timestamp.
    SubscriptionExpired = 410,
    /// The contract has allocated [`MAX_SUBSCRIPTION_ID`] subscriptions and
    /// cannot issue any more IDs. This prevents `u32` counter overflow.
    SubscriptionLimitReached = 429,
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

use soroban_sdk::{contract, contractimpl, Address, Env, Vec};

pub use state_machine::{can_transition, get_allowed_transitions, validate_status_transition};
pub use types::{BatchChargeResult, Error, Subscription, SubscriptionStatus};



#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    pub fn init(env: Env, token: Address, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin::do_init(&env, token, admin, min_topup)
    }

    /// Update the minimum top-up threshold. Only callable by admin.
    ///
    /// # Arguments
    /// * `min_topup` - Minimum amount (in token base units) required for deposit_funds.
    ///                 Prevents inefficient micro-deposits. Typical range: 1-10 USDC (1_000000 - 10_000000 for 6 decimals).


    pub fn set_min_topup(env: Env, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin::do_set_min_topup(&env, admin, min_topup)
    }

    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        admin::get_min_topup(&env)
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

        subscription::do_create_subscription(
            &env,
            subscriber,

            merchant,
            amount,
            interval_seconds,
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

        )
    }


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

    /// # State Transitions
    /// - On success: `Active` -> `Active` (no change)
    /// - On insufficient balance: `Active` -> `InsufficientBalance`
    ///
    /// Subscriptions that are `Paused` or `Cancelled` cannot be charged.

        subscription::do_deposit_funds(&env, subscription_id, subscriber, amount)
    }


    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        subscription::do_charge_subscription(&env, subscription_id)
    }

    pub fn estimate_topup_for_intervals(
        env: Env,
        subscription_id: u32,
        num_intervals: u32,
    ) -> Result<i128, Error> {
        queries::estimate_topup_for_intervals(&env, subscription_id, num_intervals)
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

    pub fn batch_charge(
        env: Env,
        subscription_ids: Vec<u32>,
    ) -> Result<Vec<BatchChargeResult>, Error> {
        admin::do_batch_charge(&env, &subscription_ids)

    }

    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_cancel_subscription(&env, subscription_id, authorizer)
    }

    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_pause_subscription(&env, subscription_id, authorizer)
    }

    pub fn resume_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_resume_subscription(&env, subscription_id, authorizer)
    }

    pub fn withdraw_merchant_funds(
        env: Env,
        merchant: Address,
        amount: i128,
    ) -> Result<(), Error> {
        merchant::withdraw_merchant_funds(&env, merchant, amount)
    }

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

        queries::get_subscription(&env, subscription_id)

    }
}

#[cfg(test)]
mod test;
