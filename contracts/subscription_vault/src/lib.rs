#![no_std]

// ── Modules ──────────────────────────────────────────────────────────────────
mod admin;
mod charge_core;
mod merchant;
mod queries;
mod state_machine;
mod subscription;
pub mod types;

#[contracterror]
#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    NotFound = 404,
    Unauthorized = 401,
    InvalidStatusTransition = 400,
    BelowMinimumTopup = 402,
    RecoveryNotAllowed = 403,
    InvalidRecoveryAmount = 405,
    /// Subscription is not Active (e.g. Paused, Cancelled).
    NotActive = 1002,
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
}

/// Emitted when a subscription is paused.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionPausedEvent {
    pub subscription_id: u32,
    pub authorizer: Address,
}

/// Emitted when a subscription is resumed.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionResumedEvent {
    pub subscription_id: u32,
    pub authorizer: Address,
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
    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
    ) -> Result<u32, Error> {
        subscription::do_create_subscription(
            &env,
            subscriber,
            merchant,
            amount,
            interval_seconds,
            usage_enabled,
        )
    }

    /// Subscriber deposits more USDC into their prepaid vault.
    ///
    /// Rejects deposits below the configured minimum threshold.
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
                return Err(Error::NotActive);
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

    /// Cancel the subscription. Allowed from Active, Paused, or InsufficientBalance.
    /// Transitions to the terminal `Cancelled` state.
    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_cancel_subscription(&env, subscription_id, authorizer)
    }

    /// Pause subscription (no charges until resumed).
    ///
    /// # State Transitions
    /// Allowed from: `Active`
    /// - Transitions to: `Paused`
    ///
    /// Only the subscriber or merchant can pause.
    /// Cannot pause a subscription that is already `Paused`, `Cancelled`, or in `InsufficientBalance`.
    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();

        let mut sub = Self::get_subscription(env.clone(), subscription_id)?;

        // Only subscriber or merchant can pause
        if authorizer != sub.subscriber && authorizer != sub.merchant {
            return Err(Error::Unauthorized);
        }

        // Validate and apply status transition
        validate_status_transition(&sub.status, &SubscriptionStatus::Paused)?;
        sub.status = SubscriptionStatus::Paused;

        env.storage().instance().set(&subscription_id, &sub);

        // Emit event
        env.events().publish(
            (Symbol::new(&env, "subscription_paused"), subscription_id),
            SubscriptionPausedEvent {
                subscription_id,
                authorizer,
            },
        );

        Ok(())
    }

    /// Resume a subscription to Active status.
    ///
    /// # State Transitions
    /// Allowed from: `Paused`, `InsufficientBalance`
    /// - Transitions to: `Active`
    ///
    /// Only the subscriber or merchant can resume.
    /// Cannot resume a `Cancelled` subscription.
    pub fn resume_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();

        let mut sub = Self::get_subscription(env.clone(), subscription_id)?;

        // Only subscriber or merchant can resume
        if authorizer != sub.subscriber && authorizer != sub.merchant {
            return Err(Error::Unauthorized);
        }

        // Validate and apply status transition
        validate_status_transition(&sub.status, &SubscriptionStatus::Active)?;
        sub.status = SubscriptionStatus::Active;

        env.storage().instance().set(&subscription_id, &sub);

        // Emit event
        env.events().publish(
            (Symbol::new(&env, "subscription_resumed"), subscription_id),
            SubscriptionResumedEvent {
                subscription_id,
                authorizer,
            },
        );

        Ok(())
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

    /// Return the total number of subscriptions for a merchant.
    pub fn get_merchant_subscription_count(env: Env, merchant: Address) -> u32 {
        queries::get_merchant_subscription_count(&env, merchant)
    }
}

#[cfg(test)]
mod test;
