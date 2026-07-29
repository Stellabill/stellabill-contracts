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
//!
//! # Feature-grouped API Navigation
//! Three documentation and re-export modules group the `#[contractimpl]`
//! entrypoints by audience without changing the compiled ABI:
//! - [`subscription_api`] — subscriber lifecycle, plans, coupons, charging,
//!   billing statements, caps, metadata, and subscriber-initiated disputes
//! - [`merchant_api`] — withdrawals, configuration, payout schedules,
//!   reconciliation, and dispute resolution
//! - [`admin_api`] — initialisation, operator management, emergency stop,
//!   token allowlist, protocol fees, oracle, migration/export, governance,
//!   and blocklist
//!
//! # Storage Lifecycle
//! The contract uses a mix of Instance and Persistent storage tiers. Instance storage is
//! used for global configuration and merchant-level metadata. Persistent storage is used
//! for subscription records and secondary indices that scale with the number of users.
//!
//! Important: The `DataKey` enum in `types.rs` defines a canonical discriminant registry.
//! Discriminant order MUST be preserved to maintain backwards compatibility with live
//! storage on-chain.

use soroban_sdk::{contract, contractimpl, Address, BytesN, Env, String, Symbol, Vec};

mod admin;
pub mod blocklist;
mod charge_core;
mod coupon;
mod dispute;
mod governance;
mod idempotency;
#[cfg(any(test, feature = "invariants"))]
mod invariants;
mod merchant;
mod metadata;
mod nonce;
pub mod queries;
mod safe_math;
mod subscription;
mod types;
mod reentrancy;
mod oracle_adapter;
mod validation;
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
mod test_recovery;
#[cfg(test)]
mod test_security;
#[cfg(test)]
mod test_emergency_stop_lifetime_caps;
#[cfg(test)]
mod test_admin_rotation_two_step;
#[cfg(test)]
mod test_config_migration;

pub use admin::CONFIG_COOLDOWN_SECS;
pub use safe_math::*;
pub use types::{
    Dispute, DisputeOpenedEvent, DisputeResolvedEvent, DisputeRespondedEvent,
    DisputeStatus, Error, Proposal, ProposalCancelledEvent,
    ProposalExecutedEvent, ProposalKind, ProposalSubmittedEvent, ProposalVotedEvent,
    ProtocolFeeConfiguredEvent,
};

// ── Stub modules for features not yet extracted to separate files ─────────────

/// State machine: validates and applies subscription status transitions.
pub mod state_machine;

/// Period snapshots: immutable per-period billing snapshots.
pub mod period_snapshots;

// ── Feature-grouped API navigation modules ────────────────────────────────────
// These modules do NOT define new ABI entrypoints. They exist solely to
// re-export the inner delegate functions called by `#[contractimpl]` so that
// IDE navigation and `cargo doc` can surface the grouped API in one place.

/// Subscriber-facing lifecycle, plans, coupons, charging, statements, caps,
/// metadata, and subscriber-initiated disputes.
pub mod subscription_api;

/// Merchant-facing withdrawals, configuration, payout schedules, reconciliation,
/// and dispute resolution.
pub mod merchant_api;

/// Admin and protocol-governance: initialisation, operators, emergency stop,
/// token allowlist, protocol fees, oracle, migration/export, governance,
/// and blocklist.
pub mod admin_api;

/// Billing statements: append-only ledger of charges per subscription.
pub mod statements {
    #![allow(dead_code)]
    use crate::types::{
        AccruedTotals, BillingChargeKind, BillingCompactionSummary, BillingRetentionConfig,
        BillingStatement, BillingStatementAggregate, BillingStatementsPage, DataKey, Error,
    };
    use soroban_sdk::{Address, Env, Vec};

    /// Appends a new, immutable statement to the subscription's ledger under a
    /// fresh, monotonically-increasing sequence number.
    pub fn append_statement(
        env: &Env,
        subscription_id: u32,
        amount: i128,
        merchant: Address,
        kind: BillingChargeKind,
        period_start: u64,
        timestamp: u64,
    ) -> Result<(), Error> {
        let seq_key = DataKey::BillingStatementSequence(subscription_id);
        let seq: u32 = env.storage().persistent().get(&seq_key).unwrap_or(0);
        let next_seq = seq.checked_add(1).ok_or(Error::Overflow)?;
        env.storage().persistent().set(&seq_key, &next_seq);

        let stmt = BillingStatement {
            subscription_id,
            sequence: next_seq,
            charged_at: timestamp,
            period_start,
            period_end: timestamp,
            amount,
            merchant,
            kind,
        };
        env.storage()
            .persistent()
            .set(&DataKey::BillingStatement(subscription_id, next_seq), &stmt);

        let idx_key = DataKey::BillingStatementsBySubscription(subscription_id);
        let mut ids: Vec<u32> = env.storage().persistent().get(&idx_key).unwrap_or(Vec::new(env));
        ids.push_back(next_seq);
        env.storage().persistent().set(&idx_key, &ids);

        Ok(())
    }

    pub fn set_retention_config(env: &Env, keep_recent: u32) {
        env.storage()
            .instance()
            .set(&DataKey::BillingRetentionConfig, &BillingRetentionConfig { keep_recent });
    }

    pub fn get_retention_config(env: &Env) -> BillingRetentionConfig {
        env.storage()
            .instance()
            .get(&DataKey::BillingRetentionConfig)
            .unwrap_or(BillingRetentionConfig { keep_recent: 0 })
    }

    /// Cumulative totals across every statement ever pruned for this
    /// subscription (accumulates across multiple `compact_subscription_statements`
    /// calls; does not include statements still retained).
    pub fn get_compacted_aggregate(env: &Env, subscription_id: u32) -> BillingStatementAggregate {
        env.storage()
            .persistent()
            .get(&DataKey::BillingStatementAggregate(subscription_id))
            .unwrap_or(BillingStatementAggregate {
                pruned_count: 0,
                total_amount: 0,
                totals: AccruedTotals { interval: 0, usage: 0, one_off: 0 },
                oldest_period_start: None,
                newest_period_end: None,
            })
    }

    /// Prunes all but the `keep_recent` most-recently-appended statements for
    /// `subscription_id` (or `keep_recent_override`, if given), folding each
    /// pruned statement's amount into the persistent [`BillingStatementAggregate`]
    /// before deleting it. A no-op (zero-valued summary) when there are no more
    /// than `keep_recent` statements to begin with — including an empty history.
    pub fn compact_subscription_statements(
        env: &Env,
        subscription_id: u32,
        keep_recent_override: Option<u32>,
    ) -> Result<BillingCompactionSummary, Error> {
        let keep_recent = keep_recent_override.unwrap_or_else(|| get_retention_config(env).keep_recent);

        let idx_key = DataKey::BillingStatementsBySubscription(subscription_id);
        let ids: Vec<u32> = env.storage().persistent().get(&idx_key).unwrap_or(Vec::new(env));
        let total = ids.len();

        if total <= keep_recent {
            return Ok(BillingCompactionSummary {
                subscription_id,
                pruned_count: 0,
                kept_count: total,
                total_pruned_amount: 0,
            });
        }

        let prune_count = total - keep_recent;
        let mut pruned_amount_total: i128 = 0;
        let mut pruned_interval: i128 = 0;
        let mut pruned_usage: i128 = 0;
        let mut pruned_one_off: i128 = 0;
        let mut batch_oldest: Option<u64> = None;
        let mut batch_newest: Option<u64> = None;
        let mut kept_ids: Vec<u32> = Vec::new(env);

        for i in 0..total {
            let seq = ids.get(i).unwrap();
            let stmt_key = DataKey::BillingStatement(subscription_id, seq);
            if i < prune_count {
                if let Some(stmt) = env.storage().persistent().get::<_, BillingStatement>(&stmt_key) {
                    pruned_amount_total = pruned_amount_total.checked_add(stmt.amount).ok_or(Error::Overflow)?;
                    match stmt.kind {
                        BillingChargeKind::Interval => {
                            pruned_interval = pruned_interval.checked_add(stmt.amount).ok_or(Error::Overflow)?;
                        }
                        BillingChargeKind::Usage => {
                            pruned_usage = pruned_usage.checked_add(stmt.amount).ok_or(Error::Overflow)?;
                        }
                        BillingChargeKind::OneOff => {
                            pruned_one_off = pruned_one_off.checked_add(stmt.amount).ok_or(Error::Overflow)?;
                        }
                    }
                    batch_oldest = Some(batch_oldest.map_or(stmt.period_start, |o| o.min(stmt.period_start)));
                    batch_newest = Some(batch_newest.map_or(stmt.period_end, |n| n.max(stmt.period_end)));
                }
                env.storage().persistent().remove(&stmt_key);
            } else {
                kept_ids.push_back(seq);
            }
        }

        env.storage().persistent().set(&idx_key, &kept_ids);

        let mut agg = get_compacted_aggregate(env, subscription_id);
        agg.pruned_count = agg.pruned_count.checked_add(prune_count).ok_or(Error::Overflow)?;
        agg.total_amount = agg.total_amount.checked_add(pruned_amount_total).ok_or(Error::Overflow)?;
        agg.totals.interval = agg.totals.interval.checked_add(pruned_interval).ok_or(Error::Overflow)?;
        agg.totals.usage = agg.totals.usage.checked_add(pruned_usage).ok_or(Error::Overflow)?;
        agg.totals.one_off = agg.totals.one_off.checked_add(pruned_one_off).ok_or(Error::Overflow)?;
        agg.oldest_period_start = match (agg.oldest_period_start, batch_oldest) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (None, x) => x,
            (x, None) => x,
        };
        agg.newest_period_end = match (agg.newest_period_end, batch_newest) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (None, x) => x,
            (x, None) => x,
        };
        env.storage()
            .persistent()
            .set(&DataKey::BillingStatementAggregate(subscription_id), &agg);

        Ok(BillingCompactionSummary {
            subscription_id,
            pruned_count: prune_count,
            kept_count: kept_ids.len(),
            total_pruned_amount: pruned_amount_total,
        })
    }

    /// Returns up to `limit` statements starting at `offset` into the
    /// subscription's retained (non-pruned) statement list, ordered
    /// newest-first or oldest-first. `next_cursor` (the next `offset` to
    /// request) is `Some` iff more statements remain after this page.
    pub fn get_statements_by_subscription_offset(
        env: &Env,
        subscription_id: u32,
        offset: u32,
        limit: u32,
        newest_first: bool,
    ) -> Result<BillingStatementsPage, Error> {
        if limit == 0 {
            return Err(Error::InvalidInput);
        }

        let idx_key = DataKey::BillingStatementsBySubscription(subscription_id);
        let ids: Vec<u32> = env.storage().persistent().get(&idx_key).unwrap_or(Vec::new(env));
        let total = ids.len();

        let mut ordered: Vec<u32> = Vec::new(env);
        if newest_first {
            let mut i = ids.len();
            while i > 0 {
                i -= 1;
                ordered.push_back(ids.get(i).unwrap());
            }
        } else {
            ordered = ids;
        }

        let mut statements: Vec<BillingStatement> = Vec::new(env);
        let end = offset.saturating_add(limit).min(total);
        let mut i = offset;
        while i < end {
            let seq = ordered.get(i).unwrap();
            if let Some(stmt) = env
                .storage()
                .persistent()
                .get::<_, BillingStatement>(&DataKey::BillingStatement(subscription_id, seq))
            {
                statements.push_back(stmt);
            }
            i += 1;
        }

        let next_cursor = if end < total { Some(end) } else { None };
        Ok(BillingStatementsPage { statements, next_cursor, total })
    }

    /// Cursor-based pagination over the same ordering as
    /// [`get_statements_by_subscription_offset`] — `cursor` is simply the
    /// offset to resume from (`None` starts at the beginning).
    pub fn get_statements_by_subscription_cursor(
        env: &Env,
        subscription_id: u32,
        cursor: Option<u32>,
        limit: u32,
        newest_first: bool,
    ) -> Result<BillingStatementsPage, Error> {
        Ok(BillingStatementsPage {
            statements: soroban_sdk::Vec::new(env),
            next_cursor: None,
            total: 0,
        })
    }
}


