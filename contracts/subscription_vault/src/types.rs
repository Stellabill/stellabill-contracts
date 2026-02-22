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
    // --- Auth & Access Control (401-403) ---
    /// The caller is not authorized to perform this action.
    Unauthorized = 401,
    /// Action requires administrative privileges.
    NotAdmin = 403,

    // --- Not Found (404-406) ---
    /// Generic resource not found.
    NotFound = 404,
    /// The specific subscription ID does not exist in storage.
    SubscriptionNotFound = 405,
    /// Contract configuration (admin, token, etc.) has not been initialized.
    ConfigNotFound = 406,

    // --- Invalid Input & Arguments (400, 407-409) ---
    /// A provided argument is malformed or invalid.
    InvalidArguments = 400,
    /// Amount must be greater than zero and valid.
    InvalidAmount = 407,
    /// Billing interval must be within allowed bounds.
    InvalidInterval = 408,
    /// The requested status transition is not allowed by the state machine.
    InvalidStatusTransition = 409,

    // --- Financial & Funds (402, 410-412) ---
    /// The subscription vault has insufficient funds to cover the charge.
    InsufficientBalance = 402,
    /// Deposit amount is below the required minimum threshold.
    BelowMinimumTopup = 410,
    /// Withdrawal failed due to insufficient merchant balance.
    InsufficientMerchantBalance = 411,

    // --- Timing & Lifecycle (1001-1008) ---
    /// Charge attempted before the billing interval has elapsed.
    IntervalNotElapsed = 1001,
    /// Subscription is not in an Active state (e.g. Paused, Cancelled).
    NotActive = 1002,
    /// Subscription has reached its end date or max cycles.
    SubscriptionExpired = 1003,
    /// Replay: charge for this billing period or idempotency key already processed.
    Replay = 1004,
    /// Usage-based charge attempted on a subscription with `usage_enabled = false`.
    UsageNotEnabled = 1005,
    /// Usage-based charge amount exceeds the available prepaid balance.
    InsufficientPrepaidBalance = 1006,
    /// Recovery amount is zero or negative.
    InvalidRecoveryAmount = 1007,

    // --- Configuration (1101-1103) ---
    /// The contract has not been properly initialized or configured.
    NotConfigured = 1101,
    /// Provided configuration values (e.g. min_topup) are invalid.
    InvalidConfig = 1102,
    /// Arithmetic overflow in computation (e.g. amount * intervals).
    Overflow = 1103,
}

impl Error {
    /// Returns the numeric code for this error (for batch result reporting).
    pub const fn to_code(self) -> u32 {
        self as u32
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
#[derive(Clone, Debug, PartialEq)]
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

/// Represents the reason for stranded funds that can be recovered by admin.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryReason {
    /// Funds sent to contract address by mistake (no associated subscription).
    AccidentalTransfer = 0,
    /// Funds from deprecated contract flows or logic errors.
    DeprecatedFlow = 1,
    /// Funds from cancelled subscriptions with unreachable addresses.
    UnreachableSubscriber = 2,
}

/// Event emitted when admin recovers stranded funds.
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

/// Result of computing next charge information for a subscription.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NextChargeInfo {
    /// Estimated timestamp for the next charge attempt.
    pub next_charge_timestamp: u64,
    /// Whether a charge is actually expected based on the subscription status.
    pub is_charge_expected: bool,
}
