#![no_std]
#![allow(clippy::too_many_arguments)]

//! # Subscription Vault — Prepaid Recurring Billing on Stellar
//!
//! This crate contains the [`SubscriptionVault`] Soroban smart contract,
//! which handles prepaid subscription billing (USDC or other tokens)
//! for the Stellabill platform.
//!
//! ## Overview
//!
//! ```text
//! Subscriber ──deposit_funds──► [Vault / prepaid_balance]
//!                                        │
//!                    charge_subscription ▼  batch_charge
//!                               [Merchant balance]
//!                                        │
//!                    withdraw_merchant_funds ▼
//!                               Merchant wallet
//! ```
//! 
//! In simple terms:
//! - Subscribers deposit funds into the vault
//! - Charges are applied over time
//! - Merchants withdraw what they’ve earned
//!
//! ## Key Concepts
//!
//! - **Subscriber** — owns a subscription and funds it upfront
//! - **Merchant** — receives recurring payments
//! - **Subscription** — defines amount, interval, status, and balance
//! - **Plan Template** — reusable subscription setup created by a merchant
//! - **Lifetime Cap** — optional limit on total charges
//! - **Emergency Stop** — admin-only switch to pause all operations
//!
//! ## Subscription Lifecycle
//!
//! ```text
//! Active
//!   │
//!   ├─ pause ─────────────► Paused
//!   ├─ insufficient funds ► InsufficientBalance
//!   └─ cancel ────────────► Cancelled (final)
//!
//! Paused / InsufficientBalance
//!   └─ resume ────────────► Active
//! ```
//!
//! More details are available in:
//! - `docs/subscription_lifecycle.md`
//! - `docs/subscription_state_machine.md`
//!
//! ## Security Model
//!
//! - **Admin**: manages setup, batch charging, emergency stop, and recovery
//! - **Merchant**: manages subscriptions and withdraws earnings
//! - **Subscriber**: creates subscriptions, deposits funds, and manages their balance
//!
//! All transfers follow the Check-Effects-Interactions (CEI) pattern,
//! with an additional reentrancy guard for safety.
//!
//! ## Docs
//!
//! - `docs/subscription_lifecycle.md`
//! - `docs/subscription_state_machine.md`
//! - `docs/batch_charge.md`
//! - `docs/billing_intervals.md`
//! - `docs/topup_estimation.md`
//! - `docs/safe_math.md`
//! - `docs/lifetime_caps.md`
//! - `docs/subscription_metadata.md`

// ── Modules ──────────────────────────────────────────────────────────────────
mod accounting;
mod admin;
mod blocklist;
mod charge_core;
mod merchant;
mod metadata;
pub mod migration;
mod oracle;
mod queries;
mod reentrancy;
pub mod safe_math;
mod state_machine;
mod statements;
mod subscription;
mod types;
#[cfg(test)]
mod test_utils;
#[cfg(test)]
mod test;
#[cfg(test)]
mod test_auth_fuzz;
#[cfg(test)]
mod test_expiration;
#[cfg(test)]
mod test_governance;
#[cfg(test)]
mod test_insufficient_balance;
#[cfg(test)]
mod test_multi_actor;
#[cfg(test)]
mod test_recovery;
#[cfg(test)]
mod test_refactor_check;
#[cfg(test)]
mod test_safe_math_regression;
#[cfg(test)]
mod test_security;
#[cfg(test)]
mod test_usage_limits;
#[cfg(test)]
mod test_deterministic_charging;
#[cfg(test)]
mod test_emergency_stop_lifetime_caps;

use soroban_sdk::{contract, contractimpl, Address, Env, String, Symbol, Vec};

