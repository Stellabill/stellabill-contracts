//! Admin / protocol-governance API surface.
//!
//! This module groups every entrypoint in [`crate::SubscriptionVault`] that
//! requires the stored admin address or affects global protocol configuration:
//! initialisation, rotation, two-step admin proposal, operator management,
//! emergency stop, token allowlists, protocol fees, billing retention,
//! migration/export, oracle configuration, governance proposals, the blocklist,
//! and snapshot restore/export.
//!
//! # Navigation
//!
//! The entrypoints themselves live in `lib.rs` inside the single
//! `#[contractimpl]` block (required by the Soroban SDK). This module re-exports
//! the *inner delegate functions* they call so that IDE navigation and
//! `cargo doc` can surface the grouped API in one place.
//!
//! # ABI Stability
//!
//! No entrypoints are defined here. All `pub fn` symbols on [`crate::SubscriptionVault`]
//! live in `lib.rs`. Adding or removing an import in this file has **zero effect**
//! on the compiled ABI; only changes to the `#[contractimpl]` block in `lib.rs` do.
//!
//! # Entrypoint Groups
//!
//! ## Initialisation & Config
//! | Entrypoint | Delegate |
//! |---|---|
//! | `init` | [`crate::admin::do_init`] |
//! | `set_min_topup` | [`crate::admin::do_set_min_topup`] |
//! | `get_min_topup` | [`crate::admin::get_min_topup`] |
//! | `get_admin` | [`crate::admin::do_get_admin`] |
//! | `get_admin_nonce` | [`crate::nonce::get_nonce`] |
//! | `rotate_admin` | [`crate::admin::do_rotate_admin`] |
//! | `rotate_merchant_address` | [`crate::merchant::do_rotate_merchant_address`] |
//! | `set_grace_period` | [`crate::admin::do_set_grace_period`] |
//! | `get_grace_period` | [`crate::admin::get_grace_period`] |
//!
//! ## Two-step Admin Proposal
//! | Entrypoint | Delegate |
//! |---|---|
//! | `propose_admin` | [`crate::admin::do_propose_admin`] |
//! | `claim_admin_role` | [`crate::admin::do_claim_admin_role`] |
//! | `cancel_admin_proposal` | [`crate::admin::do_cancel_admin_proposal`] |
//! | `get_admin_proposal` | [`crate::admin::get_admin_proposal`] |
//!
//! ## Operator Management
//! | Entrypoint | Delegate |
//! |---|---|
//! | `set_operator` | [`crate::operator::do_set_operator`] |
//! | `remove_operator` | [`crate::operator::do_remove_operator`] |
//! | `get_operator` | [`crate::operator::get_operator`] |
//! | `get_operator_nonce` | [`crate::nonce::get_nonce`] |
//! | `operator_batch_charge` | [`crate::operator::do_operator_batch_charge`] |
//! | `operator_charge_subscription` | [`crate::operator::do_operator_charge_subscription`] |
//! | `operator_charge_usage` | [`crate::operator::do_operator_charge_usage`] |
//! | `operator_charge_usage_with_ref` | [`crate::operator::do_operator_charge_usage_with_reference`] |
//!
//! ## Emergency Stop
//! | Entrypoint | Notes |
//! |---|---|
//! | `get_emergency_stop_status` | reads `DataKey::EmergencyStop` via `admin::read_config` |
//! | `enable_emergency_stop` | sets flag, emits [`crate::EmergencyStopEnabledEvent`] |
//! | `disable_emergency_stop` | clears flag, emits [`crate::EmergencyStopDisabledEvent`] |
//!
//! ## Bulk Admin Ops
//! | Entrypoint | Delegate |
//! |---|---|
//! | `batch_charge` | [`crate::admin::do_batch_charge`] |
//! | `bulk_pause_subscriptions` | [`crate::subscription::do_bulk_pause_subscriptions`] |
//! | `bulk_cancel_subscriptions` | [`crate::subscription::do_bulk_cancel_subscriptions`] |
//!
//! ## Token Allowlist
//! | Entrypoint | Delegate |
//! |---|---|
//! | `add_accepted_token` | [`crate::admin::add_accepted_token`] |
//! | `remove_accepted_token` | [`crate::admin::remove_accepted_token`] |
//! | `list_accepted_tokens` | [`crate::admin::list_accepted_tokens`] |
//!
//! ## Protocol Fees
//! | Entrypoint | Delegate |
//! |---|---|
//! | `set_protocol_fee` | [`crate::admin::set_protocol_fee`] |
//! | `get_protocol_fee_bps` | [`crate::admin::get_protocol_fee_bps`] |
//! | `set_fee_token` | [`crate::admin::set_fee_token`] |
//! | `get_fee_token` | [`crate::admin::get_fee_token`] |
//! | `queue_treasury_change` | [`crate::admin::queue_treasury_change`] |
//! | `execute_treasury_change` | [`crate::admin::execute_treasury_change`] |
//! | `cancel_treasury_change` | [`crate::admin::cancel_treasury_change`] |
//! ## Coupons
//! | Entrypoint | Delegate |
//! |---|---|
//! | `create_coupon` | [`crate::admin::create_coupon`] |
//! | `revoke_coupon` | [`crate::admin::revoke_coupon`] |
//!
//! ## Auto-pause & Subscriber Create Cap
//! | Entrypoint | Delegate |
//! |---|---|
//! | `set_auto_pause_threshold` | [`crate::admin::do_set_auto_pause_threshold`] |
//! | `get_auto_pause_threshold` | [`crate::admin::get_auto_pause_threshold`] |
//! | `set_subscriber_create_cap` | [`crate::admin::do_set_subscriber_create_cap`] |
//! | `get_subscriber_create_cap` | [`crate::admin::get_subscriber_create_cap`] |
//!
//! ## Billing Retention
//! | Entrypoint | Delegate |
//! |---|---|
//! | `set_billing_retention` | [`crate::statements::set_retention_config`] |
//! | `get_billing_retention` | [`crate::statements::get_retention_config`] |
//! | `compact_billing_statements` | [`crate::statements::compact_subscription_statements`] |
//!
//! ## Oracle Configuration
//! | Entrypoint | Delegate |
//! |---|---|
//! | `set_oracle_config` | [`crate::oracle::set_oracle_config`] |
//! | `get_oracle_config` | [`crate::oracle::get_oracle_config`] |
//! | `emit_oracle_liveness` | [`crate::oracle::emit_oracle_liveness`] |
//!
//! ## Migration & Export
//! | Entrypoint | Delegate |
//! |---|---|
//! | `migrate` | [`crate::admin::do_migrate`] |
//! | `migrate_config_to_persistent` | [`crate::admin::migrate_config_to_persistent`] |
//! | `export_contract_snapshot` | (inline in `lib.rs`) |
//! | `export_subscription_summary` | (inline in `lib.rs`) |
//! | `export_subscription_summaries` | (inline in `lib.rs`) |
//! | `export_full_snapshot_page` | (inline in `lib.rs`) |
//! | `restore_snapshot_page` | (inline in `lib.rs`; requires emergency stop) |
//! | `recover_stranded_funds` | [`crate::admin::do_recover_stranded_funds`] |
//!
//! ## Governance Proposals
//! | Entrypoint | Delegate |
//! |---|---|
//! | `submit_proposal` | [`crate::governance::do_submit_proposal`] |
//! | `vote_proposal` | [`crate::governance::do_vote_proposal`] |
//! | `execute_proposal` | [`crate::governance::do_execute_proposal`] |
//! | `cancel_proposal` | [`crate::governance::do_cancel_proposal`] |
//! | `add_guardian` | [`crate::governance::add_guardian`] |
//! | `remove_guardian` | [`crate::governance::remove_guardian`] |
//! | `get_guardian_weight` | [`crate::governance::get_guardian_weight`] |
//! | `get_current_proposal_id` | [`crate::governance::get_current_proposal_id`] |
//! | `get_proposal` | [`crate::governance::get_proposal`] |
//! | `list_guardians` | [`crate::governance::list_guardians`] |
//!
//! ## Blocklist
//! | Entrypoint | Delegate |
//! |---|---|
//! | `add_to_blocklist` | [`crate::blocklist::do_add_to_blocklist`] |
//! | `remove_from_blocklist` | [`crate::blocklist::do_remove_from_blocklist`] |
//! | `get_blocklist_entry` | [`crate::blocklist::get_blocklist_entry`] |
//! | `is_blocklisted` | [`crate::blocklist::is_blocklisted`] |
//!
//! ## Misc / Version
//! | Entrypoint | Notes |
//! |---|---|
//! | `version` | returns hard-coded `1u32` |
//! | `get_subscription_count` | reads `DataKey::NextId` |
//! | `get_schema_version` | [`crate::admin::get_schema_version`] |