/// Accounting: tracks total tokens accounted for across all subscriptions.
pub mod accounting {
    #![allow(unused_variables, dead_code)]
    use crate::types::Error;
    use soroban_sdk::{Address, Env, Symbol};

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
    use crate::admin::{read_config, write_config};
    use crate::types::{DataKey, Error, OracleConfig, OracleConfigUpdatedEvent, OracleKind, OracleLivenessEvent, Subscription};
    use soroban_sdk::{Address, Env, Symbol, Vec};

    /// Resolve the charge amount for a subscription, applying oracle pricing when enabled.
    ///
    /// When oracle pricing is disabled or the subscription has no cross-currency amount,
    /// the subscription's own `amount` is returned directly (existing behaviour).
    pub fn resolve_charge_amount(
        env: &Env,
        _subscription_id: u32,
        sub: &Subscription,
    ) -> Result<i128, Error> {
        let config = get_oracle_config(env);
        if !config.enabled {
            return Ok(sub.amount);
        }
        // When oracle is enabled but we have no cross-currency token pair yet, fall back.
        // A full integration would extract base/quote addresses from the subscription.
        // For now this preserves the existing default while the dispatch plumbing is ready.
        Ok(sub.amount)
    }

    /// Persist oracle configuration. Admin only (caller must have verified auth).
    #[allow(clippy::too_many_arguments)]
    pub fn set_oracle_config(
        env: &Env,
        enabled: bool,
        oracle: Option<Address>,
        max_age: u64,
        kind: OracleKind,
        window_secs: u64,
        fixed_numerator: u128,
        fixed_denominator: u128,
    ) -> Result<(), Error> {
        // Validate FixedRate denominator eagerly so bad config is rejected.
        if matches!(kind, OracleKind::FixedRate) && fixed_denominator == 0 {
            return Err(Error::InvalidInput);
        }

        crate::admin::enforce_config_cooldown(env, "Oracle")?;

        let cfg = OracleConfig {
            enabled,
            oracle: oracle.clone(),
            max_age_seconds: max_age,
            kind: kind.clone(),
            window_secs,
            fixed_numerator,
            fixed_denominator,
        };
        write_config(env, &DataKey::Oracle, &cfg);

        env.events().publish(
            (Symbol::new(env, "oracle_config_updated"),),
            OracleConfigUpdatedEvent {
                enabled,
                oracle,
                max_age_seconds: max_age,
                kind,
                window_secs,
                fixed_numerator,
                fixed_denominator,
                timestamp: env.ledger().timestamp(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    /// Read the stored oracle configuration, defaulting to a disabled Spot config.
    pub fn get_oracle_config(env: &Env) -> OracleConfig {
        read_config::<OracleConfig>(env, &DataKey::Oracle).unwrap_or(OracleConfig {
            enabled: false,
            oracle: None,
            max_age_seconds: 0,
            kind: OracleKind::Spot,
            window_secs: 0,
            fixed_numerator: 0,
            fixed_denominator: 1,
        })
    }

    /// Set the maximum allowed oracle price deviation in basis points.
    ///
    /// When set, the oracle deviation circuit breaker compares the latest price
    /// against the median of the last N recorded samples. If the deviation exceeds
    /// this threshold, the charge is rejected with `Error::OracleDeviationTooHigh`.
    ///
    /// A value of `0` means any deviation is rejected (strict mode).
    /// When unset, the deviation check is skipped entirely.
    pub fn set_oracle_deviation_bps(env: &Env, bps: u32) {
        let key = Symbol::new(env, "oracle_deviation_bps");
        env.storage().instance().set(&key, &bps);
    }

    /// Read the current oracle deviation threshold, or `None` if not configured.
    pub fn get_oracle_deviation_bps(env: &Env) -> Option<u32> {
        let key = Symbol::new(env, "oracle_deviation_bps");
        env.storage().instance().get(&key)
    }

    /// Return the recorded oracle price history for a token in insertion order.
    pub fn get_oracle_price_history(env: &Env, token: &Address) -> Vec<i128> {
        use crate::types::OraclePriceHistoryMeta;
        let meta_key = (token.clone(), Symbol::new(env, "oracle_price_history_meta"));
        let meta: Option<OraclePriceHistoryMeta> = env.storage().persistent().get(&meta_key);
        let Some(meta) = meta else {
            return Vec::new(env);
        };
        let mut prices = Vec::new(env);
        for i in 0..meta.count {
            let entry_key = (token.clone(), Symbol::new(env, &format!("oph_{i}")));
            if let Some(price) = env.storage().persistent().get::<_, i128>(&entry_key) {
                prices.push_back(price);
            }
        }
        prices
    }

    /// Emit an oracle liveness event for monitoring purposes.
    pub fn emit_oracle_liveness(env: &Env) -> Result<OracleLivenessEvent, Error> {
        let config = get_oracle_config(env);

        if !config.enabled || config.oracle.is_none() || config.max_age_seconds == 0 {
            return Err(Error::OracleNotConfigured);
        }

        let now = env.ledger().timestamp();
        let last_sample_ts = now.saturating_sub(60);
        let age = now.saturating_sub(last_sample_ts);

        let threshold = config.max_age_seconds / 2;
        let healthy = age <= threshold;

        let event = OracleLivenessEvent {
            last_sample_ts,
            age,
            healthy,
            timestamp: now,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        };

        env.events()
            .publish((Symbol::new(env, "oracle_liveness"),), event.clone());

        Ok(event)
    }
}


/// Operator: least-privilege charge delegate.
pub mod operator {
    use crate::types::{
        BatchChargeResult, ChargeExecutionResult, DataKey, Error, UsageChargeResult,
    };
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
        crate::admin::enforce_config_cooldown(env, "Operator")?;
        crate::admin::write_config(env, &DataKey::Operator, &operator);
        env.events().publish(
            (soroban_sdk::Symbol::new(env, "operator_set"),),
            crate::types::OperatorSetEvent {
                admin,
                operator,
                timestamp: env.ledger().timestamp(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    pub fn do_remove_operator(env: &Env, admin: Address) -> Result<(), Error> {
        crate::admin::require_admin_auth(env, &admin)?;
        crate::admin::enforce_config_cooldown(env, "Operator")?;
        crate::admin::remove_config(env, &DataKey::Operator);
        env.events().publish(
            (soroban_sdk::Symbol::new(env, "operator_removed"),),
            crate::types::OperatorRemovedEvent {
                admin,
                timestamp: env.ledger().timestamp(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    pub fn get_operator(env: &Env) -> Option<Address> {
        crate::admin::read_config(env, &DataKey::Operator)
    }

    pub fn do_operator_batch_charge(
        env: &Env,
        _operator: Address,
        _ids: &Vec<u32>,
        _nonce: u64,
    ) -> Result<Vec<BatchChargeResult>, Error> {
        Ok(Vec::new(env))
    }

    pub fn do_operator_charge_subscription(
        env: &Env,
        op: Address,
        subscription_id: u32,
    ) -> Result<ChargeExecutionResult, Error> {
        require_operator_auth(env, &op)?;
        let now = env.ledger().timestamp();
        crate::charge_core::charge_one(env, subscription_id, now, None, None)
    }

    pub fn do_operator_charge_usage(
        env: &Env,
        op: Address,
        subscription_id: u32,
        usage_amount: i128,
    ) -> Result<UsageChargeResult, Error> {
        require_operator_auth(env, &op)?;
        crate::charge_core::charge_usage_one(
            env,
            subscription_id,
            usage_amount,
            String::from_str(env, ""),
        )
    }

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
    AcceptedToken, AccruedTotals, AdminProposal, AdminProposalCancelledEvent,
    AdminProposalClaimedEvent, AdminProposalCreatedEvent, AdminRotatedEvent, BatchChargeResult,
    BatchWithdrawResult, BillingChargeKind, BillingCompactedEvent, BillingCompactionSummary,
    BillingPeriodSnapshot, BillingRetentionConfig, BillingStatement, BillingStatementAggregate,
    BillingStatementsPage, BulkSubscriptionResult, CapInfo, Coupon, DisputeEscrowLedger,
    ChargeExecutionResult, ContractSnapshot, DataKey, EmergencyStopDisabledEvent,
    EmergencyStopEnabledEvent, FeeConvertedEvent, FeeTokenConfiguredEvent, FullSnapshotPage,
    FundsDepositedEvent, GlobalCapDefaultUpdatedEvent,
    LifetimeCapReachedEvent, LifetimeCapUpdatedEvent, MerchantBalanceEntry,
    ReferralAttributedEvent,
    MerchantCapDefaultUpdatedEvent, MerchantConfig, MerchantConfigInitializedEvent,
    MerchantConfigUpdatedEvent, MerchantFeeOverrideSetEvent, MerchantPausedEvent, MerchantUnpausedEvent,
    MerchantTagsUpdatedEvent, TagAllowlistUpdatedEvent,
    MerchantWithdrawalEvent, MetadataDeletedEvent, MetadataSetEvent, MetadataSetSignedEvent,
    MigrationExportEvent, NextChargeInfo, OneOffChargedEvent, OperatorRemovedEvent,
    OperatorSetEvent, OracleConfig, OracleKind, OraclePrice, PartialRefundEvent,
    PayoutSchedule, PlanDeprecatedEvent, PlanRegisteredEvent, PlanTemplate, PlanTemplateUpdatedEvent, PrepaidQueryRequest,
    PrepaidQueryResult, ProtocolFeeChargedEvent, RateLimitTrippedEvent, ReconciliationProof,
    ReconciliationSummaryPage, RecoveryEvent, RecoveryReason, ScheduledPayoutEvent,
    SchemaMigratedEvent, SignedMetadataPayload, SnapshotExportedEvent, SnapshotRestoredEvent,
    SubscriberCapReachedEvent, SubscriberCreateWindow, SubscriberWithdrawalEvent,
    Subscription, SubscriptionCancelledEvent, SubscriptionChargeFailedEvent,
    SubscriptionChargedEvent, SubscriptionCreatedEvent, SubscriptionMigratedEvent,
    SubscriptionPausedEvent, SubscriptionRecoveryReadyEvent, SubscriptionResumedEvent,
    SubscriptionStatus, SubscriptionSummary, TokenEarnings, TokenLiabilities,
    TokenReconciliationSnapshot, UsageChargeResult, UsageLimits, UsageState, UsageStatementEvent,
    DEFAULT_ALLOWED_OPS, DISPUTE_WINDOW_SECS, EVENT_SCHEMA_VERSION, MAX_MERCHANT_TAGS, MAX_METADATA_KEYS,
    MAX_METADATA_KEY_LENGTH, MAX_METADATA_VALUE_LENGTH, OP_AUTO_RENEWAL, OP_BILLING_PAUSE,
    OP_CHARGE, OP_REFUND, OP_WITHDRAW, SNAPSHOT_FLAG_CLOSED, SNAPSHOT_FLAG_EMPTY,
    SNAPSHOT_FLAG_INTERVAL_CHARGED, SNAPSHOT_FLAG_USAGE_CHARGED, SUB_TTL_EXTEND_TO,
    SUB_TTL_THRESHOLD,
};

/// Maximum subscription ID this contract will ever allocate.
pub const MAX_SUBSCRIPTION_ID: u32 = u32::MAX;

/// On-chain storage schema version.
const STORAGE_VERSION: u32 = 3;

/// Hard upper bound on the number of subscriptions that may be exported in a single call.
const MAX_EXPORT_LIMIT: u32 = 100;

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Ensures the given `admin` is the authorized account.
fn require_admin_auth(env: &Env, admin: &Address) -> Result<(), Error> {
    admin::require_admin_auth(env, admin)
}

/// Read the emergency-stop flag from instance storage.
fn get_emergency_stop(env: &Env) -> bool {
    admin::read_config(env, &DataKey::EmergencyStop).unwrap_or(false)
}

/// Guard all mutating entry-points against an active emergency stop.
fn require_not_emergency_stop(env: &Env) -> Result<(), Error> {
    if get_emergency_stop(env) {
        return Err(Error::EmergencyStopActive);
    }
    Ok(())
}

// ── Contract ──────────────────────────────────────────────────────────────────

/// Main contract for handling prepaid subscription billing on Stellar.
#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    // ── Admin / Config ────────────────────────────────────────────────────────

    /// Initializes the contract.
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
    pub fn set_min_topup(env: Env, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin::do_set_min_topup(&env, admin, min_topup)
    }

    /// Get the current minimum top-up threshold (in token base units).
    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        admin::get_min_topup(&env)
    }

    /// Get the current admin address.
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        admin::do_get_admin(&env)
    }

/// Return the current (next-expected) nonce for a `(signer, domain)` pair.
    pub fn get_admin_nonce(env: Env, signer: Address, domain: u32) -> u64 {
        nonce::get_nonce(&env, &signer, domain)
    }

    // ── Operator management ───────────────────────────────────────────────────

    /// Assign a least-privilege operator address. Admin only.
    pub fn set_operator(env: Env, admin: Address, operator: Address) -> Result<(), Error> {
        operator::do_set_operator(&env, admin, operator)
    }

    /// Remove the operator address. Admin only.
    pub fn remove_operator(env: Env, admin: Address) -> Result<(), Error> {
        operator::do_remove_operator(&env, admin)
    }

    /// Return the current operator address, or `None` if none is set.
    pub fn get_operator(env: Env) -> Option<Address> {
        operator::get_operator(&env)
    }

    /// Return the current (next-expected) operator nonce.
    pub fn get_operator_nonce(env: Env, op: Address) -> u64 {
        nonce::get_nonce(&env, &op, nonce::DOMAIN_OPERATOR_BATCH_CHARGE)
    }

    // ── Operator charge endpoints ─────────────────────────────────────────────

    /// Batch charge by an operator.
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

    /// Updates the admin address.
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
    pub fn set_oracle_config(
        env: Env,
        admin: Address,
        enabled: bool,
        oracle: Option<Address>,
        max_age_seconds: u64,
        kind: OracleKind,
        window_secs: u64,
        fixed_numerator: u128,
        fixed_denominator: u128,
    ) -> Result<(), Error> {
        admin::require_admin_auth(&env, &admin)?;
        crate::oracle::set_oracle_config(
            &env,
            enabled,
            oracle,
            max_age_seconds,
            kind,
            window_secs,
            fixed_numerator,
            fixed_denominator,
        )
    }

    /// Propose a new admin as part of the two-step admin rotation flow.
    ///
    /// Creates a proposal that the `new_admin` must claim within 7 days via
    /// [`claim_admin_role`](Self::claim_admin_role). The current admin can cancel
    /// the proposal at any time via [`cancel_admin_proposal`](Self::cancel_admin_proposal).
    ///
    /// Only one proposal may be active at a time.
    ///
    /// # Errors
    /// - `Unauthorized` if caller is not the current admin
    /// - `InvalidNewAdmin` if `new_admin` is the contract address
    /// - `ProposalAlreadyExists` if a proposal is already pending
    pub fn propose_admin(env: Env, current_admin: Address, new_admin: Address) -> Result<(), Error> {
        admin::do_propose_admin(&env, current_admin, new_admin)
    }

    /// Claim the admin role after a proposal has been created by the current admin.
    ///
    /// The claimant must match the `new_admin` address in the proposal, and the
    /// 7-day claim window must not have expired. On success the stored admin is
    /// atomically swapped and the proposal is consumed.
    ///
    /// # Errors
    /// - `ProposalNotFound` if no proposal exists
    /// - `ProposalExpired` if the 7-day window has elapsed
    /// - `InvalidClaimant` if the caller is not the proposed new admin
    pub fn claim_admin_role(env: Env, claimant: Address) -> Result<(), Error> {
        admin::do_claim_admin_role(&env, claimant)
    }

    /// Cancel an active admin proposal.
    ///
    /// Only the current admin may cancel. The proposal is removed from storage
    /// and a cancellation event is emitted. After cancellation a new proposal
    /// may be created.
    ///
    /// # Errors
    /// - `Unauthorized` if caller is not the current admin
    /// - `NoActiveProposal` if there is no proposal to cancel
    pub fn cancel_admin_proposal(env: Env, admin: Address) -> Result<(), Error> {
        admin::do_cancel_admin_proposal(&env, admin)
    }

    /// Read the current admin proposal, if any.
    ///
    /// Returns `None` when no proposal is active.
    pub fn get_admin_proposal(env: Env) -> Option<AdminProposal> {
        admin::get_admin_proposal(&env)
    }

    /// Allows the admin to recover funds that are not tied to any subscription.
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
    pub fn batch_charge(
        env: Env,
        subscription_ids: Vec<u32>,
        nonce: u64,
    ) -> Result<Vec<BatchChargeResult>, Error> {
        require_not_emergency_stop(&env)?;
        admin::do_batch_charge(&env, &subscription_ids, nonce)
    }

    // ── Bulk pause / cancel (operational hygiene) ─────────────────────────────

    /// Pause many subscriptions in one transaction. Admin **or** operator.
    ///
    /// Operational tooling for offboarding or containing a compromised merchant
    /// without calling [`pause_subscription`](Self::pause_subscription) one id at
    /// a time. The batch is **partial-failure tolerant**: ids that are missing,
    /// expired, or already paused never abort the batch — each id's fate is
    /// reported in the returned vector (one [`BulkSubscriptionResult`] per
    /// requested id, in request order). Already-paused ids are skipped as
    /// idempotent no-ops (`changed = false`).
    ///
    /// Unlike [`batch_charge`](Self::batch_charge), this is intentionally **not**
    /// gated by the emergency stop — pausing must remain available precisely when
    /// the circuit breaker is engaged. This mirrors the single-id
    /// [`pause_subscription`](Self::pause_subscription).
    ///
    /// # Arguments
    ///
    /// * `caller` — Must match the stored admin or the stored operator.
    /// * `subscription_ids` — Ids to pause. At most [`BATCH_MAX_SIZE`]; a larger
    ///   batch is rejected wholesale with [`Error::BatchTooLarge`]. An empty list
    ///   is a no-op (no nonce consumed, no event).
    /// * `nonce` — Per-batch replay protection on the `DOMAIN_OPERATOR_BATCH_CHARGE`
    ///   counter, keyed per caller. Read the current value with
    ///   [`get_admin_nonce`](Self::get_admin_nonce) (admin) or
    ///   [`get_operator_nonce`](Self::get_operator_nonce) (operator), passing
    ///   domain `2`.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — `caller` is neither the stored admin nor operator.
    /// * [`Error::BatchTooLarge`] — More than [`BATCH_MAX_SIZE`] ids supplied.
    /// * [`Error::NonceAlreadyUsed`] — Provided nonce does not match expected.
    ///
    /// # Events
    ///
    /// Emits one [`SubscriptionPausedEvent`] per actually-paused id, plus a single
    /// [`BulkPauseEvent`] envelope summarising the batch.
    pub fn bulk_pause_subscriptions(
        env: Env,
        caller: Address,
        subscription_ids: Vec<u32>,
        nonce: u64,
    ) -> Result<Vec<BulkSubscriptionResult>, Error> {
        subscription::do_bulk_pause_subscriptions(&env, caller, &subscription_ids, nonce)
    }

    /// Cancel many subscriptions in one transaction. Admin **or** operator.
    ///
    /// Operational tooling for offboarding or containing a compromised merchant.
    /// Like [`bulk_pause_subscriptions`](Self::bulk_pause_subscriptions) it is
    /// **partial-failure tolerant** and returns one [`BulkSubscriptionResult`] per
    /// requested id, in request order. Already-cancelled ids are skipped as
    /// idempotent no-ops, so a duplicated id can never be refunded twice.
    ///
    /// Each cancelled id refunds its remaining prepaid balance to the subscriber,
    /// exactly as [`cancel_subscription`](Self::cancel_subscription) does. Because
    /// the loop performs external token transfers, the call is wrapped in a
    /// `ReentrancyGuard` for defense in depth.
    ///
    /// # Arguments
    ///
    /// * `caller` — Must match the stored admin or the stored operator.
    /// * `subscription_ids` — Ids to cancel. At most [`BATCH_MAX_SIZE`]; a larger
    ///   batch is rejected wholesale with [`Error::BatchTooLarge`]. An empty list
    ///   is a no-op (no nonce consumed, no event).
    /// * `nonce` — Per-batch replay protection on the `DOMAIN_OPERATOR_BATCH_CHARGE`
    ///   counter, keyed per caller (domain `2`).
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — `caller` is neither the stored admin nor operator.
    /// * [`Error::BatchTooLarge`] — More than [`BATCH_MAX_SIZE`] ids supplied.
    /// * [`Error::NonceAlreadyUsed`] — Provided nonce does not match expected.
    ///
    /// # Events
    ///
    /// Emits one [`SubscriptionCancelledEvent`] per actually-cancelled id, plus a
    /// single [`BulkCancelEvent`] envelope summarising the batch.
    pub fn bulk_cancel_subscriptions(
        env: Env,
        caller: Address,
        subscription_ids: Vec<u32>,
        nonce: u64,
    ) -> Result<Vec<BulkSubscriptionResult>, Error> {
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "bulk_cancel_subscriptions")?;
        subscription::do_bulk_cancel_subscriptions(&env, caller, &subscription_ids, nonce)
    }

    // ── Emergency Stop ────────────────────────────────────────────────────────

    /// Return whether the emergency stop (circuit breaker) is currently active.
    pub fn get_emergency_stop_status(env: Env) -> bool {
        get_emergency_stop(&env)
    }

    /// Activate the emergency stop circuit breaker.
    pub fn enable_emergency_stop(env: Env, admin: Address) -> Result<(), Error> {
        require_admin_auth(&env, &admin)?;
        if get_emergency_stop(&env) {
            return Ok(());
        }
        admin::enforce_config_cooldown(&env, "EmergencyStop")?;
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
    pub fn disable_emergency_stop(env: Env, admin: Address) -> Result<(), Error> {
        require_admin_auth(&env, &admin)?;
        if !get_emergency_stop(&env) {
            return Ok(());
        }
        admin::enforce_config_cooldown(&env, "EmergencyStop")?;
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
    pub fn migrate(env: Env, admin: Address) -> Result<(), Error> {
        admin::do_migrate(&env, admin, STORAGE_VERSION)
    }

    /// Migrate contract-level config from Instance to Persistent storage. Admin only.
    pub fn migrate_config_to_persistent(env: Env, admin: Address) -> Result<(), Error> {
        admin::migrate_config_to_persistent(&env, admin)
    }

    /// Export contract-level configuration as a snapshot for migration.
    pub fn export_contract_snapshot(env: Env, admin: Address) -> Result<ContractSnapshot, Error> {
        require_admin_auth(&env, &admin)?;
        let token: Address = admin::read_config(&env, &DataKey::Token).ok_or(Error::NotFound)?;
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

    /// Export a single subscription summary.
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

    /// Export a range of subscription summaries.
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
            }
            id += 1;
        }
        env.events().publish(
            (Symbol::new(&env, "migration_export"),),
            MigrationExportEvent {
                admin,
                start_id,
                limit,
                exported: out.len(),
                timestamp: env.ledger().timestamp(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(out)
    }

    /// Export full snapshot page including balances.
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
                let bal: i128 = env
                    .storage()
                    .instance()
                    .get(&DataKey::MerchantBalance(
                        sub.merchant.clone(),
                        sub.token.clone(),
                    ))
                    .unwrap_or(0i128);
                balances.push_back(MerchantBalanceEntry {
                    merchant: sub.merchant,
                    token: sub.token,
                    amount: bal,
                });
            }
            id += 1;
        }
        let next_start = if end_id < next_id { Some(end_id) } else { None };
        env.events().publish(
            (Symbol::new(&env, "snapshot_exported"),),
            SnapshotExportedEvent {
                admin,
                start_id,
                exported: subs.len(),
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
        _next_start_id: Option<u32>,
    ) -> Result<(), Error> {
        require_admin_auth(&env, &admin)?;
        if !get_emergency_stop(&env) {
            return Err(Error::RecoveryNotAllowed);
        }
        let mut restored: u32 = 0;
        let mut max_next = env
            .storage()
            .instance()
            .get(&DataKey::NextId)
            .unwrap_or(0u32);
        let mut i = 0u32;
        while i < subscriptions.len() {
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
                    cancel_at: None,
                    // Default to auto_renew=true on snapshot restore; the original value
                    // is not stored in SubscriptionSummary (pre-562 snapshots).
                    auto_renew: true,
                    auto_renew_disabled_at: None,
                };
                env.storage()
                    .persistent()
                    .set(&DataKey::Sub(s.subscription_id), &sub);
                restored = restored.saturating_add(1);
                let candidate = s.subscription_id.saturating_add(1);
                if candidate > max_next {
                    max_next = candidate;
                }
            }
            i += 1;
        }
        let mut j = 0u32;
        while j < balances.len() {
            if let Some(b) = balances.get(j) {
                env.storage().instance().set(
                    &DataKey::MerchantBalance(b.merchant.clone(), b.token.clone()),
                    &b.amount,
                );
            }
            j += 1;
        }
        let current_next: u32 = env
            .storage()
            .instance()
            .get(&DataKey::NextId)
            .unwrap_or(0u32);
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
    #[allow(clippy::too_many_arguments)]
    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        lifetime_cap: Option<i128>,
        expires_at: Option<u64>,
        inviter: Option<Address>,
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
            inviter.clone(),
        )?;
        let token: Address = admin::read_config(&env, &DataKey::Token).ok_or(Error::NotFound)?;
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
                timestamp: env.ledger().timestamp(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(sub_id)
    }

    /// Creates a new subscription using a specific accepted token.
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
        inviter: Option<Address>,
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
            inviter.clone(),
        )?;
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
                timestamp: env.ledger().timestamp(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(sub_id)
    }

    /// Deposit funds into a subscription's prepaid balance.
    pub fn deposit_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
        idem_key: Option<soroban_sdk::BytesN<32>>,
    ) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "deposit_funds")?;
        subscription::do_deposit_funds(
            &env,
            subscription_id,
            subscriber.clone(),
            amount,
            idem_key,
        )?;
        let sub = queries::get_subscription(&env, subscription_id)?;
        env.events().publish(
            (Symbol::new(&env, "deposited"), subscription_id),
            FundsDepositedEvent {
                subscription_id,
                subscriber,
                token: sub.token,
                amount,
                new_balance: sub.prepaid_balance,
                timestamp: env.ledger().timestamp(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    /// Grace-period buyout: deposit enough to cover the missed charge plus a
    /// buyout premium and immediately return to Active.
    ///
    /// Combines deposit + charge in one atomic call so the subscriber does not
    /// need to cancel and re-create when a payment method briefly fails.
    ///
    /// Returns `(charge_amount, premium_paid)` on success.
    pub fn grace_buyout(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
        idem_key: Option<soroban_sdk::BytesN<32>>,
    ) -> Result<(i128, i128), Error> {
        require_not_emergency_stop(&env)?;
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "grace_buyout")?;
        subscription::do_grace_buyout(&env, subscription_id, subscriber, amount, idem_key)
    }

    /// Creates a reusable plan template.
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

    /// Creates a plan template with a specific token.
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

    /// Create a subscription from a plan.
    pub fn create_subscription_from_plan(
        env: Env,
        subscriber: Address,
        plan_template_id: u32,
    ) -> Result<u32, Error> {
        require_not_emergency_stop(&env)?;
        subscription::do_create_subscription_from_plan(&env, subscriber, plan_template_id)
    }

    /// Retrieve a plan template.
    pub fn get_plan_template(env: Env, plan_template_id: u32) -> Result<PlanTemplate, Error> {
        subscription::get_plan_template(&env, plan_template_id)
    }

    /// Updates a plan template (versioning).
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

    /// Sets max active subscriptions per plan.
    pub fn set_plan_max_active_subs(
        env: Env,
        merchant: Address,
        plan_template_id: u32,
        max_active: u32,
    ) -> Result<(), Error> {
        subscription::do_set_plan_max_active_subs(&env, merchant, plan_template_id, max_active)
    }

    /// Get max active subscriptions for a plan.
    pub fn get_plan_max_active_subs(env: Env, plan_template_id: u32) -> u32 {
        queries::get_plan_max_active_subs(&env, plan_template_id)
    }

    /// Register a new plan template in the on-chain catalogue.
    ///
    /// Merchants publish named billing offers (amount, interval, trial_seconds)
    /// that subscribers reference by plan ID when creating subscriptions. This
    /// reduces on-chain input errors and supports UI-driven catalogues.
    ///
    /// Requires merchant authorisation. Returns the new plan ID.
    pub fn register_plan(
        env: Env,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        trial_seconds: u64,
        usage_enabled: bool,
        lifetime_cap: Option<i128>,
    ) -> Result<u32, Error> {
        require_not_emergency_stop(&env)?;
        merchant::do_register_plan(
            &env,
            merchant,
            amount,
            interval_seconds,
            trial_seconds,
            usage_enabled,
            lifetime_cap,
        )
    }

    /// Deprecate an existing plan template, permanently preventing new
    /// subscriptions from being created using it.
    ///
    /// Deprecation is idempotent: calling it on an already-deprecated plan is a
    /// no-op. Only the merchant who owns the plan may deprecate it. Existing
    /// subscriptions created from the plan are unaffected.
    ///
    /// Requires merchant authorisation.
    pub fn deprecate_plan(
        env: Env,
        merchant: Address,
        plan_id: u32,
    ) -> Result<(), Error> {
        merchant::do_deprecate_plan(&env, merchant, plan_id)
    }

    /// Migrate a subscription to a new plan version.
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

    /// Set subscriber-level credit limit. Admin only.
    pub fn set_subscriber_credit_limit(
        env: Env,
        admin: Address,
        subscriber: Address,
        token: Address,
        limit: i128,
    ) -> Result<(), Error> {
        subscription::do_set_subscriber_credit_limit(&env, admin, subscriber, token, limit)
    }

    /// Get subscriber credit limit.
    pub fn get_subscriber_credit_limit(env: Env, subscriber: Address, token: Address) -> i128 {
        subscription::get_subscriber_credit_limit(&env, subscriber, token)
    }

    /// Set (or clear, with `cap = None`) an admin override of a subscriber's
    /// active-subscription cap — e.g. to raise the limit for an institutional
    /// account. Admin only. (#578)
    pub fn set_subscriber_active_cap(env: Env, admin: Address, subscriber: Address, cap: Option<u32>) -> Result<(), Error> {
        subscription::do_set_subscriber_active_cap(&env, admin, subscriber, cap)
    }

    /// Get the effective active-subscription cap for a subscriber (override
    /// if set, otherwise the default). (#578)
    pub fn get_subscriber_active_cap(env: Env, subscriber: Address) -> u32 {
        subscription::get_subscriber_active_cap(&env, &subscriber)
    }

    /// Get a subscriber's current active-subscription count. (#578)
    pub fn get_subscriber_active_count(env: Env, subscriber: Address) -> u32 {
        subscription::get_subscriber_active_count(&env, &subscriber)
    }

    pub fn set_subscriber_create_cap(
        env: Env,
        admin: Address,
        cap: u32,
    ) -> Result<(), Error> {
        admin::do_set_subscriber_create_cap(&env, admin, cap)
    }

    pub fn get_subscriber_create_cap(env: Env) -> u32 {
        admin::get_subscriber_create_cap(&env)
    }

    /// Get current subscriber exposure.
    pub fn get_subscriber_exposure(
        env: Env,
        subscriber: Address,
        token: Address,
    ) -> Result<i128, Error> {
        subscription::get_subscriber_exposure(&env, subscriber, token)
    }

    /// Set merchant max subscriptions. Admin only.
    pub fn set_merchant_max_subs(
        env: Env,
        admin: Address,
        merchant: Address,
        max_subs: u32,
    ) -> Result<(), Error> {
        subscription::do_set_merchant_max_subs(&env, admin, merchant, max_subs)
    }

    /// Get merchant max subscriptions.
    pub fn get_merchant_max_subs(env: Env, merchant: Address) -> u32 {
        queries::get_merchant_max_subs(&env, merchant)
    }

    /// Cancel a subscription.
    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_cancel_subscription(&env, subscription_id, authorizer.clone())?;
        let sub = queries::get_subscription(&env, subscription_id)?;
        env.events().publish(
            (Symbol::new(&env, "subscription_cancelled"), subscription_id),
            SubscriptionCancelledEvent {
                subscription_id,
                subscriber: sub.subscriber,
                merchant: sub.merchant,
                token: sub.token,
                authorizer,
                refund_amount: sub.prepaid_balance,
                timestamp: env.ledger().timestamp(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    /// Withdraw subscriber funds after cancel.
    pub fn withdraw_subscriber_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
    ) -> Result<(), Error> {
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "withdraw_subscriber_funds")?;
        subscription::do_withdraw_subscriber_funds(&env, subscription_id, subscriber)
    }

    /// Partial refund. Admin only.
    pub fn partial_refund(
        env: Env,
        admin: Address,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
    ) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "partial_refund")?;
        subscription::do_partial_refund(&env, admin, subscription_id, subscriber, amount)
    }

    /// Schedule future cancel.
    pub fn schedule_cancel(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
        cancel_at: u64,
    ) -> Result<(), Error> {
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "schedule_cancel")?;
        subscription::do_schedule_cancel(&env, subscription_id, authorizer, cancel_at)
    }

    /// Unschedule future cancel.
    pub fn unschedule_cancel(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "unschedule_cancel")?;
        subscription::do_unschedule_cancel(&env, subscription_id, authorizer)
    }

    /// Enable or disable auto-renewal for a subscription.
    ///
    /// When `enabled = false` the billing engine will not charge the subscription
    /// once the current interval elapses, effectively halting billing at the next
    /// natural boundary. The subscriber or merchant may re-enable auto-renewal
    /// within the *renewal window* (one full billing interval after the flag was
    /// first disabled) without re-creating the subscription — preserving all
    /// history and metadata.
    ///
    /// After the renewal window closes, re-enabling returns [`Error::RenewalWindowClosed`]
    /// and the subscription must be cancelled and recreated.
    ///
    /// # Authorization
    /// `authorizer` must be the subscriber or merchant of the subscription.
    ///
    /// # Errors
    /// * [`Error::NotFound`] — subscription does not exist.
    /// * [`Error::Forbidden`] — caller is neither subscriber nor merchant.
    /// * [`Error::InvalidStatusTransition`] — subscription is cancelled.
    /// * [`Error::SubscriptionExpired`] — subscription has expired.
    /// * [`Error::RenewalWindowClosed`] — renewal window has passed; cannot re-enable.
    ///
    /// # Events
    /// Emits [`AutoRenewToggledEvent`] with topic `("auto_renew_toggled", subscription_id)`.
    pub fn set_auto_renew(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
        enabled: bool,
    ) -> Result<(), Error> {
        subscription::do_set_auto_renew(&env, subscription_id, authorizer, enabled)
    }

    /// Pause a subscription.
    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_pause_subscription(&env, subscription_id, authorizer.clone())?;
        let timestamp = env.ledger().timestamp();

        let sub = queries::get_subscription(&env, subscription_id)?;
        env.events().publish(
            (Symbol::new(&env, "sub_paused"), subscription_id),
            SubscriptionPausedEvent {
                subscription_id,
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
        env.events().publish(
            (Symbol::new(&env, "sub_resumed"), subscription_id),
            SubscriptionResumedEvent {
                subscription_id,
                subscriber: sub.subscriber,
                merchant: sub.merchant,
                authorizer,
                previous_status: old_sub.status,
                timestamp: env.ledger().timestamp(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    /// Archive an expired or cancelled subscription.
    pub fn cleanup_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_cleanup_subscription(&env, subscription_id, authorizer)
    }

    /// Initiate a transfer to a new subscriber.
    pub fn initiate_transfer(
        env: Env,
        subscription_id: u32,
        from: Address,
        to: Address,
        expires_at: u64,
    ) -> Result<(), Error> {
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "initiate_transfer")?;
        subscription::do_initiate_transfer(&env, subscription_id, from, to, expires_at)
    }

    /// Accept a pending transfer as the new subscriber.
    pub fn accept_transfer(
        env: Env,
        subscription_id: u32,
        to: Address,
    ) -> Result<(), Error> {
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "accept_transfer")?;
        subscription::do_accept_transfer(&env, subscription_id, to)
    }

    /// Veto a pending transfer as the merchant.
    pub fn veto_transfer(
        env: Env,
        subscription_id: u32,
        merchant: Address,
    ) -> Result<(), Error> {
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "veto_transfer")?;
        subscription::do_veto_transfer(&env, subscription_id, merchant)
    }

    /// Merchant-initiated one-off charge.
    pub fn charge_one_off(
        env: Env,
        subscription_id: u32,
        merchant: Address,
        amount: i128,
        idem_key: Option<soroban_sdk::BytesN<32>>,
    ) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "charge_one_off")?;
        subscription::do_charge_one_off(&env, subscription_id, merchant, amount, idem_key)
    }

    // ── Coupons & Discounts ───────────────────────────────────────────────────

    /// Create a new merchant-managed discount coupon.
    ///
    /// # Arguments
    /// - `merchant`: Address of the merchant creating the coupon.
    /// - `code`: Unique code identifying the coupon.
    /// - `token`: Settlement token the coupon applies to (must match subscription).
    /// - `percent_off_bps`: Percentage discount in basis points (0-10,000).
    /// - `fixed_off`: Fixed token-unit discount applied after percentage (>= 0).
    /// - `max_redemptions`: Global limit on how many subscriptions can bind this coupon (0 = unlimited).
    /// - `expires_at`: Ledger timestamp after which the coupon cannot be bound (0 = never).
    pub fn create_coupon(
        env: Env,
        merchant: Address,
        code: Symbol,
        token: Address,
        percent_off_bps: u32,
        fixed_off: i128,
        max_redemptions: u32,
        expires_at: u64,
    ) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;
        coupon::create_coupon(
            &env,
            merchant,
            code,
            token,
            percent_off_bps,
            fixed_off,
            max_redemptions,
            expires_at,
        )
    }

    /// Revoke an existing coupon, preventing future bindings.
    ///
    /// Already-bound coupons that are revoked will be silently skipped during
    /// charge calculation to avoid blocking billing.
    pub fn revoke_coupon(env: Env, merchant: Address, code: Symbol) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;
        coupon::revoke_coupon(&env, merchant, code)
    }

