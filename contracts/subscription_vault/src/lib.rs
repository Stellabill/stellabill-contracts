#![no_std]

// ── Modules ──────────────────────────────────────────────────────────────────
mod admin;
mod charge_core;
mod merchant;
mod queries;
mod state_machine;
mod subscription;
mod types;


use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol};

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

    RecoveryNotAllowed = 403,
    InvalidRecoveryAmount = 405,

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


/// Represents the reason for stranded funds that can be recovered by admin.
///
/// This enum documents the specific, well-defined cases where funds may become
/// stranded in the contract and require administrative intervention. Each case
/// must be carefully audited before recovery is permitted.
///
/// # Security Note
///
/// Recovery is an exceptional operation that should only be used for truly
/// stranded funds. All recovery operations are logged via events and should
/// be subject to governance review.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryReason {
    /// Funds sent to contract address by mistake (no associated subscription).
    /// This occurs when users accidentally send tokens directly to the contract.
    AccidentalTransfer = 0,

    /// Funds from deprecated contract flows or logic errors.
    /// Used when contract upgrades or bugs leave funds in an inaccessible state.
    DeprecatedFlow = 1,

    /// Funds from cancelled subscriptions with unreachable addresses.
    /// Subscribers may lose access to their withdrawal keys after cancellation.
    UnreachableSubscriber = 2,
}

/// Event emitted when admin recovers stranded funds.
///
/// This event provides a complete audit trail for all recovery operations,
/// including who initiated it, why, and how much was recovered.
#[contracttype]
#[derive(Clone, Debug)]
pub struct RecoveryEvent {
    /// The admin who authorized the recovery
    pub admin: Address,
    /// The destination address receiving the recovered funds
    pub recipient: Address,
    /// The amount of funds recovered
    pub amount: i128,
    /// The documented reason for recovery
    pub reason: RecoveryReason,
    /// Timestamp when recovery was executed
    pub timestamp: u64,
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
            matches!(
                to,
                SubscriptionStatus::Active | SubscriptionStatus::Cancelled
            )
        }
        SubscriptionStatus::Cancelled => false,
        SubscriptionStatus::InsufficientBalance => {
            matches!(
                to,
                SubscriptionStatus::Active | SubscriptionStatus::Cancelled
            )
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

        SubscriptionStatus::Paused => &[SubscriptionStatus::Active, SubscriptionStatus::Cancelled],
        SubscriptionStatus::Cancelled => &[],
        SubscriptionStatus::InsufficientBalance => {
            &[SubscriptionStatus::Active, SubscriptionStatus::Cancelled]
        }

    }
}

/// Checks if a transition is valid without returning an error.
///
/// Convenience wrapper around [`validate_status_transition`] for boolean checks.
pub fn can_transition(from: &SubscriptionStatus, to: &SubscriptionStatus) -> bool {
    validate_status_transition(from, to).is_ok()
}

use soroban_sdk::{contract, contractimpl, Address, Env, Vec};

use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env, Vec};


pub use state_machine::{can_transition, get_allowed_transitions, validate_status_transition};
pub use types::{
    BatchChargeResult, Error, FundsDepositedEvent, MerchantWithdrawalEvent, OneOffChargedEvent,
    Subscription, SubscriptionCancelledEvent, SubscriptionChargedEvent, SubscriptionCreatedEvent,
    SubscriptionPausedEvent, SubscriptionResumedEvent, SubscriptionStatus,
};

/// Result of computing next charge information for a subscription.
///
/// Contains the estimated next charge timestamp and a flag indicating
/// whether the charge is expected to occur based on the subscription status.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NextChargeInfo {
    /// Estimated timestamp for the next charge attempt.
    /// For Active and InsufficientBalance states, this is `last_payment_timestamp + interval_seconds`.
    /// For Paused and Cancelled states, this represents when the charge *would* occur if the
    /// subscription were Active, but `is_charge_expected` will be `false`.
    pub next_charge_timestamp: u64,

    /// Whether a charge is actually expected based on the subscription status.
    /// - `true` for Active subscriptions (charge will be attempted)
    /// - `true` for InsufficientBalance (charge will be retried after funding)
    /// - `false` for Paused subscriptions (no charges until resumed)
    /// - `false` for Cancelled subscriptions (terminal state, no future charges)
    pub is_charge_expected: bool,
}
pub mod types;

