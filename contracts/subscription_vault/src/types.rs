//! Contract types: errors, subscription data structures, and event types.
//!
//! Kept in a separate module to reduce merge conflicts when editing state machine
//! or contract entrypoints.

use soroban_sdk::{contracterror, contracttype, Address, Env, String, Vec, Bytes, BytesN};

/// Current schema version for contract events.
pub const EVENT_SCHEMA_VERSION: u32 = 2;

/// Maximum number of metadata keys per subscription.
pub const MAX_METADATA_KEYS: u32 = 10;
/// Maximum length of a metadata key in bytes.
pub const MAX_METADATA_KEY_LENGTH: u32 = 32;
/// Maximum length of a metadata value in bytes.
pub const MAX_METADATA_VALUE_LENGTH: u32 = 256;

/// Threshold below which a persistent subscription record TTL is extended.
/// If a subscription record is read or updated and its remaining TTL is less
/// than this threshold, it is extended to `SUB_TTL_EXTEND_TO`.
pub const SUB_TTL_THRESHOLD: u32 = 30 * 24 * 60 * 60; // 30 days

/// Target TTL for persistent subscription records when extended.
pub const SUB_TTL_EXTEND_TO: u32 = 365 * 24 * 60 * 60; // 365 days

/// Threshold below which a persistent billing statement secondary index TTL
/// is extended.
pub const BILLING_STATEMENT_TTL_THRESHOLD: u32 = 30 * 24 * 60 * 60; // 30 days

/// Target TTL for billing statement secondary index entries when extended.
pub const BILLING_STATEMENT_TTL_EXTEND_TO: u32 = 365 * 24 * 60 * 60; // 365 days

/// Threshold below which a persistent billing period snapshot TTL is extended.
pub const BILLING_PERIOD_SNAPSHOT_TTL_THRESHOLD: u32 = 30 * 24 * 60 * 60; // 30 days

/// Target TTL for billing period snapshot entries when extended.
pub const BILLING_PERIOD_SNAPSHOT_TTL_EXTEND_TO: u32 = 365 * 24 * 60 * 60; // 365 days

/// Replay protection domain for charge_subscription.
pub const DOMAIN_CHARGE_INTERVAL: u32 = 0;
/// Replay protection domain for deposit_funds.
pub const DOMAIN_DEPOSIT_FUNDS: u32 = 1;
/// Replay protection domain for charge_one_off.
pub const DOMAIN_CHARGE_ONEOFF: u32 = 2;

/// Number of idempotent hashes to store per subscription.
pub const IDEM_HISTORY: u32 = 32;

/// Maximum fee in basis points (100.00%).
pub const MAX_FEE_BIPS: i32 = 10000;

/// Ring buffer for subscription-scoped idempotency hashes.
#[contracttype]
#[derive(Clone, Debug)]
pub struct IdemRingBuffer {
    pub entries: Vec<BytesN<32>>,
    pub cursor: u32,
}

/// Per-merchant KYC attestation record (issued by an off-chain compliance provider).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MerchantKyc {
    /// Opaque attestation hash (provider-issued).
    pub attestation_hash: Bytes,
    /// Timestamp when the attestation was issued (ledger seconds).
    pub issued_at: u64,
    /// When true, KYC is active/valid. When false, it is revoked/inactive.
    pub status: bool,
}

