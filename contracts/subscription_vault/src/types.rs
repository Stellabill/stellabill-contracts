//! Contract types: errors, subscription data structures, and event types.
//!
//! Kept in a separate module to reduce merge conflicts when editing state machine
//! or contract entrypoints.

use soroban_sdk::{ contracterror, contracttype, Address, String, Vec };

/// Maximum number of metadata keys per subscription.
pub const MAX_METADATA_KEYS: u32 = 10;
/// Maximum length of a metadata key in bytes.
pub const MAX_METADATA_KEY_LENGTH: u32 = 32;
/// Maximum length of a metadata value in bytes.
pub const MAX_METADATA_VALUE_LENGTH: u32 = 256;

/// Storage keys for secondary indices.
///
/// ## Storage Layout — Discriminant Registry
///
/// The Soroban `#[contracttype]` macro serialises enum variants by their
/// **declaration order** (0-indexed). The discriminant numbers in the doc
/// comments below are the canonical, frozen identifiers for each key.
/// **Never reorder or remove a variant** — doing so shifts all subsequent
/// discriminants and silently corrupts live storage. Only append new variants
/// at the end.
///
/// | Discriminant | Variant | Storage tier |
/// |:---:|:---|:---|
/// | 0 | `MerchantSubs(Address)` | instance |
/// | 1 | `Token` | instance |
/// | 2 | `Admin` | instance |
/// | 3 | `MinTopup` | instance |
/// | 4 | `NextId` | instance |
/// | 5 | `SchemaVersion` | instance |
/// | 6 | `Sub(u32)` | persistent |
/// | 7 | `ChargedPeriod(u32)` | persistent |
/// | 8 | `IdemKey(u32)` | persistent |
/// | 9 | `EmergencyStop` | instance |
/// | 10 | `MerchantPaused(Address)` | instance |
/// | 11 | `BillingStatement(u32, u32)` | persistent |
/// | 12 | `BillingStatementsBySubscription(u32)` | persistent |
/// | 13 | `BillingStatementsByMerchant(Address)` | persistent |
/// | 14 | `TotalAccounted(Address)` | instance |
/// | 15 | `Recovery(String)` | persistent |
/// | 16 | `MerchantConfig(Address)` | instance |
/// | 17 | `MerchantEarnings(Address, Address)` | instance |
/// | 18 | `MerchantTokens(Address)` | instance |
/// | 19 | `UsageLimits(u32)` | instance |
/// | 20 | `UsageState(u32)` | instance |
/// | 21 | `GracePeriod` | instance |
/// | 22 | `FeeBps` | instance |
/// | 23 | `Treasury` | instance |
/// | 24 | `AcceptedTokens` | instance |
/// | 25 | `TokenDecimals(Address)` | instance |
/// | 37 | `AdminNonce(Address, u32)` | persistent |
/// | 38 | `Operator` | instance |
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Maps a merchant address to its list of subscription IDs.
    MerchantSubs(Address),
    /// USDC token contract address.
    Token,
    /// Authorized admin address.
    Admin,
    /// Minimum deposit threshold.
    MinTopup,
    /// Auto-incrementing subscription ID counter.
    NextId,
    /// On-chain storage schema version.
    SchemaVersion,
    /// Subscription record keyed by its ID.
    Sub(u32),
    /// Last charged billing-period index for replay protection.
    ChargedPeriod(u32),
    /// Idempotency key stored per subscription.
    IdemKey(u32),
    /// Emergency stop flag - when true, critical operations are blocked.
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
    /// Detailed billing statement for a subscription charge.
    BillingStatement(u32, u32),
    /// Secondary index for statements by subscription.
    BillingStatementsBySubscription(u32),
    /// Secondary index for statements by merchant.
    BillingStatementsByMerchant(Address),
    /// Total accounted balance for recovery validation.
    TotalAccounted(Address),
    /// Replay protection key for recovery operations.
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
    /// Global grace period for underfunded subscriptions.
    GracePeriod,
    /// Protocol fee in basis points (0-10,000).
    FeeBps,
    /// Treasury address for protocol fee collection.
    Treasury,
    /// List of all token addresses accepted by the vault.
    AcceptedTokens,
    /// Decimals for a specific accepted token.
    TokenDecimals(Address),
    /// Auto-incrementing plan-template ID counter.
    NextPlanId,
    /// Plan template record keyed by its plan ID.
    Plan(u32),
    /// Maps a subscription ID to its parent plan-template ID.
    SubPlan(u32),
    /// Max concurrent active subscriptions allowed for a plan.
    PlanMaxActive(u32),
    /// Per-subscriber, per-token credit limit.
    CreditLimit(Address, Address),
    /// Maps a token address to its list of subscription IDs.
    TokenSubs(Address),
    /// Maps (merchant, token) to their accumulated balance.
    MerchantBalance(Address, Address),
    /// Maps a subscriber address to their blocklist status.
    Blocklist(Address),
    /// Oracle configuration.
    Oracle,
    /// Billing period snapshot storage.
    BillingPeriodSnapshot(u32, u64),
    /// Index for billing period snapshots.
    BillingPeriodSnapshotIndex(u32),
    /// Period-end billing snapshot keyed by (subscription_id, period_index).
    BillingPeriodSnapshot(u32, u64),
    /// Secondary index of period snapshot refs for a subscription.
    BillingPeriodSnapshotIndex(u32),
    /// Admin nonce for replay protection keyed by (admin_address, domain).
    AdminNonce(Address, u32),
}

