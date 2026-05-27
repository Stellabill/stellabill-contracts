use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol, Vec};

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol};

pub const MAX_SUBSCRIPTION_ID: u32 = u32::MAX;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    NotFound = 404,
    InvalidArgument = 3,
    AlreadyInitialized = 4008,
    SubscriptionExpired = 410,
    SubscriptionLimitReached = 429,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscriptionStatus {
    Active = 0,
    Paused = 1,
    Cancelled = 2,
    InsufficientBalance = 3,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Subscription {
    pub subscriber: Address,
    pub token: Address,
    pub merchant: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub last_payment_timestamp: u64,
    pub status: SubscriptionStatus,
    pub prepaid_balance: i128,
    pub usage_enabled: bool,
    pub expires_at: Option<u64>,
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
    /// Optional Unix timestamp (seconds) after which no more charges are allowed.
    pub expiration: Option<u64>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchChargeResult {
    pub success: bool,
    pub error_code: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubscriptionStatus {
    Active = 0,
    Paused = 1,
    Cancelled = 2,
    InsufficientBalance = 3,
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
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
}

/// Storage keys for instance data.
#[derive(Clone)]
pub enum DataKey {
    Admin = 0,
    Token = 1,
    MinTopup = 2,
}

impl DataKey {
    pub fn to_symbol(&self) -> Symbol {
        match self {
            DataKey::Admin => symbol_short!("admin"),
            DataKey::Token => symbol_short!("token"),
            DataKey::MinTopup => symbol_short!("min_topup"),
        }
    }
}

<<<<<<< HEAD
use soroban_sdk::{contract, contractimpl, Address, Env, String, Symbol, Vec};

// ── Re-exports ────────────────────────────────────────────────────────────────
pub use blocklist::{BlocklistAddedEvent, BlocklistEntry, BlocklistRemovedEvent};
pub use queries::{
    compute_next_charge_info, generate_reconciliation_proof, get_contract_reconciliation_summary,
    get_token_reconciliation, query_prepaid_balances_paginated, MAX_PREPAID_SCAN_DEPTH, MAX_SCAN_DEPTH,
    MAX_SUBSCRIPTION_LIST_PAGE, MAX_TOKEN_SUMMARIES_PER_PAGE,
};
pub use state_machine::{can_transition, get_allowed_transitions, validate_status_transition};
pub use types::{
    AcceptedToken, AccruedTotals, AdminRotatedEvent, BatchChargeResult, BatchWithdrawResult,
    BillingChargeKind, BillingCompactedEvent, BillingCompactionSummary, BillingPeriodSnapshot,
    BillingRetentionConfig, BillingStatement, BillingStatementAggregate, BillingStatementsPage,
    CapInfo, ChargeExecutionResult, ContractSnapshot, DataKey, EmergencyStopDisabledEvent,
    EmergencyStopEnabledEvent, Error, FundsDepositedEvent, LifetimeCapReachedEvent, MerchantConfig,
    MerchantConfigInitializedEvent, MerchantConfigUpdatedEvent, MerchantPausedEvent,
    MerchantUnpausedEvent, MerchantWithdrawalEvent, MetadataDeletedEvent,
    MetadataSetEvent, MigrationExportEvent, NextChargeInfo, OneOffChargedEvent, OracleConfig,
    OraclePrice, PartialRefundEvent, PlanTemplate, PlanTemplateUpdatedEvent,
    ProtocolFeeChargedEvent, ProtocolFeeConfiguredEvent, RecoveryEvent, RecoveryReason,
    Subscription, SubscriptionCancelledEvent, SubscriptionChargeFailedEvent,
    SubscriptionChargedEvent, SubscriptionCreatedEvent, SubscriptionMigratedEvent,
    SubscriptionPausedEvent, SubscriptionRecoveryReadyEvent, SubscriptionResumedEvent,
    SubscriptionStatus, SubscriptionSummary, SubscriberWithdrawalEvent,
    SubscriptionArchivedEvent, SubscriptionExpiredEvent,
    TokenEarnings, TokenReconciliationSnapshot, UsageChargeResult, UsageLimits, UsageState, UsageStatementEvent,
    MAX_METADATA_KEYS, MAX_METADATA_KEY_LENGTH, MAX_METADATA_VALUE_LENGTH,
    SNAPSHOT_FLAG_CLOSED, SNAPSHOT_FLAG_EMPTY, SNAPSHOT_FLAG_INTERVAL_CHARGED,
    SNAPSHOT_FLAG_USAGE_CHARGED,
    OP_CHARGE, OP_WITHDRAW, OP_REFUND, OP_BILLING_PAUSE, OP_AUTO_RENEWAL,
    DEFAULT_ALLOWED_OPS,
    GlobalCapDefaultUpdatedEvent, LifetimeCapUpdatedEvent, MerchantCapDefaultUpdatedEvent,
    OperatorRemovedEvent, OperatorSetEvent,
    PrepaidQueryRequest, PrepaidQueryResult, ReconciliationProof, ReconciliationSummaryPage,
    TokenLiabilities,
};

/// Maximum subscription ID this contract will ever allocate.
///
/// When the counter reaches this value [`SubscriptionVault::create_subscription`]
/// returns [`Error::SubscriptionLimitReached`] instead of wrapping or panicking.
/// This sentinel prevents u32 overflow across contract upgrades.
pub const MAX_SUBSCRIPTION_ID: u32 = u32::MAX;

/// On-chain storage schema version.
///
/// Bump this constant (and add a migration path in [`migration`]) whenever
/// storage key shapes or type layouts change in an incompatible way.
const STORAGE_VERSION: u32 = 2;

/// Hard upper bound on the number of subscriptions that may be exported in a
/// single [`SubscriptionVault::export_subscription_summaries`] call.
const MAX_EXPORT_LIMIT: u32 = 100;

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Ensures the given `admin` is the authorized account.
///
/// This checks that the caller has signed the transaction and matches
/// the admin stored in contract storage. If the address doesn’t match,
/// it returns `Error::Unauthorized`.
fn require_admin_auth(env: &Env, admin: &Address) -> Result<(), Error> {
    admin::require_admin_auth(env, admin)
}

/// Read the emergency-stop flag from instance storage.
///
/// Returns `false` when the key has never been written (safe default: not stopped).
fn get_emergency_stop(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::EmergencyStop)
        .unwrap_or(false)
}

/// Guard all mutating entry-points against an active emergency stop.
///
/// Returns [`Error::EmergencyStopActive`] immediately so the transaction aborts
/// before any state is modified.
fn require_not_emergency_stop(env: &Env) -> Result<(), Error> {
    if get_emergency_stop(env) {
        return Err(Error::EmergencyStopActive);
    }
    Ok(())
}

// ── Contract ──────────────────────────────────────────────────────────────────

/// Main contract for handling prepaid subscription billing on Stellar.
///
/// See the crate-level docs for a full overview of how the system works.
=======
>>>>>>> origin/main
#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    pub fn init(env: Env, admin: Address, token: Address, min_topup: i128) -> Result<(), Error> {
        if env.storage().instance().has(&Symbol::new(&env, "admin")) {
            return Err(Error::AlreadyInitialized);
        }
        env.storage().instance().set(&Symbol::new(&env, "admin"), &admin);
        env.storage().instance().set(&Symbol::new(&env, "token"), &token);
        env.storage().instance().set(&Symbol::new(&env, "min_topup"), &min_topup);
        Ok(())
    }

    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        expires_at: Option<u64>,
    ) -> Result<u32, Error> {
        subscriber.require_auth();

        if amount <= 0 {
            return Err(Error::InvalidArgument);
        }
        if interval_seconds == 0 {
            return Err(Error::InvalidArgument);
        }
        if let Some(ts) = expires_at {
            if ts <= env.ledger().timestamp() {
                return Err(Error::InvalidArgument);
            }
        }

        let token: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "token"))
            .ok_or(Error::NotFound)?;

        let id = Self::_next_id(&env)?;
        let sub = Subscription {
            subscriber,
            token,
            merchant,
            amount,
            interval_seconds,
            last_payment_timestamp: env.ledger().timestamp(),
            status: SubscriptionStatus::Active,
            prepaid_balance: 0,
            usage_enabled,
            expires_at,
        };
        env.storage().instance().set(&id, &sub);
        Ok(id)
    }

    pub fn charge_subscription(env: Env, id: u32) -> Result<(), Error> {
        let sub: Subscription = env
            .storage()
            .instance()
            .get(&id)
            .ok_or(Error::NotFound)?;

        if let Some(exp_ts) = sub.expires_at {
            if env.ledger().timestamp() >= exp_ts {
                return Err(Error::SubscriptionExpired);
            }
        }

        Ok(())
    }

    pub fn get_subscription(env: Env, id: u32) -> Result<Subscription, Error> {
        env.storage()
            .instance()
            .get(&id)
            .ok_or(Error::NotFound)
    }

    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "min_topup"))
            .ok_or(Error::NotFound)
    }

    pub fn get_subscription_count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "next_id"))
            .unwrap_or(0)
    }

    pub fn version(_env: Env) -> u32 {
        0
    }

    fn _next_id(env: &Env) -> Result<u32, Error> {
        let key = Symbol::new(env, "next_id");
        let current: u32 = env.storage().instance().get(&key).unwrap_or(0);
        if current == MAX_SUBSCRIPTION_ID {
            return Err(Error::SubscriptionLimitReached);
        }
        env.storage().instance().set(&key, &(current + 1));
        Ok(current)
    }
}

#[cfg(test)]
mod test;
