//! Subscription-lifecycle API surface.
//!
//! This module groups every entrypoint in [`crate::SubscriptionVault`] that is
//! primarily concerned with subscriber-facing operations: creating, depositing,
//! cancelling, pausing, resuming, transferring, and charging subscriptions, as
//! well as plan-template management, coupon/discount operations, metered-usage
//! limits, billing statements, and subscriber-initiated disputes.
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
//! ## Subscription Lifecycle
//! | Entrypoint | Delegate |
//! |---|---|
//! | `create_subscription` | [`crate::subscription::do_create_subscription`] |
//! | `create_subscription_with_token` | [`crate::subscription::do_create_subscription_with_token`] |
//! | `create_subscription_with_split` | [`crate::subscription::do_create_subscription`] + split setup |
//! | `update_split_payees` | inline in `lib.rs` |
//! | `get_split_payees` | [`crate::subscription::get_split_payees`] |
//! | `deposit_funds` | [`crate::subscription::do_deposit_funds`] |
//! | `deposit_funds_on_behalf` | [`crate::subscription::do_deposit_funds_on_behalf`] |
//! | `grant_delegated_payer` | [`crate::subscription::do_grant_delegated_payer`] |
//! | `revoke_delegated_payer` | [`crate::subscription::do_revoke_delegated_payer`] |
//! | `grace_buyout` | [`crate::subscription::do_grace_buyout`] |
//! | `cancel_subscription` | [`crate::subscription::do_cancel_subscription`] |
//! | `request_emergency_withdraw` | [`crate::subscription::do_request_emergency_withdraw`] |
//! | `finalize_emergency_withdraw` | [`crate::subscription::do_finalize_emergency_withdraw`] |
//! | `withdraw_subscriber_funds` | [`crate::subscription::do_withdraw_subscriber_funds`] |
//! | `partial_refund` | [`crate::subscription::do_partial_refund`] |
//! | `schedule_cancel` | [`crate::subscription::do_schedule_cancel`] |
//! | `unschedule_cancel` | [`crate::subscription::do_unschedule_cancel`] |
//! | `set_auto_renew` | [`crate::subscription::do_set_auto_renew`] |
//! | `set_sub_exp_ledger` | [`crate::subscription::do_set_sub_exp_ledger`] |
//! | `pause_subscription` | [`crate::subscription::do_pause_subscription`] |
//! | `resume_subscription` | [`crate::subscription::do_resume_subscription`] |
//! | `cleanup_subscription` | [`crate::subscription::do_cleanup_subscription`] |
//! | `initiate_transfer` | [`crate::subscription::do_initiate_transfer`] |
//! | `accept_transfer` | [`crate::subscription::do_accept_transfer`] |
//! | `veto_transfer` | [`crate::subscription::do_veto_transfer`] |
//! | `bulk_pause_subscriptions` | [`crate::subscription::do_bulk_pause_subscriptions`] |
//! | `bulk_cancel_subscriptions` | [`crate::subscription::do_bulk_cancel_subscriptions`] |
//! | `bulk_deposit_funds` | [`crate::subscription::do_bulk_deposit_funds`] |
//!
//! ## Plan Templates & Catalogue
//! | Entrypoint | Delegate |
//! |---|---|
//! | `create_plan_template` | [`crate::subscription::do_create_plan_template`] |
//! | `create_plan_template_with_token` | [`crate::subscription::do_create_plan_template_with_token`] |
//! | `create_subscription_from_plan` | [`crate::subscription::do_create_subscription_from_plan`] |
//! | `get_plan_template` | [`crate::subscription::get_plan_template`] |
//! | `get_plan_max_active_subs` | [`crate::queries::get_plan_max_active_subs`] |
//! | `update_plan_template` | [`crate::subscription::do_update_plan_template`] |
//! | `set_plan_max_active_subs` | [`crate::subscription::do_set_plan_max_active_subs`] |
//! | `register_plan` | [`crate::merchant::do_register_plan`] |
//! | `deprecate_plan` | [`crate::merchant::do_deprecate_plan`] |
//! | `migrate_subscription_to_plan` | [`crate::subscription::do_migrate_subscription_to_plan`] |
//!
//! ## Coupons & Discounts
//! | Entrypoint | Delegate |
//! |---|---|
//! | `create_coupon` | [`crate::coupon::create_coupon`] |
//! | `revoke_coupon` | [`crate::coupon::revoke_coupon`] |
//! | `apply_coupon` | [`crate::coupon::apply_coupon`] |
//! | `get_coupon` | [`crate::coupon::get_coupon`] |
//! Coupons are token-bound discount codes with optional `percent_off_bps` and/or
//! `fixed_off`, a global `max_redemptions` cap, an `expires_at` deadline, and
//! per-subscription redemption tracking. Discounts are applied before protocol
//! fees so that `gross == discount + merchant_net + treasury_fee` remains
//! balanced.
//!
//! ## Charging
//! | Entrypoint | Delegate |
//! |---|---|
//! | `charge_subscription` | [`crate::charge_core::charge_one`] |
//! | `charge_usage` | [`crate::charge_core::charge_usage_one`] |
//! | `charge_usage_with_reference` | [`crate::charge_core::charge_usage_one`] |
//! | `charge_one_off` | [`crate::subscription::do_charge_one_off`] |
//! | `configure_usage_limits` | [`crate::subscription::do_configure_usage_limits`] |
//!
//! ## Billing Statements & Period Snapshots
//! | Entrypoint | Delegate |
//! |---|---|
//! | `get_sub_statements_offset` | [`crate::statements::get_statements_by_subscription_offset`] |
//! | `get_sub_statements_cursor` | [`crate::statements::get_statements_by_subscription_cursor`] |
//! | `get_stmt_compacted_aggregate` | [`crate::statements::get_compacted_aggregate`] |
//! | `get_period_snapshot` | [`crate::period_snapshots::get_period_snapshot`] |
//! | `list_period_snapshots` | [`crate::period_snapshots::list_period_snapshots`] |
//!
//! ## Subscriber Limits & Caps
//! | Entrypoint | Delegate |
//! |---|---|
//! | `set_subscriber_credit_limit` | [`crate::subscription::do_set_subscriber_credit_limit`] |
//! | `get_subscriber_credit_limit` | [`crate::subscription::get_subscriber_credit_limit`] |
//! | `set_subscriber_active_cap` | [`crate::subscription::do_set_subscriber_active_cap`] |
//! | `get_subscriber_active_cap` | [`crate::subscription::get_subscriber_active_cap`] |
//! | `get_subscriber_active_count` | [`crate::subscription::get_subscriber_active_count`] |
//! | `get_subscriber_exposure` | [`crate::subscription::get_subscriber_exposure`] |
//! | `set_global_cap_default` | [`crate::subscription::do_set_global_cap_default`] |
//! | `get_global_cap_default` | [`crate::subscription::get_global_cap_default`] |
//! | `set_merchant_cap_default` | [`crate::subscription::do_set_merchant_cap_default`] |
//! | `get_merchant_cap_default` | [`crate::subscription::get_merchant_cap_default`] |
//! | `update_subscription_cap` | [`crate::subscription::do_update_subscription_cap`] |
//!
//! ## Queries
//! | Entrypoint | Delegate |
//! |---|---|
//! | `get_subscription` | [`crate::queries::get_subscription`] |
//! | `estimate_topup_for_intervals` | [`crate::queries::estimate_topup_for_intervals`] |
//! | `get_next_charge_info` | [`crate::queries::get_next_charge_info`] |
//! | `list_subscriptions_by_subscriber` | [`crate::queries::list_subscriptions_by_subscriber`] |
//! | `get_cap_info` | [`crate::queries::get_cap_info`] |
//! | `get_token_subscription_count` | [`crate::queries::get_token_subscription_count`] |
//! | `get_subscriptions_by_token` | [`crate::queries::get_subscriptions_by_token`] |
//!
//! ## Metadata
//! | Entrypoint | Delegate |
//! |---|---|
//! | `set_metadata` | [`crate::metadata::set_metadata`] |
//! | `set_metadata_signed` | [`crate::metadata::do_set_metadata_signed`] |
//! | `get_metadata_signed_nonce` | [`crate::nonce::get_nonce`] |
//! | `delete_metadata` | [`crate::metadata::delete_metadata`] |
//! | `get_metadata` | [`crate::metadata::get_metadata`] |
//! | `list_metadata_keys` | [`crate::metadata::list_metadata_keys`] |
//!
//! ## Open Disputes (subscriber-initiated) & Escrow
//! | Entrypoint | Delegate |
//! |---|---|
//! | `open_dispute` | [`crate::dispute::do_open_dispute`] |
//! | `get_dispute` | [`crate::dispute::do_get_dispute`] |
//! | `get_subscription_dispute` | [`crate::dispute::do_get_subscription_dispute`] |
//! | `claim_cancellation_escrow` | [`crate::dispute::do_claim_cancellation_escrow`] |
//! | `get_cancellation_escrow` | [`crate::dispute::do_get_cancellation_escrow`] |