/// Storage keys for secondary indices.
///
/// ## Storage Layout — Discriminant Registry
///
/// The Soroban `#[contracttype]` macro serialises enum variants by their
/// **declaration order** (0-indexed). The discriminant numbers below are the
/// canonical, frozen identifiers for each key and match
/// [`DataKey::canonical_discriminant`]. **Never reorder or remove a variant** —
/// doing so shifts all subsequent discriminants and silently corrupts live
/// storage. Only append new variants at the end.
///
/// The **Storage tier** column is authoritative: every instance-tier key below
/// is also listed in [`KNOWN_INSTANCE_KEY_DISCRIMINANTS`], the allowlist that
/// [`assert_known_data_key`] checks at instance read/write sites. When you add a
/// variant, append a row here, add its arm to `canonical_discriminant`, and —
/// if it is instance-tier — add its discriminant to the allowlist.
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
/// | 26 | `NextPlanId` | instance |
/// | 27 | `Plan(u32)` | instance |
/// | 28 | `SubPlan(u32)` | instance |
/// | 29 | `PlanMaxActive(u32)` | instance |
/// | 30 | `CreditLimit(Address, Address)` | instance |
/// | 31 | `TokenSubs(Address)` | instance |
/// | 32 | `SubscriberSubs(Address)` | instance |
/// | 33 | `MerchantBalance(Address, Address)` | instance |
/// | 34 | `Blocklist(Address)` | persistent |
/// | 35 | `Oracle` | instance |
/// | 36 | `BillingPeriodSnapshot(u32, u64)` | persistent |
/// | 37 | `BillingPeriodSnapshotIndex(u32)` | persistent |
/// | 38 | `AdminNonce(Address, u32)` | persistent |
/// | 39 | `Metadata(u32, String)` | persistent |
/// | 40 | `MetadataKeys(u32)` | persistent |
/// | 41 | `Operator` | instance |
/// | 42 | `BillingRetentionConfig` | instance |
/// | 43 | `BillingStatementSequence(u32)` | persistent |
/// | 44 | `BillingStatementAggregate(u32)` | persistent |
/// | 45 | `MerchantMaxSubs(Address)` | instance |
/// | 46 | `KycRequired` | instance |
/// | 47 | `MerchantKyc(Address)` | persistent |
/// | 48 | `PayoutSchedule(Address)` | instance |
/// | 49 | `TokenDecimalsPrimary` | instance |
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
    /// Maps a subscriber address to its list of subscription IDs.
    SubscriberSubs(Address),
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
    /// Admin nonce for replay protection keyed by (admin_address, domain).
    AdminNonce(Address, u32),
    /// Per-subscription metadata key-value pair.
    Metadata(u32, String),
    /// Per-subscription list of metadata keys.
    MetadataKeys(u32),
    /// Operator key.
    Operator,
    /// Global billing statement retention configuration.
    BillingRetentionConfig,
    /// Monotonic per-subscription statement sequence counter.
    BillingStatementSequence(u32),
    /// Aggregated totals from compacted billing statements.
    BillingStatementAggregate(u32),
    /// Max concurrent active subscriptions allowed for a merchant.
    MerchantMaxSubs(Address),
    /// Global flag: when true, merchants must have an active KYC attestation to withdraw.
    KycRequired,
    /// Per-merchant KYC attestation record.
    MerchantKyc(Address),
    /// Per-merchant automated payout schedule configuration.
    PayoutSchedule(Address),
    /// Decimals for the primary token.
    TokenDecimalsPrimary,
}