/// Accrued totals by charge kind.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccruedTotals {
    pub interval: i128,
    pub usage: i128,
    pub one_off: i128,
}

/// Merchant token-scoped earnings.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenEarnings {
    pub accruals: AccruedTotals,
    pub withdrawals: i128,
    pub refunds: i128,
}

/// Snapshot for merchant earnings reporting.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationSnapshot {
    pub total_accruals: i128,
    pub total_withdrawals: i128,
    pub total_refunds: i128,
    pub computed_balance: i128,
}

/// A snapshot tied to a specific token.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenReconciliationSnapshot {
    pub token: Address,
    pub total_accruals: i128,
    pub total_withdrawals: i128,
    pub total_refunds: i128,
    pub computed_balance: i128,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageChargeResult {
    Charged = 0,
    Replay = 1,
    BurstLimitExceeded = 2,
    RateLimitExceeded = 3,
    UsageCapExceeded = 4,
}

/// Accumulated totals for a merchant's earnings.
#[contracttype]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AccruedTotals {
    pub interval: i128,
    pub usage: i128,
    pub one_off: i128,
}

/// Per-merchant, per-token earnings tracking.
#[contracttype]
#[derive(Clone, Debug)]
pub struct TokenEarnings {
    pub accruals: AccruedTotals,
    pub withdrawals: i128,
    pub refunds: i128,
}

/// Reconciliation snapshot for a token.
#[contracttype]
#[derive(Clone, Debug)]
pub struct TokenReconciliationSnapshot {
    pub token: Address,
    pub total_accruals: i128,
    pub total_withdrawals: i128,
    pub total_refunds: i128,
    pub computed_balance: i128,
    pub stored_balance: i128,
    pub matches: bool,
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
        if let Some(exp) = self.expires_at { current_time >= exp } else { false }
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
    // --- Auth Errors (1000-1099) ---
    /// Caller does not have the required authorization.
    Unauthorized = 1001,
    /// Caller is authorized but does not have permission for this specific action.
    Forbidden = 1002,
    /// Subscriber is on the blocklist and cannot create or interact with subscriptions.
    SubscriberBlocklisted = 1003,
    /// Rotation to the same admin address is not allowed.
    SelfRotation = 1004,
    /// Nonce has already been used for this signer and domain.
    NonceAlreadyUsed = 1005,

    // --- Not Found (2000-2099) ---
    /// The requested resource was not found in storage.
    NotFound = 2001,
    /// The contract or requested configuration is not initialized.
    NotInitialized = 2002,

