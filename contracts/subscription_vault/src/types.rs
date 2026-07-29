//! Contract types: errors, subscription data structures, and event types.
//!
//! Kept in a separate module to reduce merge conflicts when editing state machine
//! or contract entrypoints.

use soroban_sdk::{
    contracterror, contracttype, symbol_short, Address, Bytes, BytesN, Env, Map, String, Symbol, Vec,
};

/// Current schema version for contract events.
pub const EVENT_SCHEMA_VERSION: u32 = 2;

/// Maximum number of metadata keys per subscription.
pub const MAX_METADATA_KEYS: u32 = 10;
/// Maximum length of a metadata key in bytes.
pub const MAX_METADATA_KEY_LENGTH: u32 = 32;
/// Maximum length of a metadata value in bytes.
pub const MAX_METADATA_VALUE_LENGTH: u32 = 256;
/// Maximum number of subscription IDs accepted by a single bulk pause/cancel call.
pub const BATCH_MAX_SIZE: u32 = 100;
/// Default cap on concurrent active subscriptions per subscriber (#578).
/// Admins can override this per-subscriber via `DataKey::SubscriberActiveCapOverride`.
pub const DEFAULT_SUBSCRIBER_ACTIVE_CAP: u32 = 10;

/// Maximum number of compliance-category tags a single merchant may carry
/// (#564). Bounds the size of `DataKey::MerchantTags(merchant)` so a
/// misconfigured or adversarial admin call cannot grow a single merchant's
/// tag list without bound.
pub const MAX_MERCHANT_TAGS: u32 = 8;

/// Threshold below which a persistent subscription record TTL is extended.
/// If a subscription record is read or updated and its remaining TTL is less
/// than this threshold, it is extended to `SUB_TTL_EXTEND_TO`.
pub const SUB_TTL_THRESHOLD: u32 = 30 * 24 * 60 * 60; // 30 days

/// Target TTL for persistent subscription records when extended.
pub const SUB_TTL_EXTEND_TO: u32 = 365 * 24 * 60 * 60; // 365 days

/// Threshold below which a persistent billing statement secondary index TTL
/// is extended.
#[allow(dead_code)]
pub const BILLING_STATEMENT_TTL_THRESHOLD: u32 = 30 * 24 * 60 * 60; // 30 days

/// Target TTL for billing statement secondary index entries when extended.
#[allow(dead_code)]
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

