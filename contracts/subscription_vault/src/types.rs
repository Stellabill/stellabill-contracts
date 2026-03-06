//! Contract types: errors, subscription data structures, and event types.
//!
//! Kept in a separate module to reduce merge conflicts when editing state machine
//! or contract entrypoints.

use soroban_sdk::{contracterror, contracttype, Address};

pub const BILLING_SNAPSHOT_FLAG_CLOSED: u32 = 1 << 0;
pub const BILLING_SNAPSHOT_FLAG_INTERVAL_CHARGED: u32 = 1 << 1;
pub const BILLING_SNAPSHOT_FLAG_USAGE_CHARGED: u32 = 1 << 2;
pub const BILLING_SNAPSHOT_FLAG_EMPTY_PERIOD: u32 = 1 << 3;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    MerchantSubs(Address),
    MerchantBlocklist(Address, Address),
    EmergencyStop,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InsufficientBalanceError {
    pub available: i128,
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
    InvalidRecoveryAmount = 405,
    SubscriptionExpired = 410,
    IntervalNotElapsed = 1001,
    NotActive = 1002,
    InsufficientBalance = 1003,
    UsageNotEnabled = 1004,
    InsufficientPrepaidBalance = 1005,
    InvalidAmount = 1006,
    Replay = 1007,
    EmergencyStopActive = 1009,
    Underflow = 1010,
    RecoveryNotAllowed = 1011,
    Overflow = 1012,
    NotInitialized = 1013,
    InvalidExportLimit = 1014,
    InvalidInput = 1015,
    Reentrancy = 1016,
    UsageCapExceeded = 1018,
    RateLimitExceeded = 1019,
    InvalidFeeBps = 1020,
    TreasuryNotConfigured = 1021,
    SubscriberBlocked = 1022,
    /// Lifetime charge cap has been reached; no further charges are allowed.
    LifetimeCapReached = 1017,
    /// Contract is already initialized; init may only be called once.
    AlreadyInitialized = 1023,
    /// The contract has allocated the maximum number of subscriptions.
    SubscriptionLimitReached = 429,
}

impl Error {
    pub const fn to_code(self) -> u32 {
        self as u32
    }
}

/// Result of charging one subscription in a batch.
#[contracttype]
#[derive(Clone, Debug)]
pub struct BatchChargeResult {
    pub success: bool,
    pub error_code: u32,
}

/// Result of a batch merchant withdrawal operation.
#[contracttype]
#[derive(Clone, Debug)]
pub struct BatchWithdrawResult {
    pub success: bool,
    pub error_code: u32,
}

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

#[contracttype]
#[derive(Clone, Debug)]
pub struct Subscription {
    pub subscriber: Address,
    pub merchant: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub last_payment_timestamp: u64,
    pub status: SubscriptionStatus,
    pub prepaid_balance: i128,
    pub usage_enabled: bool,
    pub expiration: Option<u64>,
    pub billing_anchor_timestamp: u64,
    pub current_period_index: u32,
    pub current_period_usage_units: i128,
    pub usage_cap_units: Option<i128>,
    pub usage_rate_limit_max_calls: Option<u32>,
    pub usage_rate_window_secs: u64,
    pub lifetime_cap: Option<i128>,
    pub lifetime_charged: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BillingPeriodSnapshot {
    pub subscription_id: u32,
    pub period_index: u32,
    pub period_start_timestamp: u64,
    pub period_end_timestamp: u64,
    pub total_amount_charged: i128,
    pub total_usage_units: i128,
    pub status_flags: u32,
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
    pub amount: i128,
    pub interval_seconds: u64,
    pub last_payment_timestamp: u64,
    pub status: SubscriptionStatus,
    pub prepaid_balance: i128,
    pub usage_enabled: bool,
    pub lifetime_cap: Option<i128>,
    pub lifetime_charged: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MigrationExportEvent {
    pub admin: Address,
    pub start_id: u32,
    pub limit: u32,
    pub exported: u32,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct PlanTemplate {
    pub merchant: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub usage_enabled: bool,
    pub lifetime_cap: Option<i128>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NextChargeInfo {
    pub next_charge_timestamp: u64,
    pub is_charge_expected: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryReason {
    AccidentalTransfer = 0,
    DeprecatedFlow = 1,
    UnreachableSubscriber = 2,
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
#[derive(Clone, Debug)]
pub struct EmergencyStopEnabledEvent {
    pub admin: Address,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct EmergencyStopDisabledEvent {
    pub admin: Address,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct RecoveryEvent {
    pub admin: Address,
    pub recipient: Address,
    pub amount: i128,
    pub reason: RecoveryReason,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionCreatedEvent {
    pub subscriber: Address,
    pub merchant: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub lifetime_cap: Option<i128>,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct UsageCapReachedEvent {
    pub subscription_id: u32,
    pub period_index: u32,
    pub cap_units: i128,
    pub attempted_units: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct ProtocolFeeSkimmedEvent {
    pub subscription_id: u32,
    pub merchant: Address,
    pub treasury: Address,
    pub gross_amount: i128,
    pub fee_amount: i128,
    pub net_amount: i128,
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
    pub lifetime_charged: i128,
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

#[contracttype]
#[derive(Clone, Debug)]
pub struct OneOffChargedEvent {
    pub subscription_id: u32,
    pub merchant: Address,
    pub amount: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct LifetimeCapReachedEvent {
    pub subscription_id: u32,
    pub lifetime_cap: i128,
    pub lifetime_charged: i128,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MerchantBlocklistUpdatedEvent {
    pub merchant: Address,
    pub subscriber: Address,
    pub is_blocked: bool,
    pub timestamp: u64,
}
