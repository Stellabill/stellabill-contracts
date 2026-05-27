#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol};

#[contracterror]
#[repr(u32)]
pub enum Error {
    NotFound = 404,
    Unauthorized = 401,
    BelowMinimumTopup = 402,
    SubscriptionExpired = 410,
    NotActive = 1002,
    UsageNotEnabled = 1004,
    InsufficientPrepaidBalance = 1005,
    InvalidAmount = 1006,
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
    /// Initialize the contract with token, admin, and minimum top-up requirements.
    pub fn init(env: Env, token: Address, admin: Address, min_topup: i128) -> Result<(), Error> {
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "token"), &token);
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "admin"), &admin);
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "min_topup"), &min_topup);
        Ok(())
    }

    /// Update the minimum top-up threshold. Only callable by admin.
    pub fn set_min_topup(env: Env, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "admin"))
            .ok_or(Error::NotFound)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "min_topup"), &min_topup);
        Ok(())
    }

    /// Get the current minimum top-up threshold.
    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "min_topup"))
            .ok_or(Error::NotFound)
    }

    /// Create a new subscription agreement.
    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        expiration: Option<u64>,
    ) -> Result<u32, Error> {
        subscriber.require_auth();
        let sub = Subscription {
            subscriber: subscriber.clone(),
            merchant,
            amount,
            interval_seconds,
            last_payment_timestamp: env.ledger().timestamp(),
            status: SubscriptionStatus::Active,
            prepaid_balance: 0i128,
            usage_enabled,
            expiration,
        };
        let id = Self::_next_id(&env);
        env.storage().instance().set(&id, &sub);
        Ok(id)
    }

    /// Subscriber deposits funds to increase prepaid balance.
    pub fn deposit_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
    ) -> Result<(), Error> {
        subscriber.require_auth();
        let min_topup: i128 = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "min_topup"))
            .ok_or(Error::NotFound)?;
        if amount < min_topup {
            return Err(Error::BelowMinimumTopup);
        }
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;
        sub.prepaid_balance += amount;
        if sub.status == SubscriptionStatus::InsufficientBalance && sub.prepaid_balance > 0 {
            sub.status = SubscriptionStatus::Active;
        }
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    /// Record metered usage units for the subscription (Admin Authorized).
    pub fn report_usage(
        env: Env,
        admin: Address,
        subscription_id: u32,
        units: i128,
    ) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "admin"))
            .ok_or(Error::NotFound)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        if units < 0 {
            return Err(Error::InvalidAmount);
        }
        let sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;
        if sub.status != SubscriptionStatus::Active {
            return Err(Error::NotActive);
        }
        if !sub.usage_enabled {
            return Err(Error::UsageNotEnabled);
        }

        let key = (Symbol::new(&env, "usage"), subscription_id);
        let current_usage: i128 = env.storage().instance().get(&key).unwrap_or(0);
        env.storage().instance().set(&key, &(current_usage + units));
        Ok(())
    }

    /// Billing execution entry point.
    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;
        if sub.status != SubscriptionStatus::Active {
            return Err(Error::NotActive);
        }
        if let Some(exp_ts) = sub.expiration {
            if env.ledger().timestamp() >= exp_ts {
                return Err(Error::SubscriptionExpired);
            }
        }

        let charge_amount = if sub.usage_enabled {
            let key = (Symbol::new(&env, "usage"), subscription_id);
            let units: i128 = env.storage().instance().get(&key).unwrap_or(0);
            let total_charge = units * sub.amount;
            env.storage().instance().set(&key, &0i128); // Reset usage accumulated
            total_charge
        } else {
            sub.amount
        };

        if charge_amount > 0 {
            if sub.prepaid_balance < charge_amount {
                return Err(Error::InsufficientPrepaidBalance);
            }
            sub.prepaid_balance -= charge_amount;
            if sub.prepaid_balance == 0 {
                sub.status = SubscriptionStatus::InsufficientBalance;
            }
        }

        sub.last_payment_timestamp = env.ledger().timestamp();
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    /// Standalone usage charge endpoint for direct off-chain sync updates.
    pub fn charge_usage(env: Env, subscription_id: u32, usage_amount: i128) -> Result<(), Error> {
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;
        if sub.status != SubscriptionStatus::Active {
            return Err(Error::NotActive);
        }
        if !sub.usage_enabled {
            return Err(Error::UsageNotEnabled);
        }
        if usage_amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        if sub.prepaid_balance < usage_amount {
            return Err(Error::InsufficientPrepaidBalance);
        }

        sub.prepaid_balance -= usage_amount;
        if sub.prepaid_balance == 0 {
            sub.status = SubscriptionStatus::InsufficientBalance;
        }
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;
        sub.status = SubscriptionStatus::Cancelled;
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();
        let mut sub: Subscription = env
            .storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)?;
        sub.status = SubscriptionStatus::Paused;
        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    pub fn withdraw_merchant_funds(
        _env: Env,
        merchant: Address,
        _amount: i128,
    ) -> Result<(), Error> {
        merchant.require_auth();
        Ok(())
    }

    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        env.storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)
    }

    fn _next_id(env: &Env) -> u32 {
        let key = Symbol::new(env, "next_id");
        let id: u32 = env.storage().instance().get(&key).unwrap_or(0);
        env.storage().instance().set(&key, &(id + 1));
        id
    }

    /// Initialize the contract with admin, token, and minimum topup amount.
    ///
    /// # Security
    /// This function can only be called once. The admin key serves as a sentinel
    /// to detect whether initialization has already occurred. Any attempt to
    /// re-initialize will return `Error::AlreadyInitialized` and leave the existing
    /// configuration unchanged.
    ///
    /// # Arguments
    /// * `admin` - The admin address that will control the contract
    /// * `token` - The token address used for payments
    /// * `min_topup` - The minimum topup amount in token units
    ///
    /// # Errors
    /// * `Error::AlreadyInitialized` - If the contract has already been initialized
    pub fn init(env: Env, admin: Address, token: Address, min_topup: i128) -> Result<(), Error> {
        // Check if already initialized by verifying the admin key exists
        if env.storage().instance().has(&DataKey::Admin.to_symbol()) {
            return Err(Error::AlreadyInitialized);
        }

        // Store initial configuration
        env.storage()
            .instance()
            .set(&DataKey::Admin.to_symbol(), &admin);
        env.storage()
            .instance()
            .set(&DataKey::Token.to_symbol(), &token);
        env.storage()
            .instance()
            .set(&DataKey::MinTopup.to_symbol(), &min_topup);

        Ok(())
    }

    /// Get the current admin address.
    pub fn get_admin(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::Admin.to_symbol())
    }

    /// Get the token address.
    pub fn get_token(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::Token.to_symbol())
    }

    /// Get the minimum topup amount.
    pub fn get_min_topup(env: Env) -> Option<i128> {
        env.storage()
            .instance()
            .get(&DataKey::MinTopup.to_symbol())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Address, Env}; // Brings the .generate() method into scope

    fn setup_test_env() -> (
        Env,
        SubscriptionVaultClient<'static>,
        Address,
        Address,
        Address,
    ) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);

        client.init(&token, &admin, &10i128);
        (env, client, admin, subscriber, merchant)
    }

    #[test]
    fn test_fixed_interval_charging() {
        let (_env, client, _admin, subscriber, merchant) = setup_test_env();
        let sub_id =
            client.create_subscription(&subscriber, &merchant, &100i128, &3600, &false, &None);
        client.deposit_funds(&sub_id, &subscriber, &500i128);

        client.charge_subscription(&sub_id);
        let sub = client.get_subscription(&sub_id);
        assert_eq!(sub.prepaid_balance, 400);
        assert_eq!(sub.status, SubscriptionStatus::Active);
    }

    #[test]
    fn test_report_usage_and_charging_path() {
        let (_env, client, admin, subscriber, merchant) = setup_test_env();
        let sub_id =
            client.create_subscription(&subscriber, &merchant, &5i128, &3600, &true, &None);
        client.deposit_funds(&sub_id, &subscriber, &500i128);

        client.report_usage(&admin, &sub_id, &10i128);
        client.charge_subscription(&sub_id);

        let sub = client.get_subscription(&sub_id);
        assert_eq!(sub.prepaid_balance, 450);
    }

    #[test]
    fn test_zero_usage_charge() {
        let (_env, client, _admin, subscriber, merchant) = setup_test_env();
        let sub_id =
            client.create_subscription(&subscriber, &merchant, &5i128, &3600, &true, &None);
        client.deposit_funds(&sub_id, &subscriber, &100i128);

        client.charge_subscription(&sub_id);
        let sub = client.get_subscription(&sub_id);
        assert_eq!(sub.prepaid_balance, 100);
    }

    #[test]
    fn test_usage_charge_exceeding_prepaid_balance() {
        let (_env, client, admin, subscriber, merchant) = setup_test_env();
        let sub_id =
            client.create_subscription(&subscriber, &merchant, &10i128, &3600, &true, &None);
        client.deposit_funds(&sub_id, &subscriber, &50i128);

        client.report_usage(&admin, &sub_id, &10i128);
        let res = client.try_charge_subscription(&sub_id);
        assert!(res.is_err());
    }

    #[test]
    fn test_charge_usage_direct() {
        let (_env, client, _admin, subscriber, merchant) = setup_test_env();
        let sub_id =
            client.create_subscription(&subscriber, &merchant, &5i128, &3600, &true, &None);
        client.deposit_funds(&sub_id, &subscriber, &100i128);

        client.charge_usage(&sub_id, &40i128);
        let sub = client.get_subscription(&sub_id);
        assert_eq!(sub.prepaid_balance, 60);
    }

    #[test]
    fn test_min_topup_and_management() {
        let (_env, client, admin, subscriber, merchant) = setup_test_env();
        let sub_id =
            client.create_subscription(&subscriber, &merchant, &100i128, &3600, &false, &None);

        let low_deposit = client.try_deposit_funds(&sub_id, &subscriber, &5i128);
        assert!(low_deposit.is_err());

        client.set_min_topup(&admin, &50i128);
        assert_eq!(client.get_min_topup(), 50);
    }

    #[test]
    fn test_pause_and_cancel() {
        let (_env, client, _admin, subscriber, merchant) = setup_test_env();
        let sub_id =
            client.create_subscription(&subscriber, &merchant, &100i128, &3600, &false, &None);

        client.pause_subscription(&sub_id, &subscriber);
        let sub_paused = client.get_subscription(&sub_id);
        assert_eq!(sub_paused.status, SubscriptionStatus::Paused);

        client.cancel_subscription(&sub_id, &subscriber);
        let sub_cancelled = client.get_subscription(&sub_id);
        assert_eq!(sub_cancelled.status, SubscriptionStatus::Cancelled);
    }
}