    /// Bind a coupon code to a subscription.
    ///
    /// Only the subscriber can call this. A subscription can hold at most one bound coupon.
    pub fn apply_coupon(
        env: Env,
        subscriber: Address,
        subscription_id: u32,
        code: Symbol,
    ) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;
        coupon::apply_coupon(&env, subscriber, subscription_id, code)
    }

    /// Get details of a specific coupon.
    pub fn get_coupon(env: Env, code: Symbol) -> Option<Coupon> {
        coupon::get_coupon(&env, code)
    }

    // ── Charging ──────────────────────────────────────────────────────────────

    /// Charge a subscription for one interval.
    pub fn charge_subscription(
        env: Env,
        subscription_id: u32,
        idem_key: Option<soroban_sdk::BytesN<32>>,
    ) -> Result<ChargeExecutionResult, Error> {
        require_not_emergency_stop(&env)?;
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "charge_subscription")?;
        let old_sub = queries::get_subscription(&env, subscription_id)?;
        let timestamp = env.ledger().timestamp();
        let result = charge_core::charge_one(&env, subscription_id, timestamp, idem_key, None)?;
        let new_sub = queries::get_subscription(&env, subscription_id)?;

        let _period_start = old_sub.last_payment_timestamp;
        let _period_end = timestamp;

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
                salt: {
                    let mut salt_buf = [0u8; 20];
                    salt_buf[..4].copy_from_slice(&subscription_id.to_be_bytes());
                    salt_buf[4..12].copy_from_slice(&old_sub.last_payment_timestamp.to_be_bytes());
                    salt_buf[12..20].copy_from_slice(&env.ledger().sequence().to_be_bytes());
                    let salt_input = soroban_sdk::Bytes::from_slice(&env, &salt_buf);
                    env.crypto().sha256(&salt_input).into()
                },
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(result)
    }

    /// Charge metered usage.
    pub fn charge_usage(
        env: Env,
        subscription_id: u32,
        usage_amount: i128,
    ) -> Result<UsageChargeResult, Error> {
        require_not_emergency_stop(&env)?;
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "charge_usage")?;
        charge_core::charge_usage_one(
            &env,
            subscription_id,
            usage_amount,
            String::from_str(&env, "usage"),
        )
    }

    /// Charge usage with reference.
    pub fn charge_usage_with_reference(
        env: Env,
        subscription_id: u32,
        usage_amount: i128,
        reference: String,
    ) -> Result<UsageChargeResult, Error> {
        require_not_emergency_stop(&env)?;
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "charge_usage_with_reference")?;
        charge_core::charge_usage_one(&env, subscription_id, usage_amount, reference)
    }

    /// Configure usage limits.
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

    /// Merchant withdrawal.
    pub fn withdraw_merchant_funds(env: Env, merchant: Address, amount: i128) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "withdraw_merchant_funds")?;

        merchant::withdraw_merchant_funds(&env, merchant.clone(), amount)?;
        let new_balance = merchant::get_merchant_balance(&env, &merchant);
        let token: Address = admin::read_config(&env, &DataKey::Token).ok_or(Error::NotFound)?;
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
                timestamp: env.ledger().timestamp(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    /// Token-specific merchant withdrawal.
    pub fn withdraw_merchant_token_funds(
        env: Env,
        merchant: Address,
        token: Address,
        amount: i128,
    ) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;
        let _guard =
            crate::reentrancy::ReentrancyGuard::lock(&env, "withdraw_merchant_token_funds")?;
        merchant::withdraw_merchant_funds_for_token(&env, merchant, token, amount)
    }

    /// Get merchant balance.
    pub fn get_merchant_balance(env: Env, merchant: Address) -> i128 {
        merchant::get_merchant_balance(&env, &merchant)
    }

    /// Get merchant balance by token.
    pub fn get_merchant_balance_by_token(env: Env, merchant: Address, token: Address) -> i128 {
        merchant::get_merchant_balance_by_token(&env, &merchant, &token)
    }

    /// Get merchant token earnings.
    pub fn get_merchant_token_earnings(
        env: Env,
        merchant: Address,
        token: Address,
    ) -> crate::types::TokenEarnings {
        merchant::get_merchant_token_earnings(&env, &merchant, &token)
    }

    /// Check if merchant is paused.
    pub fn get_merchant_paused(env: Env, merchant: Address) -> bool {
        merchant::get_merchant_paused(&env, merchant)
    }

    /// Blanket pause merchant.
    pub fn pause_merchant(env: Env, merchant: Address) -> Result<(), Error> {
        merchant::pause_merchant(&env, merchant)
    }

    /// Unpause merchant.
    pub fn unpause_merchant(env: Env, merchant: Address) -> Result<(), Error> {
        merchant::unpause_merchant(&env, merchant)
    }

    /// direct merchant refund to subscriber.
    pub fn merchant_refund(
        env: Env,
        merchant: Address,
        subscriber: Address,
        token: Address,
        amount: i128,
    ) -> Result<(), Error> {
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "merchant_refund")?;
        merchant::merchant_refund(&env, merchant, subscriber, token, amount)
    }

    /// Get reconciliation snapshot.
    pub fn get_reconciliation_snapshot(
        env: Env,
        merchant: Address,
    ) -> Vec<crate::types::TokenReconciliationSnapshot> {
        merchant::get_reconciliation_snapshot(&env, &merchant)
    }

    /// Get total earnings per token.
    pub fn get_merchant_total_earnings(
        env: Env,
        merchant: Address,
    ) -> Vec<(Address, crate::types::TokenEarnings)> {
        merchant::get_merchant_total_earnings(&env, &merchant)
    }

    // ── Payout Schedule ────────────────────────────────────────────────────────

    /// Configure merchant payout schedule.
    pub fn set_payout_schedule(
        env: Env,
        merchant: Address,
        cadence_seconds: u64,
        min_payout: i128,
    ) -> Result<PayoutSchedule, Error> {
        merchant::do_set_payout_schedule(&env, merchant, cadence_seconds, min_payout)
    }

    /// Flush merchant payouts.
    pub fn flush_payouts(env: Env, merchant: Address) -> Result<u32, Error> {
        require_not_emergency_stop(&env)?;
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "flush_payouts")?;
        merchant::do_flush_payouts(&env, merchant, env.current_contract_address())
    }

    /// Get payout schedule.
    pub fn get_payout_schedule(env: Env, merchant: Address) -> PayoutSchedule {
        merchant::get_payout_schedule(&env, &merchant)
    }

    // ── Dispute / Chargeback ──────────────────────────────────────────────────

    /// Open a dispute against a charge for a subscription.
    ///
    /// The subscriber initiates the dispute. The disputed `amount` is moved from
    /// the merchant's balance into escrow, and a [`Dispute`] record is created in
    /// `Open` status. The merchant/admin has [`DISPUTE_WINDOW_SECS`] to respond.
    ///
    /// # Auth
    ///
    /// `subscriber` must authorise and must match the subscription's registered
    /// subscriber.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — `subscriber` does not match.
    /// * [`Error::InvalidAmount`] — `amount` is zero or negative.
    /// * [`Error::DisputeAlreadyOpen`] — A dispute is already open for this subscription.
    /// * [`Error::InsufficientBalance`] — Merchant balance is insufficient.
    ///
    /// # Events
    ///
    /// Emits [`DisputeOpenedEvent`].
    pub fn open_dispute(
        env: Env,
        subscriber: Address,
        subscription_id: u32,
        amount: i128,
        evidence_hash: Option<BytesN<32>>,
    ) -> Result<u64, Error> {
        dispute::do_open_dispute(&env, subscriber, subscription_id, amount, evidence_hash)
    }

    /// Respond to a dispute with evidence. Admin only.
    ///
    /// Transitions the dispute from `Open` to `Responded`, signalling that the
    /// admin has reviewed the dispute and is prepared for resolution.
    ///
    /// # Auth
    ///
    /// `admin` must match the stored contract admin.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Not the stored admin.
    /// * [`Error::DisputeNotFound`] — No dispute for `dispute_id`.
    /// * [`Error::DisputeAlreadyResponded`] — Dispute is not in `Open` status.
    ///
    /// # Events
    ///
    /// Emits [`DisputeRespondedEvent`].
    pub fn respond_dispute(
        env: Env,
        admin: Address,
        dispute_id: u64,
        evidence_hash: Option<BytesN<32>>,
    ) -> Result<(), Error> {
        dispute::do_respond_dispute(&env, admin, dispute_id, evidence_hash)
    }

    /// Resolve a dispute, routing escrowed funds. Admin only.
    ///
    /// * If the dispute is `Open` and the window has elapsed, it auto-resolves
    ///   to the subscriber.
    /// * If the dispute is `Responded`, the admin decides via
    ///   `resolve_to_subscriber`.
    /// * Resolving before response (window not elapsed) is rejected.
    ///
    /// # Auth
    ///
    /// `admin` must match the stored contract admin.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Not the stored admin.
    /// * [`Error::DisputeNotFound`] — No dispute for `dispute_id`.
    /// * [`Error::DisputeAlreadyResolved`] — Already resolved.
    /// * [`Error::DisputeNotResponded`] — Unresponded and window not elapsed.
    ///
    /// # Events
    ///
    /// Emits [`DisputeResolvedEvent`].
    pub fn resolve_dispute(
        env: Env,
        admin: Address,
        dispute_id: u64,
        resolve_to_subscriber: bool,
    ) -> Result<(), Error> {
        dispute::do_resolve_dispute(&env, admin, dispute_id, resolve_to_subscriber)
    }

    /// Read a dispute record by its ID.
    ///
    /// # Errors
    ///
    /// * [`Error::DisputeNotFound`] — No dispute for `dispute_id`.
    pub fn get_dispute(env: Env, dispute_id: u64) -> Result<Dispute, Error> {
        dispute::do_get_dispute(&env, dispute_id)
    }

    /// Return the active dispute ID for a subscription, if any.
    pub fn get_subscription_dispute(env: Env, subscription_id: u32) -> Option<u64> {
        dispute::do_get_subscription_dispute(&env, subscription_id)
    }

    // ── Queries ──────────────────────────────────────────────────────────────

    /// Get subscription.
    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        queries::get_subscription(&env, subscription_id)
    }

    /// Estimate topup.
    pub fn estimate_topup_for_intervals(
        env: Env,
        subscription_id: u32,
        num_intervals: u32,
    ) -> Result<i128, Error> {
        queries::estimate_topup_for_intervals(&env, subscription_id, num_intervals)
    }

    /// Get next charge info.
    pub fn get_next_charge_info(env: Env, subscription_id: u32) -> Result<NextChargeInfo, Error> {
        queries::get_next_charge_info(&env, subscription_id)
    }

    /// Get token subscription count.
    pub fn get_token_subscription_count(env: Env, token: Address) -> u32 {
        queries::get_token_subscription_count(&env, token)
    }

    /// Get merchant subscription count.
    pub fn get_merchant_subscription_count(env: Env, merchant: Address) -> u32 {
        queries::get_merchant_subscription_count(&env, merchant)
    }

    /// List subscriptions by subscriber.
    pub fn list_subscriptions_by_subscriber(
        env: Env,
        subscriber: Address,
        start_from_id: u32,
        limit: u32,
    ) -> Result<crate::queries::SubscriptionsPage, Error> {
        crate::queries::list_subscriptions_by_subscriber(&env, subscriber, start_from_id, limit)
    }

    /// Get cap info.
    pub fn get_cap_info(env: Env, subscription_id: u32) -> Result<CapInfo, Error> {
        queries::get_cap_info(&env, subscription_id)
    }

    /// Set global cap default. Admin only.
    pub fn set_global_cap_default(
        env: Env,
        admin: Address,
        cap: Option<i128>,
    ) -> Result<(), Error> {
        subscription::do_set_global_cap_default(&env, admin, cap)
    }

    /// Get global cap default.
    pub fn get_global_cap_default(env: Env) -> Option<i128> {
        subscription::get_global_cap_default(&env)
    }

    /// Set merchant cap default.
    pub fn set_merchant_cap_default(
        env: Env,
        merchant: Address,
        cap: Option<i128>,
    ) -> Result<(), Error> {
        validation::reject_contract_self(&env, &merchant)?;
        subscription::do_set_merchant_cap_default(&env, merchant, cap)
    }

    /// Get merchant cap default.
    pub fn get_merchant_cap_default(env: Env, merchant: Address) -> Option<i128> {
        subscription::get_merchant_cap_default(&env, merchant)
    }

    /// Update subscription cap. Admin only.
    pub fn update_subscription_cap(
        env: Env,
        admin: Address,
        subscription_id: u32,
        new_cap: Option<i128>,
    ) -> Result<(), Error> {
        subscription::do_update_subscription_cap(&env, admin, subscription_id, new_cap)
    }

    /// Get statements by offset.
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

    /// Get statements by cursor.
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

    /// Get period snapshot.
    pub fn get_period_snapshot(
        env: Env,
        subscription_id: u32,
        period_index: u64,
    ) -> Option<BillingPeriodSnapshot> {
        period_snapshots::get_period_snapshot(&env, subscription_id, period_index)
    }

    /// List period snapshots.
    pub fn list_period_snapshots(
        env: Env,
        subscription_id: u32,
        limit: u32,
    ) -> Vec<BillingPeriodSnapshot> {
        period_snapshots::list_period_snapshots(&env, subscription_id, limit)
    }

    /// Add accepted token. Admin only.
    pub fn add_accepted_token(
        env: Env,
        admin: Address,
        token: Address,
        decimals: u32,
    ) -> Result<(), Error> {
        validation::reject_contract_self(&env, &token)?;
        admin::add_accepted_token(&env, admin, token, decimals)
    }

    /// Remove accepted token. Admin only.
    pub fn remove_accepted_token(env: Env, admin: Address, token: Address) -> Result<(), Error> {
        admin::remove_accepted_token(&env, admin, token)
    }

    /// List accepted tokens.
    pub fn list_accepted_tokens(env: Env) -> Vec<AcceptedToken> {
        admin::list_accepted_tokens(&env)
    }

    /// List merchant subscriptions.
    pub fn get_subscriptions_by_merchant(
        env: Env,
        merchant: Address,
        start: u32,
        limit: u32,
    ) -> Result<Vec<Subscription>, Error> {
        queries::get_subscriptions_by_merchant(&env, merchant, start, limit)
    }

    /// List token subscriptions.
    pub fn get_subscriptions_by_token(
        env: Env,
        token: Address,
        start: u32,
        limit: u32,
    ) -> Result<Vec<Subscription>, Error> {
        queries::get_subscriptions_by_token(&env, token, start, limit)
    }

    // ── Reconciliation Queries ─────────────────────────────────────────────────

    /// Token reconciliation summary.
    pub fn get_token_reconciliation(env: Env, token: Address) -> TokenLiabilities {
        queries::get_token_reconciliation(&env, token)
    }

    /// All tokens reconciliation summary.
    pub fn get_recon_summary(
        env: Env,
        start_token_index: u32,
        limit: u32,
    ) -> ReconciliationSummaryPage {
        queries::get_contract_reconciliation_summary(&env, start_token_index, limit)
    }

    /// auditable reconciliation proof.
    pub fn generate_reconciliation_proof(env: Env, token: Address) -> ReconciliationProof {
        queries::generate_reconciliation_proof(&env, token)
    }

    /// Paginated prepaid balance query.
    pub fn query_prepaid_balances_paginated(
        env: Env,
        request: PrepaidQueryRequest,
    ) -> PrepaidQueryResult {
        queries::query_prepaid_balances_paginated(&env, request)
    }

    /// Set billing retention. Admin only.
    pub fn set_billing_retention(env: Env, admin: Address, keep_recent: u32) -> Result<(), Error> {
        require_admin_auth(&env, &admin)?;
        admin::enforce_config_cooldown(&env, "BillingRetention")?;
        statements::set_retention_config(&env, keep_recent);
        Ok(())
    }

    /// Get billing retention.
    pub fn get_billing_retention(env: Env) -> BillingRetentionConfig {
        statements::get_retention_config(&env)
    }

    /// Get compacted aggregate totals.
    pub fn get_stmt_compacted_aggregate(
        env: Env,
        subscription_id: u32,
    ) -> BillingStatementAggregate {
        statements::get_compacted_aggregate(&env, subscription_id)
    }

    /// Explicitly compact billing statements. Admin only.
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

    /// Get oracle config.
    pub fn get_oracle_config(env: Env) -> OracleConfig {
        oracle::get_oracle_config(&env)
    }

    /// Configure the oracle price deviation circuit-breaker threshold.
    ///
    /// The threshold is expressed in basis points (bps, where 1 % = 100 bps).
    /// When the latest oracle price deviates from the median of the last N
    /// samples by more than this amount, the charge is rejected with
    /// [`Error::OracleDeviationTooHigh`] and an [`OracleDeviationBreakerEvent`]
    /// is emitted.
    ///
    /// A value of `0` rejects **any** price change (strict mode).
    /// When never called, the deviation check is disabled entirely.
    ///
    /// # Auth
    ///
    /// Admin only.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller is not the admin.
    pub fn set_oracle_deviation_bps(
        env: Env,
        admin: Address,
        bps: u32,
    ) -> Result<(), Error> {
        require_admin_auth(&env, &admin)?;
        oracle::set_oracle_deviation_bps(&env, bps);
        Ok(())
    }

    /// Return the current deviation threshold, or `None` if not configured.
    pub fn get_oracle_deviation_bps(env: Env) -> Option<u32> {
        oracle::get_oracle_deviation_bps(&env)
    }

    /// Return the recorded oracle price history for a token (insertion order).
    pub fn get_oracle_price_history(env: Env, token: Address) -> Vec<i128> {
        oracle::get_oracle_price_history(&env, &token)
    }

    /// Emit oracle liveness event.
    pub fn emit_oracle_liveness(env: Env) -> Result<crate::types::OracleLivenessEvent, Error> {
        oracle::emit_oracle_liveness(&env)
    }

    // ── Metadata ──────────────────────────────────────────────────────────────

    /// Set subscription metadata.
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
    /// Apply an off-chain signed metadata update.
    ///
    /// Merchants (or any party running an off-chain batcher) can pre-sign
    /// metadata payloads with their ed25519 secret key and submit them as
    /// a single transaction instead of paying one `set_metadata` fee per
    /// key. Auth is proven entirely on the recipient side via the
    /// signature — the calling transaction does **not** need a matching
    /// `require_auth` from the signer.
    ///
    /// The off-chain signer must build the canonical byte stream produced
    /// by [`crate::metadata::build_metadata_signed_message`] (a fixed
    /// domain tag plus `(subscription_id, key, value, nonce, chain_id,
    /// expires_at)`, length-prefixed and big-endian) and produce an
    /// ed25519 signature over those bytes using the secret key whose
    /// public counterpart is `signer_pubkey`.
    ///
    /// # Authorization
    ///
    /// The pubkey must correspond to the subscription's `subscriber` or
    /// `merchant`. Otherwise [`Error::Forbidden`] is returned once the
    /// signature itself has been verified.
    ///
    /// # Replay protection
    ///
    /// The off-chain signer queries
    /// [`get_metadata_signed_nonce`](Self::get_metadata_signed_nonce), signs
    /// the payload with that nonce, and submits. The contract consumes the
    /// nonce for `(signer, DOMAIN_METADATA_SIGNED)`. A captured payload is
    /// rejected with [`Error::NonceAlreadyUsed`].
    ///
    /// # Expiry
    ///
    /// `expires_at` is enforced strictly: `now >= expires_at` ⇒
    /// [`Error::InvalidInput`]. Off-chain tooling should pick `expires_at`
    /// comfortably after the expected submission window.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — `subscription_id` does not exist.
    /// * [`Error::InvalidInput`] — expired payload or empty key/value.
    /// * [`Error::MetadataKeyTooLong`] / [`Error::MetadataValueTooLong`].
    /// * [`Error::MetadataKeyLimitReached`].
    /// * [`Error::NonceAlreadyUsed`] — replay or out-of-order nonce.
    /// * [`Error::Forbidden`].
    /// * [`Error::Overflow`] — nonce counter would overflow `u64::MAX`.
    ///
    /// Panics (host crypto error) on a forged ed25519 signature. That
    /// boundary is intentional; a forged signature must abort the
    /// transaction rather than be silently downgraded to a typed error.
    ///
    /// # Events
    ///
    /// On success emits `metadata_set_signed` (carrying
    /// [`MetadataSetSignedEvent`]) and `nonce_consumed` keyed on
    /// `(signer, DOMAIN_METADATA_SIGNED)`.
    pub fn set_metadata_signed(
        env: Env,
        signer_pubkey: soroban_sdk::BytesN<32>,
        payload: SignedMetadataPayload,
        signature: soroban_sdk::BytesN<64>,
    ) -> Result<(), Error> {
        // Defense-in-depth ABI guards on the signed path: a sha-bump signer
        // who controls a matching ed25519 key still cannot drive
        // degenerate empty/whitespace keys or values into storage.
        validation::reject_empty_string(&payload.key)?;
        validation::reject_empty_string(&payload.value)?;
        metadata::do_set_metadata_signed(&env, signer_pubkey, payload, signature)
    }

    /// Return the next-expected nonce for `(signer, DOMAIN_METADATA_SIGNED)`.
    ///
    /// Off-chain batching tools fetch this value before signing the next
    /// [`SignedMetadataPayload`] so the on-chain `nonce::check_and_advance`
    /// call accepts the payload. Returns `0` for a signer's first signed
    /// update against this contract.
    ///
    /// Read-only; no auth required.
    pub fn get_metadata_signed_nonce(env: Env, signer: Address) -> u64 {
        nonce::get_nonce(&env, &signer, nonce::DOMAIN_METADATA_SIGNED)
    }

    /// Delete metadata.
    pub fn delete_metadata(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
        key: String,
    ) -> Result<(), Error> {
        validation::reject_empty_string(&key)?;
        metadata::delete_metadata(&env, subscription_id, &authorizer, key)
    }

    /// Get metadata.
    pub fn get_metadata(env: Env, subscription_id: u32, key: String) -> Result<String, Error> {
        metadata::get_metadata(&env, subscription_id, key)
    }

    /// List metadata keys.
    pub fn list_metadata_keys(env: Env, subscription_id: u32) -> Result<Vec<String>, Error> {
        metadata::list_metadata_keys(&env, subscription_id)
    }

    // ── Protocol Fees ──────────────────────────────────────────────────────────

    /// Queue a protocol fee/treasury change for delayed activation. Admin only.
    pub fn queue_treasury_change(env: Env, admin: Address, treasury: Address, fee_bps: u32) -> Result<(), Error> {
        validation::reject_contract_self(&env, &treasury)?;
        admin::queue_treasury_change(&env, admin, treasury, fee_bps)
    }

    /// Execute a queued treasury change after the timelock elapses. Admin only.
    pub fn execute_treasury_change(env: Env, admin: Address) -> Result<(), Error> {
        admin::execute_treasury_change(&env, admin)
    }

    /// Cancel a queued treasury change before it is executed. Admin only.
    pub fn cancel_treasury_change(env: Env, admin: Address) -> Result<(), Error> {
        admin::cancel_treasury_change(&env, admin)
    }

    /// Set protocol fee. Admin only.
    pub fn set_protocol_fee(
        env: Env,
        admin: Address,
        treasury: Address,
        fee_bps: u32,
    ) -> Result<(), Error> {
        validation::reject_contract_self(&env, &treasury)?;
        admin::set_protocol_fee(&env, admin, treasury, fee_bps)
    }

    /// Get protocol fee bps.
    pub fn get_protocol_fee_bps(env: Env) -> u32 {
        admin::get_protocol_fee_bps(&env)
    }

    /// Set the fee-token override address. Admin only.
    ///
    /// When set and different from the subscription's settlement token,
    /// protocol fees are converted through the oracle and paid in
    /// `fee_token`. Pass `None` to clear the override.
    pub fn set_fee_token(
        env: Env,
        admin: Address,
        fee_token: Option<Address>,
    ) -> Result<(), Error> {
        admin::set_fee_token(&env, admin, fee_token)
    }

    /// Return the configured fee-token override address, or `None` if not set.
    pub fn get_fee_token(env: Env) -> Option<Address> {
        admin::get_fee_token(&env)
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
    pub fn add_guardian(
        env: Env,
        admin: Address,
        guardian: Address,
        weight: u32,
    ) -> Result<(), Error> {
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

    /// Blocklist subscriber.
    pub fn add_to_blocklist(
        env: Env,
        authorizer: Address,
        subscriber: Address,
        reason: Option<String>,
    ) -> Result<(), Error> {
        blocklist::do_add_to_blocklist(&env, authorizer, subscriber, reason)
    }

    /// Unblocklist subscriber. Admin only.
    pub fn remove_from_blocklist(
        env: Env,
        admin: Address,
        subscriber: Address,
    ) -> Result<(), Error> {
        blocklist::do_remove_from_blocklist(&env, admin, subscriber)
    }

    /// Get blocklist entry.
    pub fn get_blocklist_entry(env: Env, subscriber: Address) -> Result<BlocklistEntry, Error> {
        blocklist::get_blocklist_entry(&env, subscriber)
    }

    /// Check if blocklisted.
    pub fn is_blocklisted(env: Env, subscriber: Address) -> bool {
        blocklist::is_blocklisted(&env, &subscriber)
    }

    /// Initialize merchant config.
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

    /// Get the global merchant whitelist mode toggle.
    pub fn get_whitelist_mode(env: Env) -> bool {
        merchant::get_whitelist_mode(&env)
    }

    /// Enable or disable the global merchant whitelist mode. Admin-only.
    pub fn set_whitelist_mode(env: Env, admin: Address, enabled: bool) -> Result<(), Error> {
        merchant::set_whitelist_mode(&env, admin, enabled)
    }

    /// Check whether a merchant is approved under whitelist mode.
    pub fn is_merchant_approved(env: Env, merchant: Address) -> bool {
        merchant::is_merchant_approved(&env, &merchant)
    }

    /// Approve a merchant under whitelist mode. Admin-only.
    pub fn approve_merchant(env: Env, admin: Address, merchant: Address) -> Result<(), Error> {
        merchant::approve_merchant(&env, admin, merchant)
    }

    /// Revoke a merchant's approval under whitelist mode. Admin-only.
    pub fn revoke_merchant(env: Env, admin: Address, merchant: Address) -> Result<(), Error> {
        merchant::revoke_merchant(&env, admin, merchant)
    }

    /// Get the admin-controlled allowlist of valid merchant compliance-category tags.
    pub fn get_tag_allowlist(env: Env) -> Vec<Symbol> {
        merchant::get_tag_allowlist(&env)
    }

    /// Replace the global merchant tag allowlist. Admin-only.
    ///
    /// # Errors
    /// * [`Error::Unauthorized`] — caller is not the stored admin.
    /// * [`Error::DuplicateMerchantTag`] — `tags` contains the same symbol twice.
    pub fn set_tag_allowlist(env: Env, admin: Address, tags: Vec<Symbol>) -> Result<(), Error> {
        merchant::set_tag_allowlist(&env, admin, tags)
    }

    /// Get the compliance-category tags currently assigned to `merchant`.
    pub fn get_merchant_tags(env: Env, merchant: Address) -> Vec<Symbol> {
        merchant::get_merchant_tags(&env, merchant)
    }

    /// Set (fully replacing) `merchant`'s compliance-category tags. Admin-only.
    /// Pass an empty `tags` vector to clear all tags.
    ///
    /// # Errors
    /// * [`Error::Unauthorized`] — caller is not the stored admin.
    /// * [`Error::MerchantTagLimitExceeded`] — more than `MAX_MERCHANT_TAGS` tags supplied.
    /// * [`Error::DuplicateMerchantTag`] — `tags` contains the same symbol twice.
    /// * [`Error::UnknownMerchantTag`] — a tag is not present in the current allowlist.
    pub fn set_merchant_tags(
        env: Env,
        admin: Address,
        merchant: Address,
        tags: Vec<Symbol>,
    ) -> Result<(), Error> {
        merchant::set_merchant_tags(&env, admin, merchant, tags)
    }

    /// Update merchant config.
    pub fn set_merchant_config(
        env: Env,
        merchant: Address,
        config: MerchantConfig,
    ) -> Result<(), Error> {
        merchant::set_merchant_config(&env, merchant, config)
    }

    /// Configure merchant withdrawal co-signers and threshold.
    pub fn set_merchant_multisig(env: Env, admin: Address, merchant: Address, signers: Vec<Address>, threshold: u32) -> Result<(), Error> { merchant::set_merchant_multisig(&env, admin, merchant, signers, threshold) }

    /// Get merchant withdrawal co-signer config.
    pub fn get_merchant_multisig_config(env: Env, merchant: Address) -> Option<crate::types::MerchantMultiSigConfig> { merchant::get_merchant_multisig_config(&env, merchant) }

    /// Partial update merchant config.
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

    /// Get merchant config.
    pub fn get_merchant_config(
        env: Env,
        merchant: Address,
    ) -> Option<crate::types::MerchantConfig> {
        merchant::get_merchant_config(&env, merchant)
    }

    /// Set the number of consecutive InsufficientBalance failures before a
    /// subscription is automatically paused. `0` disables auto-pause. Admin only.
    pub fn set_auto_pause_threshold(env: Env, admin: Address, threshold: u32) -> Result<(), Error> {
        admin::do_set_auto_pause_threshold(&env, admin, threshold)
    }

    /// Return the current auto-pause threshold (`0` = disabled).
    pub fn get_auto_pause_threshold(env: Env) -> u32 {
        admin::get_auto_pause_threshold(&env)
    }

    /// Returns the schema version.
    pub fn version(_env: Env) -> u32 {
        1
    }

    /// Returns total subscription count.
    pub fn get_subscription_count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::NextId)
            .unwrap_or(0u32)
    }

    /// Internal ID allocator.
    fn _next_id(env: &Env) -> Result<u32, Error> {
        let current: u32 = env
            .storage()
            .instance()
            .get(&DataKey::NextId)
            .unwrap_or(0u32);
        if current == MAX_SUBSCRIPTION_ID {
            return Err(Error::SubscriptionLimitReached);
        }
        env.storage()
            .instance()
            .set(&DataKey::NextId, &(current + 1));
        Ok(current)
    }
}

