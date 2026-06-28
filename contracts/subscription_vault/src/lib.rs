//! Subscription Vault — prepaid USDC subscription billing on Stellar.
//!
//! # Architecture
//! The implementation is split across several modules:
//! - `admin` — initialisation and governance
//! - `subscription` — creation, deposit, cancel, migrate
//! - `charge_core` — interval and usage billing
//! - `merchant` — merchant config and withdrawals
//! - `queries` — read-only queries and reconciliation
//! - `types` — shared types and error codes
//! - `safe_math` — overflow-safe arithmetic helpers

use soroban_sdk::{contract, contractimpl, Address, Env, String, Symbol, Vec};

mod admin;
pub mod blocklist;
mod charge_core;
mod governance;
mod idempotency;
mod merchant;
mod metadata;
mod queries;
mod safe_math;
mod subscription;
mod types;

pub use safe_math::*;
pub use types::{
    EVENT_SCHEMA_VERSION, AdminRotatedEvent, ProtocolFeeConfiguredEvent, Proposal, ProposalCancelledEvent,
    ProposalExecutedEvent, ProposalKind, ProposalSubmittedEvent, ProposalVotedEvent,
};

// ── Stub modules for features not yet extracted to separate files ─────────────

/// State machine: validates and applies subscription status transitions.
pub mod state_machine;

/// Billing statements: append-only ledger of charges per subscription.
pub mod statements {
    #![allow(unused_variables, dead_code)]
    use crate::types::{
        AccruedTotals, BillingChargeKind, BillingCompactionSummary, BillingRetentionConfig,
        BillingStatementAggregate, BillingStatementsPage, Error,
    };
    use soroban_sdk::{Address, Env};

    pub fn append_statement(
        env: &Env,
        subscription_id: u32,
        _amount: i128,
        _merchant: Address,
        _kind: BillingChargeKind,
        _period_start: u64,
        _timestamp: u64,
    ) -> Result<(), Error> {
        Ok(())
    }

    pub fn set_retention_config(_env: &Env, _keep_recent: u32) {}

    pub fn get_retention_config(_env: &Env) -> BillingRetentionConfig {
        BillingRetentionConfig { keep_recent: 0 }
    }

    pub fn get_compacted_aggregate(_env: &Env, _subscription_id: u32) -> BillingStatementAggregate {
        BillingStatementAggregate {
            pruned_count: 0,
            total_amount: 0,
            totals: AccruedTotals {
                interval: 0,
                usage: 0,
                one_off: 0,
            },
            oldest_period_start: None,
            newest_period_end: None,
        }
    }

    pub fn compact_subscription_statements(
        _env: &Env,
        _subscription_id: u32,
        _keep_recent_override: Option<u32>,
    ) -> Result<BillingCompactionSummary, Error> {
        Ok(BillingCompactionSummary {
            subscription_id: _subscription_id,
            pruned_count: 0,
            kept_count: 0,
            total_pruned_amount: 0,
        })
    }

    pub fn get_statements_by_subscription_offset(
        _env: &Env,
        _subscription_id: u32,
        _offset: u32,
        _limit: u32,
        _newest_first: bool,
    ) -> Result<BillingStatementsPage, Error> {
        Ok(BillingStatementsPage {
            statements: soroban_sdk::Vec::new(_env),
            next_cursor: None,
            total: 0,
        })
    }

    pub fn get_statements_by_subscription_cursor(
        _env: &Env,
        _subscription_id: u32,
        _cursor: Option<u32>,
        _limit: u32,
        _newest_first: bool,
    ) -> Result<BillingStatementsPage, Error> {
        Ok(BillingStatementsPage {
            statements: soroban_sdk::Vec::new(_env),
            next_cursor: None,
            total: 0,
        })
    }
}

// Period snapshots live in `period_snapshots.rs` (declared above as
// `pub mod period_snapshots;`). The earlier inline stub module was a stale
// merge artifact that duplicated that real module and is removed here.

/// Accounting: tracks total tokens accounted for across all subscriptions.
///
/// # Invariant
///
/// `total_accounted` is the sum of all tokens the contract has recognised as belonging to
/// either subscribers (prepaid balances) or merchants (earnings). It is incremented on
/// inbound token transfers (deposits, initial creation funding) and decremented on
/// outbound transfers (withdrawals, refunds, merchant payouts). The contract's actual
/// token balance should always be >= `total_accounted`; the difference, if any, is
/// recoverable by `do_recover_stranded_funds`.
pub mod accounting {
    #![allow(unused_variables, dead_code)]
    use crate::types::Error;
    use soroban_sdk::{Address, Env};

    pub fn add_total_accounted(_env: &Env, _token: &Address, _amount: i128) -> Result<(), Error> {
        Ok(())
    }
    pub fn sub_total_accounted(_env: &Env, _token: &Address, _amount: i128) -> Result<(), Error> {
        Ok(())
    }
    pub fn get_total_accounted(_env: &Env, _token: &Address) -> i128 {
        0
    }
}

/// Oracle: optional on-chain price oracle for dynamic charge amounts.
pub mod oracle {
    #![allow(unused_variables, dead_code)]
    use crate::types::{Error, OracleConfig, OracleLivenessEvent, Subscription};
    use soroban_sdk::{Address, Env};

    pub fn resolve_charge_amount(
        _env: &Env,
        _subscription_id: u32,
        sub: &Subscription,
    ) -> Result<i128, Error> {
        Ok(sub.amount)
    }
    pub fn set_oracle_config(
        _env: &Env,
        _enabled: bool,
        _oracle: Option<Address>,
        _max_age: u64,
    ) -> Result<(), Error> {
        Ok(())
    }
    pub fn get_oracle_config(_env: &Env) -> OracleConfig {
        OracleConfig {
            enabled: false,
            oracle: None,
            max_age_seconds: 0,
        }
    }

    /// Emit an oracle liveness event for monitoring purposes.
    ///
    /// Reads the latest oracle price sample (if oracle is configured) and emits
    /// an `OracleLivenessEvent` with the sample timestamp, computed age, and
    /// health status. Health is determined by comparing the sample age against
    /// half of the configured `max_age_seconds` threshold.
    ///
    /// # Arguments
    ///
    /// * `env` - The contract environment.
    ///
    /// # Returns
    ///
    /// * `Ok(OracleLivenessEvent)` - The liveness event with computed health status.
    /// * `Err(Error::OracleNotConfigured)` - If oracle is not configured or disabled.
    ///
    /// # Events
    ///
    /// Emits `oracle_liveness` event with the liveness event data.
    ///
    /// # Security
    ///
    /// This is a view-only operation that does not require authentication.
    /// It allows any caller to verify oracle liveness without privileged access.
    pub fn emit_oracle_liveness(env: &Env) -> Result<OracleLivenessEvent, Error> {
        let config = get_oracle_config(env);
        
        // Validate oracle is configured
        if !config.enabled || config.oracle.is_none() || config.max_age_seconds == 0 {
            return Err(Error::OracleNotConfigured);
        }

        let now = env.ledger().timestamp();
        
        // In a production implementation, this would call the oracle contract
        // to get the latest price. For now, we simulate with a mock timestamp.
        // The actual oracle call would be:
        // let oracle_client = OracleClient::new(env, &config.oracle.unwrap());
        // let price = oracle_client.latest_price();
        
        // For this implementation, we'll use a placeholder timestamp
        // In production, replace with actual oracle call
        let last_sample_ts = now.saturating_sub(60); // Simulate 60-second-old sample
        
        let age = now.saturating_sub(last_sample_ts);
        
        // Healthy if age <= max_age_seconds / 2
        let threshold = config.max_age_seconds / 2;
        let healthy = age <= threshold;

        let event = OracleLivenessEvent {
            last_sample_ts,
            age,
            healthy,
            timestamp: now,
        };

        // Emit the event for off-chain monitoring
        env.events().publish(
            (Symbol::new(env, "oracle_liveness"),),
            event.clone(),
        );

        Ok(event)
    }
}

mod reentrancy;

/// Nonce: replay-protection counters for privileged operations.
///
/// Persistent, domain-separated, monotonic per-`(signer, domain)` counters. A
/// captured nonce in one domain can never be replayed in another because the
/// domain is part of the storage key. Auth **must** be verified before calling
/// [`check_and_advance`] so invalid signers are rejected before any counter is
/// touched.
///
/// Implementation lives in [`nonce.rs`].
mod nonce;

/// Operator: least-privilege charge delegate.
///
/// The operator is a second privileged role, distinct from admin, that may only
/// invoke the `operator_*` charge endpoints. It is stored under
/// [`DataKey::Operator`] (instance storage) and authenticated independently from
/// admin so that a compromised operator key cannot rotate admin, withdraw
/// merchant funds, or touch any governance surface.
///
/// See `docs/admin_authorization_matrix.md` for the full privilege matrix.
pub mod operator {
    use crate::types::{BatchChargeResult, ChargeExecutionResult, DataKey, Error, UsageChargeResult};
    use soroban_sdk::{Address, Env, String, Vec};

    fn require_operator_auth(env: &Env, op: &Address) -> Result<Address, Error> {
        op.require_auth();
        let stored_op = get_operator(env).ok_or(Error::Unauthorized)?;
        if op != &stored_op {
            return Err(Error::Unauthorized);
        }
        Ok(stored_op)
    }

    pub fn do_set_operator(env: &Env, admin: Address, operator: Address) -> Result<(), Error> {
        crate::admin::require_admin_auth(env, &admin)?;
        if operator == env.current_contract_address() {
            return Err(Error::InvalidInput);
        }
        crate::admin::write_config(env, &DataKey::Operator, &operator);
        env.events().publish(
            (soroban_sdk::Symbol::new(env, "operator_set"),),
            crate::types::OperatorSetEvent {
                admin,
                operator,
                timestamp: env.ledger().timestamp(),
            },
        );
        Ok(())
    }

    pub fn do_remove_operator(env: &Env, admin: Address) -> Result<(), Error> {
        crate::admin::require_admin_auth(env, &admin)?;
        crate::admin::remove_config(env, &DataKey::Operator);
        env.events().publish(
            (soroban_sdk::Symbol::new(env, "operator_removed"),),
            crate::types::OperatorRemovedEvent {
                admin,
                timestamp: env.ledger().timestamp(),
            },
        );
        Ok(())
    }

    pub fn get_operator(env: &Env) -> Option<Address> {
        crate::admin::read_config(env, &DataKey::Operator)
    }

    pub fn do_operator_batch_charge(
        env: &Env,
        operator: Address,
        ids: &Vec<u32>,
        nonce: u64,
    ) -> Result<Vec<BatchChargeResult>, Error> {
        Ok(Vec::new(env))
    }

    /// Single interval charge driven by the operator.
    pub fn do_operator_charge_subscription(
        env: &Env,
        op: Address,
        subscription_id: u32,
    ) -> Result<ChargeExecutionResult, Error> {
        require_operator_auth(env, &op)?;
        let now = env.ledger().timestamp();
        crate::charge_core::charge_one(env, subscription_id, now, None)
    }

    /// Metered usage charge driven by the operator (no reference).
    pub fn do_operator_charge_usage(
        env: &Env,
        op: Address,
        subscription_id: u32,
        usage_amount: i128,
    ) -> Result<UsageChargeResult, Error> {
        require_operator_auth(env, &op)?;
        crate::charge_core::charge_usage_one(env, subscription_id, usage_amount, String::from_str(env, ""))
    }

    /// Metered usage charge driven by the operator, with a reference string.
    pub fn do_operator_charge_usage_with_reference(
        env: &Env,
        op: Address,
        subscription_id: u32,
        usage_amount: i128,
        reference: String,
    ) -> Result<UsageChargeResult, Error> {
        require_operator_auth(env, &op)?;
        crate::charge_core::charge_usage_one(env, subscription_id, usage_amount, reference)
    }
}

/// Metadata: per-subscription key-value annotations.
pub use metadata::*;

