//! Contract types: errors and subscription data structures.
//!
//! Kept in a separate module to reduce merge conflicts when editing state machine
//! or contract entrypoints.

use soroban_sdk::{contracterror, contracttype, Address};

/// Increment this constant whenever the on-chain storage schema changes.
///
/// ⚠️ Upgrade-sensitive: written to [`DataKey::SchemaVersion`] during `init()`.
/// Migration logic must read this value and branch on it before touching storage.
pub const STORAGE_VERSION: u32 = 1;

/// Canonical storage key enum for all contract state.
///
/// ⚠️ Upgrade-sensitive: discriminant order is fixed. Never remove or reorder
/// variants — only append new ones. The integer comments are authoritative.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Merchant → subscription ID list index. Discriminant 0. ⚠️ Must stay at 0.
    MerchantSubs(Address),
    /// USDC token contract address. Discriminant 1.
    Token,
    /// Authorized admin address. Discriminant 2.
    Admin,
    /// Minimum deposit threshold. Discriminant 3.
    MinTopup,
    /// Auto-incrementing subscription ID counter. Discriminant 4.
    NextId,
    /// On-chain storage schema version. Discriminant 5.
    SchemaVersion,
    /// Subscription record keyed by its ID. Discriminant 6.
    Sub(u32),
    /// Last charged billing-period index for replay protection. Discriminant 7.
    ChargedPeriod(u32),
    /// Idempotency key stored per subscription. Discriminant 8.
    IdemKey(u32),
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
    /// Arithmetic overflow in computation (e.g. amount * intervals).
    Overflow = 403,
    /// Charge failed due to insufficient prepaid balance.
    InsufficientBalance = 1003,
    /// Usage-based charge attempted on a subscription with `usage_enabled = false`.
    UsageNotEnabled = 1004,
    /// Usage-based charge amount exceeds the available prepaid balance.
    InsufficientPrepaidBalance = 1005,
    /// The provided amount is zero or negative.
    InvalidAmount = 1006,
    /// Charge already processed for this billing period.
    Replay = 1007,
    /// Recovery amount is zero or negative.
    InvalidRecoveryAmount = 1008,
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
            Error::InsufficientBalance => 1003,
            Error::UsageNotEnabled => 1004,
            Error::InsufficientPrepaidBalance => 1005,
            Error::InvalidAmount => 1006,
            Error::Replay => 1007,
            Error::InvalidRecoveryAmount => 1008,
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
/// ⚠️ Upgrade-sensitive: field order and types are serialised as XDR by Soroban.
/// Adding fields requires a migration; removing or retyping fields is always
/// a breaking change.  New optional fields must default gracefully on old data.
///
/// The `status` field is managed by the state machine. Use the provided
/// transition helpers to modify status, never set it directly.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Subscription {
    /// Subscriber's wallet address. ⚠️ Upgrade-sensitive: position 0.
    pub subscriber: Address,
    /// Merchant receiving payments. ⚠️ Upgrade-sensitive: position 1.
    pub merchant: Address,
    /// Payment amount per billing interval (in token's smallest unit). ⚠️ Upgrade-sensitive: position 2.
    pub amount: i128,
    /// Billing interval duration in seconds. ⚠️ Upgrade-sensitive: position 3.
    pub interval_seconds: u64,
    /// Ledger timestamp of the last successful charge. ⚠️ Upgrade-sensitive: position 4.
    pub last_payment_timestamp: u64,
    /// Current lifecycle state — modified only through state-machine transitions. ⚠️ Upgrade-sensitive: position 5.
    pub status: SubscriptionStatus,
    /// Deposited funds available for future charges. ⚠️ Upgrade-sensitive: position 6.
    pub prepaid_balance: i128,
    /// Whether usage-based billing is enabled for this subscription. ⚠️ Upgrade-sensitive: position 7.
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
