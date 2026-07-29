//! Merchant payout and accumulated USDC tracking entrypoints.
//!
//! # Reentrancy Protection
//!
//! This module contains critical external calls for fund transfers:
//! - `withdraw_merchant_funds`: transfers USDC to merchant via `token.transfer()`
//! - `withdraw_merchant_funds_for_token`: transfers custom tokens to merchant
//! - `merchant_refund`: transfers tokens from merchant to subscriber
//!
//! All functions follow the **Checks-Effects-Interactions (CEI)** pattern:
//!
//! 1. **Checks**: Validate merchant authorization and sufficient balance
//! 2. **Effects**: Update internal state (merchant balance, earnings) in storage
//! 3. **Interactions**: Call token.transfer() AFTER state is consistent and persisted
//!
//! **Guard layer**: Public entry-points in `lib.rs` acquire a `ReentrancyGuard` before
//! calling these internal helpers, providing defense-in-depth protection against
//! potential callbacks during token transfers.
//!
//! See `docs/reentrancy.md` and `docs/reentrancy_hardening.md` for full details on
//! the reentrancy threat model and mitigation strategy.

use crate::safe_math::{safe_add, safe_sub};
use crate::types::{
    AccruedTotals, BillingChargeKind, DataKey, Error, MerchantApprovedEvent,
    MerchantBalanceSnapshotEvent, MerchantConfig, MerchantConfigInitializedEvent,
    MerchantConfigUpdatedEvent, MerchantFeeOverrideSetEvent, MerchantMultiSigConfig,
    MerchantPausedEvent, MerchantRevokedEvent, MerchantUnpausedEvent, MerchantWhitelistModeEvent,
    MerchantWithdrawalEvent, PayoutSchedule, PlanDeprecatedEvent, PlanRegisteredEvent,
    PlanTemplate, ScheduledPayoutEvent, TokenEarnings, TokenReconciliationSnapshot, MAX_FEE_BIPS,
    is_valid_allowed_operations, OP_CHARGE,
};
use soroban_sdk::{token, Address, Env, String, Symbol, Vec};

pub fn get_merchant_paused(env: &Env, merchant: Address) -> bool {
    // Check both legacy Pause state and new Config state if they overlap
    if let Some(config) = get_merchant_config(env, merchant.clone()) {
        if config.is_paused {
            return true;
        }
    }
    let key = DataKey::MerchantPaused(merchant);
    env.storage().instance().get(&key).unwrap_or(false)
}

pub fn set_merchant_paused(env: &Env, merchant: Address, paused: bool) {
    let key = DataKey::MerchantPaused(merchant);
    env.storage().instance().set(&key, &paused);
}