/// Key used for KYC data lookups, referenced by `DataKey::Kyc`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KycKey {
    pub account: Address,
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
#[contracttype(export = false)]
#[derive(Clone)]
pub enum DataKey {
    /// Maps a merchant address to its list of subscription IDs. Discriminant 0.
    MerchantSubs(Address),
    /// USDC token contract address. Discriminant 1.
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
    /// Emergency stop flag — when true, critical operations are blocked. Discriminant 9.
    EmergencyStop,
    /// Merchant-wide pause flag. Discriminant 10.
    MerchantPaused(Address),
    /// Detailed billing statement for a subscription charge. Discriminant 11.
    BillingStatement(u32, u32),
    /// Secondary index for statements by subscription. Discriminant 12.
    BillingStatementsBySubscription(u32),
    /// Secondary index for statements by merchant. Discriminant 13.
    BillingStatementsByMerchant(Address),
    /// Total accounted balance for recovery validation. Discriminant 14.
    TotalAccounted(Address),
    /// Replay protection key for recovery operations. Discriminant 15.
    Recovery(String),
    /// Merchant configuration (pause state, fee routing, etc.). Discriminant 16.
    MerchantConfig(Address),
    /// Per-merchant, per-token accrued earnings record. Discriminant 17.
    MerchantEarnings(Address, Address),
    /// List of token addresses a merchant has earned in. Discriminant 18.
    MerchantTokens(Address),
    /// Usage rate/cap limits for a subscription. Discriminant 19.
    UsageLimits(u32),
    /// Running usage state for a subscription within the current window. Discriminant 20.
    UsageState(u32),
    /// Global grace period for underfunded subscriptions. Discriminant 21.
    GracePeriod,
    /// Protocol fee in basis points (0-10,000). Discriminant 22.
    FeeBps,
    /// Treasury address for protocol fee collection. Discriminant 23.
    Treasury,
    /// List of all token addresses accepted by the vault. Discriminant 24.
    AcceptedTokens,
    /// Decimals for a specific accepted token. Discriminant 25.
    TokenDecimals(Address),
    /// Auto-incrementing plan-template ID counter. Discriminant 26.
    NextPlanId,
    /// Plan template record keyed by its plan ID. Discriminant 27.
    Plan(u32),
    /// Maps a subscription ID to its parent plan-template ID. Discriminant 28.
    SubPlan(u32),
    /// Max concurrent active subscriptions allowed for a plan. Discriminant 29.
    PlanMaxActive(u32),
    /// Per-subscriber, per-token credit limit. Discriminant 30.
    CreditLimit(Address, Address),
    /// Maps a token address to its list of subscription IDs. Discriminant 31.
    TokenSubs(Address),
    /// Maps a subscriber address to its list of subscription IDs. Discriminant 32.
    SubscriberSubs(Address),
    /// Maps (merchant, token) to their accumulated balance. Discriminant 33.
    MerchantBalance(Address, Address),
    /// Maps a subscriber address to their blocklist status. Discriminant 34.
    Blocklist(Address),
    /// Oracle configuration. Discriminant 35.
    Oracle,
    /// Billing period snapshot storage. Discriminant 36.
    BillingPeriodSnapshot(u32, u64),
    /// Index for billing period snapshots. Discriminant 37.
    BillingPeriodSnapshotIndex(u32),
    /// Admin nonce for replay protection keyed by (admin_address, domain). Discriminant 38.
    AdminNonce(Address, u32),
    /// Per-subscription metadata key-value pair. Discriminant 39.
    Metadata(u32, String),
    /// Per-subscription list of metadata keys. Discriminant 40.
    MetadataKeys(u32),
    /// Operator key. Discriminant 41.
    Operator,
    /// Global billing statement retention configuration. Discriminant 42.
    BillingRetentionConfig,
    /// Monotonic per-subscription statement sequence counter. Discriminant 43.
    BillingStatementSequence(u32),
    /// Aggregated totals from compacted billing statements. Discriminant 44.
    BillingStatementAggregate(u32),
    /// Max concurrent active subscriptions allowed for a merchant. Discriminant 45.
    MerchantMaxSubs(Address),
    /// Guardians voting weights for governance proposals. Discriminant 46.
    Guardians,
    /// Auto-incrementing proposal ID counter for governance. Discriminant 47.
    NextProposalId,
    /// Governance proposal record keyed by proposal ID. Discriminant 48.
    Proposal(u64),
    /// Dispute escrow amount held for a dispute (instance). Discriminant 49.
    DisputeEscrow(u64),
    /// Dispute record keyed by dispute ID (persistent). Discriminant 50.
    Dispute(u64),
    /// Auto-incrementing dispute ID counter (instance). Discriminant 51.
    NextDisputeId,
    /// Maps subscription ID to active dispute ID (instance). Discriminant 52.
    SubscriptionDispute(u32),
    /// Payout schedule configuration for a merchant. Discriminant 53.
    PayoutSchedule(Address),
    /// Pending protocol treasury/fee update queued for a later execution. Discriminant 54.
    PendingTreasuryChange,
    /// Transfer intent keyed by subscription ID (instance). Discriminant 54.
    TransferIntent(u32),
    /// KYC requirements and merchant status. Discriminant 55.
    Kyc(KycKey),
    /// Coupon configuration keyed by code. Discriminant 56.
    Coupon(soroban_sdk::Symbol),
    /// Coupon redemption counter keyed by code. Discriminant 57.
    CouponRedemptions(soroban_sdk::Symbol),
    /// Issued credentials keyed by subscription ID. Discriminant 58.
    Credential(u32),
    /// Timestamp of the most recent admin-config mutation for a given key label,
    /// hashed to `BytesN<32>` for collision-free per-key cooldown tracking.
    /// Discriminant 59.
    AdminConfigLastChangedAt(soroban_sdk::BytesN<32>),
    SubscriberCreateCap,
    SubscriberCreateWindow(Address),
    ChargeSalt(u32),
    /// Delegated payer grant keyed by (subscriber, payer). Discriminant 62.
    DelegatedPayerGrant(Address, Address),
    /// Split payees details for split-billing. Discriminant 59.
    SplitPayees(u32),
    /// Buyout premium in basis points for grace-period recovery. Discriminant 60.
    BuyoutPremiumBps,
    /// Merchant vacation window storing (start_ts, end_ts). Discriminant 62.
    MerchantVacation(Address),
    /// Coupon code bound to a subscription (persistent). Discriminant 68.
    SubCoupon(u32),
    /// Per-merchant multi-sig withdrawal quorum config (instance). Discriminant 69.
    MerchantMultiSig(Address),
    /// Count of a subscriber's currently-`Active` subscriptions (instance). Discriminant 70.
    SubscriberActiveCount(Address),
    /// Admin override of a subscriber's active-subscription cap (instance). Discriminant 71.
    SubscriberActiveCapOverride(Address),
    /// Admin-controlled allowlist of valid merchant compliance-category tags (instance,
    /// global). Discriminant 72.
    TagAllowlist,
    /// Compliance-category tags assigned to a merchant, capped at `MAX_MERCHANT_TAGS`
    /// (instance). Discriminant 73.
    MerchantTags(Address),
    /// Optional fee-token override: when set, protocol fees are paid in this
    /// token instead of the subscription's settlement token, converted through
    /// the oracle at charge time. Discriminant 64.
    FeeToken,
    /// Cancellation refund escrow record keyed by subscription ID. Discriminant 75.
    CancellationEscrow(u32),
    /// Per-merchant protocol-fee override in basis points (instance). Discriminant 76.
    MerchantFeeBps(Address),
    /// Per-token oracle price history ring-buffer metadata (instance). Discriminant 77.
    OraclePriceHistoryMeta(Address),
    /// Per-token oracle price history ring-buffer entry (instance). Discriminant 78.
    OraclePriceHistoryEntry(Address, u32),
    MerchantWhitelistMode,
    MerchantApproved(Address),
    ChargeFailureCounter(u32),
    AutoPauseThreshold,
    MerchantSubAccount(Address, Symbol),
    MerchantSubAccountList(Address),
    EmergencyWithdrawIntent(u32),
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
            DataKey::DelegatedPayerGrant(_, _) => 62,
            DataKey::Guardians => 46,
            DataKey::NextProposalId => 47,
            DataKey::Proposal(_) => 48,
            DataKey::DisputeEscrow(_) => 49,
            DataKey::Dispute(_) => 50,
            DataKey::NextDisputeId => 51,
            DataKey::SubscriptionDispute(_) => 52,
            DataKey::PayoutSchedule(_) => 53,
            DataKey::PendingTreasuryChange => 54,
            DataKey::TransferIntent(_) => 54,
            DataKey::Kyc(_) => 55,
            DataKey::Coupon(_) => 56,
            DataKey::CouponRedemptions(_) => 57,
            DataKey::Credential(_) => 58,
            DataKey::SplitPayees(_) => 59,
            DataKey::BuyoutPremiumBps => 60,
            DataKey::MerchantVacation(_) => 62,
            DataKey::AdminConfigLastChangedAt(_) => 59,
            DataKey::SubscriberCreateCap => 60,
            DataKey::SubscriberCreateWindow(_) => 61,
            DataKey::MerchantWhitelistMode => 62,
            DataKey::MerchantApproved(_) => 63,
            DataKey::ChargeSalt(_) => 64,
            DataKey::ChargeFailureCounter(_) => 65,
            DataKey::AutoPauseThreshold => 66,
            DataKey::BuyoutPremiumBps => 67,
            DataKey::SubCoupon(_) => 68,
            DataKey::SubCoupon(_) => 68,
            DataKey::MerchantMultiSig(_) => 69,
            DataKey::SubscriberActiveCount(_) => 70,
            DataKey::SubscriberActiveCapOverride(_) => 71,
            DataKey::TagAllowlist => 72,
            DataKey::MerchantTags(_) => 73,
            DataKey::FeeToken => 74,
            DataKey::CancellationEscrow(_) => 75,
            DataKey::MerchantFeeBps(_) => 76,
            DataKey::OraclePriceHistoryMeta(_) => 77,
            DataKey::OraclePriceHistoryEntry(_, _) => 78,
            DataKey::MerchantWhitelistMode => 79,
            DataKey::MerchantApproved(_) => 80,
            DataKey::ChargeFailureCounter(_) => 81,
            DataKey::AutoPauseThreshold => 82,
            DataKey::MerchantSubAccount(_, _) => 83,
            DataKey::MerchantSubAccountList(_) => 84,
            DataKey::EmergencyWithdrawIntent(_) => 85,
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
    0,  // MerchantSubs(Address)
    1,  // Token
    2,  // Admin
    3,  // MinTopup
    4,  // NextId
    5,  // SchemaVersion
    9,  // EmergencyStop
    10, // MerchantPaused(Address)
    14, // TotalAccounted(Address)
    16, // MerchantConfig(Address)
    17, // MerchantEarnings(Address, Address)
    18, // MerchantTokens(Address)
    19, // UsageLimits(u32)
    20, // UsageState(u32)
    21, // GracePeriod
    22, // FeeBps
    23, // Treasury
    24, // AcceptedTokens
    25, // TokenDecimals(Address)
    26, // NextPlanId
    27, // Plan(u32)
    28, // SubPlan(u32)
    29, // PlanMaxActive(u32)
    30, // CreditLimit(Address, Address)
    31, // TokenSubs(Address)
    32, // SubscriberSubs(Address)
    33, // MerchantBalance(Address, Address)
    35, // Oracle
    41, // Operator
    42, // BillingRetentionConfig
    45, // MerchantMaxSubs(Address)
    47, // NextProposalId
    49, // DisputeEscrow(u64)
    51, // NextDisputeId
    52, // SubscriptionDispute(u32)
    53, // PayoutSchedule(Address)
    54, // TransferIntent(u32)
    59, // BuyoutPremiumBps
    61, // MerchantMultiSig(Address)
    62, // MerchantVacation(Address)
    59, // AdminConfigLastChangedAt(BytesN<32>)
    60, // SubscriberCreateCap
    61, // SubscriberCreateWindow(Address)
    62, // MerchantWhitelistMode
    63, // MerchantApproved(Address)
    64, // ChargeSalt(u32)
    65, // ChargeFailureCounter(u32)
    66, // AutoPauseThreshold
    67, // BuyoutPremiumBps
    69, // MerchantMultiSig(Address)
    70, // SubscriberActiveCount(Address)
    71, // SubscriberActiveCapOverride(Address)
    72, // TagAllowlist
    73, // MerchantTags(Address)
    74, // FeeToken
    76, // MerchantFeeBps(Address)
    77, // OraclePriceHistoryMeta(Address)
    78, // OraclePriceHistoryEntry(Address, u32)
];

