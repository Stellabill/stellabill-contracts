//! Contract types: errors, subscription data structures, and event types.
//!
//! Kept in a separate module to reduce merge conflicts when editing state machine
//! or contract entrypoints.

use soroban_sdk::{contracterror, contracttype, Address, String, Vec};

/// Maximum number of metadata keys per subscription.
pub const MAX_METADATA_KEYS: u32 = 10;
/// Maximum length of a metadata key in bytes.
pub const MAX_METADATA_KEY_LENGTH: u32 = 32;
/// Maximum length of a metadata value in bytes.
pub const MAX_METADATA_VALUE_LENGTH: u32 = 256;

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
    /// Merchant-wide pause flag.
    MerchantPaused(Address),
    BillingStatement(u32, u32),
    BillingStatementsBySubscription(u32),
    BillingStatementsByMerchant(Address),
    TotalAccounted(Address),
    Recovery(String),
    /// Merchant configuration (pause state, fee routing, etc.).
    MerchantConfig(Address),
    /// Per-merchant, per-token accrued earnings record.
    MerchantEarnings(Address, Address),
    /// List of token addresses a merchant has earned in.
    MerchantTokens(Address),
    /// Usage rate/cap limits for a subscription.
    UsageLimits(u32),
    /// Running usage state for a subscription within the current window.
    UsageState(u32),
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    /// Subscription has automatically expired based on its expiration timestamp.
    Expired = 5,
    /// Subscription is archived (reduced storage, read-only).
    Archived = 6,
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
    /// Settlement token address used for all transfers on this subscription.
    pub token: Address,
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
    /// The timestamp when the subscription started.
    pub start_time: u64,
    /// The timestamp when the subscription expires. `None` means no expiration.
    pub expires_at: Option<u64>,
    /// Timestamp when a grace-period started. `None` means not in grace period.
    pub grace_start_timestamp: Option<u64>,
}

impl Subscription {
    pub fn is_expired(&self, current_time: u64) -> bool {
        if let Some(exp) = self.expires_at {
            current_time >= exp
        } else {
            false
        }
    }
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    // --- Auth Errors (401-403) ---
    /// Caller does not have the required authorization.
    Unauthorized = 401,
    /// Caller is authorized but does not have permission for this specific action.
    Forbidden = 403,
    /// Subscription has expired based on its expires_at timestamp.
    SubscriptionExpired = 410,

    // --- Not Found (404) ---
    /// The requested resource was not found in storage.
    NotFound = 404,

    // --- Invalid Input (400, 402) ---
    /// The requested state transition is not allowed by the state machine.
    InvalidStatusTransition = 400,
    /// The top-up amount is below the minimum required threshold.
    BelowMinimumTopup = 402,

    // --- Subscription limit (429) ---
    /// The contract has allocated the maximum number of subscriptions.
    SubscriptionLimitReached = 429,

    // --- Business Logic Errors (1001-1018) ---
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
    /// Merchant-wide pause is active for this subscription.
    MerchantPaused = 1019,

    // --- Metadata Errors (1023-1025) ---
    /// Metadata key limit reached for this subscription.
    MetadataKeyLimitReached = 1023,
    /// Metadata key exceeds maximum allowed length.
    MetadataKeyTooLong = 1024,
    /// Metadata value exceeds maximum allowed length.
    MetadataValueTooLong = 1025,

    // --- Blocklist (1026) ---
    /// Subscriber is on the blocklist and cannot create or interact with subscriptions.
    SubscriberBlocklisted = 1026,

    // --- Oracle Errors (1027-1030) ---
    /// Oracle pricing is enabled but no oracle is configured.
    OracleNotConfigured = 1027,
    /// Oracle returned an invalid or missing price payload.
    OraclePriceUnavailable = 1028,
    /// Oracle price is stale relative to configured max age.
    OraclePriceStale = 1029,
    /// Oracle returned a non-positive price.
    OraclePriceInvalid = 1030,

    // --- Subscription Plan / Credit (1031-1032) ---
    /// Subscriber has reached the maximum allowed number of active
    /// subscriptions for this plan.
    MaxConcurrentSubscriptionsReached = 1031,
    /// Subscriber's configured credit limit would be exceeded.
    CreditLimitExceeded = 1032,
    /// Usage rate limit exceeded for the current window.
    RateLimitExceeded = 1033,
    /// Usage charge would exceed the per-period cap.
    UsageCapExceeded = 1034,
    /// Usage charge attempted too soon after previous charge (burst protection).
    BurstLimitExceeded = 1035,
    /// Rotation to the same admin address is not allowed.
    SelfRotation = 1036,
    /// The provided new admin address is invalid.
    InvalidNewAdmin = 1037,
}