pub fn pause_merchant(env: &Env, merchant: Address) -> Result<(), Error> {
    merchant.require_auth();

    if get_merchant_paused(env, merchant.clone()) {
        return Ok(());
    }

    set_merchant_paused(env, merchant.clone(), true);

    env.events().publish(
        (Symbol::new(env, "merchant_paused"), merchant.clone()),
        MerchantPausedEvent {
            merchant,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

pub fn unpause_merchant(env: &Env, merchant: Address) -> Result<(), Error> {
    merchant.require_auth();

    if !get_merchant_paused(env, merchant.clone()) {
        return Ok(());
    }

    set_merchant_paused(env, merchant.clone(), false);

    env.events().publish(
        (Symbol::new(env, "merchant_unpaused"), merchant.clone()),
        MerchantUnpausedEvent {
            merchant,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

// ── Per-merchant fee override ─────────────────────────────────────────────────
//
// Allows the admin to grant selected merchants a discounted protocol fee.
// The override is stored in instance storage under `DataKey::MerchantFeeBps(merchant)`.
// During charge routing, this value supersedes the global `FeeBps` when present.
//
// Security invariants:
// 1. Admin-only: only the stored admin may set or clear an override.
// 2. Bounded: the override must be ≤ `MAX_FEE_BIPS` (10 000).
// 3. Cannot exceed global: the override must be ≤ the current global fee_bps.
//    (A merchant discount is always ≤ the standard rate.)
// 4. Clearing always succeeds regardless of the current global fee.

/// Return the per-merchant fee override in basis points, or `None` if no
/// override has been set for this merchant.
pub fn get_merchant_fee_override_bps(env: &Env, merchant: &Address) -> Option<u32> {
    let key = DataKey::MerchantFeeBps(merchant.clone());
    env.storage().instance().get(&key)
}

/// Set a per-merchant fee override in basis points. Admin only.
///
/// The override must satisfy:
/// - `fee_bps <= MAX_FEE_BIPS` (10 000) — hard upper bound.
/// - `fee_bps <= global_fee_bps` — a "discount" cannot be higher than the
///   standard rate; if it were, it would act as a surcharge, which is not
///   the intended semantics.
///
/// Emits [`MerchantFeeOverrideSetEvent`].
///
/// # Errors
/// - [`Error::Unauthorized`] if `admin` is not the stored admin.
/// - [`Error::InvalidFeeBips`] if `fee_bps > MAX_FEE_BIPS`.
/// - [`Error::InvalidFeeBips`] if `fee_bps > global_fee_bps`.
pub fn set_merchant_fee_override(
    env: &Env,
    admin: Address,
    merchant: Address,
    fee_bps: u32,
) -> Result<(), Error> {
    crate::admin::require_admin_auth(env, &admin)?;

    if fee_bps > crate::types::MAX_FEE_BIPS as u32 {
        return Err(Error::InvalidFeeBips);
    }

    let global_fee_bps = crate::admin::get_protocol_fee_bps(env);
    if fee_bps > global_fee_bps {
        return Err(Error::InvalidFeeBips);
    }

    let key = DataKey::MerchantFeeBps(merchant.clone());
    env.storage().instance().set(&key, &fee_bps);

    env.events().publish(
        (
            Symbol::new(env, "merchant_fee_override_set"),
            merchant.clone(),
        ),
        MerchantFeeOverrideSetEvent {
            merchant,
            admin,
            fee_bps: Some(fee_bps),
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Clear the per-merchant fee override, reverting to the global fee. Admin only.
///
/// Always succeeds (idempotent): clearing a non-existent override is a no-op.
/// Emits [`MerchantFeeOverrideSetEvent`] with `fee_bps = None`.
///
/// # Errors
/// - [`Error::Unauthorized`] if `admin` is not the stored admin.
pub fn clear_merchant_fee_override(
    env: &Env,
    admin: Address,
    merchant: Address,
) -> Result<(), Error> {
    crate::admin::require_admin_auth(env, &admin)?;

    let key = DataKey::MerchantFeeBps(merchant.clone());
    env.storage().instance().remove(&key);

    env.events().publish(
        (
            Symbol::new(env, "merchant_fee_override_set"),
            merchant.clone(),
        ),
        MerchantFeeOverrideSetEvent {
            merchant,
            admin,
            fee_bps: None,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

// ── Whitelist mode ───────────────────────────────────────────────────────────

/// Returns `true` if the global whitelist mode is enabled.
pub fn get_whitelist_mode(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::MerchantWhitelistMode)
        .unwrap_or(false)
}

/// Enable or disable the global merchant whitelist mode. Admin-only.
pub fn set_whitelist_mode(env: &Env, admin: Address, enabled: bool) -> Result<(), Error> {
    crate::admin::require_admin_auth(env, &admin)?;

    let key = DataKey::MerchantWhitelistMode;
    env.storage().instance().set(&key, &enabled);

    env.events().publish(
        (Symbol::new(env, "merchant_whitelist_toggled"),),
        MerchantWhitelistModeEvent {
            enabled,
            admin,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Returns `true` if `merchant` has been approved under whitelist mode.
pub fn is_merchant_approved(env: &Env, merchant: &Address) -> bool {
    let key = DataKey::MerchantApproved(merchant.clone());
    env.storage().instance().get(&key).unwrap_or(false)
}

/// Approve a merchant under whitelist mode. Admin-only.
///
/// When whitelist mode is enabled, `initialize_merchant_config` will reject
/// merchants that have not been approved. Existing approvals are preserved
/// when whitelist mode is toggled on.
pub fn approve_merchant(env: &Env, admin: Address, merchant: Address) -> Result<(), Error> {
    crate::admin::require_admin_auth(env, &admin)?;

    let key = DataKey::MerchantApproved(merchant.clone());
    env.storage().instance().set(&key, &true);

    env.events().publish(
        (Symbol::new(env, "merchant_approved"), merchant.clone()),
        MerchantApprovedEvent {
            merchant,
            admin,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Revoke a merchant's approval under whitelist mode. Admin-only.
pub fn revoke_merchant(env: &Env, admin: Address, merchant: Address) -> Result<(), Error> {
    crate::admin::require_admin_auth(env, &admin)?;

    let key = DataKey::MerchantApproved(merchant.clone());
    env.storage().instance().set(&key, &false);

    env.events().publish(
        (Symbol::new(env, "merchant_revoked"), merchant.clone()),
        MerchantRevokedEvent {
            merchant,
            admin,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Gate check: returns `Ok(())` if whitelist is disabled or merchant is approved.
fn require_whitelist_approval(env: &Env, merchant: &Address) -> Result<(), Error> {
    if get_whitelist_mode(env) && !is_merchant_approved(env, merchant) {
        return Err(Error::MerchantNotApproved);
    }
    Ok(())
}

// ── Merchant compliance-category tags (#564) ────────────────────────────────
//
// A small, admin-controlled allowlist (`DataKey::TagAllowlist`) defines the
// set of tag symbols ("saas", "media", "nonprofit", ...) that may be assigned
// to a merchant. Each merchant carries at most `MAX_MERCHANT_TAGS` tags
// (`DataKey::MerchantTags(merchant)`), letting downstream indexers group
// merchant activity by compliance category and letting admins efficiently
// identify — and, via the existing blocklist/pause machinery, act on —
// whole classes of merchants without maintaining an out-of-band mapping.
//
// Tags are purely descriptive metadata: setting or clearing them never
// touches merchant balances, pause state, or approval status, and is
// intentionally unrestricted by those states (an admin must be able to tag,
// or clear the tags of, an already-paused/blocked merchant for reporting).

/// Return the current admin-controlled tag allowlist, or an empty list if
/// none has been configured yet.
pub fn get_tag_allowlist(env: &Env) -> Vec<Symbol> {
    env.storage()
        .instance()
        .get(&DataKey::TagAllowlist)
        .unwrap_or(Vec::new(env))
}

/// Replace the global tag allowlist. Admin-only.
///
/// The allowlist itself is not bounded by `MAX_MERCHANT_TAGS` — that limit
/// applies per-merchant, not to the catalog of valid tags — but it must be
/// duplicate-free so allowlist membership checks stay unambiguous.
///
/// Tags already assigned to a merchant are **not** retroactively revalidated
/// against a shrunk allowlist: removing a tag from the allowlist only blocks
/// *future* `set_merchant_tags` calls from reusing it, mirroring how
/// `revoke_merchant` doesn't retroactively unwind past state elsewhere in
/// this module. An admin who needs to fully retire a tag can follow up with
/// `set_merchant_tags` calls to clear it from affected merchants.
///
/// # Errors
/// * [`Error::Unauthorized`] — caller is not the stored admin.
/// * [`Error::DuplicateMerchantTag`] — `tags` contains the same symbol twice.
pub fn set_tag_allowlist(env: &Env, admin: Address, tags: Vec<Symbol>) -> Result<(), Error> {
    crate::admin::require_admin_auth(env, &admin)?;

    let mut seen = Vec::new(env);
    for tag in tags.iter() {
        if seen.contains(&tag) {
            return Err(Error::DuplicateMerchantTag);
        }
        seen.push_back(tag);
    }

    env.storage().instance().set(&DataKey::TagAllowlist, &tags);

    env.events().publish(
        (Symbol::new(env, "tag_allowlist_updated"),),
        crate::types::TagAllowlistUpdatedEvent {
            admin,
            tags,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Return the compliance-category tags currently assigned to `merchant`, or
/// an empty list if none have been set.
pub fn get_merchant_tags(env: &Env, merchant: Address) -> Vec<Symbol> {
    env.storage()
        .instance()
        .get(&DataKey::MerchantTags(merchant))
        .unwrap_or(Vec::new(env))
}

/// Set (fully replacing) `merchant`'s compliance-category tags. Admin-only.
///
/// Pass an empty `tags` vector to clear all tags — this is always allowed,
/// including for a merchant that is currently paused or blocklisted, since
/// clearing compliance metadata must never be blocked by the very
/// enforcement state that metadata may have informed.
///
/// # Errors
/// * [`Error::Unauthorized`] — caller is not the stored admin.
/// * [`Error::MerchantTagLimitExceeded`] — `tags.len() > MAX_MERCHANT_TAGS`.
/// * [`Error::DuplicateMerchantTag`] — `tags` contains the same symbol twice.
/// * [`Error::UnknownMerchantTag`] — a tag is not present in the current
///   allowlist (`get_tag_allowlist`).
pub fn set_merchant_tags(
    env: &Env,
    admin: Address,
    merchant: Address,
    tags: Vec<Symbol>,
) -> Result<(), Error> {
    crate::admin::require_admin_auth(env, &admin)?;

    if tags.len() > crate::types::MAX_MERCHANT_TAGS {
        return Err(Error::MerchantTagLimitExceeded);
    }

    let allowlist = get_tag_allowlist(env);
    let mut seen = Vec::new(env);
    for tag in tags.iter() {
        if seen.contains(&tag) {
            return Err(Error::DuplicateMerchantTag);
        }
        if !allowlist.contains(&tag) {
            return Err(Error::UnknownMerchantTag);
        }
        seen.push_back(tag);
    }

    env.storage()
        .instance()
        .set(&DataKey::MerchantTags(merchant.clone()), &tags);

    env.events().publish(
        (Symbol::new(env, "merchant_tags_updated"), merchant.clone()),
        crate::types::MerchantTagsUpdatedEvent {
            merchant,
            admin,
            tags,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

fn validate_merchant_config_input(
    _payout_address: &Address,
    fee_bips: i32,
    allowed_operations: i32,
) -> Result<(), Error> {
    if fee_bips > MAX_FEE_BIPS {
        return Err(Error::InvalidFeeBips);
    }
    if !is_valid_allowed_operations(allowed_operations) {
        return Err(Error::InvalidOperations);
    }
    if allowed_operations & OP_CHARGE == 0 {
        return Err(Error::MustAllowChargeOperation);
    }
    Ok(())
}

pub fn initialize_merchant_config(
    env: &Env,
    merchant: Address,
    payout_address: Address,
    fee_bips: i32,
    allowed_operations: i32,
    fee_address: Option<Address>,
    redirect_url: String,
) -> Result<MerchantConfig, Error> {
    merchant.require_auth();
    require_whitelist_approval(env, &merchant)?;
    validate_merchant_config_input(&payout_address, fee_bips, allowed_operations)?;

    let config = MerchantConfig {
        version: 1,
        payout_address,
        fee_bips,
        allowed_operations,
        is_active: true,
        fee_address,
        redirect_url,
        is_paused: false,
        last_updated: env.ledger().timestamp(),
    };

    let key = DataKey::MerchantConfig(merchant.clone());
    env.storage().instance().set(&key, &config);

    env.events().publish(
        (Symbol::new(env, "merchant_config_initialized"),),
        MerchantConfigInitializedEvent {
            merchant: merchant.clone(),
            payout_address: config.payout_address.clone(),
            fee_bips: config.fee_bips,
            allowed_operations: config.allowed_operations,
            timestamp: config.last_updated,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(config)
}

pub fn set_merchant_config(
    env: &Env,
    merchant: Address,
    config: MerchantConfig,
) -> Result<(), Error> {
    merchant.require_auth();
    validate_merchant_config_input(
        &config.payout_address,
        config.fee_bips,
        config.allowed_operations,
    )?;

    let key = DataKey::MerchantConfig(merchant.clone());
    let timestamp = env.ledger().timestamp();
    let mut updated_config = config;
    updated_config.last_updated = timestamp;
    env.storage().instance().set(&key, &updated_config);

    env.events().publish(
        (Symbol::new(env, "merchant_config_set"),),
        MerchantConfigUpdatedEvent {
            merchant: merchant.clone(),
            payout_address: updated_config.payout_address.clone(),
            fee_bips: updated_config.fee_bips,
            allowed_operations: updated_config.allowed_operations,
            timestamp,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

pub fn get_merchant_config(env: &Env, merchant: Address) -> Option<MerchantConfig> {
    let key = DataKey::MerchantConfig(merchant);
    env.storage().instance().get(&key)
}

pub fn get_merchant_multisig_config(env: &Env, merchant: Address) -> Option<MerchantMultiSigConfig> {
    let key = DataKey::MerchantMultiSig(merchant);
    env.storage().instance().get(&key)
}

fn merchant_balance_key(merchant: &Address, token: &Address) -> DataKey {
    DataKey::MerchantBalance(merchant.clone(), token.clone())
}

pub fn get_merchant_token_earnings(
    env: &Env,
    merchant: &Address,
    token: &Address,
) -> TokenEarnings {
    let key = DataKey::MerchantEarnings(merchant.clone(), token.clone());
    env.storage().instance().get(&key).unwrap_or(TokenEarnings {
        accruals: AccruedTotals {
            interval: 0,
            usage: 0,
            one_off: 0,
        },
        withdrawals: 0,
        refunds: 0,
    })
}

fn set_merchant_token_earnings(
    env: &Env,
    merchant: &Address,
    token: &Address,
    earnings: &TokenEarnings,
) {
    let key = DataKey::MerchantEarnings(merchant.clone(), token.clone());
    env.storage().instance().set(&key, earnings);
}

fn add_merchant_token(env: &Env, merchant: &Address, token: &Address) {
    let key = DataKey::MerchantTokens(merchant.clone());
    let mut tokens: Vec<Address> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));
    if !tokens.contains(token) {
        tokens.push_back(token.clone());
        env.storage().instance().set(&key, &tokens);
    }
}

pub fn get_merchant_total_earnings(env: &Env, merchant: &Address) -> Vec<(Address, TokenEarnings)> {
    let key = DataKey::MerchantTokens(merchant.clone());
    let tokens: Vec<Address> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));
    let mut result = Vec::new(env);
    for token in tokens.iter() {
        let earnings = get_merchant_token_earnings(env, merchant, &token);
        result.push_back((token, earnings));
    }
    result
}

pub fn get_reconciliation_snapshot(
    env: &Env,
    merchant: &Address,
) -> Vec<TokenReconciliationSnapshot> {
    let key = DataKey::MerchantTokens(merchant.clone());
    let tokens: Vec<Address> = env.storage().instance().get(&key).unwrap_or(Vec::new(env));
    let mut result = Vec::new(env);

    for token in tokens.iter() {
        let earnings = get_merchant_token_earnings(env, merchant, &token);
        let total_accruals = earnings
            .accruals
            .interval
            .checked_add(earnings.accruals.usage)
            .unwrap_or(0)
            .checked_add(earnings.accruals.one_off)
            .unwrap_or(0);

        let computed_balance = total_accruals
            .checked_sub(earnings.withdrawals)
            .unwrap_or(0)
            .checked_sub(earnings.refunds)
            .unwrap_or(0);

        result.push_back(TokenReconciliationSnapshot {
            token: token.clone(),
            total_accruals,
            total_withdrawals: earnings.withdrawals,
            total_refunds: earnings.refunds,
            computed_balance,
            stored_balance: 0,              // Will be computed by caller
            matches: computed_balance == 0, // Placeholder
        });
    }
    result
}

pub fn get_merchant_balance(env: &Env, merchant: &Address) -> i128 {
    if let Ok(token_addr) = crate::admin::get_token(env) {
        return get_merchant_balance_by_token(env, merchant, &token_addr);
    }
    0
}

pub fn get_merchant_balance_by_token(env: &Env, merchant: &Address, token: &Address) -> i128 {
    let key = merchant_balance_key(merchant, token);
    env.storage().instance().get(&key).unwrap_or(0i128)
}

pub fn set_merchant_balance(env: &Env, merchant: &Address, token: &Address, balance: &i128) {
    let key = merchant_balance_key(merchant, token);
    env.storage().instance().set(&key, balance);
}

/// Credit merchant balance (used when subscription charges process).
#[allow(dead_code)]
pub fn credit_merchant_balance(
    env: &Env,
    merchant: &Address,
    amount: i128,
    kind: BillingChargeKind,
) -> Result<(), Error> {
    let token_addr = crate::admin::get_token(env)?;
    credit_merchant_balance_for_token(env, merchant, &token_addr, amount, kind)
}

pub fn credit_merchant_balance_for_token(
    env: &Env,
    merchant: &Address,
    token_addr: &Address,
    amount: i128,
    kind: BillingChargeKind,
) -> Result<(), Error> {
    if amount < 0 {
        return Err(Error::InvalidAmount);
    }

    // ── EFFECTS: balance update
    let current = get_merchant_balance_by_token(env, merchant, token_addr);
    let new_balance = safe_add(current, amount)?;
    set_merchant_balance(env, merchant, token_addr, &new_balance);

    // ── EFFECTS: earnings update (SINGLE SOURCE OF TRUTH)
    let mut earnings = get_merchant_token_earnings(env, merchant, token_addr);

    match kind {
        BillingChargeKind::Interval => {
            earnings.accruals.interval = earnings
                .accruals
                .interval
                .checked_add(amount)
                .ok_or(Error::Overflow)?;
        }
        BillingChargeKind::Usage => {
            earnings.accruals.usage = earnings
                .accruals
                .usage
                .checked_add(amount)
                .ok_or(Error::Overflow)?;
        }
        BillingChargeKind::OneOff => {
            earnings.accruals.one_off = earnings
                .accruals
                .one_off
                .checked_add(amount)
                .ok_or(Error::Overflow)?;
        }
    }

    set_merchant_token_earnings(env, merchant, token_addr, &earnings);

    // ── EFFECTS: register token
    add_merchant_token(env, merchant, token_addr);

    // ── ACCOUNTING (invariant anchor)
    crate::accounting::add_total_accounted(env, token_addr, amount)?;

    // ── INVARIANT (test/CI only)
    #[cfg(any(test, feature = "invariants"))]
    crate::invariants::assert_token_balance_invariant(env, token_addr)?;

    Ok(())
}
pub fn set_merchant_multisig(
    env: &Env,
    admin: Address,
    merchant: Address,
    signers: Vec<Address>,
    threshold: u32,
) -> Result<(), Error> {
    crate::admin::require_admin_auth(env, &admin)?;
    merchant.require_auth();

    if threshold == 0 {
        return Err(Error::InvalidInput);
    }

    if threshold > signers.len() {
        return Err(Error::InvalidInput);
    }

    let mut seen = Vec::new(env);
    for signer in signers.iter() {
        if seen.contains(&signer) {
            return Err(Error::InvalidInput);
        }
        seen.push_back(signer.clone());
    }

    let key = DataKey::MerchantMultiSig(merchant.clone());
    env.storage().instance().set(
        &key,
        &MerchantMultiSigConfig {
            signers: signers.clone(),
            threshold,
        },
    );

    Ok(())
}

pub fn withdraw_merchant_funds(env: &Env, merchant: Address, amount: i128) -> Result<(), Error> {
    let token_addr = crate::admin::get_token(env)?;
    withdraw_merchant_funds_for_token(env, merchant, token_addr, amount)
}

pub fn withdraw_merchant_funds_for_token(
    env: &Env,
    merchant: Address,
    token_addr: Address,
    amount: i128,
) -> Result<(), Error> {
    merchant.require_auth();

    if let Some(config) = get_merchant_multisig_config(env, merchant.clone()) {
        let required_signers = config.threshold.min(config.signers.len() as u32);
        let mut iter = 0u32;
        while iter < required_signers {
            if let Some(signer) = config.signers.get(iter) {
                let signer: Address = signer;
                signer.require_auth();
            }
            iter += 1;
        }
    }

    crate::blocklist::require_not_blocklisted(env, &merchant)?;

    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    if !crate::admin::is_token_accepted(env, &token_addr) {
        return Err(Error::InvalidInput);
    }

    // Ensure merchant config exists
    let _config = get_merchant_config(env, merchant.clone()).ok_or(Error::NotFound)?;

    let current = get_merchant_balance_by_token(env, &merchant, &token_addr);

    if current == 0 {
        return Err(Error::NotFound);
    }

    if amount > current {
        return Err(Error::InsufficientBalance);
    }

    // Vault balance check
    let token_client = token::Client::new(env, &token_addr);
    let contract = env.current_contract_address();

    if token_client.balance(&contract) < amount {
        return Err(Error::InsufficientBalance);
    }

    let new_balance = safe_sub(current, amount)?;

    // ─────────────── EFFECTS ───────────────
    set_merchant_balance(env, &merchant, &token_addr, &new_balance);

    let mut earnings = get_merchant_token_earnings(env, &merchant, &token_addr);
    earnings.refunds = earnings
        .refunds
        .checked_add(amount)
        .ok_or(Error::Overflow)?;
    set_merchant_token_earnings(env, &merchant, &token_addr, &earnings);
    crate::accounting::sub_total_accounted(env, &token_addr, amount)?;
    let mut earnings = get_merchant_token_earnings(env, &merchant, &token_addr);
    earnings.withdrawals = earnings
        .withdrawals
        .checked_add(amount)
        .ok_or(Error::Overflow)?;
    set_merchant_token_earnings(env, &merchant, &token_addr, &earnings);

    env.events().publish(
        (
            Symbol::new(env, "withdrawn"),
            merchant.clone(),
            token_addr.clone(),
        ),
        MerchantWithdrawalEvent {
            merchant: merchant.clone(),
            token: token_addr.clone(),
            amount,
            remaining_balance: new_balance,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );
    env.events().publish(
        (
            Symbol::new(env, "merchant_balance_snapshot"),
            merchant.clone(),
            token_addr.clone(),
        ),
        MerchantBalanceSnapshotEvent {
            merchant: merchant.clone(),
            token: token_addr.clone(),
            balance: new_balance,
            accrued: 0,
            withdrawn: 0,
            refunded: 0,
            ledger_sequence: env.ledger().sequence(),
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    // ─────────────── INTERACTION ───────────────
    token_client.transfer(&contract, &merchant, &amount);

    // ─────────────── INVARIANT ───────────────
    #[cfg(any(test, feature = "invariants"))]
    crate::invariants::assert_token_balance_invariant(env, &token_addr)?;

    Ok(())
}

pub fn merchant_refund(
    env: &Env,
    merchant: Address,
    subscriber: Address,
    token_addr: Address,
    amount: i128,
) -> Result<(), Error> {
    merchant.require_auth();
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    // Verify merchant config is initialized
    let _config = get_merchant_config(env, merchant.clone()).ok_or(Error::NotFound)?;

    let current = get_merchant_balance_by_token(env, &merchant, &token_addr);
    if current == 0 {
        return Err(Error::NotFound);
    }
    if amount > current {
        return Err(Error::InsufficientBalance);
    }

    let new_balance = current.checked_sub(amount).ok_or(Error::Underflow)?;

    // EFFECTS
    set_merchant_balance(env, &merchant, &token_addr, &new_balance);
    crate::accounting::sub_total_accounted(env, &token_addr, amount)?;

    let mut earnings = get_merchant_token_earnings(env, &merchant, &token_addr);
    earnings.refunds = earnings
        .refunds
        .checked_add(amount)
        .ok_or(Error::Overflow)?;
    set_merchant_token_earnings(env, &merchant, &token_addr, &earnings);

    env.events().publish(
        (Symbol::new(env, "merchant_refund"), merchant.clone()),
        crate::types::MerchantRefundEvent {
            merchant,
            subscriber: subscriber.clone(),
            token: token_addr.clone(),
            amount,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    // INTERACTIONS
    let token_client = token::Client::new(env, &token_addr);
    token_client.transfer(&env.current_contract_address(), &subscriber, &amount);
    #[cfg(any(test, feature = "invariants"))]
    crate::invariants::assert_token_balance_invariant(env, &token_addr)?;
    Ok(())
}

pub fn get_payout_schedule(env: &Env, merchant: &Address) -> PayoutSchedule {
    let key = DataKey::PayoutSchedule(merchant.clone());
    env.storage()
        .instance()
        .get(&key)
        .unwrap_or(PayoutSchedule {
            cadence_seconds: 0,
            min_payout: 0,
            last_payout_at: 0,
        })
}

fn set_payout_schedule(env: &Env, merchant: &Address, schedule: &PayoutSchedule) {
    let key = DataKey::PayoutSchedule(merchant.clone());
    env.storage().instance().set(&key, schedule);
}

/// Set or clear the payout schedule for a merchant.
///
/// When `cadence_seconds` is 0 and `min_payout` is 0 the schedule is cleared
/// (equivalent to "no auto-payout").  The merchant must authorize this call.
///
/// Returns the previous schedule so callers can diff the change off-chain.
pub fn do_set_payout_schedule(
    env: &Env,
    merchant: Address,
    cadence_seconds: u64,
    min_payout: i128,
) -> Result<PayoutSchedule, Error> {
    merchant.require_auth();

    if min_payout < 0 {
        return Err(Error::InvalidAmount);
    }

    let previous = get_payout_schedule(env, &merchant);
    let now = env.ledger().timestamp();

    let schedule = PayoutSchedule {
        cadence_seconds,
        min_payout,
        last_payout_at: if previous.last_payout_at == 0 {
            0
        } else {
            previous.last_payout_at
        },
    };

    set_payout_schedule(env, &merchant, &schedule);

    env.events().publish(
        (Symbol::new(env, "payout_schedule_set"), merchant.clone()),
        (cadence_seconds, min_payout, now),
    );

    Ok(previous)
}

/// Execute a single per-token payout for a merchant during a flush.
///
/// Reads the merchant's balance for `token`.  If the balance is below
/// `min_payout` the function returns 0 (no-op).  Otherwise it transfers
/// the entire balance to the merchant's payout address, updates internal
/// accounting, and returns the amount transferred.
///
/// # CEI
///
/// Effects (balance update, earnings update) are written *before* the
/// external token transfer.
fn flush_merchant_token(
    env: &Env,
    merchant: &Address,
    token: &Address,
    min_payout: i128,
) -> Result<i128, Error> {
    let balance = get_merchant_balance_by_token(env, merchant, token);
    if balance < min_payout || balance <= 0 {
        return Ok(0i128);
    }

    let config = get_merchant_config(env, merchant.clone()).ok_or(Error::NotFound)?;
    let payout_address = config.payout_address;

    // EFFECTS — update state before external call
    set_merchant_balance(env, merchant, token, &0i128);

    let mut earnings = get_merchant_token_earnings(env, merchant, token);
    earnings.withdrawals = earnings
        .withdrawals
        .checked_add(balance)
        .ok_or(Error::Overflow)?;
    set_merchant_token_earnings(env, merchant, token, &earnings);
    crate::accounting::sub_total_accounted(env, token, balance)?;

    env.events().publish(
        (
            Symbol::new(env, "withdrawn"),
            merchant.clone(),
            token.clone(),
        ),
        MerchantWithdrawalEvent {
            merchant: merchant.clone(),
            token: token.clone(),
            amount: balance,
            remaining_balance: 0,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    // INTERACTIONS
    let token_client = token::Client::new(env, token);
    token_client.transfer(&env.current_contract_address(), &payout_address, &balance);
    #[cfg(any(test, feature = "invariants"))]
    crate::invariants::assert_token_balance_invariant(env, token)?;
    Ok(balance)
}

/// Process all scheduled payouts for a merchant.
///
/// Iterates every token the merchant has earnings in.  For each token that
/// meets the configured `min_payout` threshold, the full balance is
/// transferred to the merchant's payout address.
///
/// Anyone may call this function.  The cadence check is enforced here:
/// if the configured `cadence_seconds` has not elapsed since the last flush,
/// the call is a no-op (`Err(Error::IntervalNotElapsed)`).
///
/// Returns the number of token payouts actually executed.
pub fn do_flush_payouts(env: &Env, merchant: Address, caller: Address) -> Result<u32, Error> {
    let schedule = get_payout_schedule(env, &merchant);

    // No schedule configured — nothing to do.
    if schedule.cadence_seconds == 0 && schedule.min_payout == 0 {
        return Ok(0);
    }

    let now = env.ledger().timestamp();

    // Enforce cadence: enough time since last flush?
    if schedule.last_payout_at > 0
        && now.saturating_sub(schedule.last_payout_at) < schedule.cadence_seconds
    {
        return Err(Error::IntervalNotElapsed);
    }

    // Iterate all tokens the merchant has earnings in.
    let token_key = DataKey::MerchantTokens(merchant.clone());
    let tokens: Vec<Address> = env
        .storage()
        .instance()
        .get(&token_key)
        .unwrap_or(Vec::new(env));

    let mut tokens_paid: u32 = 0;
    for token in tokens.iter() {
        let amount = flush_merchant_token(env, &merchant, &token, schedule.min_payout)?;
        if amount > 0 {
            tokens_paid = tokens_paid.saturating_add(1);
        }
    }

    // Update last_payout_at
    let mut updated_schedule = schedule;
    updated_schedule.last_payout_at = now;
    set_payout_schedule(env, &merchant, &updated_schedule);

    env.events().publish(
        (Symbol::new(env, "scheduled_payout"), merchant.clone()),
        ScheduledPayoutEvent {
            merchant,
            caller,
            tokens_paid,
            timestamp: now,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(tokens_paid)
}

pub fn update_merchant_config(
    env: &Env,
    merchant: Address,
    new_payout_address: Option<Address>,
    new_fee_bips: Option<i32>,
    new_allowed_operations: Option<i32>,
    new_is_active: Option<bool>,
    new_fee_address: Option<Option<Address>>,
    new_redirect_url: Option<soroban_sdk::String>,
    new_is_paused: Option<bool>,
) -> Result<MerchantConfig, Error> {
    merchant.require_auth();

    let key = DataKey::MerchantConfig(merchant.clone());
    let mut config: MerchantConfig = env.storage().instance().get(&key).ok_or(Error::NotFound)?;

    if let Some(payout) = new_payout_address {
        config.payout_address = payout;
    }
    if let Some(bips) = new_fee_bips {
        if bips > MAX_FEE_BIPS {
            return Err(Error::InvalidFeeBips);
        }
        config.fee_bips = bips;
    }
    if let Some(ops) = new_allowed_operations {
        if !is_valid_allowed_operations(ops) {
            return Err(Error::InvalidOperations);
        }
        if ops & OP_CHARGE == 0 {
            return Err(Error::MustAllowChargeOperation);
        }
        config.allowed_operations = ops;
    }
    if let Some(active) = new_is_active {
        config.is_active = active;
    }
    if let Some(fee_addr) = new_fee_address {
        config.fee_address = fee_addr;
    }
    if let Some(url) = new_redirect_url {
        config.redirect_url = url;
    }
    if let Some(paused) = new_is_paused {
        config.is_paused = paused;
    }

    config.last_updated = env.ledger().timestamp();
    env.storage().instance().set(&key, &config);

    env.events().publish(
        (soroban_sdk::Symbol::new(env, "merchant_config_updated"),),
        MerchantConfigUpdatedEvent {
            merchant: merchant.clone(),
            payout_address: config.payout_address.clone(),
            fee_bips: config.fee_bips,
            allowed_operations: config.allowed_operations,
            timestamp: config.last_updated,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(config)
}

/// Migrate every per-merchant storage key from `old_merchant` to `new_merchant`
/// and rewrite every `Subscription.merchant` field that points at the old address.
///
/// # Migrated keys
/// * `MerchantBalance(old, token)` → `MerchantBalance(new, token)` for every token
/// * `MerchantTokens(old)`         → `MerchantTokens(new)`
/// * `MerchantEarnings(old, token)`→ `MerchantEarnings(new, token)` for every token
/// * `MerchantConfig(old)`         → `MerchantConfig(new)`
/// * `MerchantPaused(old)`         → `MerchantPaused(new)`
/// * `MerchantSubs(old)`           → `MerchantSubs(new)` (index list)
/// * Every `Sub(id).merchant` where `merchant == old` is rewritten to `new`
///
/// # Auth
/// Admin-only. The `admin` argument must match the stored admin address.
/// Nonce `nonce` is consumed in domain `DOMAIN_MERCHANT_ROTATION` to prevent replay.
///
/// # Errors
/// * `Unauthorized`       — caller is not the stored admin
/// * `NonceAlreadyUsed`   — nonce was already consumed
/// * `SelfRotation`       — `old == new`
pub fn do_rotate_merchant_address(
    env: &Env,
    admin: Address,
    old_merchant: Address,
    new_merchant: Address,
    nonce: u64,
) -> Result<(), crate::types::Error> {
    crate::admin::require_admin_auth(env, &admin)?;

    crate::nonce::check_and_advance(env, &admin, crate::nonce::DOMAIN_MERCHANT_ROTATION, nonce)?;

    let storage = env.storage().instance();

    // ── 1. Migrate MerchantTokens list ────────────────────────────────────────
    let tokens_key_old = DataKey::MerchantTokens(old_merchant.clone());
    let tokens_key_new = DataKey::MerchantTokens(new_merchant.clone());
    let tokens: soroban_sdk::Vec<Address> = storage
        .get(&tokens_key_old)
        .unwrap_or(soroban_sdk::Vec::new(env));

    // ── 2. Migrate per-token balances and earnings ────────────────────────────
    for token in tokens.iter() {
        // MerchantBalance
        let bal_key_old = DataKey::MerchantBalance(old_merchant.clone(), token.clone());
        let bal: i128 = storage.get(&bal_key_old).unwrap_or(0i128);
        if bal != 0 {
            let bal_key_new = DataKey::MerchantBalance(new_merchant.clone(), token.clone());
            let existing: i128 = storage.get(&bal_key_new).unwrap_or(0i128);
            storage.set(&bal_key_new, &(existing + bal));
        }
        storage.remove(&bal_key_old);

        // MerchantEarnings
        let earn_key_old = DataKey::MerchantEarnings(old_merchant.clone(), token.clone());
        if let Some(earnings) = storage.get::<_, crate::types::TokenEarnings>(&earn_key_old) {
            let earn_key_new = DataKey::MerchantEarnings(new_merchant.clone(), token.clone());
            storage.set(&earn_key_new, &earnings);
            storage.remove(&earn_key_old);
        }
    }

    // Write merged token list to new address (merge with any existing list)
    if !tokens.is_empty() {
        let mut new_tokens: soroban_sdk::Vec<Address> = storage
            .get(&tokens_key_new)
            .unwrap_or(soroban_sdk::Vec::new(env));
        for token in tokens.iter() {
            if !new_tokens.contains(&token) {
                new_tokens.push_back(token);
            }
        }
        storage.set(&tokens_key_new, &new_tokens);
    }
    storage.remove(&tokens_key_old);

    // ── 3. Migrate MerchantConfig ─────────────────────────────────────────────
    let cfg_key_old = DataKey::MerchantConfig(old_merchant.clone());
    if let Some(config) = storage.get::<_, crate::types::MerchantConfig>(&cfg_key_old) {
        let cfg_key_new = DataKey::MerchantConfig(new_merchant.clone());
        storage.set(&cfg_key_new, &config);
        storage.remove(&cfg_key_old);
    }

    // ── 4. Migrate MerchantPaused ─────────────────────────────────────────────
    let pause_key_old = DataKey::MerchantPaused(old_merchant.clone());
    if let Some(paused) = storage.get::<_, bool>(&pause_key_old) {
        let pause_key_new = DataKey::MerchantPaused(new_merchant.clone());
        storage.set(&pause_key_new, &paused);
        storage.remove(&pause_key_old);
    }

    // ── 5. Migrate MerchantSubs index and rewrite Subscription.merchant ───────
    let subs_key_old = DataKey::MerchantSubs(old_merchant.clone());
    let subs_key_new = DataKey::MerchantSubs(new_merchant.clone());
    let sub_ids: soroban_sdk::Vec<u32> = storage
        .get(&subs_key_old)
        .unwrap_or(soroban_sdk::Vec::new(env));

    let mut subscriptions_updated: u32 = 0;
    for sub_id in sub_ids.iter() {
        let sub_key = DataKey::Sub(sub_id);
        if let Some(mut sub) = env
            .storage()
            .persistent()
            .get::<_, crate::types::Subscription>(&sub_key)
        {
            if sub.merchant == old_merchant {
                sub.merchant = new_merchant.clone();
                env.storage().persistent().set(&sub_key, &sub);
                subscriptions_updated += 1;
            }
        }
    }

    if !sub_ids.is_empty() {
        let subs_key_new = DataKey::MerchantSubs(new_merchant.clone());
        let mut new_sub_ids: soroban_sdk::Vec<u32> = storage
            .get(&subs_key_new)
            .unwrap_or(soroban_sdk::Vec::new(env));
        for id in sub_ids.iter() {
            if !new_sub_ids.contains(&id) {
                new_sub_ids.push_back(id);
            }
        }
        storage.set(&subs_key_new, &new_sub_ids);
    }
    storage.remove(&subs_key_old);

    // ── 6. Emit audit event after all state writes are complete ───────────────
    env.events().publish(
        (soroban_sdk::Symbol::new(env, "merchant_addr_rotated"),),
        crate::types::MerchantAddressRotatedEvent {
            admin,
            old_merchant,
            new_merchant,
            subscriptions_updated,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(())
}

/// Emit a balance snapshot event for a single (merchant, token) pair.
///
/// Admin-only. Reads the current on-chain balance and accrued/withdrawn/refunded
/// totals for the pair and publishes a `MerchantBalanceSnapshotEvent`. Safe to
/// call even when nothing has been earned yet (emits a zero-valued snapshot).
pub fn do_emit_merchant_balance_snapshot(
    env: &Env,
    admin: Address,
    merchant: Address,
    token: Address,
) -> Result<(), Error> {
    crate::admin::require_admin_auth(env, &admin)?;

    let balance = get_merchant_balance_by_token(env, &merchant, &token);
    let earnings = get_merchant_token_earnings(env, &merchant, &token);
    let accrued = earnings
        .accruals
        .interval
        .checked_add(earnings.accruals.usage)
        .unwrap_or(0)
        .checked_add(earnings.accruals.one_off)
        .unwrap_or(0);
    let withdrawn = earnings.withdrawals;
    let refunded = earnings.refunds;
    let ledger_sequence = env.ledger().sequence();
    let timestamp = env.ledger().timestamp();

    env.events().publish(
        (Symbol::new(env, "merchant_balance_snapshot"), merchant.clone(), token.clone()),
        crate::types::MerchantBalanceSnapshotEvent {
            merchant,
            token,
            balance,
            accrued,
            withdrawn,
            refunded,
            ledger_sequence,
            timestamp,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Emit balance snapshots for every distinct (merchant, token) pair referenced
/// by subscriptions in `[start_id, end_id)`. Admin-only.
///
/// Each (merchant, token) pair is snapshotted only once, even if referenced by
/// multiple subscriptions in the range. Returns the list of events emitted.
pub fn do_emit_all_balances_snapshot(
    env: &Env,
    admin: Address,
    start_id: u32,
    end_id: u32,
) -> Result<Vec<crate::types::MerchantBalanceSnapshotEvent>, Error> {
    crate::admin::require_admin_auth(env, &admin)?;

    let mut seen: Vec<(Address, Address)> = Vec::new(env);
    let mut out: Vec<crate::types::MerchantBalanceSnapshotEvent> = Vec::new(env);

    for sub_id in start_id..end_id {
        if let Some(sub) = env
            .storage()
            .persistent()
            .get::<_, crate::types::Subscription>(&DataKey::Sub(sub_id))
        {
            let pair = (sub.merchant.clone(), sub.token.clone());
            let already = seen.contains(&pair);
            if !already {
                seen.push_back(pair.clone());

                let balance = get_merchant_balance_by_token(env, &pair.0, &pair.1);
                let earnings = get_merchant_token_earnings(env, &pair.0, &pair.1);
                let accrued = earnings
                    .accruals
                    .interval
                    .checked_add(earnings.accruals.usage)
                    .unwrap_or(0)
                    .checked_add(earnings.accruals.one_off)
                    .unwrap_or(0);
                let withdrawn = earnings.withdrawals;
                let refunded = earnings.refunds;
                let ledger_sequence = env.ledger().sequence();
                let timestamp = env.ledger().timestamp();

                let ev = crate::types::MerchantBalanceSnapshotEvent {
                    merchant: pair.0.clone(),
                    token: pair.1.clone(),
                    balance,
                    accrued,
                    withdrawn,
                    refunded,
                    ledger_sequence,
                    timestamp,
                    schema_version: crate::types::EVENT_SCHEMA_VERSION,
                };

                env.events().publish(
                    (Symbol::new(env, "merchant_balance_snapshot"), pair.0.clone(), pair.1.clone()),
                    ev.clone(),
                );
                out.push_back(ev);
            }
        }
    }

    Ok(out)
}

// ── Plan Template Registry ────────────────────────────────────────────────────
//
// `register_plan` lets a merchant publish a named billing offer (amount,
// interval, trial_seconds) to the on-chain plan catalogue. Subscribers
// reference plans by their plan ID when calling `create_subscription_from_plan`,
// which reduces per-transaction input errors and supports UI-driven catalogues.
//
// `deprecate_plan` irrevocably marks a plan as deprecated so it can no longer
// be used for new subscriptions. Existing subscriptions are unaffected.
//
// Both functions validate merchant identity — only the merchant who owns a
// plan may deprecate it. Emitting canonical events (`plan_registered`,
// `plan_deprecated`) with schema-versioned payloads lets indexers maintain a
// complete audit trail without extra storage reads.

/// Register a new plan template in the on-chain catalogue.
///
/// # Arguments
/// - `merchant`          — address that owns the plan; must authorise the call.
/// - `amount`            — recurring charge per billing interval (token base units; > 0).
/// - `interval_seconds`  — billing interval in seconds (must pass `validate_interval`).
/// - `trial_seconds`     — optional free-trial period in seconds (`0` = no trial).
/// - `usage_enabled`     — whether usage-based charging is enabled for subscribers.
/// - `lifetime_cap`      — optional maximum total amount ever chargeable; `None` = uncapped.
///
/// # Security
/// - `merchant.require_auth()` prevents anyone other than the merchant from
///   publishing plans under their address.
/// - `amount` and `lifetime_cap` (when `Some`) must be positive.
/// - `interval_seconds` is validated via the shared `validate_interval` helper
///   to reject zero/pathological values.
///
/// # Returns
/// The newly-assigned plan ID (a monotonically-increasing `u32`).
pub fn do_register_plan(
    env: &Env,
    merchant: Address,
    amount: i128,
    interval_seconds: u64,
    trial_seconds: u64,
    usage_enabled: bool,
    lifetime_cap: Option<i128>,
) -> Result<u32, Error> {
    // ── Auth ──
    merchant.require_auth();

    // ── Input validation ──
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }
    crate::subscription::validate_interval(interval_seconds)?;
    if let Some(cap) = lifetime_cap {
        if cap <= 0 {
            return Err(Error::InvalidAmount);
        }
    }

    // ── Resolve default token ──
    let token = crate::admin::get_token(env)?;

    // ── Allocate a new plan ID ──
    let plan_id = crate::subscription::next_plan_id(env);

    // ── Construct and persist the plan template ──
    let plan = PlanTemplate {
        merchant: merchant.clone(),
        token: token.clone(),
        amount,
        interval_seconds,
        trial_seconds,
        usage_enabled,
        lifetime_cap,
        template_key: plan_id,
        version: 1,
        is_disabled: false,
    };
    env.storage().instance().set(&DataKey::Plan(plan_id), &plan);

    // ── Emit canonical event ──
    env.events().publish(
        (Symbol::new(env, "plan_registered"), plan_id),
        PlanRegisteredEvent {
            plan_id,
            merchant: merchant.clone(),
            token: token.clone(),
            amount,
            interval_seconds,
            trial_seconds,
            usage_enabled,
            lifetime_cap,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(plan_id)
}

/// Deprecate an existing plan template, preventing new subscriptions from
/// referencing it.
///
/// Deprecation is a one-way, idempotent operation: once deprecated a plan
/// cannot be re-enabled. All existing subscriptions created from the plan
/// continue to operate normally — only new subscription creation is blocked.
///
/// # Arguments
/// - `merchant`         — address that owns the plan; must authorise the call.
/// - `plan_id`          — ID of the plan to deprecate.
///
/// # Errors
/// - [`Error::NotFound`]   — `plan_id` does not refer to an existing plan.
/// - [`Error::Forbidden`]  — caller is not the merchant who owns the plan.
///
/// # Security
/// - `merchant.require_auth()` prevents third parties from deprecating a plan.
/// - Ownership is validated against the stored `plan.merchant`; a merchant
///   cannot deprecate another merchant's plan even with admin credentials.
/// - Uses the canonical `DataKey::Plan(plan_id)` storage key (not a raw tuple)
///   so that `get_plan_template` immediately reflects the deprecated state.
pub fn do_deprecate_plan(
    env: &Env,
    merchant: Address,
    plan_id: u32,
) -> Result<(), Error> {
    // ── Auth ──
    merchant.require_auth();

    // ── Load the plan (returns NotFound if absent) ──
    let mut plan: PlanTemplate = env
        .storage()
        .instance()
        .get(&DataKey::Plan(plan_id))
        .ok_or(Error::NotFound)?;

    // ── Ownership check ──
    if plan.merchant != merchant {
        return Err(Error::Forbidden);
    }

    // ── Idempotent: already deprecated → no-op ──
    if plan.is_disabled {
        return Ok(());
    }

    // ── Effect: mark deprecated using the canonical storage key ──
    plan.is_disabled = true;
    env.storage().instance().set(&DataKey::Plan(plan_id), &plan);

    // ── Emit canonical event ──
    env.events().publish(
        (Symbol::new(env, "plan_deprecated"), plan_id),
        PlanDeprecatedEvent {
            plan_id,
            merchant,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

// ── Merchant Sub-Accounts (#575) ─────────────────────────────────────────────
//
// Let a merchant declare labelled sub-accounts (departments) so subscriptions
// can route to specific ledgers within one merchant identity.
//
// Each sub-account has an isolated balance stored under
// `DataKey::MerchantSubAccount(merchant, label)`.  Sub-account balances
// withdraw independently but roll up to the parent for reporting — parent
// `MerchantEarnings` reflect all charges including sub-account-routed ones.
//
// Security invariants:
// 1. Only the merchant address may register or withdraw from their sub-accounts.
// 2. Duplicate labels are rejected at registration time.
// 3. Unknown (unregistered) sub-account labels are rejected at charge time.
// 4. Withdrawals follow CEI (Checks-Effects-Interactions).

/// Return `Ok(())` if `label` is a registered sub-account for `merchant`.
pub fn require_sub_account_exists(env: &Env, merchant: &Address, label: &Symbol) -> Result<(), Error> {
    let key = DataKey::MerchantSubAccount(merchant.clone(), label.clone());
    if !env.storage().instance().has(&key) {
        return Err(Error::NotFound);
    }
    Ok(())
}

/// Return the current balance of a merchant sub-account.
pub fn get_sub_account_balance(env: &Env, merchant: &Address, label: &Symbol) -> i128 {
    let key = DataKey::MerchantSubAccount(merchant.clone(), label.clone());
    env.storage().instance().get(&key).unwrap_or(0i128)
}

/// Return the list of registered sub-account labels for `merchant`.
pub fn get_sub_account_list(env: &Env, merchant: &Address) -> Vec<Symbol> {
    let key = DataKey::MerchantSubAccountList(merchant.clone());
    env.storage().instance().get(&key).unwrap_or(Vec::new(env))
}

/// Register a new labelled sub-account for the merchant.
///
/// # Arguments
/// - `merchant` — Address that will own the sub-account; must authorise.
/// - `label` — Unique label identifying the sub-account (e.g. `"sales"`).
///
/// # Errors
/// - [`Error::Unauthorized`] if `merchant` does not authorise the call.
/// - [`Error::NotFound`] if the merchant has not initialized their config.
/// - [`Error::InvalidInput`] if `label` is empty or already registered.
///
/// # Events
/// Emits [`SubAccountCreatedEvent`] with topics `("sub_account_created", merchant, label)`.
pub fn register_sub_account(
    env: &Env,
    merchant: Address,
    label: Symbol,
) -> Result<(), Error> {
    merchant.require_auth();

    // Reject empty labels
    let label_str = label.to_str(env);
    if label_str.len() == 0 {
        return Err(Error::InvalidInput);
    }

    // Ensure merchant config exists (merchant must be initialized)
    let _config = get_merchant_config(env, merchant.clone()).ok_or(Error::NotFound)?;

    // Reject duplicate
    let list_key = DataKey::MerchantSubAccountList(merchant.clone());
    let mut labels: Vec<Symbol> = env.storage().instance().get(&list_key).unwrap_or(Vec::new(env));
    if labels.contains(&label) {
        return Err(Error::InvalidInput);
    }

    // Register the sub-account with zero balance
    let bal_key = DataKey::MerchantSubAccount(merchant.clone(), label.clone());
    env.storage().instance().set(&bal_key, &0i128);

    // Update the label index
    labels.push_back(label.clone());
    env.storage().instance().set(&list_key, &labels);

    // Emit event
    env.events().publish(
        (
            Symbol::new(env, "sub_account_created"),
            merchant.clone(),
            label.clone(),
        ),
        crate::types::SubAccountCreatedEvent {
            merchant: merchant.clone(),
            label: label.clone(),
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Credit the given `amount` (in `token`) to a merchant sub-account.
///
/// Called internally by the charge engine when a subscription has a
/// `sub_account_label`.  The parent `MerchantEarnings` are unaffected
/// (they are already credited by `credit_merchant_balance_for_token`)
/// so that roll-up reporting still sees the total.
///
/// # Errors
/// - [`Error::NotFound`] if the sub-account has not been registered.
/// - [`Error::Overflow`] if the new balance exceeds `i128::MAX`.
pub fn credit_sub_account(
    env: &Env,
    merchant: &Address,
    label: &Symbol,
    _token: &Address,
    amount: i128,
) -> Result<(), Error> {
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    require_sub_account_exists(env, merchant, label)?;

    let bal_key = DataKey::MerchantSubAccount(merchant.clone(), label.clone());
    let current: i128 = env.storage().instance().get(&bal_key).unwrap_or(0i128);
    let new_balance = current.checked_add(amount).ok_or(Error::Overflow)?;
    env.storage().instance().set(&bal_key, &new_balance);

    Ok(())
}

/// Withdraw funds from a merchant sub-account to the merchant's payout address.
///
/// # Arguments
/// - `merchant` — Must authorise the call.
/// - `label` — Which sub-account to withdraw from.
/// - `token_addr` — The token to withdraw.
/// - `amount` — Amount to withdraw (must be positive and ≤ sub-account balance).
///
/// # Errors
/// - [`Error::Unauthorized`] — caller does not match `merchant`.
/// - [`Error::NotFound`] — sub-account does not exist.
/// - [`Error::InvalidAmount`] — `amount` ≤ 0.
/// - [`Error::InsufficientBalance`] — sub-account balance < `amount`.
///
/// # Events
/// Emits [`SubAccountWithdrawEvent`] with topics `("sub_account_withdrawn", merchant, label)`.
pub fn withdraw_sub_account_funds(
    env: &Env,
    merchant: Address,
    label: Symbol,
    token_addr: Address,
    amount: i128,
) -> Result<(), Error> {
    merchant.require_auth();

    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    require_sub_account_exists(env, &merchant, &label)?;

    // Checks: read sub-account balance first (before any external calls)
    let bal_key = DataKey::MerchantSubAccount(merchant.clone(), label.clone());
    let current: i128 = env.storage().instance().get(&bal_key).unwrap_or(0i128);

    if current == 0 {
        return Err(Error::NotFound);
    }
    if amount > current {
        return Err(Error::InsufficientBalance);
    }

    // Vault balance check (external call, after storage checks per CEI)
    let token_client = token::Client::new(env, &token_addr);
    let contract = env.current_contract_address();

    if token_client.balance(&contract) < amount {
        return Err(Error::InsufficientBalance);
    }

    let new_balance = current.checked_sub(amount).ok_or(Error::Underflow)?;

    // EFFECTS: Update sub-account balance
    env.storage().instance().set(&bal_key, &new_balance);

    // EFFECTS: Update parent earnings (withdrawal tracked at parent level)
    let mut earnings = get_merchant_token_earnings(env, &merchant, &token_addr);
    earnings.withdrawals = earnings
        .withdrawals
        .checked_add(amount)
        .ok_or(Error::Overflow)?;
    set_merchant_token_earnings(env, &merchant, &token_addr, &earnings);
    crate::accounting::sub_total_accounted(env, &token_addr, amount)?;

    // EFFECTS: Emit event
    env.events().publish(
        (
            Symbol::new(env, "sub_account_withdrawn"),
            merchant.clone(),
            label.clone(),
        ),
        crate::types::SubAccountWithdrawEvent {
            merchant: merchant.clone(),
            label: label.clone(),
            token: token_addr.clone(),
            amount,
            remaining_balance: new_balance,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    // INTERACTION: Transfer tokens to merchant
    token_client.transfer(&contract, &merchant, &amount);

    // INVARIANT (test/CI only)
    #[cfg(any(test, feature = "invariants"))]
    crate::invariants::assert_token_balance_invariant(env, &token_addr)?;

    Ok(())
}