// Re-export delegate functions so IDE navigation and `cargo doc` surface them
// under this feature group. No new ABI symbols are introduced; all public
// contract entrypoints remain in `lib.rs` under `#[contractimpl]`.

//! # Subscription State Machine
//!
//! The canonical `SubscriptionStatus` transition matrix is defined in
//! `docs/subscription_state_machine.md`. `transition_to` is the only allowed
//! status mutator and rejects invalid transitions with
//! `Error::InvalidStatusTransition`, keeping `Cancelled` terminal.

pub use crate::state_machine::{
    can_transition, transition_to, validate_status_transition,
};

pub use crate::charge_core::{charge_one, charge_usage_one};
pub use crate::coupon::{apply_coupon, create_coupon, get_coupon, revoke_coupon};
pub use crate::dispute::{
    do_claim_cancellation_escrow, do_get_cancellation_escrow, do_get_dispute,
    do_get_subscription_dispute, do_open_dispute,
};
pub use crate::merchant::{do_deprecate_plan, do_register_plan};
pub use crate::metadata::{
    delete_metadata, do_set_metadata_signed, get_metadata, list_metadata_keys, set_metadata,
};
pub use crate::nonce::get_nonce;
pub use crate::queries::{
    estimate_topup_for_intervals, get_cap_info, get_next_charge_info, get_subscription,
    get_subscriptions_by_token, get_token_subscription_count, list_subscriptions_by_subscriber,
    get_plan_max_active_subs,
};
pub use crate::subscription::{
    do_accept_transfer, do_bulk_cancel_subscriptions, do_bulk_deposit_funds,
    do_bulk_pause_subscriptions, do_cancel_subscription, do_charge_one_off,
    do_cleanup_subscription, do_configure_usage_limits, do_create_plan_template,
    do_create_plan_template_with_token, do_create_subscription, do_create_subscription_from_plan,
    do_create_subscription_with_token, do_deposit_funds, do_deposit_funds_on_behalf,
    do_finalize_emergency_withdraw, do_grace_buyout, do_grant_delegated_payer,
    do_initiate_transfer, do_migrate_subscription_to_plan, do_partial_refund,
    do_pause_subscription, do_request_emergency_withdraw, do_resume_subscription,
    do_revoke_delegated_payer, do_schedule_cancel, do_set_auto_renew,
    do_set_global_cap_default, do_set_merchant_cap_default, do_set_merchant_max_subs,
    do_set_plan_max_active_subs, do_set_subscriber_active_cap, do_set_subscriber_credit_limit,
    do_set_sub_exp_ledger, do_unschedule_cancel, do_update_plan_template,
    do_update_subscription_cap, do_veto_transfer, do_withdraw_subscriber_funds,
    get_global_cap_default, get_merchant_cap_default, get_plan_template,
    get_split_payees, get_subscriber_active_cap, get_subscriber_active_count,
    get_subscriber_credit_limit, get_subscriber_exposure, write_split_payees,
};