/// Returns `true` if `discriminant` is a recognised instance-storage key.
pub fn is_known_instance_discriminant(discriminant: u32) -> bool {
    KNOWN_INSTANCE_KEY_DISCRIMINANTS
        .iter()
        .any(|&known| known == discriminant)
}

/// Debug-only guard asserting that `key` belongs to the canonical instance-key
/// allowlist before it is used for an instance read or write.
#[inline]
#[allow(dead_code)]
pub fn assert_known_data_key(key: &DataKey) {
    debug_assert!(
        key.is_known_instance_key(),
        "Unknown or persistent key reached instance storage: {}",
        key.canonical_discriminant()
    );
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
    /// Optional ledger-sequence bound for expiration. When set, the subscription
    /// also expires as soon as the ledger sequence reaches this value,
    /// independently of the wall-clock `expires_at`. `None` disables the bound.
    ///
    /// Either bound being met is sufficient to consider the subscription
    /// expired for charge / deposit / state-transition purposes.
    pub expires_at_ledger: Option<u32>,
    /// Optional sub-account label for routing charges to an isolated merchant
    /// sub-account ledger (#575). `None` routes to the parent merchant balance.
    pub sub_account_label: Option<Symbol>,
    pub auto_renew_disabled_at: Option<u64>,
    pub auto_renew: bool,
}

impl Subscription {
    /// Returns true when *either* the wall-clock bound or the ledger-sequence
    /// bound is met (or both). `None` for either bound disables that check.
    pub fn is_expired(&self, current_time: u64, current_ledger: u32) -> bool {
        if let Some(exp) = self.expires_at {
            if current_time >= exp {
                return true;
            }
        }
        if let Some(exp_ledger) = self.expires_at_ledger {
            if current_ledger >= exp_ledger {
                return true;
            }
        }
        false
    }

    /// Returns `true` when the renewal window (one full interval) after
    /// auto-renewal was disabled is still open at `current_time`.
    pub fn is_in_renewal_window(&self, current_time: u64) -> bool {
        match self.auto_renew_disabled_at {
            Some(disabled_at) => {
                let window_end = disabled_at.saturating_add(self.interval_seconds);
                current_time < window_end
            }
            None => false,
        }
    }
}

/// Pending emergency-withdraw intent for a paused or cancelled subscription.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmergencyWithdrawIntent {
    pub subscription_id: u32,
    pub requested_at: u64,
    pub requested_status: SubscriptionStatus,
}

/// A non-transferable (soulbound) credential badge linking a subscription.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialBadge {
    pub subscription_id: u32,
    pub tier: u32,
    pub issued_at: u64,
    pub revoked: bool,
}

/// Split billing payees and their basis points weights.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SplitPayees {
    pub subscription_id: u32,
    pub entries: Vec<(Address, u32)>,
}

/// Event emitted when split charge is distributed.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SplitChargeEvent {
    pub subscription_id: u32,
    pub payees: Vec<(Address, i128)>,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Event emitted when a soulbound credential is issued for a new subscription.
#[contracttype]
#[derive(Clone, Debug)]
pub struct CredentialIssuedEvent {
    pub subscription_id: u32,
    pub tier: u32,
    pub issued_at: u64,
}

/// Event emitted when a soulbound credential is revoked (subscription cancelled).
#[contracttype]
#[derive(Clone, Debug)]
pub struct CredentialRevokedEvent {
    pub subscription_id: u32,
    pub timestamp: u64,
}

/// Event emitted when `auto_renew` is toggled on a subscription.
///
/// Published by `set_auto_renew` with topic `("auto_renew_toggled", subscription_id)`.
#[contracttype]
#[derive(Clone, Debug)]
pub struct AutoRenewToggledEvent {
    /// The subscription whose auto-renewal flag changed.
    pub subscription_id: u32,
    /// The subscriber who owns the subscription.
    pub subscriber: Address,
    /// The merchant who receives the recurring payment.
    pub merchant: Address,
    /// New value of the `auto_renew` flag.
    pub enabled: bool,
    /// Caller who authorized the change (subscriber or merchant).
    pub authorizer: Address,
    /// Ledger timestamp of the toggle.
    pub timestamp: u64,
    /// Event schema version for backwards-compatible indexer decoding.
    pub schema_version: u32,
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

/// Time window (in seconds) for the dispute/chargeback process.
///
/// During this window the merchant/admin may respond to a dispute. If no
/// response is received before the window elapses, the dispute may be resolved
/// in favour of the subscriber.
pub const DISPUTE_WINDOW_SECS: u64 = 14 * 24 * 60 * 60; // 14 days

/// Lifecycle status of a dispute.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisputeStatus {
    /// Dispute opened, awaiting merchant/admin response. Funds held in escrow.
    Open = 0,
    /// Merchant/admin has responded to the dispute. Awaiting final resolution.
    Responded = 1,
    /// Dispute resolved in favour of the merchant; escrow released to merchant.
    ResolvedToMerchant = 2,
    /// Dispute resolved in favour of the subscriber; escrow returned to subscriber.
    ResolvedToSubscriber = 3,
}