    // --- Invalid Args (3000-3099) ---
    /// The provided amount is zero or negative.
    InvalidAmount = 3001,
    /// Invalid input provided to a function.
    InvalidInput = 3002,
    /// Invalid recovery amount provided.
    InvalidRecoveryAmount = 3003,
    /// The provided new admin address is invalid.
    InvalidNewAdmin = 3004,
    /// Metadata key exceeds maximum allowed length.
    MetadataKeyTooLong = 3005,
    /// Metadata value exceeds maximum allowed length.
    MetadataValueTooLong = 3006,
    /// Oracle returned a non-positive price.
    OraclePriceInvalid = 3007,
    /// Cannot change usage mode after subscription creation.
    CannotChangeUsageMode = 3008,
    /// Invalid token decimals provided.
    InvalidTokenDecimals = 3009,

    // --- State Transition (4000-4099) ---
    /// The requested state transition is not allowed by the state machine.
    InvalidStatusTransition = 4001,
    /// Subscription is not in an active state for this operation.
    NotActive = 4002,
    /// Subscription has expired based on its expires_at timestamp.
    SubscriptionExpired = 4003,
    /// Charge interval has not elapsed since the last payment.
    IntervalNotElapsed = 4004,
    /// Charge already processed for this billing period (replay protection).
    Replay = 4005,
    /// Recovery operation not allowed for this reason or context.
    RecoveryNotAllowed = 4006,
    /// Emergency stop is active - critical operations are blocked.
    EmergencyStopActive = 4007,
    /// Contract is already initialized; init may only be called once.
    AlreadyInitialized = 4008,
    /// Merchant-wide pause is active for this subscription.
    MerchantPaused = 4009,
    /// Reentrancy detected - function called recursively during execution.
    Reentrancy = 4010,

    // --- Accounting (5000-5099) ---
    /// Insufficient balance in the subscription vault.
    InsufficientBalance = 5001,
    /// Insufficient prepaid balance for the requested usage charge.
    InsufficientPrepaidBalance = 5002,
    /// The top-up amount is below the minimum required threshold.
    BelowMinimumTopup = 5003,
    /// Operation would result in a negative balance or underflow.
    Underflow = 5004,
    /// Combined balance would overflow i128.
    Overflow = 5005,
    /// Oracle pricing is enabled but no oracle is configured.
    OracleNotConfigured = 5006,
    /// Oracle returned an invalid or missing price payload.
    OraclePriceUnavailable = 5007,
    /// Oracle price is stale relative to configured max age.
    OraclePriceStale = 5008,

    // --- Limits (6000-6099) ---
    /// The contract has allocated the maximum number of subscriptions.
    SubscriptionLimitReached = 6001,
    /// Lifetime charge cap has been reached; no further charges are allowed.
    LifetimeCapReached = 6002,
    /// Usage charging is not enabled for this subscription.
    UsageNotEnabled = 6003,
    /// The requested export limit exceeds the maximum allowed.
    InvalidExportLimit = 6004,
    /// Metadata key limit reached for this subscription.
    MetadataKeyLimitReached = 6005,
    /// Subscriber has reached the maximum allowed number of active subscriptions for this plan.
    MaxConcurrentSubscriptionsReached = 6006,
    /// Subscriber's configured credit limit would be exceeded.
    CreditLimitExceeded = 6007,
    /// Usage rate limit exceeded for the current window.
    RateLimitExceeded = 6008,
    /// Usage charge would exceed the per-period cap.
    UsageCapExceeded = 6009,
    /// Usage charge attempted too soon after previous charge (burst protection).
    BurstLimitExceeded = 6010,

    // --- Merchant Config (7000-7099) ---
    /// Fee basis points exceed maximum allowed value.
    InvalidFeeBips = 7001,
    /// Invalid allowed operations bitmask.
    InvalidOperations = 7002,
    /// Charge operation must be allowed for merchant.
    MustAllowChargeOperation = 7003,
    /// Invalid or unsupported token address.
    InvalidToken = 7004,
    /// Fee basis points exceed the maximum allowed (10,000).
    InvalidFeeBips = 7001,
    /// Invalid allowed operations bitmap for merchant config.
    InvalidOperations = 7002,
    /// Merchant config must allow the charge operation.
    MustAllowChargeOperation = 7003,

    // --- Token (8000-8099) ---
    /// Token decimals value is invalid (e.g. zero).
    InvalidTokenDecimals = 8001,
    /// Token address is not accepted by this contract.
    InvalidToken = 8002,