// Re-export delegate functions so IDE navigation and `cargo doc` surface them
// under this feature group. No new ABI symbols are introduced; all public
// contract entrypoints remain in `lib.rs` under `#[contractimpl]`.

pub use crate::admin::{
    add_accepted_token, cancel_treasury_change, do_batch_charge, do_cancel_admin_proposal,
    do_claim_admin_role, do_get_admin, do_init, do_migrate, do_propose_admin,
    do_recover_stranded_funds, do_rotate_admin, do_set_auto_pause_threshold, do_set_grace_period,
    do_set_min_topup, do_set_subscriber_create_cap, execute_treasury_change, get_admin_proposal,
    get_auto_pause_threshold, get_fee_token, get_grace_period, get_min_topup,
    get_protocol_fee_bps, get_schema_version, get_subscriber_create_cap, list_accepted_tokens,
    migrate_config_to_persistent, queue_treasury_change, remove_accepted_token, set_fee_token,
    set_protocol_fee,
};
pub use crate::admin::{create_coupon, revoke_coupon};
pub use crate::blocklist::{
    do_add_to_blocklist, do_remove_from_blocklist, get_blocklist_entry, is_blocklisted,
};
pub use crate::governance::{
    add_guardian, do_cancel_proposal, do_execute_proposal, do_submit_proposal, do_vote_proposal,
    get_current_proposal_id, get_guardian_weight, get_proposal, list_guardians, remove_guardian,
};
pub use crate::merchant::do_rotate_merchant_address;
pub use crate::nonce::get_nonce;
pub use crate::operator::{
    do_operator_batch_charge, do_operator_charge_subscription, do_operator_charge_usage,
    do_operator_charge_usage_with_reference, do_remove_operator, do_set_operator, get_operator,
};
pub use crate::oracle::{emit_oracle_liveness, get_oracle_config, set_oracle_config};
pub use crate::queries::{
    estimate_topup_for_intervals, get_cap_info, get_next_charge_info, get_subscription,
    get_subscriptions_by_token, get_token_subscription_count, list_subscriptions_by_subscriber,
};