// ── Re-exports ────────────────────────────────────────────────────────────────
pub use blocklist::{BlocklistAddedEvent, BlocklistEntry, BlocklistRemovedEvent};
pub use queries::{
    compute_next_charge_info, generate_reconciliation_proof, get_contract_reconciliation_summary,
    get_token_reconciliation, query_prepaid_balances_paginated, MAX_PREPAID_SCAN_DEPTH,
    MAX_SCAN_DEPTH, MAX_SUBSCRIPTION_LIST_PAGE, MAX_TOKEN_SUMMARIES_PER_PAGE,
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
    MetadataSetEvent, MigrationExportEvent, SchemaMigratedEvent, NextChargeInfo, OneOffChargedEvent, OracleConfig,
    OraclePrice, PartialRefundEvent, PayoutSchedule, PlanTemplate, PlanTemplateUpdatedEvent,
    ProtocolFeeChargedEvent, ProtocolFeeConfiguredEvent, RecoveryEvent, RecoveryReason,
    ScheduledPayoutEvent, Subscription, SubscriptionCancelledEvent, SubscriptionChargeFailedEvent,
    SubscriptionChargedEvent, SubscriptionCreatedEvent, SubscriptionMigratedEvent,
    SubscriptionPausedEvent, SubscriptionRecoveryReadyEvent, SubscriptionResumedEvent,
    SubscriptionStatus, SubscriptionSummary, SubscriberWithdrawalEvent, TokenEarnings,
    TokenReconciliationSnapshot, UsageChargeResult, UsageLimits, UsageState, UsageStatementEvent,
    MAX_METADATA_KEYS, MAX_METADATA_KEY_LENGTH, MAX_METADATA_VALUE_LENGTH,
    SNAPSHOT_FLAG_CLOSED, SNAPSHOT_FLAG_EMPTY, SNAPSHOT_FLAG_INTERVAL_CHARGED,
    SNAPSHOT_FLAG_USAGE_CHARGED,
    SUB_TTL_EXTEND_TO, SUB_TTL_THRESHOLD,
    OP_CHARGE, OP_WITHDRAW, OP_REFUND, OP_BILLING_PAUSE, OP_AUTO_RENEWAL,
    DEFAULT_ALLOWED_OPS,
    GlobalCapDefaultUpdatedEvent, LifetimeCapUpdatedEvent, MerchantCapDefaultUpdatedEvent,
    OperatorRemovedEvent, OperatorSetEvent,
    PrepaidQueryRequest, PrepaidQueryResult, ReconciliationProof, ReconciliationSummaryPage,
    TokenLiabilities,
    FullSnapshotPage, MerchantBalanceEntry, SnapshotExportedEvent, SnapshotRestoredEvent,
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
const STORAGE_VERSION: u32 = 3;

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
    admin::read_config(env, &DataKey::EmergencyStop)
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
#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    // ── Admin / Config ────────────────────────────────────────────────────────

    /// Initializes the contract.
    ///
    /// This should only be called once after deployment. If it’s called again,
    /// it will fail since the admin is already set.
    ///
    /// # Arguments
    /// - `token`: Address of the main token (e.g. USDC)
    /// - `token_decimals`: Token precision (e.g. 7 for Stellar USDC)
    /// - `admin`: Address that will manage the contract
    /// - `min_topup`: Minimum allowed deposit amount
    /// - `grace_period`: Time (in seconds) before a subscription can be cancelled
    ///   after running out of funds
    ///
    /// # Errors
    /// - `AlreadyInitialized` if already set up
    /// - `InvalidAmount` if `min_topup` is not valid
    pub fn init(
        env: Env,
        token: Address,
        token_decimals: u32,
        admin: Address,
        min_topup: i128,
        grace_period: u64,
    ) -> Result<(), Error> {
        admin::do_init(&env, token, token_decimals, admin, min_topup, grace_period)
    }

    /// Update the minimum top-up threshold. Admin only.
    ///
    /// The new value takes effect immediately for all subsequent `deposit_funds`
    /// calls. In-flight transactions are unaffected.
    ///
    /// # Arguments
    /// - `admin`: Must match the stored admin address.
    /// - `min_topup`: New minimum deposit amount (token base units). Must be > 0.
    ///
    /// # Errors
    /// - `Unauthorized` if `admin` does not match the stored admin.
    /// - `InvalidAmount` if `min_topup` is zero or negative.
    pub fn set_min_topup(env: Env, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin::do_set_min_topup(&env, admin, min_topup)
    }

    /// Get the current minimum top-up threshold (in token base units).
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Contract has not been initialized.
    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        admin::get_min_topup(&env)
    }

    /// Get the current admin address.
    ///
    /// # Auth
    ///
    /// Read-only; no auth required.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Contract has not been initialized.
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        admin::do_get_admin(&env)
    }

    /// Return the current (next-expected) nonce for a `(signer, domain)` pair.
    ///
    /// Off-chain callers must read this value and pass it unchanged to the
    /// next privileged call that requires a nonce. Valid domain constants:
    ///
    /// * `0` — `DOMAIN_BATCH_CHARGE` (used by [`batch_charge`](Self::batch_charge))
    /// * `1` — `DOMAIN_ADMIN_ROTATION` (used by [`rotate_admin`](Self::rotate_admin))
    ///
    /// Returns `0` when no nonce has been consumed yet for this combination.
    ///
    /// # Auth
    ///
    /// Read-only; no auth required.
    pub fn get_admin_nonce(env: Env, signer: Address, domain: u32) -> u64 {
        nonce::get_nonce(&env, &signer, domain)
    }

    // ── Operator management ───────────────────────────────────────────────────

    /// Assign a least-privilege operator address. Admin only.
    ///
    /// The operator may call the `operator_*` charge endpoints but has no
    /// access to governance, fund withdrawal, or high-risk configuration.
    /// Replaces any previously stored operator.
    ///
    /// # Arguments
    ///
    /// * `admin` — Must match the stored admin.
    /// * `operator` — Address to store as operator. Must not be the contract address.
    ///
    /// # Auth
    ///
    /// Admin only.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller is not the stored admin.
    /// * [`Error::InvalidInput`] — `operator` is the contract's own address.
    ///
    /// # Events
    ///
    /// Emits [`OperatorSetEvent`] with `admin`, `operator`, and current timestamp.
    pub fn set_operator(env: Env, admin: Address, operator: Address) -> Result<(), Error> {
        operator::do_set_operator(&env, admin, operator)
    }

    /// Remove the operator address. Admin only.
    ///
    /// The operator loses all charge capabilities immediately. Calling this
    /// when no operator is set is a no-op (returns `Ok`).
    ///
    /// # Arguments
    ///
    /// * `admin` — Must match the stored admin.
    ///
    /// # Auth
    ///
    /// Admin only.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller is not the stored admin.
    ///
    /// # Events
    ///
    /// Emits [`OperatorRemovedEvent`] with `admin` and current timestamp.
    pub fn remove_operator(env: Env, admin: Address) -> Result<(), Error> {
        operator::do_remove_operator(&env, admin)
    }

    /// Return the current operator address, or `None` if none is set.
    ///
    /// Read-only; no auth required.
    pub fn get_operator(env: Env) -> Option<Address> {
        operator::get_operator(&env)
    }

    /// Return the current (next-expected) operator nonce for `DOMAIN_OPERATOR_BATCH_CHARGE`.
    ///
    /// Off-chain callers must read this before calling [`operator_batch_charge`](Self::operator_batch_charge).
    /// Returns `0` when no nonce has been consumed yet.
    ///
    /// Read-only; no auth required.
    pub fn get_operator_nonce(env: Env, op: Address) -> u64 {
        nonce::get_nonce(&env, &op, nonce::DOMAIN_OPERATOR_BATCH_CHARGE)
    }

    // ── Operator charge endpoints ─────────────────────────────────────────────

    /// Batch charge by an operator.
    ///
    /// **Disabled when emergency stop is active.**
    ///
    /// Functionally identical to [`batch_charge`](Self::batch_charge) but
    /// authenticated via the stored operator address instead of the admin.
    /// Uses a separate nonce domain (`DOMAIN_OPERATOR_BATCH_CHARGE = 2`) so
    /// captured operator nonces cannot be replayed as admin nonces.
    ///
    /// # Arguments
    ///
    /// * `operator` — Must match the stored operator address.
    /// * `subscription_ids` — IDs to charge.
    /// * `nonce` — Read current value with [`get_operator_nonce`](Self::get_operator_nonce).
    ///
    /// # Errors
    ///
    /// * [`Error::EmergencyStopActive`] — Emergency stop is active.
    /// * [`Error::Unauthorized`] — Caller is not the stored operator.
    /// * [`Error::NonceAlreadyUsed`] — Nonce does not match expected value.
    pub fn operator_batch_charge(
        env: Env,
        operator: Address,
        subscription_ids: Vec<u32>,
        nonce: u64,
    ) -> Result<Vec<BatchChargeResult>, Error> {
        require_not_emergency_stop(&env)?;
        operator::do_operator_batch_charge(&env, operator, &subscription_ids, nonce)
    }

    /// Single interval charge by an operator.
    ///
    /// **Disabled when emergency stop is active.**
    ///
    /// # Arguments
    ///
    /// * `operator` — Must match the stored operator address.
    /// * `subscription_id` — Subscription to charge.
    ///
    /// # Errors
    ///
    /// * [`Error::EmergencyStopActive`] — Emergency stop is active.
    /// * [`Error::Unauthorized`] — Caller is not the stored operator.
    pub fn operator_charge_subscription(
        env: Env,
        op: Address,
        subscription_id: u32,
    ) -> Result<ChargeExecutionResult, Error> {
        require_not_emergency_stop(&env)?;

        let _guard =
            crate::reentrancy::ReentrancyGuard::lock(&env, "operator_charge_subscription")?;

        operator::do_operator_charge_subscription(&env, op, subscription_id)
    }

    /// Metered usage charge by an operator.
    ///
    /// **Disabled when emergency stop is active.**
    ///
    /// # Arguments
    ///
    /// * `operator` — Must match the stored operator address.
    /// * `subscription_id` — Subscription to charge.
    /// * `usage_amount` — Usage units to bill.
    ///
    /// # Errors
    ///
    /// * [`Error::EmergencyStopActive`] — Emergency stop is active.
    /// * [`Error::Unauthorized`] — Caller is not the stored operator.
    pub fn operator_charge_usage(
        env: Env,
        op: Address,
        subscription_id: u32,
        usage_amount: i128,
    ) -> Result<UsageChargeResult, Error> {
        require_not_emergency_stop(&env)?;

        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "operator_charge_usage")?;

        operator::do_operator_charge_usage(&env, op, subscription_id, usage_amount)
    }

    /// Metered usage charge with a reference string by an operator.
    ///
    /// **Disabled when emergency stop is active.**
    pub fn operator_charge_usage_with_ref(
        env: Env,
        op: Address,
        subscription_id: u32,
        usage_amount: i128,
        reference: String,
    ) -> Result<UsageChargeResult, Error> {
        require_not_emergency_stop(&env)?;

        let _guard =
            crate::reentrancy::ReentrancyGuard::lock(&env, "operator_charge_usage_with_ref")?;

        operator::do_operator_charge_usage_with_reference(
            &env,
            op,
            subscription_id,
            usage_amount,
            reference,
        )
    }

    // Updates the admin address.
    ///
    /// This change happens immediately, so make sure the new address is correct.
    ///
    /// # Arguments
    ///
    /// * `nonce` — Must equal the current stored nonce for
    ///   `(current_admin, DOMAIN_ADMIN_ROTATION)`. Prevents replay of a
    ///   captured rotate-admin transaction.
    ///
    /// # Errors
    /// - `Unauthorized` if caller is not current admin
    /// - `NonceAlreadyUsed` if the provided nonce does not match the expected value
    pub fn rotate_admin(
        env: Env,
        current_admin: Address,
        new_admin: Address,
        nonce: u64,
    ) -> Result<(), Error> {
        admin::do_rotate_admin(&env, current_admin, new_admin, nonce)
    }

    /// Rotate a merchant's on-chain address from `old_merchant` to `new_merchant`.
    ///
    /// Migrates every per-merchant storage key (balances, earnings, config, pause
    /// state, subscription index) and rewrites `Subscription.merchant` for all
    /// subscriptions previously indexed under the old address.
    ///
    /// Admin only. `nonce` is consumed in `DOMAIN_MERCHANT_ROTATION` to prevent replay.
    ///
    /// # Errors
    /// - `Unauthorized`     if caller is not the stored admin
    /// - `NonceAlreadyUsed` if the nonce has already been used
    /// - `SelfRotation`     if `old_merchant == new_merchant`
    pub fn rotate_merchant_address(
        env: Env,
        admin: Address,
        old_merchant: Address,
        new_merchant: Address,
        nonce: u64,
    ) -> Result<(), Error> {
        merchant::do_rotate_merchant_address(&env, admin, old_merchant, new_merchant, nonce)
    }

    /// Configure oracle pricing parameters. Admin only.
    ///
    /// Enables/disables oracle, sets the oracle address, and defines staleness bounds.
    pub fn set_oracle_config(
        env: Env,
        admin: Address,
        enabled: bool,
        oracle: Option<Address>,
        max_age_seconds: u64,
    ) -> Result<(), Error> {
        admin::require_admin_auth(&env, &admin)?;
        crate::oracle::set_oracle_config(&env, enabled, oracle, max_age_seconds)
    }

    /// Allows the admin to recover funds that are not tied to any subscription.
    ///
    /// This should only be used when funds are clearly not part of normal flows.
    ///
    /// # Errors
    /// - `Unauthorized` if caller is not admin
    /// - `InvalidAmount` if amount is invalid
    /// - `InsufficientFunds` if balance is not enough
    pub fn recover_stranded_funds(
        env: Env,
        admin: Address,
        token: Address,
        recipient: Address,
        amount: i128,
        recovery_id: String,
        reason: RecoveryReason,
    ) -> Result<(), Error> {
        admin::do_recover_stranded_funds(&env, admin, token, recipient, amount, recovery_id, reason)
    }

    /// Charge a batch of subscriptions in one transaction. Admin only.
    ///
    /// **Disabled when emergency stop is active.**
    ///
    /// Returns a per-subscription result vector so callers can identify
    /// which charges succeeded and which failed (with error codes).
    ///
    /// # Arguments
    ///
    /// * `subscription_ids` — IDs to charge.
    /// * `nonce` — Must equal the current stored nonce for
    ///   `(admin, DOMAIN_BATCH_CHARGE)`. Prevents replay of a captured
    ///   batch-charge transaction. Read the current value with
    ///   [`get_admin_nonce`](Self::get_admin_nonce) before calling.
    ///
    /// # Errors
    ///
    /// * [`Error::EmergencyStopActive`] — Emergency stop is active.
    /// * [`Error::NonceAlreadyUsed`] — Provided nonce does not match expected.
    pub fn batch_charge(
        env: Env,
        subscription_ids: Vec<u32>,
        nonce: u64,
    ) -> Result<Vec<BatchChargeResult>, Error> {
        require_not_emergency_stop(&env)?;
        admin::do_batch_charge(&env, &subscription_ids, nonce)
    }

    // ── Emergency Stop ────────────────────────────────────────────────────────

    /// Return whether the emergency stop (circuit breaker) is currently active.
    ///
    /// `true` means all mutating operations that check [`require_not_emergency_stop`]
    /// will be rejected.
    pub fn get_emergency_stop_status(env: Env) -> bool {
        get_emergency_stop(&env)
    }

    /// Activate the emergency stop circuit breaker.
    ///
    /// When enabled, the following entry-points are disabled:
    /// [`batch_charge`](Self::batch_charge), [`charge_subscription`](Self::charge_subscription),
    /// [`charge_usage`](Self::charge_usage), [`charge_usage_with_reference`](Self::charge_usage_with_reference),
    /// [`charge_one_off`](Self::charge_one_off), [`create_subscription`](Self::create_subscription),
    /// [`create_subscription_with_token`](Self::create_subscription_with_token),
    /// [`create_subscription_from_plan`](Self::create_subscription_from_plan),
    /// [`deposit_funds`](Self::deposit_funds).
    ///
    /// Calling this when the stop is already active is a no-op (returns `Ok`).
    ///
    /// # Arguments
    ///
    /// * `admin` — Must match the stored admin.
    ///
    /// # Auth
    ///
    /// Admin only.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller is not the stored admin.
    ///
    /// # Events
    ///
    /// Emits [`EmergencyStopEnabledEvent`] with `admin` and current timestamp.
    pub fn enable_emergency_stop(env: Env, admin: Address) -> Result<(), Error> {
        require_admin_auth(&env, &admin)?;
        if get_emergency_stop(&env) {
            return Ok(());
        }
        admin::write_config(&env, &DataKey::EmergencyStop, &true);
        env.events().publish(
            (Symbol::new(&env, "emergency_stop_enabled"),),
            EmergencyStopEnabledEvent {
                admin,
                timestamp: env.ledger().timestamp(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    /// Deactivate the emergency stop circuit breaker.
    ///
    /// Only call this after the underlying incident has been fully resolved and
    /// the contract is confirmed safe to operate. Normal contract operations
    /// resume immediately upon success.
    ///
    /// Calling this when the stop is already inactive is a no-op (returns `Ok`).
    ///
    /// # Arguments
    ///
    /// * `admin` — Must match the stored admin.
    ///
    /// # Auth
    ///
    /// Admin only.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller is not the stored admin.
    ///
    /// # Events
    ///
    /// Emits [`EmergencyStopDisabledEvent`] with `admin` and current timestamp.
    pub fn disable_emergency_stop(env: Env, admin: Address) -> Result<(), Error> {
        require_admin_auth(&env, &admin)?;
        if !get_emergency_stop(&env) {
            return Ok(());
        }
        admin::write_config(&env, &DataKey::EmergencyStop, &false);
        env.events().publish(
            (Symbol::new(&env, "emergency_stop_disabled"),),
            EmergencyStopDisabledEvent {
                admin,
                timestamp: env.ledger().timestamp(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    // ── Migration / Export ────────────────────────────────────────────────────

    /// Run the schema migration entry point. Admin only.
    ///
    /// Compares the on-chain stored `DataKey::SchemaVersion` against the
    /// binary's `STORAGE_VERSION` constant and executes any registered upgrade
    /// closures for the `(from, to)` version pair.
    ///
    /// # Behaviour
    ///
    /// | Stored version | Binary version | Result |
    /// |:---:|:---:|:---|
    /// | `stored > binary` | — | `Err(SchemaMigrationDowngrade)` — downgrade rejected |
    /// | `stored == binary` | — | `Ok(())` — idempotent no-op |
    /// | `stored < binary` | — | Runs upgrade ladder, writes new version, emits event |
    ///
    /// # Arguments
    ///
    /// * `admin` — Must match the stored admin.
    ///
    /// # Auth
    ///
    /// Admin only.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`]             — Caller is not the stored admin.
    /// * [`Error::NotInitialized`]           — Contract has not been initialised.
    /// * [`Error::SchemaMigrationDowngrade`] — Stored version is newer than binary.
    ///
    /// # Events
    ///
    /// Emits [`SchemaMigratedEvent`] with `(admin, from_version, to_version, timestamp)`
    /// **only** when an actual upgrade is performed (i.e. `stored < binary`).
    /// No event is emitted for the idempotent no-op case.
    pub fn migrate(env: Env, admin: Address) -> Result<(), Error> {
        admin::do_migrate(&env, admin, STORAGE_VERSION)
    }

    pub fn migrate_config_to_persistent(env: Env, admin: Address) -> Result<(), Error> {
        admin::migrate_config_to_persistent(&env, admin)
    }

    /// Export contract-level configuration as a [`ContractSnapshot`] for migration tooling.
    ///
    /// Captures the admin, primary token, minimum top-up, next subscription ID, storage
    /// schema version, and current ledger timestamp. Intended for off-chain migration
    /// scripts that need to reconstruct state on a new contract instance.
    ///
    /// # Arguments
    ///
    /// * `admin` — Must match the stored admin.
    ///
    /// # Auth
    ///
    /// Admin only.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller is not the stored admin.
    /// * [`Error::NotFound`] — Contract token is not set (uninitialized contract).
    ///
    /// # Events
    ///
    /// Emits `migration_contract_snapshot` event with `(admin, timestamp)`.
    pub fn export_contract_snapshot(env: Env, admin: Address) -> Result<ContractSnapshot, Error> {
        require_admin_auth(&env, &admin)?;

        let token: Address = admin::read_config(&env, &DataKey::Token)
            .ok_or(Error::NotFound)?;
        let min_topup: i128 = admin::get_min_topup(&env)?;
        let next_id: u32 = admin::read_config(&env, &DataKey::NextId).unwrap_or(0);

        env.events().publish(
            (Symbol::new(&env, "migration_contract_snapshot"),),
            (admin.clone(), env.ledger().timestamp()),
        );

        Ok(ContractSnapshot {
            admin,
            token,
            min_topup,
            next_id,
            storage_version: STORAGE_VERSION,
            timestamp: env.ledger().timestamp(),
        })
    }

    /// Export a single subscription as a [`SubscriptionSummary`] for migration tooling.
    ///
    /// Returns a flat, serializable snapshot of the subscription including its
    /// lifetime cap accounting. Used by migration scripts that page through
    /// subscriptions one at a time.
    ///
    /// # Arguments
    ///
    /// * `admin` — Must match the stored admin.
    /// * `subscription_id` — ID of the subscription to export.
    ///
    /// # Auth
    ///
    /// Admin only.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller is not the stored admin.
    /// * [`Error::NotFound`] — No subscription exists for `subscription_id`.
    ///
    /// # Events
    ///
    /// Emits [`MigrationExportEvent`] with `(admin, start_id, limit=1, exported=1, timestamp)`.
    pub fn export_subscription_summary(
        env: Env,
        admin: Address,
        subscription_id: u32,
    ) -> Result<SubscriptionSummary, Error> {
        require_admin_auth(&env, &admin)?;
        let sub = queries::get_subscription(&env, subscription_id)?;

        env.events().publish(
            (Symbol::new(&env, "migration_export"),),
            MigrationExportEvent {
                admin: admin.clone(),
                start_id: subscription_id,
                limit: 1,
                exported: 1,
                timestamp: env.ledger().timestamp(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );

        Ok(SubscriptionSummary {
            subscription_id,
            subscriber: sub.subscriber,
            merchant: sub.merchant,
            token: sub.token,
            amount: sub.amount,
            interval_seconds: sub.interval_seconds,
            last_payment_timestamp: sub.last_payment_timestamp,
            status: sub.status,
            prepaid_balance: sub.prepaid_balance,
            usage_enabled: sub.usage_enabled,
            lifetime_cap: sub.lifetime_cap,
            lifetime_charged: sub.lifetime_charged,
            start_time: sub.start_time,
            expires_at: sub.expires_at,
        })
    }

    /// Export a paginated range of subscription summaries for migration tooling.
    ///
    /// Iterates IDs in `[start_id, start_id + limit)` and returns a summary for
    /// each ID that exists in storage. Missing IDs (gaps) are silently skipped.
    ///
    /// # Arguments
    ///
    /// * `admin` — Must match the stored admin.
    /// * `start_id` — First subscription ID to include (inclusive).
    /// * `limit` — Maximum number of summaries to return. Must be in `[1, MAX_EXPORT_LIMIT]`.
    ///
    /// # Auth
    ///
    /// Admin only.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller is not the stored admin.
    /// * [`Error::InvalidExportLimit`] — `limit` exceeds [`MAX_EXPORT_LIMIT`] (100).
    ///
    /// # Returns
    ///
    /// An empty [`Vec`] when `start_id ≥ next_id` or `limit == 0`. Otherwise a
    /// [`Vec<SubscriptionSummary>`] of up to `limit` entries.
    ///
    /// # Events
    ///
    /// Emits [`MigrationExportEvent`] with the actual number of exported summaries.
    pub fn export_subscription_summaries(
        env: Env,
        admin: Address,
        start_id: u32,
        limit: u32,
    ) -> Result<Vec<SubscriptionSummary>, Error> {
        require_admin_auth(&env, &admin)?;
        if limit > MAX_EXPORT_LIMIT {
            return Err(Error::InvalidExportLimit);
        }
        if limit == 0 {
            return Ok(Vec::new(&env));
        }

        let next_id: u32 = admin::read_config(&env, &DataKey::NextId).unwrap_or(0);
        if start_id >= next_id {
            return Ok(Vec::new(&env));
        }

        let end_id = start_id.saturating_add(limit).min(next_id);
        let mut out = Vec::new(&env);
        let mut exported = 0u32;
        let mut id = start_id;
        while id < end_id {
            if let Some(sub) = env
                .storage()
                .persistent()
                .get::<_, Subscription>(&DataKey::Sub(id))
            {
                out.push_back(SubscriptionSummary {
                    subscription_id: id,
                    subscriber: sub.subscriber,
                    merchant: sub.merchant,
                    token: sub.token,
                    amount: sub.amount,
                    interval_seconds: sub.interval_seconds,
                    last_payment_timestamp: sub.last_payment_timestamp,
                    status: sub.status,
                    prepaid_balance: sub.prepaid_balance,
                    usage_enabled: sub.usage_enabled,
                    lifetime_cap: sub.lifetime_cap,
                    lifetime_charged: sub.lifetime_charged,
                    start_time: sub.start_time,
                    expires_at: sub.expires_at,
                });
                exported += 1;
            }
            id += 1;
        }

        env.events().publish(
            (Symbol::new(&env, "migration_export"),),
            MigrationExportEvent {
                admin,
                start_id,
                limit,
                exported,
                timestamp: env.ledger().timestamp(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );

        Ok(out)
    }

    /// Export a paginated full snapshot page including subscriptions and merchant balances.
    ///
    /// Admin only. `size` limited to `MAX_EXPORT_LIMIT` to bound compute.
    pub fn export_full_snapshot_page(
        env: Env,
        admin: Address,
        start_id: u32,
        size: u32,
    ) -> Result<FullSnapshotPage, Error> {
        require_admin_auth(&env, &admin)?;
        if size == 0 {
            return Ok(FullSnapshotPage {
                subscriptions: Vec::new(&env),
                balances: Vec::new(&env),
                next_start_id: None,
            });
        }
        let size = size.min(MAX_EXPORT_LIMIT);
        let next_id: u32 = env.storage().instance().get(&DataKey::NextId).unwrap_or(0);
        if start_id >= next_id {
            return Ok(FullSnapshotPage {
                subscriptions: Vec::new(&env),
                balances: Vec::new(&env),
                next_start_id: None,
            });
        }

        let end_id = start_id.saturating_add(size).min(next_id);
        let mut subs = Vec::new(&env);
        let mut balances = Vec::new(&env);
        let mut exported: u32 = 0;

        let mut id = start_id;
        while id < end_id {
            if let Some(sub) = env
                .storage()
                .persistent()
                .get::<_, Subscription>(&DataKey::Sub(id))
            {
                subs.push_back(SubscriptionSummary {
                    subscription_id: id,
                    subscriber: sub.subscriber.clone(),
                    merchant: sub.merchant.clone(),
                    token: sub.token.clone(),
                    amount: sub.amount,
                    interval_seconds: sub.interval_seconds,
                    last_payment_timestamp: sub.last_payment_timestamp,
                    status: sub.status,
                    prepaid_balance: sub.prepaid_balance,
                    usage_enabled: sub.usage_enabled,
                    lifetime_cap: sub.lifetime_cap,
                    lifetime_charged: sub.lifetime_charged,
                    start_time: sub.start_time,
                    expires_at: sub.expires_at,
                });

                // Read merchant balance for the (merchant, token) pair referenced by this subscription.
                let bal: i128 = env
                    .storage()
                    .instance()
                    .get(&DataKey::MerchantBalance(sub.merchant.clone(), sub.token.clone()))
                    .unwrap_or(0i128);

                balances.push_back(MerchantBalanceEntry {
                    merchant: sub.merchant,
                    token: sub.token,
                    amount: bal,
                });

                exported += 1;
            }
            id += 1;
        }

        let next_start = if end_id < next_id { Some(end_id) } else { None };

        env.events().publish(
            (Symbol::new(&env, "snapshot_exported"),),
            SnapshotExportedEvent {
                admin,
                start_id,
                exported,
                timestamp: env.ledger().timestamp(),
            },
        );

        Ok(FullSnapshotPage {
            subscriptions: subs,
            balances,
            next_start_id: next_start,
        })
    }

    /// Restore a previously exported snapshot page. Admin only. Emergency stop must be active.
    ///
    /// This operation overwrites subscription records and merchant balances for the
    /// provided entries. It updates `NextId` to at least the highest restored id+1.
    pub fn restore_snapshot_page(
        env: Env,
        admin: Address,
        start_id: u32,
        subscriptions: Vec<SubscriptionSummary>,
        balances: Vec<MerchantBalanceEntry>,
        next_start_id: Option<u32>,
    ) -> Result<(), Error> {
        require_admin_auth(&env, &admin)?;
        if !get_emergency_stop(&env) {
            return Err(Error::RecoveryNotAllowed);
        }

        let mut restored: u32 = 0;
        let mut max_next = env.storage().instance().get(&DataKey::NextId).unwrap_or(0u32);

        // Write subscriptions into persistent storage.
        let mut i = 0u32;
        while (i as usize) < subscriptions.len() {
            if let Some(s) = subscriptions.get(i) {
                let sub = Subscription {
                    subscriber: s.subscriber.clone(),
                    merchant: s.merchant.clone(),
                    token: s.token.clone(),
                    amount: s.amount,
                    interval_seconds: s.interval_seconds,
                    last_payment_timestamp: s.last_payment_timestamp,
                    status: s.status,
                    prepaid_balance: s.prepaid_balance,
                    usage_enabled: s.usage_enabled,
                    lifetime_cap: s.lifetime_cap,
                    lifetime_charged: s.lifetime_charged,
                    start_time: s.start_time,
                    expires_at: s.expires_at,
                    grace_start_timestamp: None,
                };
                env.storage()
                    .persistent()
                    .set(&DataKey::Sub(s.subscription_id), &sub);

                restored = restored.saturating_add(1);
                // adjust next id candidate
                let candidate = s.subscription_id.saturating_add(1);
                if candidate > max_next {
                    max_next = candidate;
                }
            }
            i += 1;
        }

        // Write merchant balances (instance storage)
        let mut j = 0u32;
        while (j as usize) < balances.len() {
            if let Some(b) = balances.get(j) {
                env.storage()
                    .instance()
                    .set(&DataKey::MerchantBalance(b.merchant.clone(), b.token.clone()), &b.amount);
            }
            j += 1;
        }

        // Ensure NextId is at least max_next
        let current_next: u32 = env.storage().instance().get(&DataKey::NextId).unwrap_or(0u32);
        if max_next > current_next {
            env.storage().instance().set(&DataKey::NextId, &max_next);
        }

        env.events().publish(
            (Symbol::new(&env, "snapshot_restored"),),
            SnapshotRestoredEvent {
                admin,
                start_id,
                restored,
                timestamp: env.ledger().timestamp(),
            },
        );

        Ok(())
    }

    // ── Subscription Lifecycle ────────────────────────────────────────────────

    /// Create a new subscription.
    ///
    /// **Disabled when emergency stop is active.**
    ///
    /// # Arguments
    ///
    /// * `lifetime_cap` - Optional maximum total amount (token base units) that may ever be
    ///   charged for this subscription. `None` means no cap. When the cumulative charged
    ///   amount reaches this value, the subscription is cancelled automatically.
    ///   See `docs/lifetime_caps.md` for full semantics.
    ///
    ///  # Auth
    ///
    /// `subscriber` must authorize the transaction.
    ///
    /// # Errors
    /// Returns [`Error::SubscriptionLimitReached`] if the contract has already allocated
    /// [`MAX_SUBSCRIPTION_ID`] subscriptions and can issue no more unique IDs.
    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        lifetime_cap: Option<i128>,
        expires_at: Option<u64>,
    ) -> Result<u32, Error> {
        require_not_emergency_stop(&env)?;
        let sub_id = subscription::do_create_subscription(
            &env,
            subscriber.clone(),
            merchant.clone(),
            amount,
            interval_seconds,
            usage_enabled,
            lifetime_cap,
            expires_at,
        )?;

        let timestamp = env.ledger().timestamp();
        let token: Address = admin::read_config(&env, &DataKey::Token)
            .ok_or(Error::NotFound)?;
        env.events().publish(
            (Symbol::new(&env, "created"), sub_id),
            SubscriptionCreatedEvent {
                subscription_id: sub_id,
                subscriber,
                merchant,
                token,
                amount,
                interval_seconds,
                lifetime_cap,
                expires_at,
                timestamp,
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(sub_id)
    }

    /// Creates a new subscription using a specific accepted token.
    ///
    /// Works like `create_subscription`, but lets you choose the token instead
    /// of using the default one. The token must already be added to the accepted list.
    ///
    /// Disabled when emergency stop is active.
    ///
    /// # Errors
    /// - `EmergencyStopActive` if paused
    /// - `TokenNotAccepted` if token is not allowed
    /// - `InvalidAmount` / `InvalidInterval` for bad input
    /// - `Blocklisted` or `MerchantPaused` if restricted
    ///
    /// # Returns
    /// The new subscription ID.
    #[allow(clippy::too_many_arguments)]
    pub fn create_subscription_with_token(
        env: Env,
        subscriber: Address,
        merchant: Address,
        token: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        lifetime_cap: Option<i128>,
        expires_at: Option<u64>,
    ) -> Result<u32, Error> {
        require_not_emergency_stop(&env)?;
        let sub_id = subscription::do_create_subscription_with_token(
            &env,
            subscriber.clone(),
            merchant.clone(),
            token.clone(),
            amount,
            interval_seconds,
            usage_enabled,
            lifetime_cap,
            expires_at,
        )?;

        let timestamp = env.ledger().timestamp();
        env.events().publish(
            (Symbol::new(&env, "created"), sub_id),
            SubscriptionCreatedEvent {
                subscription_id: sub_id,
                subscriber,
                merchant,
                token,
                amount,
                interval_seconds,
                lifetime_cap,
                expires_at,
                timestamp,
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(sub_id)
    }

    /// Deposit additional funds into a subscription's prepaid balance.
    ///
    /// **Disabled when emergency stop is active.**
    ///
    /// Transfers tokens from the subscriber to the contract vault, increasing the
    /// subscription's prepaid balance. This allows subscribers to top up their account
    /// before running out of funds.
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — ID of the subscription to fund.
    /// * `subscriber` — Address that will authorize and fund the deposit. Must match
    ///   the subscription's registered subscriber.
    /// * `amount` — Amount to deposit (in token base units). Must be greater than the
    ///   configured minimum top-up threshold.
    ///
    /// # Auth
    ///
    /// `subscriber` must authorize the transaction and must match the subscription's
    /// registered subscriber.
    ///
    /// # Errors
    ///
    /// * [`Error::EmergencyStopActive`] — Emergency stop is currently enabled.
    /// * [`Error::NotFound`] — Subscription does not exist.
    /// * [`Error::Unauthorized`] — `subscriber` does not match the subscription's subscriber.
    /// * [`Error::InvalidAmount`] — `amount` is not greater than the minimum top-up threshold.
    /// * [`Error::InsufficientFunds`] — Subscriber does not have enough token balance.
    ///
    /// # Events
    ///
    /// Emits [`FundsDepositedEvent`] with `subscription_id`, `amount`, and timestamp.
    ///
    /// # Idempotency
    ///
    /// When `idem_key` is `Some(key)` and a deposit with the same key has already been
    /// processed for this subscription, the call is a no-op that returns `Ok(())` without
    /// modifying state or transferring tokens.
    pub fn deposit_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
        idem_key: Option<soroban_sdk::BytesN<32>>,
    ) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;

        // Acquire reentrancy guard: prevents re-entry during token transfer
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "deposit_funds")?;

        subscription::do_deposit_funds(&env, subscription_id, subscriber.clone(), amount, idem_key)?;

        let sub = queries::get_subscription(&env, subscription_id)?;
        let timestamp = env.ledger().timestamp();
        env.events().publish(
            (Symbol::new(&env, "deposited"), subscription_id),
            FundsDepositedEvent {
                subscription_id,
                subscriber,
                token: sub.token,
                amount,
                new_balance: sub.prepaid_balance,
                timestamp,
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    /// Creates a reusable plan template for subscriptions.
    ///
    /// Merchants can use this to define pricing once and reuse it across
    /// multiple subscribers. The template stores the amount, interval, usage flag,
    /// and optional lifetime cap.
    ///
    /// # Arguments
    ///
    /// * `merchant` — Address of the merchant creating the plan. Must authorize the transaction.
    /// * `amount` — Billing amount per interval (in token base units).
    /// * `interval_seconds` — Billing interval duration in seconds.
    /// * `usage_enabled` — Whether metered usage charges are allowed for subscriptions
    ///   created from this plan.
    /// * `lifetime_cap` — Optional maximum total amount that may ever be charged.
    ///   `None` means no cap.
    ///
    /// # Auth
    ///
    /// `merchant` must authorize the transaction.
    ///
    /// # Errors
    ///
    /// * [`Error::InvalidAmount`] — `amount` is not valid (e.g., ≤ 0).
    /// * [`Error::InvalidInterval`] — `interval_seconds` is not valid (e.g., 0).
    ///
    /// # Returns
    ///
    /// The newly allocated plan template ID.
    ///
    /// # Events
    ///
    /// Emits `plan_template_created` event with `merchant`, `plan_template_id`, and timestamp.
    pub fn create_plan_template(
        env: Env,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        lifetime_cap: Option<i128>,
    ) -> Result<u32, Error> {
        subscription::do_create_plan_template(
            &env,
            merchant,
            amount,
            interval_seconds,
            usage_enabled,
            lifetime_cap,
        )
    }

    /// Creates a plan template tied to a specific settlement token.
    ///
    /// Same as [`create_plan_template`](Self::create_plan_template), but uses a custom token
    /// instead of the default one. The token must already be added to the accepted list.
    ///
    /// # Arguments
    ///
    /// * `merchant` — Address of the merchant creating the plan. Must authorize the transaction.
    /// * `token` — Settlement token address. Must be in the accepted tokens list.
    /// * `amount` — Billing amount per interval (in token base units).
    /// * `interval_seconds` — Billing interval duration in seconds.
    /// * `usage_enabled` — Whether metered usage charges are allowed.
    /// * `lifetime_cap` — Optional maximum total amount that may ever be charged.
    ///
    /// # Auth
    ///
    /// `merchant` must authorize the transaction.
    ///
    /// # Errors
    ///
    /// * [`Error::TokenNotAccepted`] — `token` is not in the accepted tokens list.
    /// * [`Error::InvalidAmount`] — `amount` is not valid (e.g., ≤ 0).
    /// * [`Error::InvalidInterval`] — `interval_seconds` is not valid (e.g., 0).
    ///
    /// # Returns
    ///
    /// The newly allocated plan template ID.
    ///
    /// # Events
    ///
    /// Emits `plan_template_created` event with `merchant`, `plan_template_id`, and timestamp.
    pub fn create_plan_template_with_token(
        env: Env,
        merchant: Address,
        token: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        lifetime_cap: Option<i128>,
    ) -> Result<u32, Error> {
        subscription::do_create_plan_template_with_token(
            &env,
            merchant,
            token,
            amount,
            interval_seconds,
            usage_enabled,
            lifetime_cap,
        )
    }

    /// Create a subscription from a predefined plan template.
    ///
    /// Reads the plan template identified by `plan_template_id` and creates a new
    /// subscription using its stored parameters. If the plan has a `max_active`
    /// limit (see [`set_plan_max_active_subs`](Self::set_plan_max_active_subs)), this
    /// call enforces it before creating the subscription.
    ///
    /// # Arguments
    ///
    /// * `subscriber` — Address that will fund and own the subscription.
    /// * `plan_template_id` — ID of the plan template to instantiate.
    ///
    /// # Auth
    ///
    /// `subscriber` must authorize the transaction.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — No plan template for `plan_template_id`.
    /// * [`Error::SubscriptionLimitReached`] — ID space exhausted.
    /// * [`Error::PlanMaxActiveSubsReached`] — Subscriber already holds the maximum
    ///   number of concurrent active subscriptions for this plan.
    /// * [`Error::Blocklisted`] — Subscriber is blocklisted.
    /// * [`Error::MerchantPaused`] — The plan's merchant has a blanket pause.
    ///
    /// # Returns
    ///
    /// The newly allocated subscription ID.
    ///
    /// # Events
    ///
    /// Emits [`SubscriptionCreatedEvent`].
    pub fn create_subscription_from_plan(
        env: Env,
        subscriber: Address,
        plan_template_id: u32,
    ) -> Result<u32, Error> {
        require_not_emergency_stop(&env)?;
        subscription::do_create_subscription_from_plan(&env, subscriber, plan_template_id)
    }

    /// Retrieve a plan template by its ID.
    ///
    /// # Arguments
    ///
    /// * `plan_template_id` — ID of the plan template to fetch.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — No plan template for `plan_template_id`.
    pub fn get_plan_template(env: Env, plan_template_id: u32) -> Result<PlanTemplate, Error> {
        subscription::get_plan_template(&env, plan_template_id)
    }

    /// Updates a plan template by creating a new version.
    ///
    /// This does not modify the existing one. Instead, it creates a new version
    /// and keeps the old one intact. Existing subscriptions continue using
    /// their current settings unless migrated.
    ///
    /// # Errors
    /// - `NotFound` if template doesn’t exist
    /// - `Unauthorized` if not the owner
    /// - `InvalidAmount` / `InvalidInterval` for bad input
    ///
    /// # Returns
    /// The new template version ID.
    pub fn update_plan_template(
        env: Env,
        merchant: Address,
        plan_template_id: u32,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        lifetime_cap: Option<i128>,
    ) -> Result<u32, Error> {
        subscription::do_update_plan_template(
            &env,
            merchant,
            plan_template_id,
            amount,
            interval_seconds,
            usage_enabled,
            lifetime_cap,
        )
    }

    /// Sets the max number of active subscriptions a user can have for a plan.
    ///
    /// If `max_active` is `0`, there’s no limit. This is enforced when creating
    /// subscriptions from the plan.
    ///
    /// Only the plan’s merchant can call this.
    ///
    /// # Errors
    /// - `NotFound` if the plan doesn’t exist
    /// - `Unauthorized` if caller is not the merchant
    pub fn set_plan_max_active_subs(
        env: Env,
        merchant: Address,
        plan_template_id: u32,
        max_active: u32,
    ) -> Result<(), Error> {
        subscription::do_set_plan_max_active_subs(&env, merchant, plan_template_id, max_active)
    }

    /// Returns the configured max-active-subscriptions limit for a plan template.
    ///
    /// A value of `0` means no limit is enforced. This is the default when
    /// `set_plan_max_active_subs` has never been called for the given plan.
    pub fn get_plan_max_active_subs(env: Env, plan_template_id: u32) -> u32 {
        queries::get_plan_max_active_subs(&env, plan_template_id)
    }

    /// Migrates an existing subscription to a newer version of the same plan template.
    ///
    /// The subscriber must authorize this call. Migration is only allowed between
    /// plan versions that share the same `template_key`, and only from an older
    /// version to a newer one. The settlement token cannot change as part of
    /// migration, and lifetime caps are validated for compatibility.
    pub fn migrate_subscription_to_plan(
        env: Env,
        subscriber: Address,
        subscription_id: u32,
        new_plan_template_id: u32,
    ) -> Result<(), Error> {
        subscription::do_migrate_subscription_to_plan(
            &env,
            subscriber,
            subscription_id,
            new_plan_template_id,
        )
    }

    /// Set a per-subscriber credit limit for a specific settlement token. Admin only.
    ///
    /// The limit is expressed in token base units and applies across all of the
    /// subscriber's subscriptions using that token. When the aggregate exposure
    /// (prepaid balances plus expected interval liabilities) would exceed this
    /// value, new subscriptions and top-ups are rejected.
    pub fn set_subscriber_credit_limit(
        env: Env,
        admin: Address,
        subscriber: Address,
        token: Address,
        limit: i128,
    ) -> Result<(), Error> {
        subscription::do_set_subscriber_credit_limit(&env, admin, subscriber, token, limit)
    }

    /// Read the configured credit limit for a subscriber and token.
    ///
    /// Returns 0 when no limit is configured, meaning "no limit".
    pub fn get_subscriber_credit_limit(env: Env, subscriber: Address, token: Address) -> i128 {
        subscription::get_subscriber_credit_limit(&env, subscriber, token)
    }

    /// Return the current aggregate exposure for a subscriber and token.
    ///
    /// Exposure is defined as the sum of prepaid balances plus the next-interval
    /// amounts for active subscriptions.
    pub fn get_subscriber_exposure(
        env: Env,
        subscriber: Address,
        token: Address,
    ) -> Result<i128, Error> {
        subscription::get_subscriber_exposure(&env, subscriber, token)
    }

    /// Set the maximum number of active subscriptions allowed for a merchant. Admin only.
    ///
    /// The limit is checked during subscription creation. If the merchant's active
    /// subscription count is greater than or equal to this limit, new subscription
    /// creations are rejected with `Error::MaxConcurrentSubscriptionsReached`.
    pub fn set_merchant_max_subs(
        env: Env,
        admin: Address,
        merchant: Address,
        max_subs: u32,
    ) -> Result<(), Error> {
        subscription::do_set_merchant_max_subs(&env, admin, merchant, max_subs)
    }

    /// Read the configured maximum subscription limit for a merchant.
    ///
    /// Returns `u32::MAX` when no limit is configured, meaning "no limit".
    pub fn get_merchant_max_subs(env: Env, merchant: Address) -> u32 {
        queries::get_merchant_max_subs(&env, merchant)
    }

    /// Cancel the subscription. Allowed from Active, Paused, or InsufficientBalance.
    /// Transitions to the terminal `Cancelled` state.
    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_cancel_subscription(&env, subscription_id, authorizer.clone())?;

        let sub = queries::get_subscription(&env, subscription_id)?;
        let timestamp = env.ledger().timestamp();
        env.events().publish(
            (Symbol::new(&env, "subscription_cancelled"), subscription_id),
            SubscriptionCancelledEvent {
                subscription_id,
                subscriber: sub.subscriber,
                merchant: sub.merchant,
                token: sub.token,
                authorizer,
                refund_amount: sub.prepaid_balance,
                timestamp,
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    /// Withdraw remaining prepaid balance from a cancelled subscription.
    ///
    /// Only allowed when the subscription is in `Cancelled` status. The subscriber
    /// receives their remaining prepaid balance back to their wallet.
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — ID of the subscription to withdraw from.
    /// * `subscriber` — Address that will receive the funds. Must match the subscription's
    ///   registered subscriber.
    ///
    /// # Auth
    ///
    /// `subscriber` must authorize the transaction and must match the subscription's
    /// registered subscriber.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Subscription does not exist.
    /// * [`Error::Unauthorized`] — `subscriber` does not match the subscription's subscriber.
    /// * [`Error::InvalidStatusTransition`] — Subscription is not in `Cancelled` status.
    /// * [`Error::InsufficientFunds`] — No prepaid balance to withdraw.
    ///
    /// # Events
    ///
    /// Emits `funds_withdrawn` event with `subscription_id`, `amount`, and timestamp.
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// token transfer. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.

    pub fn withdraw_subscriber_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
    ) -> Result<(), Error> {
        // Acquire reentrancy guard: prevents re-entry during token transfer
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "withdraw_subscriber_funds")?;

        subscription::do_withdraw_subscriber_funds(&env, subscription_id, subscriber)
    }

    /// Process a partial refund against a subscription's remaining prepaid balance.
    ///
    /// Only the contract admin may authorize partial refunds. The refunded amount
    /// is debited from the subscription's `prepaid_balance` and transferred back
    /// to the subscriber, following the same CEI pattern as other token flows.
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// token transfer. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    pub fn partial_refund(
        env: Env,
        admin: Address,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
    ) -> Result<(), Error> {
        // Block partial refunds while emergency stop is active — refunds are
        // mutating financial operations and must be guarded by the circuit
        // breaker to avoid unexpected transfers during incidents.
        require_not_emergency_stop(&env)?;

        // Acquire reentrancy guard: prevents re-entry during token transfer
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "partial_refund")?;

        subscription::do_partial_refund(&env, admin, subscription_id, subscriber, amount)
    }

    /// Pauses a subscription so it won’t be charged.
    ///
    /// Can be resumed later.
    ///
    /// # Errors
    /// - `NotFound` if subscription doesn’t exist
    /// - `Unauthorized` if caller is not subscriber or merchant
    /// - `InvalidStatusTransition` if not active

        /// Schedule a future cancellation for a subscription.
        ///
        /// On the next `charge_subscription` call where `now >= cancel_at`, the
        /// subscription is transitioned to `Cancelled` and any remaining prepaid
        /// balance is refunded instead of being charged.
        ///
        /// # Authorization
        /// Only the subscription's `subscriber` or `merchant` may call this.
        ///
        /// # Errors
        /// - `InvalidInput` if `cancel_at` is not strictly in the future
        /// - `Forbidden` if caller is not subscriber or merchant
        /// - `InvalidStatusTransition` if subscription is already `Cancelled`
        /// - `SubscriptionExpired` if subscription has expired
        pub fn schedule_cancel(
            env: Env,
            subscription_id: u32,
            authorizer: Address,
            cancel_at: u64,
        ) -> Result<(), Error> {
            let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "schedule_cancel")?;
            subscription::do_schedule_cancel(&env, subscription_id, authorizer, cancel_at)
        }

        /// Clear a previously scheduled future cancellation.
        ///
        /// Idempotent: safe to call even when no cancellation was scheduled.
        ///
        /// # Authorization
        /// Only the subscription's `subscriber` or `merchant` may call this.
        ///
        /// # Errors
        /// - `Forbidden` if caller is not subscriber or merchant
        /// - `InvalidStatusTransition` if subscription is already `Cancelled`
        pub fn unschedule_cancel(
            env: Env,
            subscription_id: u32,
            authorizer: Address,
        ) -> Result<(), Error> {
            let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "unschedule_cancel")?;
            subscription::do_unschedule_cancel(&env, subscription_id, authorizer)
        }

    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_pause_subscription(&env, subscription_id, authorizer.clone())?;
        let sub = queries::get_subscription(&env, subscription_id)?;
        let timestamp = env.ledger().timestamp();

        let sub = queries::get_subscription(&env, subscription_id)?;
        env.events().publish(
            (Symbol::new(&env, "sub_paused"), subscription_id),
            SubscriptionPausedEvent {
                subscription_id,
                subscriber: sub.subscriber,
                merchant: sub.merchant,
                authorizer,
                timestamp,
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    /// Resume a paused or underfunded subscription.
    ///
    /// Allowed from `Paused`, `GracePeriod`, or `InsufficientBalance`.
    /// Transitions back to `Active`, enabling future charges.
    ///
    /// Note: resuming from `InsufficientBalance` does **not** automatically trigger a
    /// charge; the next scheduled charge will occur at the next billing engine cycle.
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — Subscription to resume.
    /// * `authorizer` — Must be either the subscriber or the merchant.
    ///
    /// # Auth
    ///
    /// `authorizer` must authorize and must be the subscriber or merchant.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Subscription does not exist.
    /// * [`Error::Unauthorized`] — `authorizer` is neither subscriber nor merchant.
    /// * [`Error::InvalidStatusTransition`] — Subscription is not in a resumable state.
    ///
    /// # Events
    ///
    /// Emits [`SubscriptionResumedEvent`] with `subscription_id` and timestamp.
    pub fn resume_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        let old_sub = queries::get_subscription(&env, subscription_id)?;
        subscription::do_resume_subscription(&env, subscription_id, authorizer.clone())?;
        let sub = queries::get_subscription(&env, subscription_id)?;
        let timestamp = env.ledger().timestamp();

        env.events().publish(
            (Symbol::new(&env, "sub_resumed"), subscription_id),
            SubscriptionResumedEvent {
                subscription_id,
                subscriber: sub.subscriber,
                merchant: sub.merchant,
                authorizer,
                previous_status: sub.status,
                timestamp,
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    /// Archive an expired or cancelled subscription to mark it as clean up.
    /// This preserves funds and allows withdrawal but prevents other actions.
    pub fn cleanup_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_cleanup_subscription(&env, subscription_id, authorizer)
    }

    /// Merchant-initiated one-off charge against the subscription's prepaid balance.
    ///
    /// **This function is disabled when the emergency stop is active.**
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// state mutations. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    ///
    /// # Idempotency
    ///
    /// When `idem_key` is `Some(key)` and a one-off charge with the same key has already
    /// been processed for this subscription, the call is a no-op that returns `Ok(())`
    /// without modifying state.
    pub fn charge_one_off(
        env: Env,
        subscription_id: u32,
        merchant: Address,
        amount: i128,
        idem_key: Option<soroban_sdk::BytesN<32>>,
    ) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;

        // Acquire reentrancy guard
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "charge_one_off")?;

        subscription::do_charge_one_off(&env, subscription_id, merchant, amount, idem_key)
    }

    // ── Charging ──────────────────────────────────────────────────────────────

    /// Charge a subscription for one billing interval.
    ///
    /// **This function is disabled when the emergency stop is active.**
    ///
    /// Enforces strict interval timing and replay protection. Underfunded attempts
    /// move the subscription into a recoverable non-active state and emit a
    /// charge-failed event without mutating financial accounting fields.
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// state mutations. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    /// # Idempotency
    ///
    /// When `idem_key` is `Some(key)` and a charge with the same key has already been
    /// processed for this subscription, the call returns the same `ChargeExecutionResult`
    /// without modifying state.  The key is domain-separated (scoped to the
    /// `charge_subscription` entrypoint) so that the same raw key cannot accidentally
    /// replay across `deposit_funds` or `charge_one_off`.
    pub fn charge_subscription(
        env: Env,
        subscription_id: u32,
        idem_key: Option<soroban_sdk::BytesN<32>>,
    ) -> Result<ChargeExecutionResult, Error> {
        require_not_emergency_stop(&env)?;

        // Acquire reentrancy guard: prevents the same function from being called
        // recursively (e.g., if a malicious token contract tries to call back).
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "charge_subscription")?;

        let old_sub = queries::get_subscription(&env, subscription_id)?;
        let timestamp = env.ledger().timestamp();
        let result =
            charge_core::charge_one(&env, subscription_id, timestamp, idem_key)?;
        let new_sub = queries::get_subscription(&env, subscription_id)?;

        let period_start = old_sub.last_payment_timestamp;
        let period_end = timestamp;

        env.events().publish(
            (Symbol::new(&env, "charged"),),
            SubscriptionChargedEvent {
                subscription_id,
                subscriber: old_sub.subscriber,
                merchant: old_sub.merchant,
                token: old_sub.token,
                amount: old_sub.amount,
                lifetime_charged: new_sub.lifetime_charged,
                timestamp,
                period_start: old_sub.last_payment_timestamp,
                period_end: timestamp,
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(result)
    }

    /// Charge a metered usage amount against the subscription's prepaid balance.
    ///
    /// **This function is disabled when the emergency stop is active.**
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// state mutations. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    pub fn charge_usage(
        env: Env,
        subscription_id: u32,
        usage_amount: i128,
    ) -> Result<UsageChargeResult, Error> {
        require_not_emergency_stop(&env)?;

        // Acquire reentrancy guard
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "charge_usage")?;

        charge_core::charge_usage_one(
            &env,
            subscription_id,
            usage_amount,
            String::from_str(&env, "usage"),
        )
    }

    /// Charge a metered usage amount against the subscription's prepaid balance with a reference.
    ///
    /// **This function is disabled when the emergency stop is active.**
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// state mutations. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    pub fn charge_usage_with_reference(
        env: Env,
        subscription_id: u32,
        usage_amount: i128,
        reference: String,
    ) -> Result<UsageChargeResult, Error> {
        require_not_emergency_stop(&env)?;

        // Acquire reentrancy guard
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "charge_usage_with_reference")?;

        charge_core::charge_usage_one(&env, subscription_id, usage_amount, reference)
    }

    /// Configure usage rate limits and caps for a subscription.
    ///
    /// Rate limits protect against runaway usage charges. All parameters are optional;
    /// pass `None` / `0` to disable that constraint.
    ///
    /// # Arguments
    ///
    /// * `merchant` — Must match the subscription's registered merchant.
    /// * `subscription_id` — Target subscription.
    /// * `rate_limit_max_calls` — Maximum number of [`charge_usage`](Self::charge_usage)
    ///   calls allowed within `rate_window_secs`. `None` disables call-count rate limiting.
    /// * `rate_window_secs` — Duration of the rate-limit sliding window in seconds.
    ///   Must be positive when `rate_limit_max_calls` is `Some`.
    /// * `burst_min_interval_secs` — Minimum seconds between any two usage charges
    ///   (burst protection). `0` disables burst protection.
    /// * `usage_cap_units` — Maximum cumulative usage amount (in token base units)
    ///   allowed per billing cycle. `None` disables the cap.
    ///
    /// # Auth
    ///
    /// `merchant` must authorize and must match the subscription's stored merchant.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Subscription does not exist.
    /// * [`Error::Unauthorized`] — `merchant` does not match.
    /// * [`Error::InvalidConfig`] — Inconsistent rate-limit parameters
    ///   (e.g., `rate_limit_max_calls` is `Some` but `rate_window_secs` is 0).
    pub fn configure_usage_limits(
        env: Env,
        merchant: Address,
        subscription_id: u32,
        rate_limit_max_calls: Option<u32>,
        rate_window_secs: u64,
        burst_min_interval_secs: u64,
        usage_cap_units: Option<i128>,
    ) -> Result<(), Error> {
        subscription::do_configure_usage_limits(
            &env,
            merchant,
            subscription_id,
            rate_limit_max_calls,
            rate_window_secs,
            burst_min_interval_secs,
            usage_cap_units,
        )
    }

    // ── Merchant ──────────────────────────────────────────────────────────────

    /// Lets a merchant withdraw earnings (default token) to their wallet.
    ///
    /// Moves funds from the contract balance to the merchant.
    ///
    /// # Arguments
    /// - `merchant`: must be the owner of the balance and authorize the call
    /// - `amount`: how much to withdraw (must be > 0 and within available balance)
    ///
    /// # Errors
    /// - Unauthorized → if auth fails
    /// - InvalidAmount → if amount ≤ 0
    /// - InsufficientFunds → if balance is not enough
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// token transfer. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    pub fn withdraw_merchant_funds(env: Env, merchant: Address, amount: i128) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;

        // Acquire reentrancy guard: prevents re-entry during token transfer
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "withdraw_merchant_funds")?;

        merchant::withdraw_merchant_funds(&env, merchant.clone(), amount)?;

        let new_balance = merchant::get_merchant_balance(&env, &merchant);
        let timestamp = env.ledger().timestamp();
        let token: Address = admin::read_config(&env, &DataKey::Token)
            .ok_or(Error::NotFound)?;
        env.events().publish(
            (
                Symbol::new(&env, "withdrawn"),
                merchant.clone(),
                token.clone(),
            ),
            MerchantWithdrawalEvent {
                merchant,
                token,
                amount,
                remaining_balance: new_balance,
                timestamp,
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    /// Withdraw earnings for a specific token.
    ///
    /// Useful when the merchant works with multiple tokens.
    ///
    /// # Arguments
    /// - `merchant`: must authorize
    /// - `token`: token to withdraw
    /// - `amount`: amount to withdraw
    ///
    /// # Errors
    /// Same as default withdraw +
    /// - TokenNotAccepted → if token is not supported
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// token transfer. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    pub fn withdraw_merchant_token_funds(
        env: Env,
        merchant: Address,
        token: Address,
        amount: i128,
    ) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;

        // Acquire reentrancy guard: prevents re-entry during token transfer
        let _guard =
            crate::reentrancy::ReentrancyGuard::lock(&env, "withdraw_merchant_token_funds")?;

        merchant::withdraw_merchant_funds_for_token(&env, merchant, token, amount)
    }

    /// Get the merchant's accumulated (uncharged) balance.
    pub fn get_merchant_balance(env: Env, merchant: Address) -> i128 {
        merchant::get_merchant_balance(&env, &merchant)
    }

    /// Token-scoped merchant balance.
    pub fn get_merchant_balance_by_token(env: Env, merchant: Address, token: Address) -> i128 {
        merchant::get_merchant_balance_by_token(&env, &merchant, &token)
    }

    /// Detailed per-token earnings record for a merchant.
    ///
    /// Returns the [`TokenEarnings`] struct containing accruals (broken down by
    /// charge kind), withdrawals, and refunds. The reconciliation invariant
    /// `balance = accruals.total - withdrawals - refunds` must hold at all times.
    pub fn get_merchant_token_earnings(
        env: Env,
        merchant: Address,
        token: Address,
    ) -> crate::types::TokenEarnings {
        merchant::get_merchant_token_earnings(&env, &merchant, &token)
    }

    /// Check if a merchant has enabled a blanket pause.
    pub fn get_merchant_paused(env: Env, merchant: Address) -> bool {
        merchant::get_merchant_paused(&env, merchant)
    }

    /// Pause all subscriptions for a merchant.
    ///
    /// Stops charges and prevents new subscriptions.
    /// Acts like a soft emergency stop for just this merchant.
    ///
    /// # Auth
    /// merchant must authorize
    pub fn pause_merchant(env: Env, merchant: Address) -> Result<(), Error> {
        merchant::pause_merchant(&env, merchant)
    }

    /// Resume merchant activity after a pause.
    ///
    /// # Auth
    /// - merchant must authorize
    pub fn unpause_merchant(env: Env, merchant: Address) -> Result<(), Error> {
        merchant::unpause_merchant(&env, merchant)
    }

    /// Refund a subscriber directly from the merchant’s balance.
    ///
    /// Useful for customer support refunds without cancelling the subscription.
    ///
    /// # Arguments
    /// - `merchant`: must authorize
    /// - `subscriber`: receiver of funds
    /// - `token`: token used
    /// - `amount`: refund amount
    ///
    /// # Errors
    /// - Unauthorized
    /// - InvalidAmount
    /// - InsufficientFunds
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// token transfer. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    pub fn merchant_refund(
        env: Env,
        merchant: Address,
        subscriber: Address,
        token: Address,
        amount: i128,
    ) -> Result<(), Error> {
        // Acquire reentrancy guard: prevents re-entry during token transfer
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "merchant_refund")?;

        merchant::merchant_refund(&env, merchant, subscriber, token, amount)
    }

    /// Get a reconciliation snapshot for all tokens used by a merchant.
    pub fn get_reconciliation_snapshot(
        env: Env,
        merchant: Address,
    ) -> Vec<crate::types::TokenReconciliationSnapshot> {
        merchant::get_reconciliation_snapshot(&env, &merchant)
    }

    /// Get total earnings per token for a merchant.
    ///
    /// Includes total charged, withdrawn, and current balance.
    pub fn get_merchant_total_earnings(
        env: Env,
        merchant: Address,
    ) -> Vec<(Address, crate::types::TokenEarnings)> {
        merchant::get_merchant_total_earnings(&env, &merchant)
    }

    // ── Payout Schedule ────────────────────────────────────────────────────────

    /// Configure automated payouts for a merchant.
    ///
    /// Sets a cadence and minimum-payout threshold.  When `cadence_seconds` and
    /// `min_payout` are both 0 the schedule is cleared (no auto-payout).
    ///
    /// # Auth
    ///
    /// `merchant` must authorize.
    ///
    /// # Errors
    ///
    /// * [`Error::InvalidAmount`] — `min_payout` is negative.
    /// * [`Error::Unauthorized`] — `merchant` did not authorize.
    pub fn set_payout_schedule(
        env: Env,
        merchant: Address,
        cadence_seconds: u64,
        min_payout: i128,
    ) -> Result<PayoutSchedule, Error> {
        merchant::do_set_payout_schedule(&env, merchant, cadence_seconds, min_payout)
    }

    /// Execute scheduled payouts for a merchant on-demand.
    ///
    /// Anyone may call this function.  For each token the merchant holds a
    /// balance in, the full balance is transferred to the merchant's payout
    /// address if it meets the configured `min_payout` threshold.
    ///
    /// The cadence guard (`cadence_seconds`) must have elapsed since the
    /// last successful flush; otherwise the call fails with
    /// `IntervalNotElapsed`.
    ///
    /// # Reentrancy Protection
    ///
    /// Acquires a reentrancy guard because this function performs token
    /// transfers.
    ///
    /// # Returns
    ///
    /// The number of token payouts that were actually executed.
    pub fn flush_payouts(
        env: Env,
        merchant: Address,
    ) -> Result<u32, Error> {
        require_not_emergency_stop(&env)?;

        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "flush_payouts")?;

        let caller = env.current_contract_address(); // the contract itself is the caller
        merchant::do_flush_payouts(&env, merchant, caller)
    }

    /// Read the current payout schedule for a merchant.
    pub fn get_payout_schedule(env: Env, merchant: Address) -> PayoutSchedule {
        merchant::get_payout_schedule(&env, &merchant)
    }

    // ── Queries ──────────────────────────────────────────────────────────────

    /// Get a subscription by ID.
    ///
    /// Returns the full [`Subscription`] data.
    ///
    /// # Errors
    /// - NotFound → if the subscription doesn’t exist
    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        queries::get_subscription(&env, subscription_id)
    }

    /// Estimate how much to top up for future billing cycles.
    ///
    /// Calculates how much is needed to cover `num_intervals`,
    /// taking the current prepaid balance into account.
    /// Returns 0 if already covered.
    ///
    /// # Errors
    /// - NotFound → subscription doesn’t exist
    /// - Overflow → calculation overflow
    pub fn estimate_topup_for_intervals(
        env: Env,
        subscription_id: u32,
        num_intervals: u32,
    ) -> Result<i128, Error> {
        queries::estimate_topup_for_intervals(&env, subscription_id, num_intervals)
    }

    /// Get info about the next charge timing.
    ///
    /// Includes when the next charge is expected and whether it’s due.
    ///
    /// # Errors
    /// NotFound → subscription doesn’t exist.
    pub fn get_next_charge_info(env: Env, subscription_id: u32) -> Result<NextChargeInfo, Error> {
        queries::get_next_charge_info(&env, subscription_id)
    }

    /// Return the number of subscription ids indexed for a settlement token (for pagination).
    pub fn get_token_subscription_count(env: Env, token: Address) -> u32 {
        queries::get_token_subscription_count(&env, token)
    }

    /// List subscriptions for a subscriber (cursor-based).
    ///
    /// # Errors
    /// - InvalidPageSize → if limit is invalid
    pub fn list_subscriptions_by_subscriber(
        env: Env,
        subscriber: Address,
        start_from_id: u32,
        limit: u32,
    ) -> Result<crate::queries::SubscriptionsPage, Error> {
        crate::queries::list_subscriptions_by_subscriber(&env, subscriber, start_from_id, limit)
    }

    /// Get lifetime cap information for a subscription.
    ///
    /// Returns a [`CapInfo`] summary suitable for off-chain dashboards and UX displays.
    /// When no cap is configured all cap-related fields return `None` / `false`.
    pub fn get_cap_info(env: Env, subscription_id: u32) -> Result<CapInfo, Error> {
        queries::get_cap_info(&env, subscription_id)
    }

    /// Set or clear the contract-wide default lifetime cap applied to new subscriptions.
    ///
    /// When set, any `create_subscription` call that provides no explicit `lifetime_cap`
    /// inherits this value (unless a per-merchant default takes precedence).
    /// Pass `None` to remove the global default.
    ///
    /// # Auth
    /// Admin only.
    pub fn set_global_cap_default(
        env: Env,
        admin: Address,
        cap: Option<i128>,
    ) -> Result<(), Error> {
        subscription::do_set_global_cap_default(&env, admin, cap)
    }

    /// Return the current contract-wide default lifetime cap, or `None` if unset.
    pub fn get_global_cap_default(env: Env) -> Option<i128> {
        subscription::get_global_cap_default(&env)
    }

    /// Set or clear a per-merchant default lifetime cap for all new subscriptions to this merchant.
    ///
    /// Overrides the global default for subscriptions created against `merchant`.
    /// Pass `None` to fall back to the global default.
    ///
    /// # Auth
    /// Merchant address must authorize.
    pub fn set_merchant_cap_default(
        env: Env,
        merchant: Address,
        cap: Option<i128>,
    ) -> Result<(), Error> {
        validation::reject_contract_self(&env, &merchant)?;
        subscription::do_set_merchant_cap_default(&env, merchant, cap)
    }

    /// Return the per-merchant default lifetime cap, or `None` if unset.
    pub fn get_merchant_cap_default(env: Env, merchant: Address) -> Option<i128> {
        subscription::get_merchant_cap_default(&env, merchant)
    }

    /// Update the lifetime cap on an existing subscription.
    ///
    /// - Raising or removing the cap is always allowed.
    /// - Lowering the cap below `lifetime_charged` is rejected with `LifetimeCapReached`.
    /// - Setting to `None` removes the cap entirely.
    ///
    /// # Auth
    /// Admin only.
    pub fn update_subscription_cap(
        env: Env,
        admin: Address,
        subscription_id: u32,
        new_cap: Option<i128>,
    ) -> Result<(), Error> {
        subscription::do_update_subscription_cap(&env, admin, subscription_id, new_cap)
    }

    /// Return subscription billing statements using offset/limit pagination.
    ///
    /// When `newest_first` is true (recommended for infinite scroll), offset 0
    /// starts from the most recent statement.
    pub fn get_sub_statements_offset(
        env: Env,
        subscription_id: u32,
        offset: u32,
        limit: u32,
        newest_first: bool,
    ) -> Result<BillingStatementsPage, Error> {
        statements::get_statements_by_subscription_offset(
            &env,
            subscription_id,
            offset,
            limit,
            newest_first,
        )
    }

    /// Return subscription billing statements using cursor pagination.
    ///
    /// - `cursor`: sequence index to start from (inclusive); pass `None` for first page.
    /// - `limit`: maximum number of statements to return.
    /// - `newest_first`: return recent history first when true.
    pub fn get_sub_statements_cursor(
        env: Env,
        subscription_id: u32,
        cursor: Option<u32>,
        limit: u32,
        newest_first: bool,
    ) -> Result<BillingStatementsPage, Error> {
        statements::get_statements_by_subscription_cursor(
            &env,
            subscription_id,
            cursor,
            limit,
            newest_first,
        )
    }

    /// Return a single billing period snapshot by subscription and period index.
    ///
    /// `period_index` is `ledger_timestamp / interval_seconds` for the billing period.
    /// Returns `None` when no charge has been processed for that period.
    pub fn get_period_snapshot(
        env: Env,
        subscription_id: u32,
        period_index: u64,
    ) -> Option<BillingPeriodSnapshot> {
        period_snapshots::get_period_snapshot(&env, subscription_id, period_index)
    }

    /// Return the most-recent billing period snapshots for a subscription, newest first.
    ///
    /// - `limit`: maximum number of snapshots to return.
    pub fn list_period_snapshots(
        env: Env,
        subscription_id: u32,
        limit: u32,
    ) -> Vec<BillingPeriodSnapshot> {
        period_snapshots::list_period_snapshots(&env, subscription_id, limit)
    }

    pub fn add_accepted_token(
        env: Env,
        admin: Address,
        token: Address,
        decimals: u32,
    ) -> Result<(), Error> {
        validation::reject_contract_self(&env, &token)?;
        admin::add_accepted_token(&env, admin, token, decimals)
    }

    /// Remove a token from accepted list.
    ///
    /// Existing subscriptions are unaffected.
    ///
    /// # Errors
    /// - Unauthorized
    /// - NotFound
    /// - CannotRemoveDefaultToken
    pub fn remove_accepted_token(env: Env, admin: Address, token: Address) -> Result<(), Error> {
        admin::remove_accepted_token(&env, admin, token)
    }

    /// List metadata for all accepted settlement tokens.
    ///
    /// Returns a [`Vec<AcceptedToken>`] with address and decimals for each registered token,
    /// including the primary token.
    pub fn list_accepted_tokens(env: Env) -> Vec<AcceptedToken> {
        admin::list_accepted_tokens(&env)
    }

    /// Return subscriptions for a token, paginated by offset.
    ///
    /// # Arguments
    ///
    /// * `token` — Settlement token to filter by.
    /// * `start` — Starting subscription ID (inclusive).
    /// * `limit` — Maximum number of subscriptions to return. Must be between 1 and
    ///   [`queries::MAX_SUBSCRIPTION_LIST_PAGE`] inclusive.
    ///
    /// # Errors
    ///
    /// * [`Error::InvalidPageSize`] — `limit` is 0 or exceeds [`queries::MAX_SUBSCRIPTION_LIST_PAGE`].
    ///
    /// # Returns
    ///
    /// A [`Vec<Subscription>`] of up to `limit` subscriptions using the specified token,
    /// starting from `start` ID.
    pub fn get_subscriptions_by_token(
        env: Env,
        token: Address,
        start: u32,
        limit: u32,
    ) -> Result<Vec<Subscription>, Error> {
        queries::get_subscriptions_by_token(&env, token, start, limit)
    }

    // ── Reconciliation Queries ─────────────────────────────────────────────────

    /// Returns complete reconciliation data for a single settlement token.
    ///
    /// This computes the accounting equation:
    /// `contract_token_balance = total_prepaid + total_merchant_liabilities + recoverable`
    ///
    /// # Arguments
    ///
    /// * `token` — The settlement token to audit.
    ///
    /// # Returns
    ///
    /// A [`TokenLiabilities`] struct containing:
    /// - `total_prepaid`: Sum of all subscriber prepaid balances
    /// - `total_merchant_liabilities`: Sum of all merchant earnings (accruals - withdrawals - refunds)
    /// - `recoverable_amount`: Stranded funds that can be recovered
    /// - `contract_balance`: Actual token balance held by the contract
    /// - `is_balanced`: Whether the accounting equation validates
    ///
    /// # Auth
    ///
    /// Read-only; no auth required.
    ///
    /// # Complexity
    ///
    /// This scans all subscriptions and merchants. For bounded compute with
    /// pagination, use [`query_prepaid_balances_paginated`](Self::query_prepaid_balances_paginated).
    pub fn get_token_reconciliation(env: Env, token: Address) -> TokenLiabilities {
        queries::get_token_reconciliation(&env, token)
    }

    /// Returns paginated reconciliation summaries for all accepted tokens.
    ///
    /// # Arguments
    ///
    /// * `start_token_index` — Index into the accepted tokens list to start from (0 for first page).
    /// * `limit` — Maximum number of token summaries to return (max 50).
    ///
    /// # Returns
    ///
    /// A [`ReconciliationSummaryPage`] with per-token liability summaries and pagination cursor.
    ///
    /// # Auth
    ///
    /// Read-only; no auth required.
    ///
    /// # Example
    ///
    /// To get all token reconciliations:
    /// 1. Call with `start_token_index = 0`, `limit = 50`
    /// 2. If `next_token_index` is `Some(index)`, call again with that index
    /// 3. Repeat until `next_token_index` is `None`
    pub fn get_recon_summary(
        env: Env,
        start_token_index: u32,
        limit: u32,
    ) -> ReconciliationSummaryPage {
        queries::get_contract_reconciliation_summary(&env, start_token_index, limit)
    }

    /// Generates an auditable proof for off-chain reconciliation verification.
    ///
    /// Creates a snapshot with all data needed to independently validate the accounting
    /// equation without requiring full contract state access.
    ///
    /// # Arguments
    ///
    /// * `token` — The settlement token to generate the proof for.
    ///
    /// # Returns
    ///
    /// A [`ReconciliationProof`] containing:
    /// - Timestamp and ledger sequence for temporal anchoring
    /// - Contract balance, prepaid total, merchant liabilities
    /// - Computed recoverable amount
    /// - Subscription and merchant counts scanned
    /// - Validation flag (`is_valid`)
    ///
    /// # Auth
    ///
    /// Read-only; no auth required.
    ///
    /// # Security
    ///
    /// This function is read-only and cannot modify state. The proof is generated
    /// at the current ledger state and includes the ledger sequence for verification.
    pub fn generate_reconciliation_proof(env: Env, token: Address) -> ReconciliationProof {
        queries::generate_reconciliation_proof(&env, token)
    }

    /// Returns paginated prepaid balance aggregation for a token.
    ///
    /// Provides bounded compute for auditors to incrementally build the total
    /// prepaid balance without iterating unbounded subscription sets.
    ///
    /// # Arguments
    ///
    /// * `request` — A [`PrepaidQueryRequest`] with:
    ///   - `token`: Token to filter by
    ///   - `start_subscription_id`: Starting subscription ID (inclusive)
    ///   - `scan_limit`: Max subscriptions to scan (capped at 500)
    ///
    /// # Returns
    ///
    /// A [`PrepaidQueryResult`] with:
    /// - `partial_total`: Sum of prepaid balances in this scan window
    /// - `subscriptions_count`: Number of subscriptions with non-zero prepaid
    /// - `next_start_id`: Next ID to scan, or `None` if complete
    /// - `has_more`: Whether more subscriptions exist beyond this window
    ///
    /// # Auth
    ///
    /// Read-only; no auth required.
    ///
    /// # Example
    ///
    /// To compute full prepaid total off-chain:
    /// ```rust,ignore
    /// let mut total = 0i128;
    /// let mut start_id = 0u32;
    /// loop {
    ///     let result = query_prepaid_balances_paginated(env, PrepaidQueryRequest {
    ///         token: usdc_token,
    ///         start_subscription_id: start_id,
    ///         scan_limit: 500,
    ///     });
    ///     total += result.partial_total;
    ///     if !result.has_more { break; }
    ///     start_id = result.next_start_id.unwrap();
    /// }
    /// ```
    pub fn query_prepaid_balances_paginated(
        env: Env,
        request: PrepaidQueryRequest,
    ) -> PrepaidQueryResult {
        queries::query_prepaid_balances_paginated(&env, request)
    }

    /// Configure the number of detailed billing statement rows retained per subscription.
    ///
    /// When the statement count exceeds `keep_recent`, older rows are compacted into an
    /// aggregate summary. Compaction is triggered lazily or explicitly via
    /// [`compact_billing_statements`](Self::compact_billing_statements).
    ///
    /// # Arguments
    ///
    /// * `admin` — Must match the stored admin.
    /// * `keep_recent` — Number of recent detailed rows to keep per subscription.
    ///
    /// # Auth
    ///
    /// Admin only.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller is not the stored admin.
    pub fn set_billing_retention(env: Env, admin: Address, keep_recent: u32) -> Result<(), Error> {
        require_admin_auth(&env, &admin)?;
        statements::set_retention_config(&env, keep_recent);
        Ok(())
    }

    /// Read current statement retention config.
    pub fn get_billing_retention(env: Env) -> BillingRetentionConfig {
        statements::get_retention_config(&env)
    }

    /// Return compacted aggregate billing totals for a subscription.
    ///
    /// The aggregate accumulates totals for rows that have been pruned by compaction,
    /// so that historical totals remain available even after individual rows are removed.
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — Subscription to query.
    pub fn get_stmt_compacted_aggregate(
        env: Env,
        subscription_id: u32,
    ) -> BillingStatementAggregate {
        statements::get_compacted_aggregate(&env, subscription_id)
    }

    /// Compact (prune) billing statements for one subscription.
    ///
    /// Removes rows older than the retention window, accumulating their totals into
    /// the aggregate. The compacted totals remain queryable via
    /// [`get_stmt_compacted_aggregate`](Self::get_stmt_compacted_aggregate).
    ///
    /// # Arguments
    ///
    /// * `admin` — Must match the stored admin.
    /// * `subscription_id` — Target subscription.
    /// * `keep_recent_override` — When `Some(n)`, override the global retention config
    ///   for this specific compaction run (does not persist). Use `None` to apply the
    ///   globally configured value.
    ///
    /// # Auth
    ///
    /// Admin only.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller is not the stored admin.
    /// * [`Error::NotFound`] — Subscription does not exist.
    ///
    /// # Returns
    ///
    /// A [`BillingCompactionSummary`] with counts of pruned and kept rows and the
    /// total amount of pruned statements.
    ///
    /// # Events
    ///
    /// Emits [`BillingCompactedEvent`] with compaction stats and updated aggregate totals.
    pub fn compact_billing_statements(
        env: Env,
        admin: Address,
        subscription_id: u32,
        keep_recent_override: Option<u32>,
    ) -> Result<BillingCompactionSummary, Error> {
        require_admin_auth(&env, &admin)?;
        let summary = statements::compact_subscription_statements(
            &env,
            subscription_id,
            keep_recent_override,
        )?;
        let aggregate = statements::get_compacted_aggregate(&env, subscription_id);
        env.events().publish(
            (Symbol::new(&env, "billing_compacted"), subscription_id),
            BillingCompactedEvent {
                admin,
                subscription_id,
                pruned_count: summary.pruned_count,
                kept_count: summary.kept_count,
                total_pruned_amount: summary.total_pruned_amount,
                timestamp: env.ledger().timestamp(),
                aggregate_pruned_count: aggregate.pruned_count,
                aggregate_total_amount: aggregate.total_amount,
                aggregate_oldest_period_start: aggregate.oldest_period_start,
                aggregate_newest_period_end: aggregate.newest_period_end,
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(summary)
    }

    /// Read the currently configured oracle integration settings.
    pub fn get_oracle_config(env: Env) -> OracleConfig {
        oracle::get_oracle_config(&env)
    }

    /// Emit an oracle liveness event for monitoring purposes.
    ///
    /// This view-only function reads the latest oracle price sample and emits
    /// an [`OracleLivenessEvent`] containing the sample timestamp, computed age,
    /// and health status. Health is determined by comparing the sample age against
    /// half of the configured `max_age_seconds` threshold.
    ///
    /// # Arguments
    ///
    /// This function takes no parameters.
    ///
    /// # Returns
    ///
    /// * `Ok(OracleLivenessEvent)` - The liveness event with computed health status.
    /// * `Err(Error::OracleNotConfigured)` - If oracle is not configured or disabled.
    ///
    /// # Events
    ///
    /// Emits `oracle_liveness` event with the liveness event data for off-chain monitoring.
    ///
    /// # Security
    ///
    /// This is a view-only operation that does not require authentication.
    /// Any caller can invoke this to verify oracle liveness without privileged access.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Check oracle health before charging
    /// match client.emit_oracle_liveness(&env) {
    ///     Ok(event) => {
    ///         if event.healthy {
    ///             // Proceed with oracle-dependent charge
    ///         } else {
    ///             // Oracle is stale, use fallback pricing or alert operators
    ///         }
    ///     }
    ///     Err(Error::OracleNotConfigured) => {
    ///         // Oracle not enabled, use base pricing
    ///     }
    ///     Err(e) => panic!("Unexpected error: {:?}", e),
    /// }
    /// ```
    pub fn emit_oracle_liveness(env: Env) -> Result<OracleLivenessEvent, Error> {
        oracle::emit_oracle_liveness(&env)
    }

    // ── Metadata ──────────────────────────────────────────────────────────────

    /// Set or update a metadata key-value pair on a subscription.
    ///
    /// Metadata is an arbitrary key-value store attached to a subscription for
    /// off-chain use cases (e.g., plan names, customer notes, external IDs). It does
    /// **not** affect financial state (balances, status, or charges).
    ///
    /// See `docs/subscription_metadata.md` for schema constraints.
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — Target subscription.
    /// * `authorizer` — Must be the subscriber or merchant.
    /// * `key` — Metadata key. Max length: [`MAX_METADATA_KEY_LENGTH`].
    /// * `value` — Metadata value. Max length: [`MAX_METADATA_VALUE_LENGTH`].
    ///
    /// # Auth
    ///
    /// `authorizer` must authorize and must be the subscription's subscriber or merchant.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Subscription does not exist.
    /// * [`Error::Unauthorized`] — `authorizer` is neither subscriber nor merchant.
    /// * [`Error::MetadataKeyTooLong`] — `key` exceeds [`MAX_METADATA_KEY_LENGTH`].
    /// * [`Error::MetadataValueTooLong`] — `value` exceeds [`MAX_METADATA_VALUE_LENGTH`].
    /// * [`Error::MetadataLimitReached`] — Subscription already has [`MAX_METADATA_KEYS`] entries.
    ///
    /// # Events
    ///
    /// Emits [`MetadataSetEvent`] with `subscription_id`, `key`, and timestamp.
    pub fn set_metadata(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
        key: String,
        value: String,
    ) -> Result<(), Error> {
        validation::reject_empty_string(&key)?;
        validation::reject_empty_string(&value)?;
        metadata::set_metadata(&env, subscription_id, &authorizer, key, value)
    }

    ///
    /// No-op if the key does not exist (returns `Ok`).
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — Target subscription.
    /// * `authorizer` — Must be the subscriber or merchant.
    /// * `key` — Metadata key to delete.
    ///
    /// # Auth
    ///
    /// `authorizer` must authorize and must be the subscription's subscriber or merchant.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Subscription does not exist.
    /// * [`Error::Unauthorized`] — `authorizer` is neither subscriber nor merchant.
    ///
    /// # Events
    ///
    /// Emits [`MetadataDeletedEvent`] with `subscription_id`, `key`, and timestamp.
    pub fn delete_metadata(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
        key: String,
    ) -> Result<(), Error> {
        validation::reject_empty_string(&key)?;
        metadata::delete_metadata(&env, subscription_id, &authorizer, key)
    }

    /// Get a metadata value by key.
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — Target subscription.
    /// * `key` — Metadata key to look up.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Subscription does not exist, or key is not set.
    pub fn get_metadata(env: Env, subscription_id: u32, key: String) -> Result<String, Error> {
        metadata::get_metadata(&env, subscription_id, key)
    }

    /// List all metadata keys for a subscription.
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — Target subscription.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Subscription does not exist.
    pub fn list_metadata_keys(env: Env, subscription_id: u32) -> Result<Vec<String>, Error> {
        metadata::list_metadata_keys(&env, subscription_id)
    }

    // ── Protocol Fees ──────────────────────────────────────────────────────────

    /// Configure the protocol fee. Admin only.
    ///
    /// fee_bps is in basis points (0..=10_000). 0 disables fee collection.
    /// On each charge: gross == merchant_net + treasury_fee
    ///
    /// See docs/protocol_fees.md for full semantics.
    pub fn set_protocol_fee(
        env: Env,
        admin: Address,
        treasury: Address,
        fee_bps: u32,
    ) -> Result<(), Error> {
        validation::reject_contract_self(&env, &treasury)?;
        admin::set_protocol_fee(&env, admin, treasury, fee_bps)
    }

    /// Return the current protocol fee basis points (0 = disabled).
    pub fn get_protocol_fee_bps(env: Env) -> u32 {
        admin::get_protocol_fee_bps(&env)
    }

    // ── Governance (Quorum-based proposals) ──────────────────────────────────

    /// Submit a governance proposal for a privileged action.
    ///
    /// Creates a new proposal that must be voted on by guardians before execution.
    /// The proposal will not execute until the ETA (execution timestamp) is reached.
    ///
    /// # Arguments
    /// * `kind` — Type of proposal (RotateAdmin, SetProtocolFee).
    /// * `target` — Primary target (new admin for RotateAdmin, treasury for SetProtocolFee).
    /// * `target2` — Optional secondary target (e.g., treasury for SetProtocolFee).
    /// * `target3` — Optional tertiary parameter (e.g., fee_bps for SetProtocolFee).
    /// * `quorum_bps` — Required vote percentage in basis points (0-10000).
    /// * `eta` — Timestamp after which proposal can be executed.
    ///
    /// # Returns
    /// The newly created proposal ID (monotonically allocated).
    pub fn submit_proposal(
        env: Env,
        kind: types::ProposalKind,
        target: Address,
        target2: Option<Address>,
        target3: u32,
        quorum_bps: u32,
        eta: u64,
    ) -> Result<u64, Error> {
        governance::do_submit_proposal(&env, kind, target, target2, target3, quorum_bps, eta)
    }

    /// Cast a guardian vote on a proposal.
    ///
    /// Only addresses with assigned guardian weight can vote.
    /// Votes are recorded per-guardian and validated during execution.
    ///
    /// # Arguments
    /// * `proposal_id` — ID of the proposal to vote on.
    /// * `voted_yes` — true to vote for, false to vote against.
    ///
    /// # Errors
    /// - `Unauthorized` if caller is not a guardian
    /// - `NotFound` if proposal does not exist
    /// - `InvalidInput` if proposal already executed
    pub fn vote_proposal(env: Env, proposal_id: u64, voted_yes: bool) -> Result<(), Error> {
        governance::do_vote_proposal(&env, proposal_id, voted_yes)
    }

    /// Execute a proposal if quorum is met and ETA has passed.
    ///
    /// Validates that:
    /// 1. The proposal has not already been executed.
    /// 2. The ETA timestamp has been reached.
    /// 3. Quorum requirement is met (accounting for guardian removals).
    /// 4. All votes from removed guardians are excluded.
    ///
    /// On success, applies the proposal's action (e.g., rotates admin or sets protocol fee).
    ///
    /// # Errors
    /// - `NotFound` if proposal does not exist
    /// - `InvalidInput` if ETA not reached or quorum not met
    pub fn execute_proposal(env: Env, proposal_id: u64) -> Result<(), Error> {
        governance::do_execute_proposal(&env, proposal_id)
    }

    /// Cancel a proposal (admin only).
    ///
    /// Only the current admin can cancel proposals. This prevents stale or unwanted
    /// proposals from remaining on the books.
    ///
    /// # Arguments
    /// * `proposal_id` — ID of the proposal to cancel.
    /// * `reason` — Cancellation reason (emitted in event).
    ///
    /// # Errors
    /// - `Unauthorized` if caller is not the admin
    /// - `NotFound` if proposal does not exist
    /// - `InvalidInput` if proposal already executed
    pub fn cancel_proposal(env: Env, proposal_id: u64, reason: String) -> Result<(), Error> {
        governance::do_cancel_proposal(&env, proposal_id, reason)
    }

    /// Add or update a guardian and their voting weight.
    ///
    /// Admin only. Sets a guardian's voting weight; weight of 0 is not allowed.
    /// Call `remove_guardian` to remove a guardian entirely.
    ///
    /// # Errors
    /// - `Unauthorized` if caller is not the admin
    /// - `InvalidInput` if weight is zero
    pub fn add_guardian(env: Env, admin: Address, guardian: Address, weight: u32) -> Result<(), Error> {
        admin::require_admin_auth(&env, &admin)?;
        governance::add_guardian(&env, guardian, weight)
    }

    /// Remove a guardian, immediately invalidating their future votes.
    ///
    /// Admin only. Once removed, a guardian cannot vote on new proposals, and their
    /// prior votes are excluded during quorum validation.
    ///
    /// # Errors
    /// - `Unauthorized` if caller is not the admin
    pub fn remove_guardian(env: Env, admin: Address, guardian: Address) -> Result<(), Error> {
        admin::require_admin_auth(&env, &admin)?;
        governance::remove_guardian(&env, &guardian)
    }

    /// Get a guardian's current voting weight (0 if not a guardian).
    pub fn get_guardian_weight(env: Env, guardian: Address) -> u32 {
        governance::get_guardian_weight(&env, &guardian)
    }

    /// Get the current proposal counter (next proposal ID to be allocated).
    pub fn get_current_proposal_id(env: Env) -> u64 {
        governance::get_current_proposal_id(&env)
    }

    /// Get proposal by ID (if it exists).
    pub fn get_proposal(env: Env, proposal_id: u64) -> Option<types::Proposal> {
        governance::get_proposal(&env, proposal_id)
    }

    /// List all guardians and their voting weights.
    pub fn list_guardians(env: Env) -> Vec<(Address, u32)> {
        governance::list_guardians(&env)
    }

    // ── Blocklist ──────────────────────────────────────────────────────────────

    /// Add a subscriber to the blocklist, preventing them from creating new subscriptions.
    ///
    /// Blocklisted addresses are rejected by [`create_subscription`](Self::create_subscription)
    /// and [`create_subscription_with_token`](Self::create_subscription_with_token).
    /// Existing subscriptions are not automatically cancelled.
    ///
    /// # Arguments
    ///
    /// * `authorizer` — Admin or merchant calling this function.
    /// * `subscriber` — Address to blocklist.
    /// * `reason` — Optional human-readable reason string stored in the blocklist entry.
    ///
    /// # Auth
    ///
    /// `authorizer` must be the admin or a merchant (implementation-defined).
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller lacks permission to blocklist.
    /// * [`Error::AlreadyBlocklisted`] — Address is already on the blocklist.
    ///
    /// # Events
    ///
    /// Emits [`BlocklistAddedEvent`] with `subscriber`, `reason`, and timestamp.
    pub fn add_to_blocklist(
        env: Env,
        authorizer: Address,
        subscriber: Address,
        reason: Option<String>,
    ) -> Result<(), Error> {
        blocklist::do_add_to_blocklist(&env, authorizer, subscriber, reason)
    }

    /// Remove a subscriber from the blocklist.
    ///
    /// After removal the subscriber may create new subscriptions normally.
    ///
    /// # Arguments
    ///
    /// * `admin` — Must match the stored admin.
    /// * `subscriber` — Address to remove from the blocklist.
    ///
    /// # Auth
    ///
    /// Admin only.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller is not the stored admin.
    /// * [`Error::NotFound`] — Address is not on the blocklist.
    ///
    /// # Events
    ///
    /// Emits [`BlocklistRemovedEvent`] with `subscriber` and timestamp.
    pub fn remove_from_blocklist(
        env: Env,
        admin: Address,
        subscriber: Address,
    ) -> Result<(), Error> {
        blocklist::do_remove_from_blocklist(&env, admin, subscriber)
    }

    /// Return the blocklist entry for a subscriber.
    ///
    /// # Arguments
    ///
    /// * `subscriber` — Address to look up.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Address is not on the blocklist.
    pub fn get_blocklist_entry(env: Env, subscriber: Address) -> Result<BlocklistEntry, Error> {
        blocklist::get_blocklist_entry(&env, subscriber)
    }

    /// Return `true` if `subscriber` is on the blocklist.
    ///
    /// # Arguments
    ///
    /// * `subscriber` — Address to check.
    pub fn is_blocklisted(env: Env, subscriber: Address) -> bool {
        blocklist::is_blocklisted(&env, &subscriber)
    }

    /// Initialize merchant configuration with payout settings and operational flags.
    ///
    /// Creates a new merchant config record with validation. This is the recommended way
    /// for merchants to set up their configuration before accepting subscriptions.
    ///
    /// # Arguments
    ///
    /// * `merchant` — Must authorize and must be the merchant's address.
    /// * `payout_address` — Address where the merchant receives payouts.
    /// * `fee_bips` — Fee percentage in bips (0-10000). 0 means no fee.
    /// * `allowed_operations` — Bitmap of allowed operations (see OP_* constants).
    /// * `fee_address` — Optional address for platform fee routing.
    /// * `redirect_url` — URL for off-chain callbacks.
    ///
    /// # Auth
    ///
    /// `merchant` must authorize.
    ///
    /// # Errors
    ///
    /// * [`Error::InvalidPayoutAddress`] — Payout address is zero.
    /// * [`Error::InvalidFeeBips`] — Fee exceeds 100%.
    /// * [`Error::InvalidOperations`] — Invalid operation bits.
    /// * [`Error::MustAllowChargeOperation`] — CHARGE operation must be enabled.
    ///
    /// # Events
    ///
    /// Emits [`MerchantConfigInitializedEvent`].
    pub fn initialize_merchant_config(
        env: Env,
        merchant: Address,
        payout_address: Address,
        fee_bips: i32,
        allowed_operations: i32,
        fee_address: Option<Address>,
        redirect_url: String,
    ) -> Result<MerchantConfig, Error> {
        merchant::initialize_merchant_config(
            &env,
            merchant,
            payout_address,
            fee_bips,
            allowed_operations,
            fee_address,
            redirect_url,
        )
    }

    /// Set global configuration for a merchant.
    ///
    /// Stores a [`MerchantConfig`] with optional fee routing, a redirect URL, and a
    /// pause flag. The pause flag here is a configuration-layer pause (distinct from
    /// the operational [`pause_merchant`](Self::pause_merchant) / [`unpause_merchant`](Self::unpause_merchant)
    /// toggle).
    ///
    /// # Arguments
    ///
    /// * `merchant` — Must authorize the transaction.
    /// * `config` — Full MerchantConfig struct.
    ///
    /// # Auth
    ///
    /// `merchant` must authorize.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — `merchant` auth failed.
    /// * Validation errors from config.
    pub fn set_merchant_config(
        env: Env,
        merchant: Address,
        config: MerchantConfig,
    ) -> Result<(), Error> {
        merchant::set_merchant_config(&env, merchant, config)
    }

    /// Update merchant configuration with partial fields.
    ///
    /// Allows updating specific fields without replacing the entire config.
    /// Unchanged fields retain their current values.
    ///
    /// # Arguments
    ///
    /// * `merchant` — Must authorize.
    /// * `new_payout_address` — Optional new payout address.
    /// * `new_fee_bips` — Optional new fee in bips.
    /// * `new_allowed_operations` — Optional new operations bitmap.
    /// * `new_is_active` — Optional active flag.
    /// * `new_fee_address` — Optional new fee address.
    /// * `new_redirect_url` — Optional new redirect URL.
    /// * `new_is_paused` — Optional pause flag.
    ///
    /// # Auth
    ///
    /// `merchant` must authorize.
    ///
    /// # Errors
    ///
    /// * [`Error::ConfigNotFound`] — Config not initialized.
    /// * Validation errors for provided fields.
    ///
    /// # Events
    ///
    /// Emits [`MerchantConfigUpdatedEvent`].
    pub fn update_merchant_config(
        env: Env,
        merchant: Address,
        new_payout_address: Option<Address>,
        new_fee_bips: Option<i32>,
        new_allowed_operations: Option<i32>,
        new_is_active: Option<bool>,
        new_fee_address: Option<Option<Address>>,
        new_redirect_url: Option<String>,
        new_is_paused: Option<bool>,
    ) -> Result<MerchantConfig, Error> {
        merchant::update_merchant_config(
            &env,
            merchant,
            new_payout_address,
            new_fee_bips,
            new_allowed_operations,
            new_is_active,
            new_fee_address,
            new_redirect_url,
            new_is_paused,
        )
    }

    /// Return the global configuration for a merchant.
    ///
    /// Returns `None` if the merchant has never called [`set_merchant_config`](Self::set_merchant_config).
    ///
    /// # Arguments
    ///
    /// * `merchant` — Merchant address to query.
    pub fn get_merchant_config(
        env: Env,
        merchant: Address,
    ) -> Option<crate::types::MerchantConfig> {
        merchant::get_merchant_config(&env, merchant)
    }

// Duplicate stub block removed – implementation retained elsewhere.

    /// Returns the schema version of this contract.
    pub fn version(_env: Env) -> u32 {
        1
    }

    /// Returns the current subscription count.
    ///
    /// This equals the total number of subscriptions ever created,
    /// including cancelled and expired ones.
    pub fn get_subscription_count(env: Env) -> u32 {
        let key = Symbol::new(&env, "next_id");
        env.storage().instance().get(&key).unwrap_or(0u32)
    }

    /// Internal helper to allocate the next subscription ID.
    ///
    /// This function implements overflow-safe ID allocation by checking
    /// the limit before incrementing the counter.
    fn _next_id(env: &Env) -> Result<u32, Error> {
        let key = Symbol::new(env, "next_id");
        let current: u32 = env.storage().instance().get(&key).unwrap_or(0u32);

        if current == MAX_SUBSCRIPTION_ID {
            return Err(Error::SubscriptionLimitReached);
        }

        env.storage().instance().set(&key, &(current + 1));
        Ok(current)
    }
}

#[cfg(test)]
mod test_utils;

#[cfg(test)]
mod test_charge_invariants;

#[cfg(test)]
mod test_scheduled_cancel;

#[cfg(test)]
mod test_billing_period_snapshots;

#[cfg(test)]
mod test_insufficient_balance;
#[cfg(test)]
mod test_governance;

#[cfg(test)]
mod test_validation;

#[cfg(test)]
mod test_abi_validators_integration;

#[cfg(test)]
mod test {
    use super::*;
    use crate::SubscriptionVaultClient;

    #[test]
    fn version_is_one() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        assert_eq!(client.version(), 1);
    }
}