// ── Re-exports ────────────────────────────────────────────────────────────────
pub use blocklist::{BlocklistAddedEvent, BlocklistEntry, BlocklistRemovedEvent};
pub use queries::{compute_next_charge_info, MAX_SCAN_DEPTH, MAX_SUBSCRIPTION_LIST_PAGE};
pub use state_machine::{can_transition, get_allowed_transitions, validate_status_transition};
pub use types::{
    AcceptedToken, AccruedTotals, AdminRotatedEvent, BatchChargeResult, BatchWithdrawResult,
    BillingChargeKind, BillingCompactedEvent, BillingCompactionSummary, BillingRetentionConfig,
    BillingStatement, BillingStatementAggregate, BillingStatementsPage, CapInfo,
    ChargeExecutionResult, ContractSnapshot, DataKey, EmergencyStopDisabledEvent,
    EmergencyStopEnabledEvent, Error, FundsDepositedEvent, LifetimeCapReachedEvent, MerchantConfig,
    MerchantPausedEvent, MerchantUnpausedEvent, MerchantWithdrawalEvent, MetadataDeletedEvent,
    MetadataSetEvent, MigrationExportEvent, NextChargeInfo, OneOffChargedEvent, OracleConfig,
    OraclePrice, PartialRefundEvent, PlanTemplate, PlanTemplateUpdatedEvent, ProtocolFeeChargedEvent,
    ProtocolFeeConfiguredEvent, RecoveryEvent,
    RecoveryReason, Subscription, SubscriptionCancelledEvent, SubscriptionChargeFailedEvent,
    SubscriptionChargedEvent, SubscriptionCreatedEvent, SubscriptionMigratedEvent,
    SubscriptionPausedEvent, SubscriptionResumedEvent, SubscriptionStatus, SubscriptionSummary,
    TokenEarnings, TokenReconciliationSnapshot, UsageLimits, UsageState, UsageStatementEvent,
    MAX_METADATA_KEYS, MAX_METADATA_KEY_LENGTH, MAX_METADATA_VALUE_LENGTH,
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
        .get(&Symbol::new(env, "emergency_stop"))
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

    // Updates the admin address.
    ///
    /// This change happens immediately, so make sure the new address is correct.
    ///
    /// # Errors
    /// - `Unauthorized` if caller is not current admin
    pub fn rotate_admin(env: Env, current_admin: Address, new_admin: Address) -> Result<(), Error> {
        admin::do_rotate_admin(&env, current_admin, new_admin)
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
    pub fn batch_charge(
        env: Env,
        subscription_ids: Vec<u32>,
    ) -> Result<Vec<BatchChargeResult>, Error> {
        require_not_emergency_stop(&env)?;
        admin::do_batch_charge(&env, &subscription_ids)
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
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "emergency_stop"), &true);
        env.events().publish(
            (Symbol::new(&env, "emergency_stop_enabled"),),
            EmergencyStopEnabledEvent {
                admin,
                timestamp: env.ledger().timestamp(),
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
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "emergency_stop"), &false);
        env.events().publish(
            (Symbol::new(&env, "emergency_stop_disabled"),),
            EmergencyStopDisabledEvent {
                admin,
                timestamp: env.ledger().timestamp(),
            },
        );
        Ok(())
    }

    // ── Migration / Export ────────────────────────────────────────────────────

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

        let token: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "token"))
            .ok_or(Error::NotFound)?;
        let min_topup: i128 = admin::get_min_topup(&env)?;
        let next_id: u32 = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "next_id"))
            .unwrap_or(0);

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

        let next_id: u32 = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "next_id"))
            .unwrap_or(0);
        if start_id >= next_id {
            return Ok(Vec::new(&env));
        }

        let end_id = start_id.saturating_add(limit).min(next_id);
        let mut out = Vec::new(&env);
        let mut exported = 0u32;
        let mut id = start_id;
        while id < end_id {
            if let Some(sub) = env.storage().instance().get::<u32, Subscription>(&id) {
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
            },
        );

        Ok(out)
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
        subscription::do_create_subscription(
            &env,
            subscriber,
            merchant,
            amount,
            interval_seconds,
            usage_enabled,
            lifetime_cap,
            expires_at,
        )
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
        subscription::do_create_subscription_with_token(
            &env,
            subscriber,
            merchant,
            token,
            amount,
            interval_seconds,
            usage_enabled,
            lifetime_cap,
            expires_at,
        )
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
    pub fn deposit_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
    ) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;
        
        // Acquire reentrancy guard: prevents re-entry during token transfer
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "deposit_funds")?;
        
        subscription::do_deposit_funds(&env, subscription_id, subscriber, amount)
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

    /// Cancel the subscription. Allowed from Active, Paused, or InsufficientBalance.
    /// Transitions to the terminal `Cancelled` state.
    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_cancel_subscription(&env, subscription_id, authorizer)
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
    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_pause_subscription(&env, subscription_id, authorizer)
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
        subscription::do_resume_subscription(&env, subscription_id, authorizer)
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
    pub fn charge_one_off(
        env: Env,
        subscription_id: u32,
        merchant: Address,
        amount: i128,
    ) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;
        
        // Acquire reentrancy guard
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "charge_one_off")?;
        
        subscription::do_charge_one_off(&env, subscription_id, merchant, amount)
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
    pub fn charge_subscription(
        env: Env,
        subscription_id: u32,
    ) -> Result<ChargeExecutionResult, Error> {
        require_not_emergency_stop(&env)?;
        
        // Acquire reentrancy guard: prevents the same function from being called
        // recursively (e.g., if a malicious token contract tries to call back).
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "charge_subscription")?;
        
        charge_core::charge_one(&env, subscription_id, env.ledger().timestamp(), None)
    }

    /// Charge a metered usage amount against the subscription's prepaid balance.
    ///
    /// **This function is disabled when the emergency stop is active.**
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// state mutations. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    pub fn charge_usage(env: Env, subscription_id: u32, usage_amount: i128) -> Result<(), Error> {
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
    ) -> Result<(), Error> {
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
        // Acquire reentrancy guard: prevents re-entry during token transfer
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "withdraw_merchant_funds")?;
        
        merchant::withdraw_merchant_funds(&env, merchant, amount)
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
        // Acquire reentrancy guard: prevents re-entry during token transfer
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "withdraw_merchant_token_funds")?;
        
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
        let sub = queries::get_subscription(&env, subscription_id)?;
        Ok(compute_next_charge_info(&sub))
    }

    /// Return subscriptions for a merchant, paginated.
    ///
    /// `limit` must be between 1 and [`queries::MAX_SUBSCRIPTION_LIST_PAGE`] inclusive.
    pub fn get_subscriptions_by_merchant(
        env: Env,
        merchant: Address,
        start: u32,
        limit: u32,
    ) -> Result<Vec<Subscription>, Error> {
        queries::get_subscriptions_by_merchant(&env, merchant, start, limit)
    }

    /// Return the total number of subscriptions ever created.
    pub fn get_subscription_count(env: Env) -> u32 {
        let key = Symbol::new(&env, "next_id");
        env.storage().instance().get(&key).unwrap_or(0u32)
    }

    /// Return the total number of subscriptions for a merchant.
    pub fn get_merchant_subscription_count(env: Env, merchant: Address) -> u32 {
        queries::get_merchant_subscription_count(&env, merchant)
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

/// Add a new accepted token.
///
/// # Auth
/// - Admin only
///
/// # Errors
/// - Unauthorized
/// - TokenAlreadyAccepted
    pub fn add_accepted_token(
        env: Env,
        admin: Address,
        token: Address,
        decimals: u32,
    ) -> Result<(), Error> {
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
            },
        );
        Ok(summary)
    }