impl Error {
    /// Returns the numeric code for this error (for batch result reporting).
    pub const fn to_code(self) -> u32 {
        match self {
            Error::NotFound => 404,
            Error::Unauthorized => 401,
            Error::Forbidden => 403,
            Error::SubscriptionExpired => 410,
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
            Error::MerchantPaused => 1019,
            Error::MetadataKeyLimitReached => 1023,
            Error::MetadataKeyTooLong => 1024,
            Error::MetadataValueTooLong => 1025,
            Error::SubscriberBlocklisted => 1026,
            Error::OracleNotConfigured => 1027,
            Error::OraclePriceUnavailable => 1028,
            Error::OraclePriceStale => 1029,
            Error::OraclePriceInvalid => 1030,
            Error::SubscriptionLimitReached => 429,
            Error::MaxConcurrentSubscriptionsReached => 1031,
            Error::CreditLimitExceeded => 1032,
            Error::RateLimitExceeded => 1033,
            Error::UsageCapExceeded => 1034,
            Error::BurstLimitExceeded => 1035,
            Error::SelfRotation => 1036,
            Error::InvalidNewAdmin => 1037,
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
    pub token: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub last_payment_timestamp: u64,
    pub status: SubscriptionStatus,
    pub prepaid_balance: i128,
    pub usage_enabled: bool,
    pub lifetime_cap: Option<i128>,
    pub lifetime_charged: i128,
    pub start_time: u64,
    pub expires_at: Option<u64>,
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
    /// Settlement token used by subscriptions created from this plan.
    pub token: Address,
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
    /// Logical template group identifier.
    ///
    /// All versions of the same logical template share this value. The initial
    /// version of a template uses its own plan ID as the template key.
    pub template_key: u32,
    /// Monotonic version number within the template group (starts at 1).
    pub version: u32,
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

/// Canonical charge category used for billing statement history.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BillingChargeKind {
    Interval = 0,
    Usage = 1,
    OneOff = 2,
}

/// Immutable billing statement row for a subscription charge action.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BillingStatement {
    pub subscription_id: u32,
    /// Monotonic per-subscription sequence number (starts at 0).
    pub sequence: u32,
    /// Timestamp the charge operation was processed.
    pub charged_at: u64,
    /// Charge period start, in ledger timestamp seconds.
    pub period_start: u64,
    /// Charge period end, in ledger timestamp seconds.
    pub period_end: u64,
    /// Debited amount in token base units.
    pub amount: i128,
    pub merchant: Address,
    pub kind: BillingChargeKind,
}

/// Paginated page of billing statements.
#[contracttype]
#[derive(Clone, Debug)]
pub struct BillingStatementsPage {
    pub statements: Vec<BillingStatement>,
    /// Cursor for the next page. `None` means no more rows.
    pub next_cursor: Option<u32>,
    /// Total statements recorded for the subscription.
    pub total: u32,
}

/// Retention policy for billing statements.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BillingRetentionConfig {
    /// Number of most-recent detailed rows to keep per subscription.
    pub keep_recent: u32,
}

/// Accrued totals by charge kind.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccruedTotals {
    /// Total from interval charges.
    pub interval: i128,
    /// Total from usage charges.
    pub usage: i128,
    /// Total from one-off charges.
    pub one_off: i128,
}

/// Merchant earnings by token.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenEarnings {
    /// Accumulated earnings by charge kind.
    pub accruals: AccruedTotals,
    /// Total amount withdrawn by merchant.
    pub withdrawals: i128,
    /// Total amount refunded to subscribers.
    pub refunds: i128,
}

/// Token reconciliation snapshot for merchant.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenReconciliationSnapshot {
    /// Token address.
    pub token: Address,
    /// Total accrued earnings from all charge kinds.
    pub total_accruals: i128,
    /// Total amount withdrawn.
    pub total_withdrawals: i128,
    /// Total amount refunded.
    pub total_refunds: i128,
    /// Computed balance: accruals - withdrawals - refunds.
    pub computed_balance: i128,
}

