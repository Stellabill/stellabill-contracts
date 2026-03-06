//! Contract types: errors, subscription data structures, and event types.
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
    /// Emergency stop flag - when true, critical operations are blocked. Discriminant 9.
    EmergencyStop,
    /// Global referral program configuration. Discriminant 10.
    ReferralCfg,
    /// Per-subscription referral record keyed by subscription ID. Discriminant 11.
    Referral(u32),
    /// Per-referrer accumulated reward balance. Discriminant 12.
    ReferralBalance(Address),
}

/// Detailed error information for insufficient balance scenarios.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InsufficientBalanceError {
    /// The current available prepaid balance in the subscription vault.
    pub available: i128,
    /// The required amount to complete the charge.
    pub required: i128,
}

impl InsufficientBalanceError {
    pub const fn new(available: i128, required: i128) -> Self {
        Self {
            available,
            required,
        }
    }

    pub fn shortfall(&self) -> i128 {
        self.required - self.available
    }
}

#[contracterror]
#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    // --- Auth Errors (401-403) ---
    /// Caller does not have the required authorization.
    Unauthorized = 401,
    /// Caller is authorized but does not have permission for this specific action.
    Forbidden = 403,

    // --- Not Found (404) ---
    /// The requested resource was not found in storage.
    NotFound = 404,

    // --- Invalid Input (400, 402, 405-409) ---
    /// The requested state transition is not allowed by the state machine.
    InvalidStatusTransition = 400,
    /// The top-up amount is below the minimum required threshold.
    BelowMinimumTopup = 402,

    // --- Business Logic Errors (1001-1005, 1010, 1012-1016) ---
    /// Charge interval has not elapsed since the last payment.
    IntervalNotElapsed = 1001,
    /// Subscription is not in an active state for this operation.
    NotActive = 1002,
    /// Insufficient balance in the subscription vault.
    InsufficientBalance = 1003,
    /// Usage charging is not enabled for this subscription.
    UsageNotEnabled = 1004,
    /// Insufficient prepaid balance for the requested usage charge.
    InsufficientPrepaidBalance = 1005,
    /// The provided amount is zero or negative.
    InvalidAmount = 1006,
    /// Charge already processed for this billing period (replay protection).
    Replay = 1007,
    /// Invalid recovery amount provided.
    InvalidRecoveryAmount = 1008,
    /// Emergency stop is active - critical operations are blocked.
    EmergencyStopActive = 1009,
    /// Operation would result in a negative balance or underflow.
    Underflow = 1010,
    /// Recovery operation not allowed for this reason or context.
    RecoveryNotAllowed = 1011,
    /// Combined balance would overflow i128.
    Overflow = 1012,
    /// The contract or requested configuration is not initialized.
    NotInitialized = 1013,
    /// The requested export limit exceeds the maximum allowed.
    InvalidExportLimit = 1014,
    /// Invalid input provided to a function.
    InvalidInput = 1015,
    /// Reentrancy detected - function called recursively during execution.
    Reentrancy = 1016,
    /// Lifetime charge cap has been reached; no further charges are allowed.
    LifetimeCapReached = 1017,
    /// Contract is already initialized; init may only be called once.
    AlreadyInitialized = 1018,
    /// The contract has allocated the maximum number of subscriptions.
    SubscriptionLimitReached = 429,
    /// A referral has already been registered for this subscription.
    ReferralAlreadyRegistered = 1019,
}

impl Error {
    /// Returns the numeric code for this error (for batch result reporting).
    pub const fn to_code(self) -> u32 {
        match self {
            Error::NotFound => 404,
            Error::Unauthorized => 401,
            Error::Forbidden => 403,
            Error::IntervalNotElapsed => 1001,
            Error::NotActive => 1002,
            Error::InvalidStatusTransition => 400,
            Error::BelowMinimumTopup => 402,
            Error::Overflow => 1012,
            Error::Underflow => 1010,
            Error::InsufficientBalance => 1003,
            Error::InvalidAmount => 1006,
            Error::UsageNotEnabled => 1004,
            Error::InsufficientPrepaidBalance => 1005,
            Error::Replay => 1007,
            Error::InvalidRecoveryAmount => 1008,
            Error::EmergencyStopActive => 1009,
            Error::RecoveryNotAllowed => 1011,
            Error::InvalidInput => 1015,
            Error::NotInitialized => 1013,
            Error::InvalidExportLimit => 1014,
            Error::Reentrancy => 1016,
            Error::LifetimeCapReached => 1017,
            Error::AlreadyInitialized => 1018,
            Error::SubscriptionLimitReached => 429,
            Error::ReferralAlreadyRegistered => 1019,
        }
    }
}