    // --- Subscription Update (9000-9099) ---
    /// Attempting to change usage_enabled on an existing subscription is not allowed.
    CannotChangeUsageMode = 9001,
}

impl Error {
    /// Returns the numeric code for this error (for batch result reporting).
    pub const fn to_code(self) -> u32 {
        self as u32
    }
}

/// Event emitted when an admin nonce is consumed by a privileged operation.
///
/// Allows off-chain indexers to track the nonce sequence for each signer/domain
/// pair and detect anomalies such as gaps or unexpected resets.
#[contracttype]
#[derive(Clone, Debug)]
pub struct NonceConsumedEvent {
    /// The admin address that consumed the nonce.
    pub signer: Address,
    /// Domain tag identifying the operation class (see `nonce::DOMAIN_*` constants).
    pub domain: u32,
    /// The nonce value that was consumed.
    pub nonce: u64,
    /// Ledger timestamp when the nonce was consumed.
    pub timestamp: u64,
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
    /// Whether this plan has been disabled from accepting new subscriptions.
    pub is_disabled: bool,
}

/// Result of computing next charge information for a subscription.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NextChargeInfo {
    /// Estimated timestamp for the next charge attempt.
    pub next_charge_timestamp: u64,
    /// Whether a charge is actually expected based on the subscription status.
    pub is_charge_expected: bool,
    /// Current status of the subscription.
    pub status: SubscriptionStatus,
    /// Stable reason for the current charge state (e.g. symbol_short!("active"), symbol_short!("paused")).
    pub reason: soroban_sdk::Symbol,
    /// Next charge amount.
    pub amount: i128,
    /// Token address for the charge.
    pub token: soroban_sdk::Address,
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

/// Snapshot closed — no further mutations allowed.
pub const SNAPSHOT_FLAG_CLOSED: u32 = 1 << 0;
/// An interval charge was processed in this period.
pub const SNAPSHOT_FLAG_INTERVAL_CHARGED: u32 = 1 << 1;
/// At least one usage charge was processed in this period.
pub const SNAPSHOT_FLAG_USAGE_CHARGED: u32 = 1 << 2;
/// Period closed with no successful charges.
pub const SNAPSHOT_FLAG_EMPTY: u32 = 1 << 3;

/// Immutable per-period summary written after each successful interval charge.
///
/// Keyed by `(subscription_id, period_index)` where `period_index = timestamp / interval_seconds`.
/// Once `SNAPSHOT_FLAG_CLOSED` is set, the record cannot be overwritten.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BillingPeriodSnapshot {
    pub subscription_id: u32,
    pub period_index: u64,
    /// Ledger timestamp of the start of this billing period.
    pub period_start: u64,
    /// Ledger timestamp of the end of this billing period (charge time).
    pub period_end: u64,
    /// Total amount charged (interval + any usage) in token base units.
    pub total_charged: i128,
    /// Total usage units debited in this period.
    pub total_usage_units: i128,
    /// Bitmask of SNAPSHOT_FLAG_* constants.
    pub status_flags: u32,
    /// Ledger timestamp when the snapshot was finalized.
    pub finalized_at: u64,
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

// ── Period-end billing statement types ───────────────────────────────────────

/// Reason a period billing statement was finalized.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BillingStatementFinalization {
    /// A recurring billing period closed normally after a successful charge.
    PeriodClosed = 0,
    /// The subscription was cancelled; this covers the current partial period.
    Cancellation = 1,
    /// Subscriber withdrew remaining prepaid balance; final net settlement recorded.
    FinalSettlement = 2,
}

/// Lightweight index entry stored per-subscription and per-merchant.
///
/// Avoids scanning all contract state for pagination queries.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BillingStatementRef {
    pub subscription_id: u32,
    pub period_index: u32,
    /// `period_end_timestamp` is stored here so time-range filters can run on
    /// the index alone without loading each full statement.
    pub period_end_timestamp: u64,
}

/// Event emitted when a period billing statement is written or overwritten.
#[contracttype]
#[derive(Clone, Debug)]
pub struct BillingStatementPersistedEvent {
    pub subscription_id: u32,
    pub period_index: u32,
    pub merchant: Address,
    pub finalized_by: BillingStatementFinalization,
}