impl DataKey {
    /// Canonical, declaration-order discriminant for this key.
    pub const fn canonical_discriminant(&self) -> u32 {
        match self {
            DataKey::MerchantSubs(_) => 0,
            DataKey::Token => 1,
            DataKey::Admin => 2,
            DataKey::MinTopup => 3,
            DataKey::NextId => 4,
            DataKey::SchemaVersion => 5,
            DataKey::Sub(_) => 6,
            DataKey::ChargedPeriod(_) => 7,
            DataKey::IdemKey(_) => 8,
            DataKey::EmergencyStop => 9,
            DataKey::MerchantPaused(_) => 10,
            DataKey::BillingStatement(_, _) => 11,
            DataKey::BillingStatementsBySubscription(_) => 12,
            DataKey::BillingStatementsByMerchant(_) => 13,
            DataKey::TotalAccounted(_) => 14,
            DataKey::Recovery(_) => 15,
            DataKey::MerchantConfig(_) => 16,
            DataKey::MerchantEarnings(_, _) => 17,
            DataKey::MerchantTokens(_) => 18,
            DataKey::UsageLimits(_) => 19,
            DataKey::UsageState(_) => 20,
            DataKey::GracePeriod => 21,
            DataKey::FeeBps => 22,
            DataKey::Treasury => 23,
            DataKey::AcceptedTokens => 24,
            DataKey::TokenDecimals(_) => 25,
            DataKey::NextPlanId => 26,
            DataKey::Plan(_) => 27,
            DataKey::SubPlan(_) => 28,
            DataKey::PlanMaxActive(_) => 29,
            DataKey::CreditLimit(_, _) => 30,
            DataKey::TokenSubs(_) => 31,
            DataKey::SubscriberSubs(_) => 32,
            DataKey::MerchantBalance(_, _) => 33,
            DataKey::Blocklist(_) => 34,
            DataKey::Oracle => 35,
            DataKey::BillingPeriodSnapshot(_, _) => 36,
            DataKey::BillingPeriodSnapshotIndex(_) => 37,
            DataKey::AdminNonce(_, _) => 38,
            DataKey::Metadata(_, _) => 39,
            DataKey::MetadataKeys(_) => 40,
            DataKey::Operator => 41,
            DataKey::BillingRetentionConfig => 42,
            DataKey::BillingStatementSequence(_) => 43,
            DataKey::BillingStatementAggregate(_) => 44,
            DataKey::MerchantMaxSubs(_) => 45,
            DataKey::KycRequired => 46,
            DataKey::MerchantKyc(_) => 47,
            DataKey::PayoutSchedule(_) => 48,
            DataKey::TokenDecimalsPrimary => 49,
        }
    }

    /// Returns `true` if this key belongs to the canonical **instance**-storage
    /// allowlist ([`KNOWN_INSTANCE_KEY_DISCRIMINANTS`]).
    pub fn is_known_instance_key(&self) -> bool {
        is_known_instance_discriminant(self.canonical_discriminant())
    }
}

/// Canonical set of [`DataKey`] discriminants that legitimately live in
/// **instance** storage.
pub const KNOWN_INSTANCE_KEY_DISCRIMINANTS: &[u32] = &[
    0, 1, 2, 3, 4, 5, 9, 10, 14, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 35, 41, 42, 45, 46, 48, 49,
];

/// Returns `true` if `discriminant` is a recognised instance-storage key.
pub fn is_known_instance_discriminant(discriminant: u32) -> bool {
    KNOWN_INSTANCE_KEY_DISCRIMINANTS.iter().any(|&known| known == discriminant)
}

/// Debug-only guard asserting that `key` belongs to the canonical instance-key
/// allowlist before it is used for an instance read or write.
#[inline]
pub fn assert_known_data_key(key: &DataKey) {
    debug_assert!(key.is_known_instance_key(), "Unknown or persistent key reached instance storage: {}", key.canonical_discriminant());
}

/// Convenience wrapper over [`assert_known_data_key`] for instance storage helpers.
#[macro_export]
macro_rules! debug_assert_known_key {
    ($key:expr) => {
        $crate::types::assert_known_data_key($key)
    };
}

/// Represents the lifecycle state of a subscription.
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
#[contracttype]
#[derive(Clone, Debug)]
pub struct Subscription {
    pub subscriber: Address,
    pub merchant: Address,
    /// Settlement token address used for all transfers on this subscription.
    pub token: Address,
    /// Recurring charge amount per billing interval (in token base units).
    pub amount: i128,
    /// Billing interval in seconds.
    pub interval_seconds: u64,
    pub last_payment_timestamp: u64,
    /// Current lifecycle state.
    pub status: SubscriptionStatus,
    /// Subscriber's prepaid balance held in escrow by the contract.
    pub prepaid_balance: i128,
    pub usage_enabled: bool,
    /// Optional maximum total amount that may ever be charged over the entire lifespan.
    pub lifetime_cap: Option<i128>,
    /// Cumulative total of all amounts successfully charged so far.
    pub lifetime_charged: i128,
    /// The timestamp when the subscription started.
    pub start_time: u64,
    /// The timestamp when the subscription expires. `None` means no expiration.
    pub expires_at: Option<u64>,
    /// Timestamp when a grace-period started. `None` means not in grace period.
    pub grace_start_timestamp: Option<u64>,
    /// Scheduled future cancellation timestamp.
    pub cancel_at: Option<u64>,
}

