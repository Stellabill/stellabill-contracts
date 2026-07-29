//! Merchant-facing API surface.
//!
//! This module groups every entrypoint in [`crate::SubscriptionVault`] that is
//! primarily concerned with merchant-facing operations: withdrawals, balance
//! queries, configuration, payout schedules, and dispute resolution.
//!
//! # Navigation
//!
//! The entrypoints themselves live in `lib.rs` inside the single
//! `#[contractimpl]` block (required by the Soroban SDK). This module re-exports
//! the *inner delegate functions* they call so that IDE navigation and
//! `cargo doc` can surface the grouped API in one place.
//!
//! # Entrypoint Groups
//!
//! ## Withdrawals & Balances
//! | Entrypoint | Delegate |
//! |---|---|
//! | `withdraw_merchant_funds` | [`crate::merchant::withdraw_merchant_funds`] |
//! | `withdraw_merchant_token_funds` | [`crate::merchant::withdraw_merchant_funds_for_token`] |
//! | `get_merchant_balance` | [`crate::merchant::get_merchant_balance`] |
//! | `get_merchant_balance_by_token` | [`crate::merchant::get_merchant_balance_by_token`] |
//! | `get_merchant_token_earnings` | [`crate::merchant::get_merchant_token_earnings`] |
//! | `get_merchant_total_earnings` | [`crate::merchant::get_merchant_total_earnings`] |
//! | `merchant_refund` | [`crate::merchant::merchant_refund`] |
//!
//! ## Merchant Config
//! | Entrypoint | Delegate |
//! |---|---|
//! | `initialize_merchant_config` | [`crate::merchant::initialize_merchant_config`] |
//! | `set_merchant_config` | [`crate::merchant::set_merchant_config`] |
//! | `update_merchant_config` | [`crate::merchant::update_merchant_config`] |
//! | `get_merchant_config` | [`crate::merchant::get_merchant_config`] |
//! | `set_merchant_multisig` | [`crate::merchant::set_merchant_multisig`] |
//! | `get_merchant_multisig_config` | [`crate::merchant::get_merchant_multisig_config`] |
//!
//! ## Pause / Unpause
//! | Entrypoint | Delegate |
//! |---|---|
//! | `get_merchant_paused` | [`crate::merchant::get_merchant_paused`] |
//! | `pause_merchant` | [`crate::merchant::pause_merchant`] |
//! | `unpause_merchant` | [`crate::merchant::unpause_merchant`] |
//!
//! ## Payout Schedules
//! | Entrypoint | Delegate |
//! |---|---|
//! | `set_payout_schedule` | [`crate::merchant::do_set_payout_schedule`] |
//! | `flush_payouts` | [`crate::merchant::do_flush_payouts`] |
//! | `get_payout_schedule` | [`crate::merchant::get_payout_schedule`] |
//!
//! ## Reconciliation
//! | Entrypoint | Delegate |
//! |---|---|
//! | `get_reconciliation_snapshot` | [`crate::merchant::get_reconciliation_snapshot`] |
//! | `get_token_reconciliation` | [`crate::queries::get_token_reconciliation`] |
//! | `get_recon_summary` | [`crate::queries::get_contract_reconciliation_summary`] |
//! | `generate_reconciliation_proof` | [`crate::queries::generate_reconciliation_proof`] |
//! | `query_prepaid_balances_paginated` | [`crate::queries::query_prepaid_balances_paginated`] |
//!
//! ## Dispute Resolution (merchant / admin side)
//! | Entrypoint | Delegate |
//! |---|---|
//! | `respond_dispute` | [`crate::dispute::do_respond_dispute`] |
//! | `resolve_dispute` | [`crate::dispute::do_resolve_dispute`] |
//!
//! ## Subscription Queries
//! | Entrypoint | Delegate |
//! |---|---|
//! | `get_subscriptions_by_merchant` | [`crate::queries::get_subscriptions_by_merchant`] |
//! | `get_merchant_subscription_count` | [`crate::queries::get_merchant_subscription_count`] |
//! | `get_merchant_max_subs` | [`crate::queries::get_merchant_max_subs`] |

// Re-export delegate functions so IDE navigation and `cargo doc` surface them
// under this feature group. No new ABI symbols are introduced; all public
// contract entrypoints remain in `lib.rs` under `#[contractimpl]`.

pub use crate::dispute::{do_lodge_escrow_dispute, do_resolve_dispute, do_respond_dispute};
pub use crate::merchant::{
    do_flush_payouts, do_set_payout_schedule, get_merchant_balance, get_merchant_balance_by_token,
    get_merchant_config, get_merchant_multisig_config, get_merchant_paused,
    get_merchant_token_earnings, get_merchant_total_earnings, get_payout_schedule,
    get_reconciliation_snapshot, initialize_merchant_config, merchant_refund, pause_merchant,
    set_merchant_config, set_merchant_multisig, unpause_merchant, update_merchant_config,
    withdraw_merchant_funds, withdraw_merchant_funds_for_token,
};
pub use crate::queries::{
    generate_reconciliation_proof, get_contract_reconciliation_summary,
    get_merchant_max_subs, get_merchant_subscription_count, get_subscriptions_by_merchant,
    get_token_reconciliation, query_prepaid_balances_paginated,
};