/// Grouped financial amounts for a single billing period.
///
/// Passed as a single parameter to [`SubscriptionVault::finalize_billing_statement`] so
/// the function stays within Soroban's 10-parameter limit.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeriodStatementAmounts {
    /// Sum of all charges (interval + usage + one-off) debited this period.
    pub total_amount_charged: i128,
    /// Total metered usage units billed (0 for non-usage subscriptions).
    pub total_usage_units: i128,
    /// Protocol fee withheld from the charge (0 if disabled).
    pub protocol_fee_amount: i128,
    /// Net amount credited to the merchant after fees.
    pub net_amount_to_merchant: i128,
    /// Total refunded to the subscriber this period.
    pub refund_amount: i128,
}

/// Compact per-period billing record written at period close, cancellation, or final settlement.
///
/// Indexed by `(subscription_id, period_index)`. Immutable once written; a
/// subsequent upsert with the same key replaces the record and updates indices.
#[contracttype]
#[derive(Clone, Debug)]
pub struct PeriodBillingStatement {
    pub subscription_id: u32,
    /// Monotonic period counter for this subscription (0-indexed from creation).
    pub period_index: u32,
    /// Period index of the associated billing snapshot, if any.
    pub snapshot_period_index: u32,
    pub merchant: Address,
    pub subscriber: Address,
    pub token: Address,
    pub period_start_timestamp: u64,
    pub period_end_timestamp: u64,
    /// Sum of all charges (interval + usage + one-off) debited this period.
    pub total_amount_charged: i128,
    /// Total metered usage units billed this period (0 for non-usage subscriptions).
    pub total_usage_units: i128,
    /// Protocol fee withheld from the charge (0 if fee routing is disabled).
    pub protocol_fee_amount: i128,
    /// Net amount credited to the merchant after fees.
    pub net_amount_to_merchant: i128,
    /// Total amount refunded to the subscriber in this period.
    pub refund_amount: i128,
    /// Bit flags encoding per-period status. See `docs/billing_statements.md`.
    pub status_flags: u32,
    pub subscription_status: SubscriptionStatus,
    pub finalized_by: BillingStatementFinalization,
    pub finalized_at: u64,
}

// ── status_flags bit constants (used by PeriodBillingStatement.status_flags) ─

/// Period had at least one interval charge.
pub const STMT_FLAG_INTERVAL_CHARGED: u32 = 0b0000_0001;
/// Period had at least one usage charge.
pub const STMT_FLAG_USAGE_CHARGED: u32    = 0b0000_0010;
/// Period had at least one one-off charge.
pub const STMT_FLAG_ONEOFF_CHARGED: u32   = 0b0000_0100;
/// Subscription was cancelled during this period.
pub const STMT_FLAG_CANCELLED: u32        = 0b0000_1000;
/// Subscriber withdrew remaining balance; period is fully settled.
pub const STMT_FLAG_SETTLED: u32          = 0b0001_0000;

// ─────────────────────────────────────────────────────────────────────────────

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

/// Event emitted when oracle configuration is updated by an admin.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleConfigUpdatedEvent {
    pub enabled: bool,
    pub oracle: Option<Address>,
    pub max_age_seconds: u64,
    pub timestamp: u64,
}

/// Event emitted when a cross-currency charge resolves its amount via oracle.
#[contracttype]
#[derive(Clone, Debug)]
pub struct OracleChargeResolvedEvent {
    pub subscription_id: u32,
    pub quote_amount: i128,
    pub token_amount: i128,
    pub price: i128,
    pub price_timestamp: u64,
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

/// Event emitted when an admin assigns an operator address.
#[contracttype]
#[derive(Clone, Debug)]
pub struct OperatorSetEvent {
    pub admin: Address,
    pub operator: Address,
    pub timestamp: u64,
}

/// Event emitted when an admin removes the operator address.
#[contracttype]
#[derive(Clone, Debug)]
pub struct OperatorRemovedEvent {
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
    /// Accidental transfer of funds to the contract.
    AccidentalTransfer = 4,
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
    pub token: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub lifetime_cap: Option<i128>,
    pub expires_at: Option<u64>,
    pub timestamp: u64,
}

/// Event emitted when funds are deposited into a subscription vault.
#[contracttype]
#[derive(Clone, Debug)]
pub struct FundsDepositedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    /// Settlement token deposited.
    pub token: Address,
    pub amount: i128,
    /// Total prepaid balance after this deposit.
    pub new_balance: i128,
    pub timestamp: u64,
}

