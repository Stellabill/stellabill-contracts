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

/// Detailed error information for insufficient balance scenarios.
///
/// This struct provides machine-parseable information about why a charge failed
/// due to insufficient balance, enabling better error handling in clients.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InsufficientBalanceError {
    /// The current available prepaid balance in the subscription vault.
    pub available: i128,
    /// The required amount to complete the charge.
    pub required: i128,
}

impl InsufficientBalanceError {
    /// Creates a new InsufficientBalanceError with the given available and required amounts.
    pub const fn new(available: i128, required: i128) -> Self {
        Self {
            available,
            required,
        }
    }

    /// Returns the shortfall amount (required - available).
    pub fn shortfall(&self) -> i128 {
        self.required - self.available
    }
}

#[contracterror]
#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    // --- Auth Errors (401-403) ---
    /// Caller does not have the required authorization or is not the admin.
    /// Typically occurs when a required signature is missing.
    Unauthorized = 401,
    /// Caller is authorized but does not have permission for this specific action.
    /// Occurs when a non-admin attempts to perform an admin-only operation.
    Forbidden = 403,

    // --- Not Found (404) ---
    /// The requested resource (e.g. subscription) was not found in storage.
    NotFound = 404,

    // --- Invalid Input (400, 405-409) ---
    /// The requested state transition is not allowed by the state machine.
    /// The requested state transition is not allowed by the state machine.
    /// E.g., attempting to resume a 'Cancelled' subscription.
    InvalidStatusTransition = 400,
    /// The top-up amount is below the minimum required threshold configured by the admin.
    BelowMinimumTopup = 402,
    /// The provided amount is zero or negative.
    InvalidAmount = 405,
    /// Recovery amount is zero or negative (used in admin fund recovery).
    InvalidRecoveryAmount = 406,
    /// Usage-based charge attempted on a subscription where usage billing is disabled.
    UsageNotEnabled = 407,
    /// Invalid parameters provided to the function (e.g., a pagination limit of 0).
    InvalidInput = 408,
    /// Export limit exceeds allowed maximum.
    InvalidExportLimit = 409,

    // --- Insufficient Funds (10xx) ---
    /// Subscription failed due to insufficient prepaid balance in the vault for a recurring charge.
    /// This causes the subscription to transition to the 'InsufficientBalance' state.
    InsufficientBalance = 1001,
    /// Usage-based charge exceeds the current available prepaid balance.
    InsufficientPrepaidBalance = 1002,

    // --- Timing & Lifecycle Errors (11xx) ---
    /// Charge attempted before the 'interval_seconds' has elapsed since the last payment.
    IntervalNotElapsed = 1101,
    /// Charge already processed for the current billing period (replay protection).
    Replay = 1102,
    /// Subscription is not in the 'Active' state (e.g. it is Paused or Cancelled).
    NotActive = 1103,

    // --- Algebra & Overflow (12xx) ---
    /// Arithmetic overflow in computation (e.g. total amount calculation).
    Overflow = 1201,
    /// Arithmetic underflow (e.g. subtracting an amount greater than the balance).
    Underflow = 1202,

    // --- Configuration & System (13xx) ---
    /// Contract is already initialized. The 'init' function can only be called once.
    AlreadyInitialized = 1301,
    /// Contract has not been initialized. Most operations require 'init' to be called first.
    NotInitialized = 1302,
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
/// See `docs/subscription_lifecycle.md` for how each status is entered and exited and for invariants.
///
/// # State Machine
///
/// The subscription status follows a defined state machine with specific allowed transitions:
///
/// - **Active**: Subscription is active and charges can be processed.
///   - Can transition to: `Paused`, `Cancelled`, `InsufficientBalance`, `GracePeriod`
///
/// - **Paused**: Subscription is temporarily suspended, no charges are processed.
///   - Can transition to: `Active`, `Cancelled`
///
/// - **Cancelled**: Subscription is permanently terminated, no further changes allowed.
///   - No outgoing transitions (terminal state)
///
/// - **InsufficientBalance**: Subscription failed due to insufficient funds.
///   - This status is automatically set when a charge attempt fails due to insufficient
///     prepaid balance.
///   - Can transition to: `Active` (after deposit + resume), `Cancelled`
///   - The subscription cannot be charged while in this status.
///
/// # When InsufficientBalance Occurs
///
/// A subscription transitions to `InsufficientBalance` when:
/// 1. A [`crate::SubscriptionVault::charge_subscription`] call finds `prepaid_balance < amount`
/// 2. A [`crate::SubscriptionVault::charge_usage`] call drains the balance to zero
///
/// # Recovery from InsufficientBalance
///
/// To recover from `InsufficientBalance`:
/// 1. Subscriber calls [`crate::SubscriptionVault::deposit_funds`] to add funds
/// 2. Subscriber calls [`crate::SubscriptionVault::resume_subscription`] to transition back to `Active`
/// 3. Subsequent charges will succeed if sufficient balance exists
///
/// - **GracePeriod**: Subscription is in grace period after a missed charge.
///   - Can transition to: `Active` (after deposit), `InsufficientBalance`, `Cancelled`
///
/// Invalid transitions (e.g., `Cancelled` -> `Active`) are rejected with
/// [`Error::InvalidStatusTransition`].
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubscriptionStatus {
    /// Subscription is active and ready for charging.
    ///
    /// Only in this state can [`crate::SubscriptionVault::charge_subscription`] and
    /// [`crate::SubscriptionVault::charge_usage`] successfully process charges.
    Active = 0,
    /// Subscription is temporarily paused, no charges processed.
    ///
    /// Pausing preserves the subscription agreement but prevents charges.
    /// Use [`crate::SubscriptionVault::resume_subscription`] to return to Active.
    Paused = 1,
    /// Subscription is permanently cancelled (terminal state).
    ///
    /// Once cancelled, the subscription cannot be resumed or modified.
    /// Remaining funds can be withdrawn by the subscriber.
    Cancelled = 2,
    /// Subscription failed due to insufficient balance for charging.
    ///
    /// This status indicates that the last charge attempt failed because the
    /// prepaid balance was insufficient. The subscription cannot be charged
    /// until the subscriber adds more funds.
    ///
    /// # Client Handling
    ///
    /// UI should:
    /// - Display a "payment required" message to the subscriber
    /// - Provide a way to initiate a deposit
    /// - Optionally auto-retry after deposit (if using resume)
    InsufficientBalance = 3,
    /// Subscription failed resulting in entry into grace period before suspension.
    GracePeriod = 4,
}

