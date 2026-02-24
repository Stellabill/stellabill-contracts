//! Contract types: errors and subscription data structures.
//!
//! Kept in a separate module to reduce merge conflicts when editing state machine
//! or contract entrypoints.

use soroban_sdk::{contracterror, contracttype, Address};

/// Storage keys for secondary indices.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Maps a merchant address to its list of subscription IDs.
    MerchantSubs(Address),
}

#[contracterror]
#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    NotFound = 404,
    Unauthorized = 401,
    /// Charge attempted before `last_payment_timestamp + interval_seconds`.
    IntervalNotElapsed = 1001,
    /// Subscription is not Active (e.g. Paused, Cancelled).
    NotActive = 1002,
    InvalidStatusTransition = 400,
    BelowMinimumTopup = 402,
    /// Arithmetic overflow in computation (e.g. amount * intervals).
    Overflow = 403,
    /// Arithmetic underflow (e.g. negative amount or balance would go negative).
    Underflow = 1004,
    /// Charge failed due to insufficient prepaid balance.
    InsufficientBalance = 1003,
    /// Usage-based charge attempted on a subscription with `usage_enabled = false`.
    UsageNotEnabled = 1005,
    /// Usage-based charge amount exceeds the available prepaid balance.
    InsufficientPrepaidBalance = 1010,
    /// The provided amount is zero or negative.
    InvalidAmount = 1006,
    /// Charge already processed for this billing period.
    Replay = 1007,
    /// Recovery amount is zero or negative.
    InvalidRecoveryAmount = 1008,
    /// Recovery operation not allowed for this reason or context.
    RecoveryNotAllowed = 1009,
}

impl Error {
    /// Returns the numeric code for this error (for batch result reporting).
    pub const fn to_code(self) -> u32 {
        match self {
            Error::NotFound => 404,
            Error::Unauthorized => 401,
            Error::IntervalNotElapsed => 1001,
            Error::NotActive => 1002,
            Error::InvalidStatusTransition => 400,
            Error::BelowMinimumTopup => 402,
            Error::Overflow => 403,
            Error::Underflow => 1004,
            Error::InsufficientBalance => 1003,
            Error::UsageNotEnabled => 1005,
            Error::InsufficientPrepaidBalance => 1010,
            Error::InvalidAmount => 1006,
            Error::Replay => 1007,
            Error::InvalidRecoveryAmount => 1008,
            Error::RecoveryNotAllowed => 1009,
        }
    }
}

/// Result of charging one subscription in a batch. Used by [`crate::SubscriptionVault::batch_charge`].
#[contracttype]
#[derive(Clone, Debug)]
pub struct BatchChargeResult {
    /// True if the charge succeeded.
    pub success: bool,
    /// If success is false, the error code (e.g. from [`Error::to_code`]); otherwise 0.
    pub error_code: u32,
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

/// Defines a reusable subscription plan template.
///
/// Plan templates allow merchants to define standard subscription offerings
/// (e.g., "Basic Plan", "Premium Plan") with predefined parameters. Subscribers
/// can then create subscriptions from these templates without manually specifying
/// all parameters, ensuring consistency and reducing errors.
///
/// # Usage
///
/// - Use templates for standardized subscription offerings
/// - Use direct subscription creation for custom one-off subscriptions
#[contracttype]
#[derive(Clone, Debug)]
pub struct PlanTemplate {
    /// Merchant who owns this plan template.
    pub merchant: Address,
    /// Recurring charge amount per interval.
    pub amount: i128,
    /// Billing interval in seconds.
    pub interval_seconds: u64,
    /// Whether usage-based charging is enabled.
    pub usage_enabled: bool,
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

/// Computes the estimated next charge timestamp for a subscription.
///
/// This is a readonly helper that does not mutate contract state. It provides
/// information for off-chain scheduling systems and UX displays.
pub fn compute_next_charge_info(subscription: &Subscription) -> NextChargeInfo {
    let next_charge_timestamp = subscription
        .last_payment_timestamp
        .saturating_add(subscription.interval_seconds);

    let is_charge_expected = match subscription.status {
        SubscriptionStatus::Active => true,
        SubscriptionStatus::InsufficientBalance => true, // Will be retried after funding
        SubscriptionStatus::Paused => false,
        SubscriptionStatus::Cancelled => false,
    };

    NextChargeInfo {
        next_charge_timestamp,
        is_charge_expected,
    }
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

// Event types
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionCreatedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub amount: i128,
    pub interval_seconds: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct FundsDepositedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub amount: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionChargedEvent {
    pub subscription_id: u32,
    pub merchant: Address,
    pub amount: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionCancelledEvent {
    pub subscription_id: u32,
    pub authorizer: Address,
    pub refund_amount: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionPausedEvent {
    pub subscription_id: u32,
    pub authorizer: Address,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionResumedEvent {
    pub subscription_id: u32,
    pub authorizer: Address,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantWithdrawalEvent {
    pub merchant: Address,
    pub amount: i128,
}

/// Emitted when a merchant-initiated one-off charge is applied to a subscription.
#[contracttype]
#[derive(Clone, Debug)]
pub struct OneOffChargedEvent {
    pub subscription_id: u32,
    pub merchant: Address,
    pub amount: i128,
}