/// Configure price oracle integration.
///
/// # Auth
/// - Admin only
///
/// # Errors
/// - Unauthorized
/// - InvalidConfig
    pub fn set_oracle_config(
        env: Env,
        admin: Address,
        enabled: bool,
        oracle: Option<Address>,
        max_age_seconds: u64,
    ) -> Result<(), Error> {
        require_admin_auth(&env, &admin)?;
        oracle::set_oracle_config(&env, enabled, oracle, max_age_seconds)
    }

    /// Read the currently configured oracle integration settings.
    pub fn get_oracle_config(env: Env) -> OracleConfig {
        oracle::get_oracle_config(&env)
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
        metadata::do_set_metadata(&env, subscription_id, &authorizer, key, value)
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
        metadata::do_delete_metadata(&env, subscription_id, &authorizer, key)
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
        metadata::do_get_metadata(&env, subscription_id, key)
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
        metadata::do_list_metadata_keys(&env, subscription_id)
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
        admin::set_protocol_fee(&env, admin, treasury, fee_bps)
    }

    /// Return the current protocol fee basis points (0 = disabled).
    pub fn get_protocol_fee_bps(env: Env) -> u32 {
        admin::get_protocol_fee_bps(&env)
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
    /// * `fee_address` — Optional address to receive a portion of charges (platform fee routing).
    /// * `redirect_url` — Merchant-supplied URL used by off-chain frontends for post-payment redirects.
    /// * `is_paused` — Convenience pause flag mirrored in config storage.
    ///
    /// # Auth
    ///
    /// `merchant` must authorize.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — `merchant` auth failed.
    pub fn set_merchant_config(
        env: Env,
        merchant: Address,
        fee_address: Option<Address>,
        redirect_url: String,
        is_paused: bool,
    ) -> Result<(), Error> {
        let config = crate::types::MerchantConfig {
            fee_address,
            redirect_url,
            is_paused,
        };
        merchant::set_merchant_config(&env, merchant, config)
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
}