/// Event emitted when a subscription interval charge succeeds.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionChargedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    /// Settlement token charged.
    pub token: Address,
    /// Amount charged in this interval (gross amount before fees).
    pub amount: i128,
    /// Cumulative total charged over subscription lifetime.
    pub lifetime_charged: i128,
    pub timestamp: u64,
    pub period_start: u64,
    pub period_end: u64,
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
    pub subscriber: Address,
    pub merchant: Address,
    pub token: Address,
    pub authorizer: Address,
    /// Remaining prepaid balance available for subscriber withdrawal.
    pub refund_amount: i128,
    pub timestamp: u64,
}

/// Event emitted when a subscription is paused.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionPausedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub authorizer: Address,
    pub timestamp: u64,
}

/// Event emitted when a subscription enters grace period.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GracePeriodEnteredEvent {
    pub subscription_id: u32,
    pub previous_status: SubscriptionStatus,
    pub grace_expires_at: u64,
    pub timestamp: u64,
}

/// Event emitted when a subscription is resumed.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionResumedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub authorizer: Address,
    pub previous_status: SubscriptionStatus,
    pub timestamp: u64,
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
    /// Merchant's accumulated balance remaining after withdrawal.
    pub remaining_balance: i128,
    pub timestamp: u64,
}

/// Event emitted when a subscriber withdraws funds after cancellation.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriberWithdrawalEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub token: Address,
    pub amount: i128,
    pub timestamp: u64,
}

/// Event emitted when a merchant-initiated one-off charge is applied.
#[contracttype]
#[derive(Clone, Debug)]
pub struct OneOffChargedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub token: Address,
    pub amount: i128,
    /// Prepaid balance remaining after this charge.
    pub remaining_balance: i128,
    pub timestamp: u64,
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
pub struct PlanTemplateCreatedEvent {
    pub plan_template_id: u32,
    pub merchant: Address,
    pub token: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub usage_enabled: bool,
    pub lifetime_cap: Option<i128>,
    pub timestamp: u64,
}

/// Event emitted when a plan template is updated.
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