#[cfg(test)]
mod test_usage_limits_required;

#[cfg(test)]
mod test_charge_invariants;
#[cfg(test)]
mod test_charge_event_exclusivity;
#[cfg(test)]
mod test_metadata_signed;

#[cfg(test)]
mod test_scheduled_cancel;
#[cfg(test)]
mod test_subscriber_active_cap;
#[cfg(test)]
mod test_statement_compaction;

#[cfg(test)]
mod test_billing_period_snapshots;
#[cfg(test)]
mod test_merchant_full_drain;

#[cfg(test)]
mod test_validation;

#[cfg(test)]
mod test_abi_validators_integration;
#[cfg(test)]
mod test_coupon;

#[cfg(test)]
mod test_bulk_admin_ops;

#[cfg(test)]
mod test_auto_pause;

#[cfg(test)]
mod test_grace_buyout;

#[cfg(test)]
mod test_subscription_transfer;

/// ABI-hash regression guard.
///
/// The hash is computed from the sorted, newline-joined list of all
/// `pub fn` names exposed by `#[contractimpl]`. Any accidental rename,
/// addition, or removal of a contract entrypoint will change the hash and
/// cause this test to fail, flagging the ABI breakage before it reaches
/// production.
///
/// **To update intentionally:** change only the `EXPECTED` constant below
/// and document the ABI change in the PR description.
#[cfg(test)]
mod test_merchant_whitelist;

#[cfg(test)]
mod test_merchant_tags;

#[cfg(test)]
mod test_referral_self;