/// Aggregated compacted history for pruned rows.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BillingStatementAggregate {
    pub pruned_count: u32,
    pub total_amount: i128,
    pub totals: AccruedTotals,
    pub oldest_period_start: Option<u64>,
    pub newest_period_end: Option<u64>,
}

/// Result of a compaction run.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BillingCompactionSummary {
    pub subscription_id: u32,
    pub pruned_count: u32,
    pub kept_count: u32,
    pub total_pruned_amount: i128,
}

/// Event emitted when statement compaction executes.
///
/// `aggregate_*` fields mirror [`BillingStatementAggregate`] after this run so indexers can
/// verify on-chain totals without a follow-up `get_stmt_compacted_aggregate` call (optional).
#[contracttype]
#[derive(Clone, Debug)]
pub struct BillingCompactedEvent {
    pub admin: Address,
    pub subscription_id: u32,
    pub pruned_count: u32,
    pub kept_count: u32,
    pub total_pruned_amount: i128,
    pub timestamp: u64,
    pub aggregate_pruned_count: u32,
    pub aggregate_total_amount: i128,
    pub aggregate_oldest_period_start: Option<u64>,
    pub aggregate_newest_period_end: Option<u64>,
}

/// Optional oracle pricing configuration for cross-currency plans.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleConfig {
    pub enabled: bool,
    pub oracle: Option<Address>,
    /// Maximum acceptable price age in seconds.
    pub max_age_seconds: u64,
}

/// Price payload returned by oracle contract view methods.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OraclePrice {
    /// Quote units per 1 token.
    pub price: i128,
    /// Timestamp when quote was published by oracle.
    pub timestamp: u64,
}

/// Token registry entry.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedToken {
    pub token: Address,
    pub decimals: u32,
}

/// Event emitted when emergency stop is enabled.
#[contracttype]
#[derive(Clone, Debug)]
pub struct EmergencyStopEnabledEvent {
    pub admin: Address,
    pub timestamp: u64,
}

/// Event emitted when admin is rotated to a new address.
#[contracttype]
#[derive(Clone, Debug)]
pub struct AdminRotatedEvent {
    pub old_admin: Address,
    pub new_admin: Address,
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
    /// Overpayment by user, e.g. sending tokens directly to the contract.
    UserOverpayment = 0,
    /// Transfer failed or stalled in an unexpected state.
    FailedTransfer = 1,
    /// Escrow expired or subscription cancelled with unreachable user.
    ExpiredEscrow = 2,
    /// System or logic correction.
    SystemCorrection = 3,
}

/// Event emitted when admin recovers stranded funds.
#[contracttype]
#[derive(Clone, Debug)]
pub struct RecoveryEvent {
    pub admin: Address,
    pub recipient: Address,
    pub token: Address,
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
    pub expires_at: Option<u64>,
}

/// Event emitted when funds are deposited into a subscription vault.
#[contracttype]
#[derive(Clone, Debug)]
pub struct FundsDepositedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub amount: i128,
    pub prepaid_balance: i128,
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

/// Event emitted when an interval charge attempt cannot be completed due to
/// insufficient prepaid balance.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionChargeFailedEvent {
    pub subscription_id: u32,
    pub merchant: Address,
    pub required_amount: i128,
    pub available_balance: i128,
    pub shortfall: i128,
    pub resulting_status: SubscriptionStatus,
    pub timestamp: u64,
}

/// Event emitted after a deposit when a previously underfunded subscription is
/// ready to be resumed.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionRecoveryReadyEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub prepaid_balance: i128,
    pub required_amount: i128,
    pub timestamp: u64,
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

/// Event emitted when a subscription is automatically expired.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionExpiredEvent {
    pub subscription_id: u32,
    pub timestamp: u64,
}

/// Event emitted when a subscription is archived.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionArchivedEvent {
    pub subscription_id: u32,
    pub timestamp: u64,
}

/// Event emitted when a merchant withdraws funds.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantWithdrawalEvent {
    pub merchant: Address,
    pub token: Address,
    pub amount: i128,
    pub remaining_balance: i128,
}

/// Event emitted when a subscriber withdraws funds after cancellation.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriberWithdrawalEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
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