/// Result of charging one subscription in a batch.
#[contracttype]
#[derive(Clone, Debug)]
pub struct BatchChargeResult {
    /// True if the charge succeeded.
    pub success: bool,
    /// If success is false, the error code; otherwise 0.
    pub error_code: u32,
}

/// Result of a batch merchant withdrawal operation.
#[contracttype]
#[derive(Clone, Debug)]
pub struct BatchWithdrawResult {
    pub success: bool,
    pub error_code: u32,
}

/// Represents the lifecycle state of a subscription.
///
/// See `docs/subscription_lifecycle.md` for how each status is entered and exited.
///
/// # State Machine
///
/// - **Active**: Subscription is active and charges can be processed.
///   - Can transition to: `Paused`, `Cancelled`, `InsufficientBalance`, `GracePeriod`
/// - **Paused**: Subscription is temporarily suspended, no charges processed.
///   - Can transition to: `Active`, `Cancelled`
/// - **Cancelled**: Subscription is permanently terminated (terminal state).
///   - No outgoing transitions
/// - **InsufficientBalance**: Subscription failed due to insufficient funds.
///   - Can transition to: `Active` (after deposit + resume), `Cancelled`
/// - **GracePeriod**: Subscription is in grace period after a missed charge.
///   - Can transition to: `Active`, `InsufficientBalance`, `Cancelled`
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
    /// Subscription is in grace period after a missed charge.
    GracePeriod = 4,
}

/// Stores subscription details and current state.
///
/// The `status` field is managed by the state machine. Use the provided
/// transition helpers to modify status, never set it directly.
/// See `docs/subscription_lifecycle.md` for lifecycle and on-chain representation.
///
/// # Storage Schema
///
/// This is a named-field struct encoded on-ledger as a ScMap keyed by field names.
/// Adding new fields at the end with conservative defaults is a storage-extending change.
/// Changing field types or removing fields is a breaking change.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Subscription {
    pub subscriber: Address,
    pub merchant: Address,
    /// Recurring charge amount per billing interval (in token base units, e.g. stroops for USDC).
    pub amount: i128,
    /// Billing interval in seconds.
    pub interval_seconds: u64,
    pub last_payment_timestamp: u64,
    /// Current lifecycle state. Modified only through state machine transitions.
    pub status: SubscriptionStatus,
    /// Subscriber's prepaid balance held in escrow by the contract.
    pub prepaid_balance: i128,
    pub usage_enabled: bool,
    /// Optional maximum total amount (in token base units) that may ever be charged
    /// over the entire lifespan of this subscription. `None` means no cap.
    ///
    /// Units: same as `amount` (token base units, e.g. 1 USDC = 1_000_000 for 6 decimals).
    pub lifetime_cap: Option<i128>,
    /// Cumulative total of all amounts successfully charged so far.
    ///
    /// Incremented on every successful interval charge and usage charge.
    /// When `lifetime_cap` is `Some(cap)` and `lifetime_charged >= cap`, no
    /// further charges are processed and the subscription transitions to `Cancelled`.
    pub lifetime_charged: i128,
}

/// A read-only snapshot of the contract's configuration and current state.
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

/// A summary of a subscription's current state, intended for migration or reporting.
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
    pub lifetime_cap: Option<i128>,
    pub lifetime_charged: i128,
}

/// Event emitted when subscriptions are exported for migration.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MigrationExportEvent {
    pub admin: Address,
    pub start_id: u32,
    pub limit: u32,
    pub exported: u32,
    pub timestamp: u64,
}

/// Defines a reusable subscription plan template.
///
/// Plan templates allow merchants to define standard subscription offerings
/// with predefined parameters. Subscribers can create subscriptions from these
/// templates without manually specifying all parameters.
#[contracttype]
#[derive(Clone, Debug)]
pub struct PlanTemplate {
    /// Merchant who owns this plan template.
    pub merchant: Address,
    /// Recurring charge amount per interval (token base units).
    pub amount: i128,
    /// Billing interval in seconds.
    pub interval_seconds: u64,
    /// Whether usage-based charging is enabled.
    pub usage_enabled: bool,
    /// Optional lifetime cap applied to subscriptions created from this template.
    ///
    /// When `Some(cap)`, subscriptions created via this template will inherit the cap.
    /// `None` means subscriptions created from this template have no lifetime cap.
    pub lifetime_cap: Option<i128>,
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

/// View of a subscription's lifetime cap status.
///
/// Returned by `get_cap_info` for off-chain dashboards and UX displays.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapInfo {
    /// The configured lifetime cap, or `None` if no cap is set.
    pub lifetime_cap: Option<i128>,
    /// Total amount charged over the subscription's lifetime so far.
    pub lifetime_charged: i128,
    /// Remaining chargeable amount before cap is hit (`cap - charged`).
    /// `None` when no cap is configured.
    pub remaining_cap: Option<i128>,
    /// True when the cap has been reached and no further charges are allowed.
    pub cap_reached: bool,
}