/// Event emitted when a plan template is disabled.
#[contracttype]
#[derive(Clone, Debug)]
pub struct PlanTemplateDisabledEvent {
    /// The ID of the plan template that was disabled.
    pub plan_template_id: u32,
    /// Merchant that owns this plan template.
    pub merchant: Address,
    /// Timestamp when disabled.
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
#[derive(Clone, Debug)]
pub struct UsageChargeRejectedEvent {
    pub subscription_id: u32,
    pub merchant: Address,
    pub token: Address,
    pub usage_amount: i128,
    pub timestamp: u64,
    pub reference: String,
    pub result: UsageChargeResult,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct UsageLimitsConfiguredEvent {
    pub subscription_id: u32,
    pub merchant: Address,
    pub rate_limit_max_calls: Option<u32>,
    pub rate_window_secs: u64,
    pub burst_min_interval_secs: u64,
    pub usage_cap_units: Option<i128>,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChargeExecutionResult {
    Charged = 0,
    InsufficientBalance = 1,
    LifetimeCapReached = 2,
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
    pub token: Address,
    /// Amount refunded in token base units.
    pub amount: i128,
    /// Ledger timestamp when the refund was processed.
    pub timestamp: u64,
}

/// Operation flags for merchant configuration.
/// Each flag is a bit in the allowed_operations bitmap.
pub const OP_CHARGE: i32 = 1 << 0; // 0x01 - Can charge subscribers
pub const OP_WITHDRAW: i32 = 1 << 1; // 0x02 - Can withdraw earnings
pub const OP_REFUND: i32 = 1 << 2; // 0x04 - Can issue refunds to subscribers
pub const OP_BILLING_PAUSE: i32 = 1 << 3; // 0x08 - Can pause subscriptions globally
pub const OP_AUTO_RENEWAL: i32 = 1 << 4; // 0x10 - Auto-renewal enabled

/// Default allowed operations for a new merchant config.
pub const DEFAULT_ALLOWED_OPS: i32 = OP_CHARGE | OP_WITHDRAW | OP_REFUND | OP_AUTO_RENEWAL;

/// Maximum fee in bips (100% = 10000 bips).
pub const MAX_FEE_BIPS: i32 = 10000;

/// Validates that the allowed_operations bitmap contains only valid operation bits.
pub fn is_valid_allowed_operations(ops: i32) -> bool {
    let valid_mask = OP_CHARGE | OP_WITHDRAW | OP_REFUND | OP_BILLING_PAUSE | OP_AUTO_RENEWAL;
    ops & !valid_mask == 0
}

/// Extended merchant configuration with payout settings and operational flags.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct MerchantConfig {
    /// Version for forward-compatible config upgrades.
    pub version: i32,
    /// Address where merchant receives payouts.
    pub payout_address: Address,
    /// Fee percentage in bips (0-10000, where 10000 = 100%).
    pub fee_bips: i32,
    /// Bitmap of allowed operations (see OP_* constants).
    pub allowed_operations: i32,
    /// Whether the merchant can receive charges and payouts.
    pub is_active: bool,
    /// Address for fee routing (optional).
    pub fee_address: Option<Address>,
    /// Redirect URL for off-chain callbacks.
    pub redirect_url: String,
    /// Global pause for all merchant plans (legacy, prefer is_active).
    pub is_paused: bool,
    /// Timestamp of last config update.
    pub last_updated: u64,
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
    pub timestamp: u64,
}

/// Event emitted when protocol fees are configured.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ProtocolFeeConfiguredEvent {
    pub admin: Address,
    pub treasury: Address,
    pub fee_bps: u32,
    pub timestamp: u64,
}

/// Event emitted when merchant config is initialized.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantConfigInitializedEvent {
    pub merchant: Address,
    pub payout_address: Address,
    pub fee_bips: i32,
    pub allowed_operations: i32,
    pub timestamp: u64,
}

/// Event emitted when merchant config is updated.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantConfigUpdatedEvent {
    pub merchant: Address,
    pub payout_address: Address,
    pub fee_bips: i32,
    pub allowed_operations: i32,
    pub timestamp: u64,
}

/// Event emitted when a protocol fee is charged.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ProtocolFeeChargedEvent {
    pub subscription_id: u32,
    pub merchant: Address,
    pub token: Address,
    pub fee_amount: i128,
    pub treasury: Address,
    pub timestamp: u64,
}

/// Event emitted when a plan template is created.
#[contracttype]
#[derive(Clone, Debug)]
pub struct PlanTemplateCreatedEvent {
    pub plan_id: u32,
    pub admin: Address,
    pub interval: u64,
    pub amount: i128,
    pub usage_enabled: bool,
    pub timestamp: u64,
}

/// Event emitted when global cap default is updated.
#[contracttype]
#[derive(Clone, Debug)]
pub struct GlobalCapDefaultUpdatedEvent {
    pub admin: Address,
    pub cap: i128,
    pub timestamp: u64,
}

/// Event emitted when lifetime cap is updated.
#[contracttype]
#[derive(Clone, Debug)]
pub struct LifetimeCapUpdatedEvent {
    pub admin: Address,
    pub cap: i128,
    pub timestamp: u64,
}

/// Event emitted when merchant cap default is updated.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantCapDefaultUpdatedEvent {
    pub admin: Address,
    pub cap: i128,
/// Event emitted when a protocol fee is charged during a subscription billing.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ProtocolFeeChargedEvent {
    pub subscription_id: u32,
    pub treasury: Address,
    pub fee_amount: i128,
    pub merchant_amount: i128,
    pub timestamp: u64,
}

/// Event emitted when a merchant's config is first initialized.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantConfigInitializedEvent {
    pub merchant: Address,
    pub payout_address: Address,
    pub fee_bips: i32,
    pub allowed_operations: i32,
    pub timestamp: u64,
}