/// Event emitted when metadata is set or updated on a subscription.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MetadataSetEvent {
    pub subscription_id: u32,
    pub key: String,
    pub authorizer: Address,
}

/// Event emitted when metadata is deleted from a subscription.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MetadataDeletedEvent {
    pub subscription_id: u32,
    pub key: String,
    pub authorizer: Address,
}

/// Event emitted when a plan template is updated to a new version.
#[contracttype]
#[derive(Clone, Debug)]
pub struct PlanTemplateUpdatedEvent {
    /// Logical template group identifier shared by all versions.
    pub template_key: u32,
    /// Previous plan template ID.
    pub old_plan_id: u32,
    /// Newly created plan template ID representing the updated version.
    pub new_plan_id: u32,
    /// Version number of the new plan template.
    pub version: u32,
    /// Merchant that owns this plan template.
    pub merchant: Address,
    /// Timestamp when the update occurred.
    pub timestamp: u64,
}

/// Event emitted when a plan's max-active-subscriptions limit is configured.
///
/// A `max_active` value of `0` means "no limit enforced".
#[contracttype]
#[derive(Clone, Debug)]
pub struct PlanMaxActiveUpdatedEvent {
    /// Plan template whose limit was changed.
    pub plan_template_id: u32,
    /// Merchant that owns the plan and authorized the change.
    pub merchant: Address,
    /// New limit value (`0` = unlimited).
    pub max_active: u32,
    /// Ledger timestamp when the change was applied.
    pub timestamp: u64,
}

/// Event emitted when a subscription is migrated from one plan template
/// version to another.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionMigratedEvent {
    pub subscription_id: u32,
    /// Logical template group identifier shared by all versions.
    pub template_key: u32,
    /// Plan template ID the subscription was previously pinned to.
    pub from_plan_id: u32,
    /// Plan template ID the subscription is now pinned to.
    pub to_plan_id: u32,
    /// Merchant that owns the plan templates.
    pub merchant: Address,
    /// Subscriber that authorized the migration.
    pub subscriber: Address,
    /// Timestamp when the migration occurred.
    pub timestamp: u64,
}

/// Event emitted when a usage statement is logged.
#[contracttype]
#[derive(Clone, Debug)]
pub struct UsageStatementEvent {
    pub subscription_id: u32,
    pub merchant: Address,
    pub usage_amount: i128,
    pub token: Address,
    pub timestamp: u64,
    pub reference: String,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChargeExecutionResult {
    Charged = 0,
    InsufficientBalance = 1,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageLimits {
    pub rate_limit_max_calls: Option<u32>,
    pub rate_window_secs: u64,
    pub burst_min_interval_secs: u64,
    pub usage_cap_units: Option<i128>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageState {
    pub last_usage_timestamp: u64,
    pub window_start_timestamp: u64,
    pub window_call_count: u32,
    pub current_period_usage_units: i128,
    pub period_index: u64,
}

/// Event emitted when a partial refund is processed for a subscription.
#[contracttype]
#[derive(Clone, Debug)]
pub struct PartialRefundEvent {
    /// Subscription receiving the refund.
    pub subscription_id: u32,
    /// Subscriber who receives the refunded amount.
    pub subscriber: Address,
    /// Amount refunded in token base units.
    pub amount: i128,
    /// Ledger timestamp when the refund was processed.
    pub timestamp: u64,
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct MerchantConfig {
    pub fee_address: Option<Address>,
    pub redirect_url: String, // e.g., for off-chain success callbacks
    pub is_paused: bool,      // Global pause for all merchant plans
}

/// Event emitted when a merchant enables their blanket pause.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantPausedEvent {
    pub merchant: Address,
    pub timestamp: u64,
}

/// Event emitted when a merchant disables their blanket pause.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantUnpausedEvent {
    pub merchant: Address,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantRefundEvent {
    pub merchant: Address,
    pub subscriber: Address,
    pub token: Address,
    pub amount: i128,
}

/// Event emitted when protocol fee is configured.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ProtocolFeeConfiguredEvent {
    pub admin: Address,
    pub treasury: Address,
    pub fee_bps: u32,
    pub timestamp: u64,
}

/// Event emitted when a protocol fee is charged.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ProtocolFeeChargedEvent {
    pub subscription_id: u32,
    pub treasury: Address,
    pub fee_amount: i128,
    pub merchant_amount: i128,
    pub timestamp: u64,
}