/// Event emitted when emergency stop is enabled.
#[contracttype]
#[derive(Clone, Debug)]
pub struct EmergencyStopEnabledEvent {
    pub admin: Address,
    pub timestamp: u64,
}

/// Event emitted when emergency stop is disabled.
#[contracttype]
#[derive(Clone, Debug)]
pub struct EmergencyStopDisabledEvent {
    pub admin: Address,
    pub timestamp: u64,
}

/// Represents the reason for stranded funds that can be recovered by admin.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryReason {
    /// Funds sent to contract address by mistake.
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
    pub admin: Address,
    pub recipient: Address,
    pub amount: i128,
    pub reason: RecoveryReason,
    pub timestamp: u64,
}

/// Event emitted when a subscription is created.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionCreatedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub lifetime_cap: Option<i128>,
}

/// Event emitted when funds are deposited into a subscription vault.
#[contracttype]
#[derive(Clone, Debug)]
pub struct FundsDepositedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub amount: i128,
}

/// Event emitted when a subscription interval charge succeeds.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionChargedEvent {
    pub subscription_id: u32,
    pub merchant: Address,
    pub amount: i128,
    pub lifetime_charged: i128,
}

/// Event emitted when a subscription is cancelled.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionCancelledEvent {
    pub subscription_id: u32,
    pub authorizer: Address,
    pub refund_amount: i128,
}

/// Event emitted when a subscription is paused.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionPausedEvent {
    pub subscription_id: u32,
    pub authorizer: Address,
}

/// Event emitted when a subscription is resumed.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionResumedEvent {
    pub subscription_id: u32,
    pub authorizer: Address,
}

/// Event emitted when a merchant withdraws funds.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantWithdrawalEvent {
    pub merchant: Address,
    pub amount: i128,
}

/// Event emitted when a merchant-initiated one-off charge is applied.
#[contracttype]
#[derive(Clone, Debug)]
pub struct OneOffChargedEvent {
    pub subscription_id: u32,
    pub merchant: Address,
    pub amount: i128,
}

/// Event emitted when the lifetime charge cap is reached.
///
/// Signals that the subscription has been cancelled because it has been charged
/// up to its configured maximum total amount.
#[contracttype]
#[derive(Clone, Debug)]
pub struct LifetimeCapReachedEvent {
    pub subscription_id: u32,
    /// The configured lifetime cap that was reached.
    pub lifetime_cap: i128,
    /// Total charged at the point the cap was reached.
    pub lifetime_charged: i128,
    /// Timestamp when the cap was reached.
    pub timestamp: u64,
}

// ── Referral Rewards ──────────────────────────────────────────────────────────

/// Global configuration for the referral rewards program.
///
/// Set by admin via `configure_referral_program`. Controls how referral rewards
/// are computed and whether the program is currently active.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferralConfig {
    /// Reward rate in basis points (1 bp = 0.01%). Range: 0–10 000.
    ///
    /// Applied to the first successful charge amount.
    /// E.g., 500 = 5%, 100 = 1%, 10000 = 100%.
    pub reward_bps: u32,
    /// Optional per-referral reward ceiling in token base units.
    ///
    /// When `Some(cap)`, the computed reward is clamped to at most `cap`.
    /// `None` means no ceiling — reward is exactly `charge_amount * reward_bps / 10_000`.
    pub max_reward: Option<i128>,
    /// Whether the program is currently active and credits rewards.
    pub enabled: bool,
}

/// Tracks a single referral relationship for one subscription.
///
/// Stored per subscription. Records who referred the subscriber and whether
/// the one-time reward has already been credited.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferralRecord {
    /// Address of the referrer who will receive the reward.
    pub referrer: Address,
    /// True once the first qualifying charge has been processed and reward credited.
    pub rewarded: bool,
    /// Reward amount credited. Zero until the first qualifying charge.
    pub reward_amount: i128,
}

/// Event emitted when a referrer is registered for a subscription.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ReferralRegisteredEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub referrer: Address,
}

/// Event emitted when a referral reward is credited to a referrer's balance.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ReferralRewardCreditedEvent {
    pub subscription_id: u32,
    pub referrer: Address,
    pub amount: i128,
}

/// Event emitted when a referrer withdraws accumulated rewards.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ReferralRewardWithdrawnEvent {
    pub referrer: Address,
    pub amount: i128,
}