/// Event emitted when a merchant's config is updated.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantConfigUpdatedEvent {
    pub merchant: Address,
    pub payout_address: Address,
    pub fee_bips: i32,
    pub allowed_operations: i32,
    pub is_active: bool,
    pub timestamp: u64,
}

/// Event emitted when the global lifetime cap default is updated.
#[contracttype]
#[derive(Clone, Debug)]
pub struct GlobalCapDefaultUpdatedEvent {
    pub admin: Address,
    pub old_default: Option<i128>,
    pub new_default: Option<i128>,
    pub timestamp: u64,
}

/// Event emitted when a merchant's cap default is updated.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantCapDefaultUpdatedEvent {
    pub merchant: Address,
    pub old_default: Option<i128>,
    pub new_default: Option<i128>,
    pub timestamp: u64,
}

/// Event emitted when a subscription's lifetime cap is updated.
#[contracttype]
#[derive(Clone, Debug)]
pub struct LifetimeCapUpdatedEvent {
    pub subscription_id: u32,
    pub old_cap: Option<i128>,
    pub new_cap: Option<i128>,
    pub timestamp: u64,
}

/// Summary of all liabilities for a single settlement token.
///
/// Used by auditors to validate the accounting equation:
/// `contract_token_balance = total_prepaid + total_merchant_liabilities + recoverable`
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenLiabilities {
    /// Token contract address.
    pub token: Address,
    /// Sum of all subscriber prepaid balances in subscriptions using this token.
    pub total_prepaid: i128,
    /// Sum of all merchant earnings (accruals - withdrawals - refunds) for this token.
    pub total_merchant_liabilities: i128,
    /// Amount that can be recovered (stranded funds).
    pub recoverable_amount: i128,
    /// Contract's actual token balance at query time.
    pub contract_balance: i128,
    /// Computed total: prepaid + merchant liabilities + recoverable.
    pub computed_total: i128,
    /// Whether the accounting equation balances (contract_balance == computed_total).
    pub is_balanced: bool,
}

/// Paginated result for reconciliation queries across all tokens.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationSummaryPage {
    /// Per-token liability summaries.
    pub token_summaries: Vec<TokenLiabilities>,
    /// Cursor for next page if more tokens exist. `None` when complete.
    pub next_token_index: Option<u32>,
}

/// Proof structure for auditors to validate accounting off-chain.
///
/// Contains all data needed to independently verify the accounting equation
/// without requiring full contract state access.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationProof {
    /// Timestamp when the proof was generated.
    pub timestamp: u64,
    /// Ledger sequence at proof generation.
    pub ledger_sequence: u32,
    /// Token being audited.
    pub token: Address,
    /// Contract's token balance at query time.
    pub contract_balance: i128,
    /// Total prepaid balances across all subscriptions for this token.
    pub total_prepaid: i128,
    /// Total merchant earnings liabilities for this token.
    pub total_merchant_liabilities: i128,
    /// Computed recoverable amount (contract_balance - prepaid - merchant_liabilities).
    pub computed_recoverable: i128,
    /// Number of subscriptions scanned for the prepaid total.
    pub subscription_count: u32,
    /// Number of merchants scanned for the earnings total.
    pub merchant_count: u32,
    /// Whether the accounting equation validates.
    pub is_valid: bool,
}

/// Request for paginated prepaid balance aggregation.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrepaidQueryRequest {
    /// Token to filter by (required).
    pub token: Address,
    /// Starting subscription ID for pagination (inclusive).
    pub start_subscription_id: u32,
    /// Maximum number of subscriptions to scan in this call.
    pub scan_limit: u32,
}

/// Result of a paginated prepaid balance query.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrepaidQueryResult {
    /// Token that was queried.
    pub token: Address,
    /// Sum of prepaid balances found in this scan window.
    pub partial_total: i128,
    /// Number of subscriptions with non-zero prepaid balances found.
    pub subscriptions_count: u32,
    /// Next subscription ID to scan, or `None` if complete.
    pub next_start_id: Option<u32>,
    /// Whether more subscriptions may exist beyond this scan window.
    pub has_more: bool,
}