/// Dispute / chargeback record tracking contested charges.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Dispute {
    /// Unique dispute ID (auto-incremented).
    pub id: u64,
    /// Subscription the dispute is against.
    pub subscription_id: u32,
    /// Subscriber who opened the dispute.
    pub subscriber: Address,
    /// Merchant who received the original payment.
    pub merchant: Address,
    /// Amount held in escrow pending resolution (token base units).
    pub amount: i128,
    /// Ledger timestamp when the dispute was opened.
    pub opened_at: u64,
    /// Current status of the dispute.
    pub status: DisputeStatus,
    /// Optional evidence hash provided by the subscriber.
    pub evidence_hash: Option<soroban_sdk::BytesN<32>>,
    /// Ledger timestamp when the admin responded (None if not yet responded).
    pub responded_at: Option<u64>,
    /// Optional evidence hash provided by the admin (merchant side).
    pub admin_evidence_hash: Option<soroban_sdk::BytesN<32>>,
}

/// Event emitted when a dispute is opened.
#[contracttype]
#[derive(Clone, Debug)]
pub struct DisputeOpenedEvent {
    pub dispute_id: u64,
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub amount: i128,
    pub evidence_hash: Option<soroban_sdk::BytesN<32>>,
    pub timestamp: u64,
    /// Event schema version for backwards-compatible indexer decoding.
    pub schema_version: u32,
}

/// Event emitted when an admin responds to a dispute.
#[contracttype]
#[derive(Clone, Debug)]
pub struct DisputeRespondedEvent {
    pub dispute_id: u64,
    pub subscription_id: u32,
    pub admin_evidence_hash: Option<soroban_sdk::BytesN<32>>,
    pub timestamp: u64,
    /// Event schema version for backwards-compatible indexer decoding.
    pub schema_version: u32,
}

/// Cumulative escrow ledger for a dispute.
///
/// Tracks the original escrowed amount and the cumulative amount disbursed
/// across one or more resolution steps. Every resolution is checked against
/// this ledger so that `total_disbursed <= original_amount` at all times,
/// preventing overpay even if partial-resolution logic is added later.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisputeEscrowLedger {
    /// The total amount escrowed when the dispute was opened.
    pub original_amount: i128,
    /// Cumulative amount already disbursed via resolutions.
    pub total_disbursed: i128,
}

/// Event emitted when a dispute is resolved.
#[contracttype]
#[derive(Clone, Debug)]
pub struct DisputeResolvedEvent {
    pub dispute_id: u64,
    pub subscription_id: u32,
    /// Final status of the dispute after resolution.
    pub resolution: DisputeStatus,
    pub timestamp: u64,
    /// Event schema version for backwards-compatible indexer decoding.
    pub schema_version: u32,
}

/// The privileged action a governance proposal executes once quorum is reached.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProposalKind {
    /// Rotate the contract admin to `Proposal::target`.
    RotateAdmin = 0,
    /// Set the protocol fee (bps in `target3`) and, optionally, the treasury
    /// address (`target2`).
    SetProtocolFee = 1,
    /// Reserved for a future contract-upgrade action.
    UpgradeContract = 2,
}

/// A quorum-gated governance proposal.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Proposal {
    pub id: u64,
    pub kind: ProposalKind,
    pub target: Address,
    pub target2: Option<Address>,
    pub target3: u32,
    /// Required approval quorum, in basis points of total guardian weight.
    pub quorum_bps: u32,
    /// Per-guardian vote: `true` = yes, `false` = no.
    pub votes: Map<Address, bool>,
    /// Ledger timestamp at/after which this proposal may execute.
    pub eta: u64,
    pub submitted_at: u64,
    pub executed: bool,
}