/// Stores subscription details and current state.
///
/// The `status` field is managed by the state machine. Use the provided
/// transition helpers to modify status, never set it directly.
/// See `docs/subscription_lifecycle.md` for lifecycle and on-chain representation.
///
/// Serialization: This named-field struct is encoded on-ledger as a ScMap keyed
/// by the field names. Renaming fields, reordering is inconsequential to map
/// semantics but still alters the encoded bytes and will break golden vectors.
/// Changing any field type or the representation of [`SubscriptionStatus`] is
/// a storage-breaking change. To extend, prefer adding new optional fields at
/// the end with conservative defaults; doing so still changes bytes and must
/// be treated as a versioned change.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Subscription {
    /// Identity of the subscriber. Renaming or changing this field breaks the
    /// encoded form and must be treated as a breaking change.
    pub subscriber: Address,
    /// Identity of the merchant. Renaming or changing this field breaks the
    /// encoded form and must be treated as a breaking change.
    pub merchant: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub last_payment_timestamp: u64,
    /// Current lifecycle state. Modified only through state machine transitions.
    /// Changing the enum or this field name affects the encoded form.
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

/// Exported snapshot of contract-level configuration for migration tooling.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ContractSnapshot {
    pub admin: Address,
    pub token: Address,
    pub min_topup: i128,
    pub next_id: u32,
    pub storage_version: u32,
    pub timestamp: u64,
}

/// Exported summary of a subscription for migration tooling.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionSummary {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub last_payment_timestamp: u64,
    pub status: SubscriptionStatus,
    pub prepaid_balance: i128,
    pub usage_enabled: bool,
}

/// Event emitted when a migration export is requested.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MigrationExportEvent {
    pub admin: Address,
    pub start_id: u32,
    pub limit: u32,
    pub exported: u32,
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