// ── Re-exports (used by tests and external consumers) ────────────────────────
pub use state_machine::{can_transition, get_allowed_transitions, validate_status_transition};
pub use types::*;

pub use queries::compute_next_charge_info;
use soroban_sdk::{contract, contractimpl, Address, Env, Vec};

// ── Contract ─────────────────────────────────────────────────────────────────




#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    // ── Admin / Config ───────────────────────────────────────────────────

    /// Initialize the contract: set token address, admin, and minimum top-up.
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

    /// Get the current minimum top-up threshold.
    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        admin::get_min_topup(&env)
    }

    /// Get the current admin address.
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        admin::do_get_admin(&env)
    }

    /// Rotate admin to a new address. Only callable by current admin.
    ///
    /// # Security
    ///
    /// - Immediate effect — old admin loses access instantly.
    /// - Irreversible without the new admin's cooperation.
    /// - Emits an `admin_rotation` event for audit trail.
    pub fn rotate_admin(env: Env, current_admin: Address, new_admin: Address) -> Result<(), Error> {
        admin::do_rotate_admin(&env, current_admin, new_admin)
    }

    /// **ADMIN ONLY**: Recover stranded funds from the contract.
    ///
    /// Tightly-scoped mechanism for recovering funds that have become
    /// inaccessible through normal operations. Each recovery emits a
    /// `RecoveryEvent` with full audit details.
    pub fn recover_stranded_funds(
        env: Env,
        admin: Address,
        recipient: Address,
        amount: i128,
        reason: RecoveryReason,
    ) -> Result<(), Error> {
        admin::do_recover_stranded_funds(&env, admin, recipient, amount, reason)
    }

    /// Charge a batch of subscriptions in one transaction. Admin only.
    ///
    /// Returns a per-subscription result vector so callers can identify
    /// which charges succeeded and which failed (with error codes).
    pub fn batch_charge(
        env: Env,
        subscription_ids: Vec<u32>,
    ) -> Result<Vec<BatchChargeResult>, Error> {
        admin::do_batch_charge(&env, &subscription_ids)
    }

    // ── Subscription lifecycle ───────────────────────────────────────────

    /// Create a new subscription. Caller deposits initial USDC; contract stores agreement.
    ///
    /// # Arguments
    /// * `expiration` - Optional Unix timestamp (seconds). If `Some(ts)`, charges are blocked
    ///                  at or after `ts`. Pass `None` for an open-ended subscription.
    ///
    /// # Errors
    /// Returns [`Error::SubscriptionLimitReached`] if the contract has already allocated
    /// [`MAX_SUBSCRIPTION_ID`] subscriptions and can issue no more unique IDs.



    /// Create a new subscription. Caller deposits initial USDC; contract stores agreement.

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


        subscriber.require_auth();
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



        };
        let id = Self::_next_id(&env);
        env.storage().instance().set(&id, &sub);
        Ok(id)
        )
    }

    /// Subscriber deposits more USDC into their prepaid vault.
    ///
    /// # Minimum top-up enforcement
    /// Rejects deposits below the configured minimum threshold to prevent inefficient
    /// micro-transactions that waste gas and complicate accounting. The minimum is set
    /// globally at contract initialization and adjustable by admin via `set_min_topup`.

    /// Rejects deposits below the configured minimum threshold.
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

    /// Charge one subscription for the current billing interval. Optional `idempotency_key` enables
    /// safe retries: repeated calls with the same key return success without double-charging.
    pub fn charge_subscription(
        env: Env,
        subscription_id: u32,
        idempotency_key: Option<soroban_sdk::BytesN<32>>,
    ) -> Result<(), Error> {
        subscription::do_charge_subscription(&env, subscription_id, idempotency_key)

    }

        subscriber.require_auth();

        let min_topup: i128 = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "min_topup"))
            .ok_or(Error::NotFound)?;
        if amount < min_topup {
            return Err(Error::BelowMinimumTopup);
        }


        // TODO: transfer USDC from subscriber, increase prepaid_balance for subscription_id
        let _ = (env, subscription_id, amount);
        Ok(())
    }



    /// Billing engine (backend) calls this to charge one interval. Deducts from vault, pays merchant.
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
            if sub.status == SubscriptionStatus::Cancelled
                || sub.status == SubscriptionStatus::Paused
            {
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

    pub fn batch_charge(
        env: Env,
        subscription_ids: Vec<u32>,
    ) -> Result<Vec<BatchChargeResult>, Error> {
        admin::do_batch_charge(&env, &subscription_ids)


        Ok(())

        subscription::do_deposit_funds(&env, subscription_id, subscriber, amount)
    }

    /// Cancel the subscription. Allowed from Active, Paused, or InsufficientBalance.
    /// Transitions to the terminal `Cancelled` state.
    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_cancel_subscription(&env, subscription_id, authorizer)
    }

    /// Pause subscription (no charges until resumed). Allowed from Active.
    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_pause_subscription(&env, subscription_id, authorizer)
    }

    /// Resume a subscription to Active. Allowed from Paused or InsufficientBalance.
    pub fn resume_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_resume_subscription(&env, subscription_id, authorizer)
    }

    // ── Charging ─────────────────────────────────────────────────────────

    /// Billing engine calls this to charge one interval.
    ///
    /// Enforces strict interval timing and replay protection.
    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        charge_core::charge_one(&env, subscription_id, None)
    }

    /// Charge a metered usage amount against the subscription's prepaid balance.
    ///
    /// Designed for integration with an **off-chain usage metering service**:
    /// the service measures consumption, then calls this entrypoint with the
    /// computed `usage_amount` to debit the subscriber's vault.
    ///
    /// # Requirements
    ///
    /// * The subscription must be `Active`.
    /// * `usage_enabled` must be `true` on the subscription.
    /// * `usage_amount` must be positive (`> 0`).
    /// * `prepaid_balance` must be >= `usage_amount`.
    ///
    /// # Behaviour
    ///
    /// On success, `prepaid_balance` is reduced by `usage_amount`.  If the
    /// debit drains the balance to zero the subscription transitions to
    /// `InsufficientBalance` status, signalling that no further charges
    /// (interval or usage) can proceed until the subscriber tops up.
    ///
    /// # Errors
    ///
    /// | Variant | Reason |
    /// |---------|--------|
    /// | `NotFound` | Subscription ID does not exist. |
    /// | `NotActive` | Subscription is not `Active`. |
    /// | `UsageNotEnabled` | `usage_enabled` is `false`. |
    /// | `InvalidAmount` | `usage_amount` is zero or negative. |
    /// | `InsufficientPrepaidBalance` | Prepaid balance cannot cover the debit. |
    pub fn charge_usage(env: Env, subscription_id: u32, usage_amount: i128) -> Result<(), Error> {
        charge_core::charge_usage_one(&env, subscription_id, usage_amount)
    }

    // ── Merchant ─────────────────────────────────────────────────────────

    /// Merchant withdraws accumulated USDC to their wallet.
    pub fn withdraw_merchant_funds(env: Env, merchant: Address, amount: i128) -> Result<(), Error> {
        merchant::withdraw_merchant_funds(&env, merchant, amount)
    }

    // ── Queries ──────────────────────────────────────────────────────────

    /// Read subscription by id.
    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        queries::get_subscription(&env, subscription_id)
    }

    /// Estimate how much a subscriber needs to deposit to cover N future intervals.
    pub fn estimate_topup_for_intervals(
        env: Env,
        subscription_id: u32,
        num_intervals: u32,
    ) -> Result<i128, Error> {
        queries::estimate_topup_for_intervals(&env, subscription_id, num_intervals)
    }

    /// Get estimated next charge info (timestamp + whether charge is expected).
    pub fn get_next_charge_info(env: Env, subscription_id: u32) -> Result<NextChargeInfo, Error> {
        let sub = queries::get_subscription(&env, subscription_id)?;
        Ok(compute_next_charge_info(&sub))
    }

    /// Return subscriptions for a merchant, paginated.
    pub fn get_subscriptions_by_merchant(
        env: Env,
        merchant: Address,
        start: u32,
        limit: u32,
    ) -> Vec<Subscription> {
        queries::get_subscriptions_by_merchant(&env, merchant, start, limit)
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


    fn _next_id(env: &Env) -> u32 {
        let key = Symbol::new(env, "next_id");
        let id: u32 = env.storage().instance().get(&key).unwrap_or(0);
        env.storage().instance().set(&key, &(id + 1));
        id

    /// Return the total number of subscriptions for a merchant.
    pub fn get_merchant_subscription_count(env: Env, merchant: Address) -> u32 {
        queries::get_merchant_subscription_count(&env, merchant)
    }
}

#[cfg(test)]
mod test;