/// Event emitted when a new governance proposal is submitted.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ProposalSubmittedEvent {
    pub proposal_id: u64,
    pub kind: ProposalKind,
    pub target: Address,
    pub quorum_bps: u32,
    pub eta: u64,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Event emitted when a guardian votes on a proposal.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ProposalVotedEvent {
    pub proposal_id: u64,
    pub guardian: Address,
    pub voted_yes: bool,
    pub guardian_weight: u32,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Event emitted when a vote is rejected because the proposal's ETA has passed
/// and votes are locked. The ETA (timelock) marks the earliest moment a proposal
/// may be executed; after it, no votes can be added or changed.
#[contracttype]
#[derive(Clone, Debug)]
pub struct VoteLockedEvent {
    pub proposal_id: u64,
    pub guardian: Address,
    pub eta: u64,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Event emitted when a proposal is executed after reaching quorum.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ProposalExecutedEvent {
    pub proposal_id: u64,
    pub kind: ProposalKind,
    pub votes_for: u32,
    pub votes_against: u32,
    pub total_weight: u32,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Event emitted when a proposal is cancelled before execution.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ProposalCancelledEvent {
    pub proposal_id: u64,
    pub reason: String,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracterror(export = false)]
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
    /// Batch size exceeds maximum allowed size.
    BatchTooLarge = 1006,

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
    /// Expiration timestamp is at or before the current ledger time.
    InvalidExpiration = 3008,
    /// Oracle price deviation exceeds configured threshold (circuit breaker).
    OracleDeviationTooHigh = 3009,

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
    /// The scheduled treasury change has not yet reached its effective timestamp.
    TimelockNotElapsed = 4011,
    /// Subscription is not in GracePeriod for a buyout operation.
    NotInGracePeriod = 4013,
    /// Merchant vacation mode is active — charges blocked during vacation window.
    VacationActive = 4014,

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
    /// Usage limits are required but not configured.
    UsageLimitsRequired = 6018,
    /// Coupon code does not exist.
    CouponNotFound = 6011,
    /// Coupon has passed its expiration timestamp.
    CouponExpired = 6012,
    /// Coupon has reached its maximum global redemption count.
    CouponRedemptionLimitReached = 6013,
    /// Coupon has been explicitly revoked by the merchant.
    CouponRevoked = 6014,
    /// A coupon with this code already exists.
    CouponAlreadyExists = 6015,
    /// This subscription already has a coupon bound to it.
    CouponAlreadyApplied = 6016,
    /// Coupon token does not match the subscription's settlement token.
    CouponTokenMismatch = 6017,

    // --- Merchant Config (7000-7099) ---
    /// Fee basis points exceed maximum allowed value.
    InvalidFeeBips = 7001,
    /// Invalid allowed operations bitmask.
    InvalidOperations = 7002,
    /// Charge operation must be allowed for merchant.
    MustAllowChargeOperation = 7003,
    /// Merchant is not approved under whitelist mode.
    MerchantNotApproved = 7004,
    /// Tag is not present in the admin-controlled tag allowlist.
    UnknownMerchantTag = 7005,
    /// The same tag appears more than once in a single `set_merchant_tags` call.
    DuplicateMerchantTag = 7006,
    /// Subscriber has been rate-limited.
    SubscriberRateLimited = 7008,
    /// Self-referral is not allowed.
    SelfReferralNotAllowed = 7009,
    /// Merchant tag would exceed the maximum allowed per merchant.
    MerchantTagLimitExceeded = 7007,

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
    /// Stored schema version does not match the code's expected version; migration rejected.
    SchemaVersionMismatch = 9102,

    // --- Dispute / Chargeback (10000-10099) ---
    /// The requested dispute was not found.
    DisputeNotFound = 10001,
    /// The dispute has already been resolved; no further actions allowed.
    DisputeAlreadyResolved = 10002,
    /// Cannot resolve an unresponded dispute before the dispute window elapses.
    DisputeNotResponded = 10003,
    /// The dispute window has elapsed. Auto-resolution conditions apply.
    DisputeWindowElapsed = 10004,
    /// A dispute is already open for this subscription; double-open rejected.
    DisputeAlreadyOpen = 10005,
    /// The dispute has already been responded to by the admin.
    DisputeAlreadyResponded = 10006,
    /// Dispute resolution would overpay — total disbursed cannot exceed escrowed amount.
    DisputeOverpay = 10007,

    // --- Subscription Transfer (11000-11099) ---
    /// The transfer intent was not found or has expired.
    TransferIntentNotFound = 11001,
    /// The transfer intent has expired.
    TransferIntentExpired = 11002,
    /// The transfer target is invalid.
    InvalidTransferTarget = 11003,
    /// Emergency withdraw state is invalid for this operation.
    EmergencyWithdrawInvalidState = 11004,
    /// Emergency withdraw cooldown is active.
    EmergencyWithdrawCooldownActive = 11005,
    /// Emergency withdraw was not requested.
    EmergencyWithdrawNotRequested = 11006,
    /// Emergency withdraw state has changed.
    EmergencyWithdrawStateChanged = 11007,

    // --- Admin Config Cooldown (12000-12099) ---
    /// A protocol-wide config mutation was attempted within the per-key cooldown window.
    CooldownActive = 12001,

    // --- Delegated Payer (13000-13099) ---
    /// The delegated payer grant was not found.
    DelegatedPayerGrantNotFound = 13001,
    /// The delegated payer grant has expired.
    DelegatedPayerGrantExpired = 13002,
    /// The deposit amount exceeds the grant's max_amount.
    DelegatedPayerAmountExceeded = 13003,
    // --- Auto-Renewal (12000-12099) ---
    /// The renewal window (one billing interval after auto_renew was disabled)
    /// has elapsed; the subscription must be cancelled and recreated to resume billing.
    RenewalWindowClosed = 12002,

    // --- Admin Proposal (14000-14099) ---
    /// No admin proposal exists for claiming.
    ProposalNotFound = 14001,
    /// The admin proposal window has expired.
    ProposalExpired = 14002,
    /// The claimant does not match the proposed new admin.
    InvalidClaimant = 14003,
    /// An admin proposal is already active; cancel it first.
    ProposalAlreadyExists = 14004,
    /// No active proposal to cancel.
    NoActiveProposal = 14005,

    // --- Cancellation Escrow (13000-13099) ---
    /// No cancellation escrow found for this subscription.
    EscrowNotFound = 13004,
    /// The cancellation escrow release window has not elapsed yet.
    EscrowNotReleased = 13005,
}

impl Error {
    /// Returns the numeric code for this error.
    pub const fn to_code(self) -> u32 {
        self as u32
    }
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubscriberCreateWindow {
    pub start_ts: u64,
    pub count: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct RateLimitTrippedEvent {
    pub subscriber: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Event emitted when an admin nonce is consumed by a privileged operation.
///
/// Allows off-chain indexers to track the nonce sequence for each signer/domain
/// pair and detect anomalies such as gaps or unexpected resets.
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
    /// Optional ledger-sequence bound for expiration (mirrors
    /// `Subscription::expires_at_ledger`). `None` means no ledger bound.
    pub expires_at_ledger: Option<u32>,
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
    /// Optional free-trial period in seconds. During this window the subscriber
    /// is not charged for the first billing interval. `0` means no trial.
    pub trial_seconds: u64,
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
    /// Event schema version for backwards-compatible indexer decoding.
    pub schema_version: u32,
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
#[allow(dead_code)]
pub const STMT_FLAG_INTERVAL_CHARGED: u32 = 0b0000_0001;
/// Period had at least one usage charge.
#[allow(dead_code)]
pub const STMT_FLAG_USAGE_CHARGED: u32 = 0b0000_0010;
/// Period had at least one one-off charge.
#[allow(dead_code)]
pub const STMT_FLAG_ONEOFF_CHARGED: u32 = 0b0000_0100;
/// Subscription was cancelled during this period.
#[allow(dead_code)]
pub const STMT_FLAG_CANCELLED: u32 = 0b0000_1000;
/// Subscriber withdrew remaining balance; period is fully settled.
#[allow(dead_code)]
pub const STMT_FLAG_SETTLED: u32 = 0b0001_0000;

// ─────────────────────────────────────────────────────────────────────────────

// ── Coupon types ─────────────────────────────────────────────────────────────

/// Merchant-managed discount coupon.
///
/// Coupons are stored in persistent storage under `DataKey::Coupon(code)` and
/// are identified by a unique symbol code. Subscription binding is tracked
/// separately via `DataKey::SubCoupon(subscription_id)`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Coupon {
    /// Human-readable coupon code (also the storage key).
    pub code: soroban_sdk::Symbol,
    /// Merchant who created and owns this coupon.
    pub merchant: Address,
    /// Settlement token this coupon applies to.
    ///
    /// Must match the subscription's token when `apply_coupon` is called.
    pub token: Address,
    /// Percentage discount in basis points (0..=10_000). 0 = no percent discount.
    ///
    /// Applied first: `discounted = gross * (10_000 - bps) / 10_000`.
    pub percent_off_bps: u32,
    /// Fixed token-unit discount applied after the percentage discount. 0 = disabled.
    ///
    /// The final payable amount is clamped to zero if the combined discount
    /// exceeds the gross charge amount.
    pub fixed_off: i128,
    /// Maximum total subscriptions that may bind this coupon globally. 0 = unlimited.
    pub max_redemptions: u32,
    /// Ledger timestamp after which the coupon can no longer be applied. 0 = no expiry.
    pub expires_at: u64,
    /// Set to `true` when the merchant explicitly revokes this coupon.
    pub revoked: bool,
}

/// Event emitted when a merchant creates a new coupon.
#[contracttype]
#[derive(Clone, Debug)]
pub struct CouponCreatedEvent {
    pub merchant: Address,
    pub code: soroban_sdk::Symbol,
    pub token: Address,
    pub percent_off_bps: u32,
    pub fixed_off: i128,
    pub max_redemptions: u32,
    pub expires_at: u64,
    pub timestamp: u64,
    /// Event schema version for backwards-compatible indexer decoding.
    pub schema_version: u32,
}

/// Event emitted when a merchant revokes a coupon.
#[contracttype]
#[derive(Clone, Debug)]
pub struct CouponRevokedEvent {
    pub merchant: Address,
    pub code: soroban_sdk::Symbol,
    pub timestamp: u64,
    /// Event schema version for backwards-compatible indexer decoding.
    pub schema_version: u32,
}

/// Event emitted when a subscriber binds a coupon to a subscription.
#[contracttype]
#[derive(Clone, Debug)]
pub struct CouponAppliedEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub code: soroban_sdk::Symbol,
    pub timestamp: u64,
    /// Event schema version for backwards-compatible indexer decoding.
    pub schema_version: u32,
}

/// Event emitted when a coupon discount is applied during a charge.
#[contracttype]
#[derive(Clone, Debug)]
pub struct DiscountAppliedEvent {
    pub subscription_id: u32,
    /// Original gross charge amount before discount.
    pub gross_amount: i128,
    /// Amount deducted as discount.
    pub discount_amount: i128,
    /// Payable amount after discount (fed into fee split and merchant credit).
    pub discounted_amount: i128,
    pub coupon_code: soroban_sdk::Symbol,
    pub timestamp: u64,
    /// Event schema version for backwards-compatible indexer decoding.
    pub schema_version: u32,
}

// ─────────────────────────────────────────────────────────────────────────────

/// Pricing strategy used to resolve a cross-currency charge amount.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OracleKind {
    /// One-shot latest price from the configured oracle.
    Spot = 0,
    /// Median price across a sliding time window (`window_secs`).
    Twap = 1,
    /// Deterministic fixed ratio (`fixed_numerator` / `fixed_denominator`); no oracle reads.
    FixedRate = 2,
}

/// Optional oracle pricing configuration for cross-currency plans.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleConfig {
    pub enabled: bool,
    pub oracle: Option<Address>,
    pub max_age_seconds: u64,
    /// Which pricing strategy to use when resolving charge amounts.
    pub kind: OracleKind,
    /// TWAP: length of the sliding observation window in seconds.
    /// Ignored when `kind != Twap`.
    pub window_secs: u64,
    /// FixedRate: numerator of the fixed price ratio (scaled to 10^7).
    /// Ignored when `kind != FixedRate`.
    pub fixed_numerator: u128,
    /// FixedRate: denominator of the fixed price ratio. Must be non-zero.
    /// Ignored when `kind != FixedRate`.
    pub fixed_denominator: u128,
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
    pub kind: OracleKind,
    pub window_secs: u64,
    pub fixed_numerator: u128,
    pub fixed_denominator: u128,
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

/// Two-step admin proposal stored when `propose_admin` is called.
///
/// The proposal must be claimed by `new_admin` before `expires_at`.
#[contracttype]
#[derive(Clone, Debug)]
pub struct AdminProposal {
    pub new_admin: Address,
    pub proposed_at: u64,
    pub expires_at: u64,
}

/// Event emitted when a two-step admin proposal is created.
#[contracttype]
#[derive(Clone, Debug)]
pub struct AdminProposalCreatedEvent {
    pub old_admin: Address,
    pub new_admin: Address,
    pub expires_at: u64,
    pub timestamp: u64,
}

/// Event emitted when a two-step admin proposal is claimed (rotation completes).
#[contracttype]
#[derive(Clone, Debug)]
pub struct AdminProposalClaimedEvent {
    pub old_admin: Address,
    pub new_admin: Address,
    pub timestamp: u64,
}

/// Event emitted when a two-step admin proposal is cancelled by the current admin.
#[contracttype]
#[derive(Clone, Debug)]
pub struct AdminProposalCancelledEvent {
    pub admin: Address,
    pub timestamp: u64,
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

/// Event emitted when a protocol-wide admin config key is mutated (after the
/// cooldown check passes).  `key_label` is the human-readable label (e.g.
/// `"MinTopup"`) and `prev_ts` is the timestamp of the *previous* mutation
/// for that same key (0 if this is the first mutation).
#[contracttype]
#[derive(Clone, Debug)]
pub struct AdminConfigChangedEvent {
    pub key_label: soroban_sdk::String,
    pub prev_ts: u64,
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

/// Short topic for [`RecoveryEvent`].
///
/// This fits Soroban's nine-character `symbol_short!` limit and therefore does
/// not need host-side symbol interning when it is emitted.
pub const TOPIC_RECOVERY: Symbol = symbol_short!("recovery");

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

/// Legacy short topic for [`SubscriptionCreatedEvent`] emitters.
///
/// The longer `"subscription_created"` topic intentionally remains a
/// `Symbol::new` at its emit sites because it exceeds `symbol_short!` capacity.
pub const TOPIC_CREATED: Symbol = symbol_short!("created");

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
    /// Optional ledger-sequence expiration bound. `None` means no ledger bound.
    pub expires_at_ledger: Option<u32>,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Event emitted when a referral rebate is attributed to an inviter.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ReferralAttributedEvent {
    pub subscription_id: u32,
    pub inviter: Address,
    pub subscriber: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Delegated payer grant: authorizes `payer` to deposit into `subscriber`'s vault.
#[contracttype]
#[derive(Clone, Debug)]
pub struct DelegatedPayerGrant {
    pub subscriber: Address,
    pub payer: Address,
    pub expires_at: u64,
    pub max_amount: i128,
}

/// Event emitted when a delegated payer grant is created.
#[contracttype]
#[derive(Clone, Debug)]
pub struct DelegatedPayerGrantedEvent {
    pub subscriber: Address,
    pub payer: Address,
    pub expires_at: u64,
    pub max_amount: i128,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Event emitted when a delegated payer grant is revoked.
#[contracttype]
#[derive(Clone, Debug)]
pub struct DelegatedPayerRevokedEvent {
    pub subscriber: Address,
    pub payer: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Event emitted when a delegated payer deposits funds on behalf of a subscriber.
#[contracttype]
#[derive(Clone, Debug)]
pub struct DelegatedDepositEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub payer: Address,
    pub token: Address,
    pub amount: i128,
    pub new_balance: i128,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Event emitted when a subscriber's active-subscription cap blocks creation (#578).
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriberCapReachedEvent {
    pub subscriber: Address,
    /// The subscriber's active-subscription count at the time of the attempt.
    pub active_count: u32,
    /// The effective cap (override if set, otherwise `DEFAULT_SUBSCRIBER_ACTIVE_CAP`).
    pub cap: u32,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Short topic for [`FundsDepositedEvent`].
pub const TOPIC_DEPOSITED: Symbol = symbol_short!("deposited");

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

/// Short topic for [`SubscriptionChargedEvent`].
pub const TOPIC_CHARGED: Symbol = symbol_short!("charged");

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
    pub salt: soroban_sdk::BytesN<32>,
    pub schema_version: u32,
}

/// Generic, catch-all failure event emitted by [`crate::charge_core::charge_fail`]
/// on every charge error path (topic `"charge_failed_v2"`), regardless of the
/// specific [`Error`] variant. Distinct from [`SubscriptionChargeFailedEvent`],
/// which carries richer balance-shortfall detail for the insufficient-balance
/// case specifically.
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

/// Event emitted when a subscription is cancelled.
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

/// Per-id outcome of a bulk pause/cancel operation.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BulkSubscriptionResult {
    pub subscription_id: u32,
    pub success: bool,
    /// `true` if this id's state actually changed; `false` for idempotent no-ops.
    pub changed: bool,
    /// Numeric error code from the `Error` enum, or `0` on success.
    pub error_code: u32,
}

/// Envelope event summarising the outcome counts of a bulk-pause batch.
#[contracttype]
#[derive(Clone, Debug)]
pub struct BulkPauseEvent {
    pub caller: Address,
    pub requested: u32,
    pub paused: u32,
    pub skipped: u32,
    pub failed: u32,
    pub nonce: u64,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Envelope event summarising the outcome counts of a bulk-cancel batch.
#[contracttype]
#[derive(Clone, Debug)]
pub struct BulkCancelEvent {
    pub caller: Address,
    pub requested: u32,
    pub cancelled: u32,
    pub skipped: u32,
    pub failed: u32,
    pub nonce: u64,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Per-id outcome of a bulk deposit operation.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BulkDepositResult {
    pub subscription_id: u32,
    pub success: bool,
    /// Numeric error code from the `Error` enum, or `0` on success.
    pub error_code: u32,
}

/// Envelope event summarising the outcome counts of a bulk-deposit batch.
#[contracttype]
#[derive(Clone, Debug)]
pub struct BulkDepositEvent {
    pub caller: Address,
    pub requested: u32,
    pub deposited: u32,
    pub failed: u32,
    pub total_amount: i128,
    pub nonce: u64,
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraceBuyoutEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub token: Address,
    pub deposit_amount: i128,
    pub charge_amount: i128,
    pub premium_paid: i128,
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

/// Event emitted when a subscription's ledger-sequence expiration bound is
/// updated by the subscriber or merchant.
///
/// Setting `expires_at_ledger` to `None` clears the bound; setting it to a
/// concrete sequence replaces the previous bound. The wall-clock bound
/// (`expires_at`) is unaffected by this event.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ExpirationLedgerSetEvent {
    pub subscription_id: u32,
    /// New ledger-sequence bound. `None` means the bound has been cleared.
    pub expires_at_ledger: Option<u32>,
    /// Previous ledger-sequence bound, if any. `None` if no prior bound was set.
    pub previous_expires_at_ledger: Option<u32>,
    pub authorizer: Address,
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
    /// Event schema version for backwards-compatible indexer decoding.
    pub schema_version: u32,
}

/// Legacy short topic shared by merchant and subscriber withdrawal events.
pub const TOPIC_WITHDRAWN: Symbol = symbol_short!("withdrawn");

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

/// Audit event emitted when a merchant's address is rotated to a new one.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantAddressRotatedEvent {
    pub admin: Address,
    pub old_merchant: Address,
    pub new_merchant: Address,
    pub subscriptions_updated: u32,
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
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriberEmergencyWithdrawEvent {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub amount: i128,
    pub cooldown_started_at: u64,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Legacy short topic for [`OneOffChargedEvent`].
pub const TOPIC_ONE_OFF_CHARGED: Symbol = symbol_short!("oneoff_ch");

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

/// Legacy short topic for [`LifetimeCapReachedEvent`] in the one-off path.
///
/// Other paths use the longer `"lifetime_cap_reached"` topic, which must keep
/// using `Symbol::new` because it is longer than nine characters.
pub const TOPIC_CAP_REACH: Symbol = symbol_short!("cap_reach");

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

/// Off-chain-signed metadata update payload, applied via `set_metadata_signed`.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SignedMetadataPayload {
    pub subscription_id: u32,
    pub key: String,
    pub value: String,
    /// Must equal the signer's next-expected nonce for the metadata-signed domain.
    pub nonce: u64,
    /// Ledger timestamp after which this payload is no longer valid.
    pub expires_at: u64,
}

/// Event emitted when metadata is updated via an off-chain-signed payload.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MetadataSetSignedEvent {
    pub subscription_id: u32,
    pub key: String,
    pub signer: Address,
    pub nonce: u64,
    pub timestamp: u64,
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

/// Emitted when a merchant registers a new plan template via `register_plan`.
///
/// Carries the full plan definition so indexers can reconstruct the catalogue
/// without additional storage reads.
#[contracttype]
#[derive(Clone, Debug)]
pub struct PlanRegisteredEvent {
    /// Newly-assigned plan ID.
    pub plan_id: u32,
    pub merchant: Address,
    pub token: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    /// Free-trial period in seconds (`0` = no trial).
    pub trial_seconds: u64,
    pub usage_enabled: bool,
    pub lifetime_cap: Option<i128>,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Emitted when a merchant deprecates an existing plan template via `deprecate_plan`.
///
/// Once deprecated the plan can no longer be used to create new subscriptions.
/// Existing subscriptions created from the plan are unaffected.
#[contracttype]
#[derive(Clone, Debug)]
pub struct PlanDeprecatedEvent {
    pub plan_id: u32,
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
    /// Charge silently skipped because `auto_renew` is `false` and the interval has elapsed.
    Skipped = 4,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageLimits {
    pub merchant: Address,
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingTreasuryChange {
    pub new_treasury: Address,
    pub new_fee_bps: u32,
    pub effective_at: u64,
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
pub struct TreasuryChangeQueuedEvent {
    pub admin: Address,
    pub treasury: Address,
    pub fee_bps: u32,
    pub effective_at: u64,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct TreasuryChangeExecutedEvent {
    pub admin: Address,
    pub treasury: Address,
    pub fee_bps: u32,
    pub effective_at: u64,
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MerchantMultiSigConfig {
    pub signers: Vec<Address>,
    pub threshold: u32,
}

/// Merchant vacation window: when active (current time within [start_ts, end_ts]),
/// all charges to this merchant's subscriptions are blocked with `Error::VacationActive`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MerchantVacation {
    /// Start of the vacation window (ledger timestamp, seconds).
    pub start_ts: u64,
    /// End of the vacation window (ledger timestamp, seconds). Must be > start_ts.
    pub end_ts: u64,
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

/// Emitted when a merchant enters vacation mode, auto-pausing all subscriptions.
#[contracttype]
#[derive(Clone, Debug)]
pub struct VacationStartedEvent {
    pub merchant: Address,
    pub start_ts: u64,
    pub end_ts: u64,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Emitted when a merchant exits vacation mode before the scheduled end time.
#[contracttype]
#[derive(Clone, Debug)]
pub struct VacationEndedEvent {
    pub merchant: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantWhitelistModeEvent {
    pub enabled: bool,
    pub admin: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantApprovedEvent {
    pub merchant: Address,
    pub admin: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantRevokedEvent {
    pub merchant: Address,
    pub admin: Address,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Emitted when the admin sets or clears a per-merchant protocol fee override.
///
/// `fee_bps` is `Some(value)` when an override is set, and `None` when it is cleared.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantFeeOverrideSetEvent {
    /// Merchant whose override was changed.
    pub merchant: Address,
    /// Admin who authorized the change.
    pub admin: Address,
    /// The new override value in basis points, or `None` when cleared.
    pub fee_bps: Option<u32>,
    pub timestamp: u64,
    /// Event schema version for backwards-compatible indexer decoding.
    pub schema_version: u32,
}

/// Emitted when the admin replaces the global merchant tag allowlist
/// (`merchant::set_tag_allowlist`).
#[contracttype]
#[derive(Clone, Debug)]
pub struct TagAllowlistUpdatedEvent {
    pub admin: Address,
    pub tags: Vec<Symbol>,
    pub timestamp: u64,
    pub schema_version: u32,
}

/// Emitted when the admin sets (or clears, with an empty `tags` vector) a
/// merchant's compliance-category tags (`merchant::set_merchant_tags`).
#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantTagsUpdatedEvent {
    pub merchant: Address,
    pub admin: Address,
    pub tags: Vec<Symbol>,
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




// ════════════════════════════════════════════════════════════════════
// Stub types — pre-existing references needing definitions
// ════════════════════════════════════════════════════════════════════

#[contracttype]
#[derive(Clone, Debug)]
pub struct AcceptedToken { pub token: Address, pub decimals: u32 }

#[contracttype]
#[derive(Clone, Debug)]
pub struct FeeConvertedEvent { pub subscription_id: u32, pub source_token: Address, pub target_token: Address, pub original_fee_amount: i128, pub converted_fee_amount: i128, pub rate: u128, pub timestamp: u64, pub schema_version: u32 }

#[contracttype]
#[derive(Clone, Debug)]
pub struct FeeTokenConfiguredEvent { pub admin: Address, pub fee_token: Option<Address>, pub timestamp: u64, pub schema_version: u32 }

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionAutoPausedEvent { pub subscription_id: u32, pub consecutive_failures: u32, pub threshold: u32, pub timestamp: u64, pub schema_version: u32 }

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionPausedEvent { pub subscription_id: u32, pub authorizer: Address, pub timestamp: u64, pub schema_version: u32 }

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubAccountCreatedEvent { pub merchant: Address, pub label: Symbol, pub timestamp: u64, pub schema_version: u32 }

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubAccountWithdrawEvent { pub merchant: Address, pub label: Symbol, pub token: Address, pub amount: i128, pub remaining_balance: i128, pub timestamp: u64, pub schema_version: u32 }

#[contracttype]
#[derive(Clone, Debug)]
pub struct CancellationEscrow { pub subscription_id: u32, pub subscriber: Address, pub merchant: Address, pub token: Address, pub amount: i128, pub opened_at: u64, pub releases_at: u64, pub released_at: u64 }

#[contracttype]
#[derive(Clone, Debug)]
pub struct CancellationEscrowOpenedEvent { pub subscription_id: u32, pub subscriber: Address, pub merchant: Address, pub token: Address, pub amount: i128, pub released_at: u64, pub timestamp: u64, pub schema_version: u32 }

#[contracttype]
#[derive(Clone, Debug)]
pub struct CancellationEscrowDisputedEvent { pub subscription_id: u32, pub merchant: Address, pub dispute_id: u64, pub amount: i128, pub timestamp: u64, pub schema_version: u32 }

#[contracttype]
#[derive(Clone, Debug)]
pub struct CancellationEscrowReleasedEvent { pub subscription_id: u32, pub subscriber: Address, pub amount: i128, pub timestamp: u64, pub schema_version: u32 }

#[contracttype]
#[derive(Clone, Debug)]
pub struct TransferIntent { pub subscription_id: u32, pub from: Address, pub to: Address, pub created_at: u64, pub expires_at: u64 }

#[contracttype]
#[derive(Clone, Debug)]
pub struct TransferIntentCreatedEvent { pub subscription_id: u32, pub from: Address, pub to: Address, pub expires_at: u64, pub timestamp: u64, pub schema_version: u32 }

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionTransferredEvent { pub subscription_id: u32, pub from: Address, pub to: Address, pub timestamp: u64, pub schema_version: u32 }

#[contracttype]
#[derive(Clone, Debug)]
pub struct TransferVetoedEvent { pub subscription_id: u32, pub merchant: Address, pub timestamp: u64, pub schema_version: u32 }

#[contracttype]
#[derive(Clone, Debug)]
pub struct OraclePriceHistoryMeta { pub token: Address, pub cursor: u32, pub count: u32 }

pub fn normalize_amount(_env: &Env, _token: &Address, amount: i128) -> Result<i128, Error> { Ok(amount) }

pub const CANCELLATION_ESCROW_WINDOW_SECS: u64 = 14 * 24 * 60 * 60;