impl Subscription {
    pub fn is_expired(&self, current_time: u64) -> bool {
        self.expires_at.map_or(false, |exp| current_time >= exp)
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
        Self { available, required }
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

    // --- Token (8000-8099) ---
    /// Token decimals value is invalid (e.g. zero).
    InvalidTokenDecimals = 8001,
    /// Token address is not accepted by this contract.
    InvalidToken = 8002,

    // --- Subscription Update (9000-9099) ---
    /// Attempting to change usage_enabled on an existing subscription is not allowed.
    CannotChangeUsageMode = 9001,

    // --- Schema Migration (9100-9199) ---
    /// Stored schema version is newer than the binary's STORAGE_VERSION; downgrade rejected.
    SchemaMigrationDowngrade = 9101,
}

impl Error {
    /// Returns the numeric code for this error.
    pub const fn to_code(self) -> u32 { self as u32 }
}

/// Event emitted when an admin nonce is consumed.
#[contracttype]
#[derive(Clone, Debug)]
pub struct NonceConsumedEvent {
    pub signer: Address,
    pub domain: u32,
    pub nonce: u64,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct BatchChargeResult {
    pub success: bool,
    pub error_code: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct BatchWithdrawResult {
    pub success: bool,
    pub error_code: u32,
}

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

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantBalanceEntry {
    pub merchant: Address,
    pub token: Address,
    pub amount: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct FullSnapshotPage {
    pub subscriptions: Vec<SubscriptionSummary>,
    pub balances: Vec<MerchantBalanceEntry>,
    pub next_start_id: Option<u32>,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SnapshotExportedEvent {
    pub admin: Address,
    pub start_id: u32,
    pub exported: u32,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SnapshotRestoredEvent {
    pub admin: Address,
    pub start_id: u32,
    pub restored: u32,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MigrationExportEvent {
    pub admin: Address,
    pub start_id: u32,
    pub limit: u32,
    pub exported: u32,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SchemaMigratedEvent {
    pub admin: Address,
    pub from_version: u32,
    pub to_version: u32,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct PlanTemplate {
    pub merchant: Address,
    pub token: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub usage_enabled: bool,
    pub lifetime_cap: Option<i128>,
    pub template_key: u32,
    pub version: u32,
    pub is_disabled: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NextChargeInfo {
    pub next_charge_timestamp: u64,
    pub is_charge_expected: bool,
    pub status: SubscriptionStatus,
    pub reason: soroban_sdk::Symbol,
    pub amount: i128,
    pub token: soroban_sdk::Address,
    pub grace_deadline: Option<u64>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapInfo {
    pub lifetime_cap: Option<i128>,
    pub lifetime_charged: i128,
    pub remaining_cap: Option<i128>,
    pub cap_reached: bool,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BillingChargeKind {
    Interval = 0,
    Usage = 1,
    OneOff = 2,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BillingStatement {
    pub subscription_id: u32,
    pub sequence: u32,
    pub charged_at: u64,
    pub period_start: u64,
    pub period_end: u64,
    pub amount: i128,
    pub merchant: Address,
    pub kind: BillingChargeKind,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct BillingStatementsPage {
    pub statements: Vec<BillingStatement>,
    pub next_cursor: Option<u32>,
    pub total: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BillingRetentionConfig {
    pub keep_recent: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccruedTotals {
    pub interval: i128,
    pub usage: i128,
    pub one_off: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BillingStatementAggregate {
    pub pruned_count: u32,
    pub total_amount: i128,
    pub totals: AccruedTotals,
    pub oldest_period_start: Option<u64>,
    pub newest_period_end: Option<u64>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BillingCompactionSummary {
    pub subscription_id: u32,
    pub pruned_count: u32,
    pub kept_count: u32,
    pub total_pruned_amount: i128,
}

pub const SNAPSHOT_FLAG_CLOSED: u32 = 1 << 0;
pub const SNAPSHOT_FLAG_INTERVAL_CHARGED: u32 = 1 << 1;
pub const SNAPSHOT_FLAG_USAGE_CHARGED: u32 = 1 << 2;
pub const SNAPSHOT_FLAG_EMPTY: u32 = 1 << 3;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BillingPeriodSnapshot {
    pub subscription_id: u32,
    pub period_index: u64,
    pub period_start: u64,
    pub period_end: u64,
    pub total_charged: i128,
    pub total_usage_units: i128,
    pub status_flags: u32,
    pub finalized_at: u64,
}

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
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleConfig {
    pub enabled: bool,
    pub oracle: Option<Address>,
    pub max_age_seconds: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OraclePrice {
    pub price: i128,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleConfigUpdatedEvent {
    pub enabled: bool,
    pub oracle: Option<Address>,
    pub max_age_seconds: u64,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct OracleChargeResolvedEvent {
    pub subscription_id: u32,
    pub quote_amount: i128,
    pub token_amount: i128,
    pub price: i128,
    pub price_timestamp: u64,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleLivenessEvent {
    pub last_sample_ts: u64,
    pub age: u64,
    pub healthy: bool,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedToken {
    pub token: Address,
    pub decimals: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct EmergencyStopEnabledEvent {
    pub admin: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct AdminRotatedEvent {
    pub old_admin: Address,
    pub new_admin: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct EmergencyStopDisabledEvent {
    pub admin: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct OperatorSetEvent {
    pub admin: Address,
    pub operator: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct OperatorRemovedEvent {
    pub admin: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryReason {
    UserOverpayment = 0,
    FailedTransfer = 1,
    ExpiredEscrow = 2,
    SystemCorrection = 3,
    AccidentalTransfer = 4,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct RecoveryEvent {
    pub admin: Address,
    pub recipient: Address,
    pub token: Address,
    pub amount: i128,
    pub reason: RecoveryReason,
    pub timestamp: u64,
    pub schema_version: u32,
}

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
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct FundsDepositedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub token: Address,
    pub amount: i128,
    pub new_balance: i128,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionChargedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub token: Address,
    pub amount: i128,
    pub lifetime_charged: i128,
    pub timestamp: u64,
    pub period_start: u64,
    pub period_end: u64,
    pub schema_version: u32,
}

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
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionRecoveryReadyEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub prepaid_balance: i128,
    pub required_amount: i128,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionCancelledEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub token: Address,
    pub authorizer: Address,
    pub refund_amount: i128,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionCancelScheduledEvent {
    pub subscription_id: u32,
    pub cancel_at: u64,
    pub scheduled_by: Address,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionCancelUnscheduledEvent {
    pub subscription_id: u32,
    pub unscheduled_by: Address,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionPausedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub authorizer: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GracePeriodEnteredEvent {
    pub subscription_id: u32,
    pub previous_status: SubscriptionStatus,
    pub grace_expires_at: u64,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionResumedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub authorizer: Address,
    pub previous_status: SubscriptionStatus,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionExpiredEvent {
    pub subscription_id: u32,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionArchivedEvent {
    pub subscription_id: u32,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct PayoutSchedule {
    pub cadence_seconds: u64,
    pub min_payout: i128,
    pub last_payout_at: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct ScheduledPayoutEvent {
    pub merchant: Address,
    pub caller: Address,
    pub tokens_paid: u32,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantWithdrawalEvent {
    pub merchant: Address,
    pub token: Address,
    pub amount: i128,
    pub remaining_balance: i128,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriberWithdrawalEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub token: Address,
    pub amount: i128,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct OneOffChargedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub token: Address,
    pub amount: i128,
    pub remaining_balance: i128,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct LifetimeCapReachedEvent {
    pub subscription_id: u32,
    pub lifetime_cap: i128,
    pub lifetime_charged: i128,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MetadataSetEvent {
    pub subscription_id: u32,
    pub key: String,
    pub authorizer: Address,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MetadataDeletedEvent {
    pub subscription_id: u32,
    pub key: String,
    pub authorizer: Address,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct PlanTemplateUpdatedEvent {
    pub template_key: u32,
    pub old_plan_id: u32,
    pub new_plan_id: u32,
    pub version: u32,
    pub merchant: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct PlanTemplateCreatedEvent {
    pub plan_id: u32,
    pub merchant: Address,
    pub token: Address,
    pub amount: i128,
    pub interval: u64,
    pub usage_enabled: bool,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct PlanTemplateDisabledEvent {
    pub plan_template_id: u32,
    pub merchant: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct PlanMaxActiveUpdatedEvent {
    pub plan_template_id: u32,
    pub merchant: Address,
    pub max_active: u32,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantMaxSubsUpdatedEvent {
    pub merchant: Address,
    pub max_subs: u32,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionMigratedEvent {
    pub subscription_id: u32,
    pub template_key: u32,
    pub from_plan_id: u32,
    pub to_plan_id: u32,
    pub merchant: Address,
    pub subscriber: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct UsageStatementEvent {
    pub subscription_id: u32,
    pub merchant: Address,
    pub usage_amount: i128,
    pub token: Address,
    pub timestamp: u64,
    pub reference: String,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageChargeResult {
    Charged = 0,
    InsufficientBalance = 1,
    LifetimeCapReached = 2,
    Replay = 3,
    BurstLimitExceeded = 4,
    RateLimitExceeded = 5,
    UsageCapExceeded = 6,
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
    pub schema_version: u32,
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
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChargeExecutionResult {
    Charged = 0,
    InsufficientBalance = 1,
    LifetimeCapReached = 2,
    ScheduledCancellation = 3,
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

#[contracttype]
#[derive(Clone, Debug)]
pub struct PartialRefundEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub token: Address,
    pub amount: i128,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantRefundEvent {
    pub merchant: Address,
    pub subscriber: Address,
    pub token: Address,
    pub amount: i128,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct ProtocolFeeConfiguredEvent {
    pub admin: Address,
    pub treasury: Address,
    pub fee_bps: u32,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct ProtocolFeeChargedEvent {
    pub subscription_id: u32,
    pub merchant: Address,
    pub token: Address,
    pub fee_amount: i128,
    pub treasury: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct ChargeFailureEvent {
    pub subscription_id: u32,
    pub error_code: u32,
    pub attempted_amount: i128,
    pub ledger: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct GlobalCapDefaultUpdatedEvent {
    pub admin: Address,
    pub cap: i128,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct LifetimeCapUpdatedEvent {
    pub admin: Address,
    pub cap: i128,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantCapDefaultUpdatedEvent {
    pub admin: Address,
    pub cap: i128,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenReconciliationSnapshot {
    pub token: Address,
    pub total_accruals: i128,
    pub total_withdrawals: i128,
    pub total_refunds: i128,
    pub computed_balance: i128,
    pub stored_balance: i128,
    pub matches: bool,
}

pub const OP_CHARGE: i32 = 1 << 0;
pub const OP_WITHDRAW: i32 = 1 << 1;
pub const OP_REFUND: i32 = 1 << 2;
pub const OP_BILLING_PAUSE: i32 = 1 << 3;
pub const OP_AUTO_RENEWAL: i32 = 1 << 4;
pub const DEFAULT_ALLOWED_OPS: i32 = OP_CHARGE | OP_WITHDRAW | OP_REFUND | OP_AUTO_RENEWAL;

pub fn is_valid_allowed_operations(ops: i32) -> bool {
    let all_ops = OP_CHARGE | OP_WITHDRAW | OP_REFUND | OP_BILLING_PAUSE | OP_AUTO_RENEWAL;
    (ops & !all_ops) == 0
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct MerchantConfig {
    pub version: i32,
    pub payout_address: Address,
    pub fee_bips: i32,
    pub allowed_operations: i32,
    pub is_active: bool,
    pub fee_address: Option<Address>,
    pub redirect_url: String,
    pub is_paused: bool,
    pub last_updated: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantPausedEvent {
    pub merchant: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantUnpausedEvent {
    pub merchant: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantConfigInitializedEvent {
    pub merchant: Address,
    pub payout_address: Address,
    pub fee_bips: i32,
    pub allowed_operations: i32,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantConfigUpdatedEvent {
    pub merchant: Address,
    pub payout_address: Address,
    pub fee_bips: i32,
    pub allowed_operations: i32,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantBalanceSnapshotEvent {
    pub merchant: Address,
    pub token: Address,
    pub balance: i128,
    pub accrued: i128,
    pub withdrawn: i128,
    pub refunded: i128,
    pub ledger_sequence: u32,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenEarnings {
    pub accruals: AccruedTotals,
    pub withdrawals: i128,
    pub refunds: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenLiabilities {
    pub token: Address,
    pub total_prepaid: i128,
    pub total_merchant_liabilities: i128,
    pub recoverable_amount: i128,
    pub contract_balance: i128,
    pub computed_total: i128,
    pub is_balanced: bool,
    pub normalized_prepaid: i128,
    pub normalized_merchant_liab: i128,
    pub normalized_recoverable: i128,
    pub normalized_contract_balance: i128,
    pub normalized_computed_total: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationSummaryPage {
    pub token_summaries: Vec<TokenLiabilities>,
    pub next_token_index: Option<u32>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationProof {
    pub timestamp: u64,
    pub ledger_sequence: u32,
    pub token: Address,
    pub contract_balance: i128,
    pub total_prepaid: i128,
    pub total_merchant_liabilities: i128,
    pub computed_recoverable: i128,
    pub subscription_count: u32,
    pub merchant_count: u32,
    pub is_valid: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrepaidQueryRequest {
    pub token: Address,
    pub start_subscription_id: u32,
    pub scan_limit: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrepaidQueryResult {
    pub token: Address,
    pub partial_total: i128,
    pub subscriptions_count: u32,
    pub next_start_id: Option<u32>,
    pub has_more: bool,
}

pub fn normalize_amount(env: &Env, token: &Address, amount: i128) -> Result<i128, Error> {
    let decimals: u32 = env.storage().instance().get(&DataKey::TokenDecimals(token.clone()))
        .ok_or(Error::InvalidToken)?;
    if decimals == 0 || decimals > 18 { return Err(Error::InvalidTokenDecimals); }
    if decimals == 9 { return Ok(amount); }
    if decimals < 9 {
        let factor = 10i128.checked_pow(9 - decimals).ok_or(Error::Overflow)?;
        amount.checked_mul(factor).ok_or(Error::Overflow)
    } else {
        let factor = 10i128.checked_pow(decimals - 9).ok_or(Error::Overflow)?;
        Ok(amount / factor)
    }
}

pub fn denormalize_amount(env: &Env, token: &Address, amount: i128) -> Result<i128, Error> {
    let decimals: u32 = env.storage().instance().get(&DataKey::TokenDecimals(token.clone()))
        .ok_or(Error::InvalidToken)?;
    if decimals == 0 || decimals > 18 { return Err(Error::InvalidTokenDecimals); }
    if decimals == 9 { return Ok(amount); }
    if decimals < 9 {
        let factor = 10i128.checked_pow(9 - decimals).ok_or(Error::Overflow)?;
        Ok(amount / factor)
    } else {
        let factor = 10i128.checked_pow(decimals - 9).ok_or(Error::Overflow)?;
        amount.checked_mul(factor).ok_or(Error::Overflow)
    }
}

#[cfg(test)]
mod known_keys_tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env, String};

    fn all_variants(env: &Env) -> std::vec::Vec<(DataKey, bool)> {
        let a = Address::generate(env);
        let b = Address::generate(env);
        let s = String::from_str(env, "k");
        std::vec![
            (DataKey::MerchantSubs(a.clone()), true),
            (DataKey::Token, true),
            (DataKey::Admin, true),
            (DataKey::MinTopup, true),
            (DataKey::NextId, true),
            (DataKey::SchemaVersion, true),
            (DataKey::Sub(1), false),
            (DataKey::ChargedPeriod(1), false),
            (DataKey::IdemKey(1), false),
            (DataKey::EmergencyStop, true),
            (DataKey::MerchantPaused(a.clone()), true),
            (DataKey::BillingStatement(1, 2), false),
            (DataKey::BillingStatementsBySubscription(1), false),
            (DataKey::BillingStatementsByMerchant(a.clone()), false),
            (DataKey::TotalAccounted(a.clone()), true),
            (DataKey::Recovery(s.clone()), false),
            (DataKey::MerchantConfig(a.clone()), true),
            (DataKey::MerchantEarnings(a.clone(), b.clone()), true),
            (DataKey::MerchantTokens(a.clone()), true),
            (DataKey::UsageLimits(1), true),
            (DataKey::UsageState(1), true),
            (DataKey::GracePeriod, true),
            (DataKey::FeeBps, true),
            (DataKey::Treasury, true),
            (DataKey::AcceptedTokens, true),
            (DataKey::TokenDecimals(a.clone()), true),
            (DataKey::NextPlanId, true),
            (DataKey::Plan(1), true),
            (DataKey::SubPlan(1), true),
            (DataKey::PlanMaxActive(1), true),
            (DataKey::CreditLimit(a.clone(), b.clone()), true),
            (DataKey::TokenSubs(a.clone()), true),
            (DataKey::SubscriberSubs(a.clone()), true),
            (DataKey::MerchantBalance(a.clone(), b.clone()), true),
            (DataKey::Blocklist(a.clone()), false),
            (DataKey::Oracle, true),
            (DataKey::BillingPeriodSnapshot(1, 2), false),
            (DataKey::BillingPeriodSnapshotIndex(1), false),
            (DataKey::AdminNonce(a.clone(), 1), false),
            (DataKey::Metadata(1, s.clone()), false),
            (DataKey::MetadataKeys(1), false),
            (DataKey::Operator, true),
            (DataKey::BillingRetentionConfig, true),
            (DataKey::BillingStatementSequence(1), false),
            (DataKey::BillingStatementAggregate(1), false),
            (DataKey::MerchantMaxSubs(a.clone()), true),
            (DataKey::KycRequired, true),
            (DataKey::MerchantKyc(a.clone()), false),
            (DataKey::PayoutSchedule(a.clone()), true),
            (DataKey::TokenDecimalsPrimary, true),
        ]
    }

    #[test]
    fn every_instance_variant_is_accepted() {
        let env = Env::default();
        for (key, is_instance) in all_variants(&env) {
            if is_instance {
                assert!(key.is_known_instance_key());
                assert_known_data_key(&key);
            }
        }
    }

    #[test]
    fn persistent_variants_are_rejected() {
        let env = Env::default();
        for (key, is_instance) in all_variants(&env) {
            if !is_instance {
                assert!(!key.is_known_instance_key());
            }
        }
    }

    #[test]
    fn synthetic_unknown_key_is_rejected() {
        assert!(!is_known_instance_discriminant(50));
    }

    #[test]
    fn discriminants_are_unique_and_contiguous() {
        let env = Env::default();
        let variants = all_variants(&env);
        let mut seen = [false; 50];
        for (key, _) in &variants {
            let d = key.canonical_discriminant() as usize;
            assert!(!seen[d]);
            seen[d] = true;
        }
        assert!(seen.iter().all(|&s| s));
        assert_eq!(variants.len(), 50);
    }
}
