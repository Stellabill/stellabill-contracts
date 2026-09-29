//! Admin and config: init, min_topup, batch_charge, single charge.
//!
//! **PRs that only change admin or batch behavior should edit this file only.**

#![allow(dead_code)]

use crate::types::{
    AcceptedToken, AdminConfigChangedEvent, AdminProposal, AdminProposalCancelledEvent,
    AdminProposalClaimedEvent, AdminProposalCreatedEvent, AdminRotatedEvent, BatchChargeResult,
    DataKey, Error, FeeTokenConfiguredEvent, PendingTreasuryChange, RecoveryEvent, RecoveryReason,
    TreasuryChangeExecutedEvent, TreasuryChangeQueuedEvent, TreasurySplitConfig,
    TreasurySplitConfiguredEvent, TreasurySplitEntry, TOPIC_RECOVERY, SUB_TTL_EXTEND_TO,
    SUB_TTL_THRESHOLD,
};
use crate::{
    charge_core::{charge_one, charge_usage_one},
    ChargeExecutionResult,
};
use soroban_sdk::{token, Address, Bytes, Env, String, Symbol, Vec};

pub fn get_schema_version(env: &Env) -> u32 {
    if let Some(v) = env
        .storage()
        .persistent()
        .get::<_, u32>(&DataKey::SchemaVersion)
    {
        v
    } else if let Some(v) = env
        .storage()
        .instance()
        .get::<_, u32>(&DataKey::SchemaVersion)
    {
        v
    } else {
        0
    }
}

pub fn read_config<T>(env: &Env, key: &DataKey) -> Option<T>
where
    T: soroban_sdk::IntoVal<Env, soroban_sdk::Val> + soroban_sdk::TryFromVal<Env, soroban_sdk::Val>,
{
    if let Some(val) = env.storage().persistent().get::<_, T>(key) {
        return Some(val);
    }
    if get_schema_version(env) < 3 {
        if let Some(val) = env.storage().instance().get::<_, T>(key) {
            return Some(val);
        }
    }
    None
}

pub fn write_config<T>(env: &Env, key: &DataKey, value: &T)
where
    T: soroban_sdk::IntoVal<Env, soroban_sdk::Val> + soroban_sdk::TryFromVal<Env, soroban_sdk::Val>,
{
    let version = get_schema_version(env);
    if version >= 3 {
        env.storage().persistent().set(key, value);
        crate::subscription::maybe_extend_ttl(env, key, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        env.storage().instance().remove(key);
    } else {
        env.storage().instance().set(key, value);
    }
}

pub fn has_config(env: &Env, key: &DataKey) -> bool {
    if env.storage().persistent().has(key) {
        return true;
    }
    if get_schema_version(env) < 3 {
        if env.storage().instance().has(key) {
            return true;
        }
    }
    false
}

pub fn remove_config(env: &Env, key: &DataKey) {
    env.storage().persistent().remove(key);
    env.storage().instance().remove(key);
}

// ── Admin-config cooldown ────────────────────────────────────────────────────

/// Default per-key cooldown in seconds between protocol-wide admin config
/// mutations.  Six hours (21 600 s) gives guardians time to detect and respond
/// to a compromised admin key while keeping legitimate operations fast.
pub const CONFIG_COOLDOWN_SECS: u64 = 6 * 60 * 60;

/// Hash a human-readable `key_label` (e.g. `"MinTopup"`) into a
/// collision-free `BytesN<32>` used as the persistent-storage key for the
/// per-config-key cooldown timestamp.
fn hash_key_label(env: &Env, key_label: &str) -> soroban_sdk::BytesN<32> {
    let label_bytes = Bytes::from_slice(env, key_label.as_bytes());
    env.crypto().sha256(&label_bytes).into()
}

/// Enforce a per-key cooldown on protocol-wide admin config mutations.
///
/// 1. Hashes `key_label` to derive the storage key.
/// 2. Reads the previous mutation timestamp (0 if this is the first mutation).
/// 3. If the current ledger timestamp is within [`CONFIG_COOLDOWN_SECS`] of
///    `prev_ts`, returns [`Error::CooldownActive`].
/// 4. Otherwise, records the current timestamp and emits
///    [`AdminConfigChangedEvent`].
///
/// Call this **before** each config-mutating write.  Because Soroban
/// transactions are atomic, if the subsequent mutation fails the entire
/// transaction (including the timestamp write and event) reverts.
///
/// Governance proposals that execute through `do_execute_proposal` call
/// `write_config` directly and intentionally **bypass** this guard, allowing
/// guardians to override the cooldown when a supermajority agrees.
pub fn enforce_config_cooldown(env: &Env, key_label: &str) -> Result<u64, Error> {
    let hash = hash_key_label(env, key_label);
    let storage_key = DataKey::AdminConfigLastChangedAt(hash);

    let prev_ts: u64 = env
        .storage()
        .persistent()
        .get::<_, u64>(&storage_key)
        .unwrap_or(0);

    let now = env.ledger().timestamp();
    if prev_ts > 0 && now.saturating_sub(prev_ts) < CONFIG_COOLDOWN_SECS {
        return Err(Error::CooldownActive);
    }

    env.storage().persistent().set(&storage_key, &now);
    env.storage()
        .persistent()
        .extend_ttl(&storage_key, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
    crate::subscription::maybe_extend_ttl(env, &storage_key, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);

    env.events().publish(
        (Symbol::new(env, "admin_config_changed"),),
        AdminConfigChangedEvent {
            key_label: String::from_str(env, key_label),
            prev_ts,
            timestamp: now,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(prev_ts)
}

fn accepted_tokens_key() -> DataKey {
    DataKey::AcceptedTokens
}

fn accepted_token_decimals_key(token: &Address) -> DataKey {
    DataKey::TokenDecimals(token.clone())
}

pub fn do_init(
    env: &Env,
    token: Address,
    token_decimals: u32,
    admin: Address,
    min_topup: i128,
    grace_period: u64,
) -> Result<(), Error> {
    if has_config(env, &DataKey::Token) || has_config(env, &DataKey::Admin) {
        return Err(Error::AlreadyInitialized);
    }
    if min_topup <= 0 {
        return Err(Error::InvalidAmount);
    }
    if token_decimals > 19 {
        return Err(Error::InvalidTokenDecimals);
    }
    if token == env.current_contract_address() {
        return Err(Error::InvalidToken);
    }

    // Set schema version to target 3 in persistent storage first
    env.storage()
        .persistent()
        .set(&DataKey::SchemaVersion, &crate::STORAGE_VERSION);
    crate::subscription::maybe_extend_ttl(
        env,
        &DataKey::SchemaVersion,
        SUB_TTL_THRESHOLD,
        SUB_TTL_EXTEND_TO,
    );

    write_config(env, &DataKey::Token, &token);

    let instance = env.storage().instance();
    instance.set(&accepted_token_decimals_key(&token), &token_decimals);
    let mut tokens = Vec::new(env);
    tokens.push_back(token.clone());
    instance.set(&accepted_tokens_key(), &tokens);

    write_config(env, &DataKey::Admin, &admin);
    write_config(env, &DataKey::MinTopup, &min_topup);
    instance.set(&DataKey::GracePeriod, &grace_period);

    env.events().publish(
        (Symbol::new(env, "initialized"),),
        (token, admin, min_topup, grace_period),
    );
    Ok(())
}

pub fn require_admin(env: &Env) -> Result<Address, Error> {
    read_config(env, &DataKey::Admin).ok_or(Error::NotInitialized)
}

pub fn require_admin_auth(env: &Env, admin: &Address) -> Result<(), Error> {
    admin.require_auth();
    let stored_admin = require_admin(env)?;
    if admin != &stored_admin {
        return Err(Error::Forbidden);
    }
    Ok(())
}

pub fn require_stored_admin_auth(env: &Env) -> Result<Address, Error> {
    let stored_admin = require_admin(env)?;
    stored_admin.require_auth();
    Ok(stored_admin)
}

/// Authorize `caller` as **either** the stored admin **or** the stored operator.
///
/// Used by the bulk pause/cancel operational tooling, which both privileged roles
/// may invoke. `caller.require_auth()` runs first (so an unauthenticated caller is
/// rejected before any identity comparison), then the address must match the
/// stored admin or operator; anything else returns [`Error::Unauthorized`].
///
/// This deliberately does **not** widen any other surface: the operator still has
/// no access to fund withdrawal, admin rotation, or governance.
pub fn require_admin_or_operator_auth(env: &Env, caller: &Address) -> Result<(), Error> {
    caller.require_auth();

    let stored_admin = require_admin(env)?;
    if caller == &stored_admin {
        return Ok(());
    }

    if let Some(stored_op) = crate::operator::get_operator(env) {
        if caller == &stored_op {
            return Ok(());
        }
    }

    Err(Error::Unauthorized)
}

pub fn do_set_min_topup(env: &Env, admin: Address, min_topup: i128) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;
    if min_topup <= 0 {
        return Err(Error::InvalidAmount);
    }
    enforce_config_cooldown(env, "MinTopup")?;
    write_config(env, &DataKey::MinTopup, &min_topup);
    env.events()
        .publish((Symbol::new(env, "min_topup_updated"),), min_topup);
    Ok(())
}

pub fn get_min_topup(env: &Env) -> Result<i128, Error> {
    read_config(env, &DataKey::MinTopup).ok_or(Error::NotInitialized)
}

pub fn do_set_grace_period(env: &Env, admin: Address, grace_period: u64) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;
    enforce_config_cooldown(env, "GracePeriod")?;
    env.storage()
        .instance()
        .set(&DataKey::GracePeriod, &grace_period);
    Ok(())
}

pub fn get_grace_period(env: &Env) -> Result<u64, Error> {
    Ok(env
        .storage()
        .instance()
        .get(&DataKey::GracePeriod)
        .unwrap_or(0))
}

pub fn do_set_subscriber_create_cap(env: &Env, admin: Address, cap: u32) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;
    write_config(env, &DataKey::SubscriberCreateCap, &cap);
    env.events().publish(
        (Symbol::new(env, "subscriber_create_cap_updated"),),
        cap,
    );
    Ok(())
}

pub fn get_subscriber_create_cap(env: &Env) -> u32 {
    read_config(env, &DataKey::SubscriberCreateCap).unwrap_or(50u32)
}

pub fn get_token(env: &Env) -> Result<Address, Error> {
    read_config(env, &DataKey::Token).ok_or(Error::NotFound)
}

pub fn get_token_decimals(env: &Env, token: &Address) -> Result<u32, Error> {
    env.storage()
        .instance()
        .get(&accepted_token_decimals_key(token))
        .ok_or(Error::NotFound)
}

pub fn is_token_accepted(env: &Env, token: &Address) -> bool {
    env.storage()
        .instance()
        .has(&accepted_token_decimals_key(token))
}

pub fn add_accepted_token(
    env: &Env,
    admin: Address,
    token: Address,
    decimals: u32,
) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;

    let storage = env.storage().instance();
    if !storage.has(&accepted_token_decimals_key(&token)) {
        enforce_config_cooldown(env, "AcceptedTokens")?;
        let mut tokens: Vec<Address> = storage.get(&accepted_tokens_key()).unwrap_or(Vec::new(env));
        tokens.push_back(token.clone());
        storage.set(&accepted_tokens_key(), &tokens);
    }
    storage.set(&accepted_token_decimals_key(&token), &decimals);
    Ok(())
}

pub fn remove_accepted_token(env: &Env, admin: Address, token: Address) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;

    let default_token = get_token(env)?;
    if token == default_token {
        return Err(Error::InvalidInput);
    }

    enforce_config_cooldown(env, "AcceptedTokens")?;

    let storage = env.storage().instance();
    storage.remove(&accepted_token_decimals_key(&token));

    let tokens: Vec<Address> = storage.get(&accepted_tokens_key()).unwrap_or(Vec::new(env));
    let mut next = Vec::new(env);
    for t in tokens.iter() {
        if t != token {
            next.push_back(t);
        }
    }
    storage.set(&accepted_tokens_key(), &next);
    Ok(())
}

pub fn list_accepted_tokens(env: &Env) -> Vec<AcceptedToken> {
    let storage = env.storage().instance();
    let tokens: Vec<Address> = storage.get(&accepted_tokens_key()).unwrap_or(Vec::new(env));
    let mut out = Vec::new(env);
    for token in tokens.iter() {
        if let Some(decimals) = storage.get::<_, u32>(&accepted_token_decimals_key(&token)) {
            out.push_back(AcceptedToken { token, decimals, added_at: 0 });
        }
    }
    out
}

/// Cached admin configuration values to avoid repeated instance-storage
/// lookups inside batch loops.
pub(crate) struct CachedAdminConfig {
    pub fee_bps: u32,
    pub treasury: Option<Address>,
    pub grace_duration: u64,
    pub auto_pause_threshold: u32,
}

/// Read all admin charge-config values from storage at once.
/// Returns `Err` when `get_grace_period` fails (contract not initialized).
pub(crate) fn read_cached_admin_config(env: &Env) -> Result<CachedAdminConfig, Error> {
    Ok(CachedAdminConfig {
        fee_bps: get_protocol_fee_bps(env),
        treasury: get_treasury(env),
        grace_duration: get_grace_period(env)?,
        auto_pause_threshold: get_auto_pause_threshold(env),
    })
}

/// Execute the core batch-charge loop without any auth or nonce checks.
///
/// Called by both `do_batch_charge` (admin path) and
/// `operator::do_operator_batch_charge` (operator path) after their respective
/// auth/nonce guards have been satisfied.
pub(crate) fn execute_batch_charge(
    env: &Env,
    subscription_ids: &Vec<u32>,
) -> Vec<BatchChargeResult> {
    let now = env.ledger().timestamp();
    // Read all admin config values once so they are cached across the batch loop.
    let cached_admin = read_cached_admin_config(env);
    let mut results = Vec::new(env);
    for id in subscription_ids.iter() {
        let admin_ref = match &cached_admin {
            Ok(cfg) => Some(cfg),
            Err(_) => None,
        };
        let r = charge_one(env, id, now, None, admin_ref);
        let res = match r {
            Ok(ChargeExecutionResult::Charged) => BatchChargeResult {
                success: true,
                error_code: 0,
            },
            Ok(ChargeExecutionResult::InsufficientBalance) => BatchChargeResult {
                success: false,
                error_code: Error::InsufficientBalance.to_code(),
            },
            Ok(ChargeExecutionResult::LifetimeCapReached) => BatchChargeResult {
                success: false,
                error_code: Error::LifetimeCapReached.to_code(),
            },
            Ok(ChargeExecutionResult::ScheduledCancellation) => BatchChargeResult {
                success: true,
                error_code: 0,
            },
            // auto_renew=false and interval elapsed: silently skip without error.
            Ok(ChargeExecutionResult::Skipped) => BatchChargeResult {
                success: true,
                error_code: 0,
            },
            Err(e) => BatchChargeResult {
                success: false,
                error_code: e.to_code(),
            },
        };
        results.push_back(res);
    }
    results
}

pub fn do_batch_charge(
    env: &Env,
    subscription_ids: &Vec<u32>,
    nonce: u64,
) -> Result<Vec<BatchChargeResult>, Error> {
    let admin = require_stored_admin_auth(env)?;

    // Nonce check must run before any state mutation to prevent replay.
    // Domain DOMAIN_BATCH_CHARGE separates this counter from other admin ops.
    crate::nonce::check_and_advance(env, &admin, crate::nonce::DOMAIN_BATCH_CHARGE, nonce)?;

    Ok(execute_batch_charge(env, subscription_ids))
}

/// Performs a single interval-based charge. Admin only.
///
/// Loads the stored admin from `DataKey::Admin` and requires a valid
/// authorization signature. Non-admin callers are rejected before any
/// state mutation occurs.
pub fn do_charge_subscription(
    env: &Env,
    subscription_id: u32,
) -> Result<ChargeExecutionResult, Error> {
    let _admin = require_stored_admin_auth(env)?;

    let now = env.ledger().timestamp();
    charge_one(env, subscription_id, now, None, None)
}

/// Performs a single usage-based charge. Admin only.
pub fn do_charge_usage(
    env: &Env,
    subscription_id: u32,
    usage_amount: i128,
    reference: String,
) -> Result<(), Error> {
    let _admin = require_stored_admin_auth(env)?;

    charge_usage_one(env, subscription_id, usage_amount, reference)?;
    Ok(())
}

pub fn do_get_admin(env: &Env) -> Result<Address, Error> {
    read_config(env, &DataKey::Admin).ok_or(Error::NotInitialized)
}

pub fn do_rotate_admin(
    env: &Env,
    current_admin: Address,
    new_admin: Address,
    nonce: u64,
) -> Result<(), Error> {
    require_admin_auth(env, &current_admin)?;

    // Consume nonce for this domain before any other state mutation.
    crate::nonce::check_and_advance(
        env,
        &current_admin,
        crate::nonce::DOMAIN_ADMIN_ROTATION,
        nonce,
    )?;

    // Disallow self-rotation: rotating to the same address is a no-op that
    // could mask misconfiguration and wastes a transaction.
    if new_admin == current_admin {
        return Err(Error::SelfRotation);
    }

    // Disallow rotating to the contract itself: that would permanently lock
    // admin privileges since the contract cannot sign transactions.
    if new_admin == env.current_contract_address() {
        return Err(Error::InvalidNewAdmin);
    }

    enforce_config_cooldown(env, "Admin")?;

    // Atomic swap: write new admin before emitting the event so any indexer
    // that reads state on the event sees the already-updated value.
    write_config(env, &DataKey::Admin, &new_admin);

    env.events().publish(
        (Symbol::new(env, "admin_rotated"),),
        AdminRotatedEvent {
            old_admin: current_admin,
            new_admin,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

pub fn do_recover_stranded_funds(
    env: &Env,
    admin: Address,
    token: Address,
    recipient: Address,
    amount: i128,
    recovery_id: String,
    reason: RecoveryReason,
) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;

    if amount <= 0 {
        return Err(Error::InvalidRecoveryAmount);
    }

    // Check for replay protection
    let recovery_key = DataKey::Recovery(recovery_id.clone());
    if env.storage().persistent().has(&recovery_key) {
        return Err(Error::Replay);
    }

    // Validate available recoverable balance
    let token_client = token::Client::new(env, &token);
    let contract_balance = token_client.balance(&env.current_contract_address());
    let accounted_balance = crate::accounting::get_total_accounted(env, &token);

    let recoverable = contract_balance
        .checked_sub(accounted_balance)
        .ok_or(Error::Underflow)?;
    if amount > recoverable {
        return Err(Error::InsufficientBalance);
    }

    // Mark recovery as executed
    env.storage().persistent().set(&recovery_key, &true);

    let recovery_event = RecoveryEvent {
        admin: admin.clone(),
        recipient: recipient.clone(),
        token: token.clone(),
        amount,
        reason,
        timestamp: env.ledger().timestamp(),
        schema_version: crate::types::EVENT_SCHEMA_VERSION,
    };

    env.events().publish(
        (TOPIC_RECOVERY, admin.clone()),
        recovery_event,
    );

    // Actual token transfer logic
    token_client.transfer(&env.current_contract_address(), &recipient, &amount);

    Ok(())
}

// ── Protocol fee helpers ──────────────────────────────────────────────────────

/// Set protocol fee basis points and treasury address. Admin only.
///
/// fee_bps must be in 0..=10_000. Setting fee_bps to 0 disables fee collection.
const TREASURY_CHANGE_DELAY_SECS: u64 = 48 * 24 * 60 * 60;

pub fn queue_treasury_change(
    env: &Env,
    admin: Address,
    treasury: Address,
    fee_bps: u32,
) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;
    if fee_bps > 10_000 {
        return Err(Error::InvalidInput);
    }
    if env.storage().persistent().has(&DataKey::PendingTreasuryChange) {
        return Err(Error::InvalidInput);
    }

    let effective_at = env.ledger().timestamp().saturating_add(TREASURY_CHANGE_DELAY_SECS);
    let pending = PendingTreasuryChange {
        new_treasury: treasury.clone(),
        new_fee_bps: fee_bps,
        effective_at,
    };
    env.storage().persistent().set(&DataKey::PendingTreasuryChange, &pending);
    env.storage()
        .persistent()
        .extend_ttl(&DataKey::PendingTreasuryChange, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);

    enforce_config_cooldown(env, "ProtocolFee")?;
    write_config(env, &DataKey::FeeBps, &fee_bps);
    write_config(env, &DataKey::Treasury, &treasury);
    env.events().publish(
        (Symbol::new(env, "treasury_change_queued"),),
        TreasuryChangeQueuedEvent {
            admin: admin.clone(),
            treasury,
            fee_bps,
            effective_at,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );
    Ok(())
}

pub fn execute_treasury_change(env: &Env, admin: Address) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;
    let pending = env
        .storage()
        .persistent()
        .get::<_, PendingTreasuryChange>(&DataKey::PendingTreasuryChange)
        .ok_or(Error::NotFound)?;

    let now = env.ledger().timestamp();
    if now < pending.effective_at {
        return Err(Error::TimelockNotElapsed);
    }

    write_config(env, &DataKey::FeeBps, &pending.new_fee_bps);
    write_config(env, &DataKey::Treasury, &pending.new_treasury);
    env.storage().persistent().remove(&DataKey::PendingTreasuryChange);

    env.events().publish(
        (Symbol::new(env, "treasury_change_executed"),),
        TreasuryChangeExecutedEvent {
            admin: admin.clone(),
            treasury: pending.new_treasury.clone(),
            fee_bps: pending.new_fee_bps,
            effective_at: pending.effective_at,
            timestamp: now,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );
    Ok(())
}

pub fn cancel_treasury_change(env: &Env, admin: Address) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;
    if !env.storage().persistent().has(&DataKey::PendingTreasuryChange) {
        return Err(Error::NotFound);
    }
    env.storage().persistent().remove(&DataKey::PendingTreasuryChange);
    Ok(())
}

pub fn set_protocol_fee(
    env: &Env,
    admin: Address,
    treasury: Address,
    fee_bps: u32,
) -> Result<(), Error> {
    queue_treasury_change(env, admin, treasury, fee_bps)
}

/// Return the configured protocol fee in basis points (0 = disabled).
pub fn get_protocol_fee_bps(env: &Env) -> u32 {
    read_config(env, &DataKey::FeeBps).unwrap_or(0u32)
}

/// Return the configured treasury address, or None if not set.
pub fn get_treasury(env: &Env) -> Option<Address> {
    read_config(env, &DataKey::Treasury)
}

/// Set the fee-token override address. Admin only.
///
/// When set, protocol fees are charged in `fee_token` instead of the
/// subscription's settlement token, converted through the oracle at charge
/// time. Pass `None` to clear the override and revert to the default behaviour
/// (fees paid in the subscription's settlement token).
pub fn set_fee_token(
    env: &Env,
    admin: Address,
    fee_token: Option<Address>,
) -> Result<(), crate::types::Error> {
    admin.require_auth();
    let stored = require_admin(env)?;
    if admin != stored {
        return Err(crate::types::Error::Unauthorized);
    }
    enforce_config_cooldown(env, "FeeToken")?;
    if let Some(ref token) = fee_token {
        write_config(env, &DataKey::FeeToken, token);
    } else {
        remove_config(env, &DataKey::FeeToken);
    }
    env.events().publish(
        (Symbol::new(env, "fee_token_configured"),),
        FeeTokenConfiguredEvent {
            admin,
            fee_token: fee_token.clone(),
            old_token: None,
            new_token: fee_token,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );
    Ok(())
}

/// Return the configured fee-token override address, or `None` if not set.
pub fn get_fee_token(env: &Env) -> Option<Address> {
    read_config(env, &DataKey::FeeToken)
}

/// Return the configured buyout premium in basis points, defaulting to 0.
pub fn get_buyout_premium_bps(env: &Env) -> u32 {
    read_config(env, &DataKey::BuyoutPremiumBps).unwrap_or(0u32)
}

/// Validate a treasury split configuration.
///
/// # Validation rules
/// - Must contain at least one entry.
/// - No duplicate beneficiary addresses.
/// - Each `bps` must be > 0.
/// - The sum of all `bps` values must equal exactly 10_000.
///
/// Returns [`Error::InvalidFeeBips`] on any validation failure.
pub fn validate_treasury_split(entries: &Vec<TreasurySplitEntry>) -> Result<(), Error> {
    if entries.is_empty() {
        return Err(Error::InvalidFeeBips);
    }

    let mut total_bps: u32 = 0;
    for i in 0..entries.len() {
        let entry = entries.get(i).unwrap();
        if entry.bps == 0 {
            return Err(Error::InvalidFeeBips);
        }
        total_bps = total_bps
            .checked_add(entry.bps)
            .ok_or(Error::Overflow)?;

        // Check for duplicate beneficiaries
        for j in (i + 1)..entries.len() {
            let other = entries.get(j).unwrap();
            if entry.beneficiary == other.beneficiary {
                return Err(Error::InvalidFeeBips);
            }
        }
    }

    if total_bps != 10_000 {
        return Err(Error::InvalidFeeBips);
    }

    Ok(())
}

/// Set the multi-beneficiary treasury split. Admin only.
///
/// When a treasury split is configured, protocol fees are distributed across
/// the listed beneficiaries according to their basis-point allocation instead
/// of being sent to the single `DataKey::Treasury` address.
///
/// The sum of all `bps` values must equal exactly 10_000. Duplicate
/// beneficiaries are rejected.
pub fn set_treasury_split(
    env: &Env,
    admin: Address,
    entries: Vec<TreasurySplitEntry>,
) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;
    enforce_config_cooldown(env, "TreasurySplit")?;

    validate_treasury_split(&entries)?;

    let config = TreasurySplitConfig {
        entries: entries.clone(),
    };
    write_config(env, &DataKey::TreasurySplit, &config);

    env.events().publish(
        (Symbol::new(env, "treasury_split_configured"),),
        TreasurySplitConfiguredEvent {
            admin,
            entries,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );
    Ok(())
}

/// Return the configured treasury split, or `None` if not set.
pub fn get_treasury_split(env: &Env) -> Option<TreasurySplitConfig> {
    read_config(env, &DataKey::TreasurySplit)
}

/// Clear the treasury split, reverting protocol fees to the single-treasury
/// address stored in `DataKey::Treasury`. Admin only.
pub fn clear_treasury_split(env: &Env, admin: Address) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;
    enforce_config_cooldown(env, "TreasurySplit")?;
    remove_config(env, &DataKey::TreasurySplit);
    Ok(())
}

/// Set the auto-pause threshold (number of consecutive InsufficientBalance failures
/// before a subscription is automatically paused). `0` disables auto-pause.
pub fn do_set_auto_pause_threshold(env: &Env, admin: Address, threshold: u32) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;
    env.storage()
        .instance()
        .set(&DataKey::AutoPauseThreshold, &threshold);
    Ok(())
}

// ── Two-step admin proposal ──────────────────────────────────────────────────

const PROPOSAL_WINDOW_SECS: u64 = 7 * 24 * 60 * 60;

fn proposal_key(env: &Env) -> Symbol {
    Symbol::new(env, "admin_proposal")
}

pub fn do_propose_admin(env: &Env, current_admin: Address, new_admin: Address) -> Result<(), Error> {
    current_admin.require_auth();
    let stored = require_admin(env)?;
    if current_admin != stored {
        return Err(Error::Unauthorized);
    }

    if new_admin == env.current_contract_address() {
        return Err(Error::InvalidNewAdmin);
    }

    let storage = env.storage().instance();
    if storage.has(&proposal_key(env)) {
        return Err(Error::ProposalAlreadyExists);
    }

    let now = env.ledger().timestamp();
    let proposal = AdminProposal {
        new_admin: new_admin.clone(),
        proposed_at: now,
        expires_at: now.saturating_add(PROPOSAL_WINDOW_SECS),
    };
    storage.set(&proposal_key(env), &proposal);

    env.events().publish(
        (Symbol::new(env, "admin_proposal_created"),),
        AdminProposalCreatedEvent {
            old_admin: current_admin,
            new_admin,
            expires_at: proposal.expires_at,
            timestamp: now,
        },
    );
    Ok(())
}

pub fn do_claim_admin_role(env: &Env, claimant: Address) -> Result<(), Error> {
    claimant.require_auth();

    let storage = env.storage().instance();
    let proposal: AdminProposal = storage
        .get(&proposal_key(env))
        .ok_or(Error::ProposalNotFound)?;

    let now = env.ledger().timestamp();
    if now > proposal.expires_at {
        storage.remove(&proposal_key(env));
        return Err(Error::ProposalExpired);
    }

    if claimant != proposal.new_admin {
        return Err(Error::InvalidClaimant);
    }

    let old_admin: Address = require_admin(env)?;

    storage.remove(&proposal_key(env));
    write_config(env, &DataKey::Admin, &claimant);

    env.events().publish(
        (Symbol::new(env, "admin_proposal_claimed"),),
        AdminProposalClaimedEvent {
            old_admin,
            new_admin: claimant,
            timestamp: now,
        },
    );
    Ok(())
}

pub fn do_cancel_admin_proposal(env: &Env, admin: Address) -> Result<(), Error> {
    admin.require_auth();
    let stored = require_admin(env)?;
    if admin != stored {
        return Err(Error::Unauthorized);
    }

    let storage = env.storage().instance();
    if !storage.has(&proposal_key(env)) {
        return Err(Error::NoActiveProposal);
    }

    storage.remove(&proposal_key(env));

    env.events().publish(
        (Symbol::new(env, "admin_proposal_cancelled"),),
        AdminProposalCancelledEvent {
            admin,
            timestamp: env.ledger().timestamp(),
        },
    );
    Ok(())
}

pub fn get_admin_proposal(env: &Env) -> Option<AdminProposal> {
    env.storage().instance().get(&proposal_key(env))
}

/// Return the configured auto-pause threshold. `0` means disabled.
pub fn get_auto_pause_threshold(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::AutoPauseThreshold)
        .unwrap_or(0u32)
}

#[cfg(test)]
mod rotate_admin_adversarial_tests {
    use super::CONFIG_COOLDOWN_SECS;
    use crate::{types::DataKey, Error, SubscriptionVault, SubscriptionVaultClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger as _},
        Address, Env,
    };

    fn setup() -> (Env, SubscriptionVaultClient<'static>, Address) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000_000);

        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

        (env, client, admin)
    }

    fn admin_nonce(env: &Env, contract_id: &Address, signer: &Address) -> u64 {
        env.as_contract(contract_id, || {
            crate::nonce::get_nonce(env, signer, crate::nonce::DOMAIN_ADMIN_ROTATION)
        })
    }

    #[test]
    fn successful_rotation_updates_admin_and_consumes_nonce() {
        let (env, client, admin) = setup();
        let new_admin = Address::generate(&env);

        client.rotate_admin(&admin, &new_admin, &0);

        assert_eq!(client.get_admin(), new_admin);
        assert_eq!(admin_nonce(&env, &client.address, &admin), 1);
    }

    #[test]
    fn rejected_callers_and_targets_leave_nonce_available() {
        let (env, client, admin) = setup();
        let stranger = Address::generate(&env);
        let new_admin = Address::generate(&env);

        assert_eq!(
            client.try_rotate_admin(&stranger, &new_admin, &0),
            Err(Ok(Error::Forbidden))
        );
        assert_eq!(client.get_admin(), admin);
        assert_eq!(admin_nonce(&env, &client.address, &admin), 0);

        assert_eq!(
            client.try_rotate_admin(&admin, &admin, &0),
            Err(Ok(Error::SelfRotation))
        );
        assert_eq!(
            client.try_rotate_admin(&admin, &client.address, &0),
            Err(Ok(Error::InvalidNewAdmin))
        );
        assert_eq!(client.get_admin(), admin);
        assert_eq!(admin_nonce(&env, &client.address, &admin), 0);

        client.rotate_admin(&admin, &new_admin, &0);
        assert_eq!(client.get_admin(), new_admin);
    }

    #[test]
    fn skipped_nonce_is_rejected_without_changing_state() {
        let (env, client, admin) = setup();
        let new_admin = Address::generate(&env);

        assert_eq!(
            client.try_rotate_admin(&admin, &new_admin, &1),
            Err(Ok(Error::NonceAlreadyUsed))
        );
        assert_eq!(client.get_admin(), admin);
        assert_eq!(admin_nonce(&env, &client.address, &admin), 0);

        client.rotate_admin(&admin, &new_admin, &0);
        assert_eq!(client.get_admin(), new_admin);
    }

    #[test]
    fn maximum_nonce_overflow_does_not_change_admin_or_nonce() {
        let (env, client, admin) = setup();
        let contract_id = client.address.clone();
        env.as_contract(&contract_id, || {
            env.storage().persistent().set(
                &DataKey::AdminNonce(admin.clone(), crate::nonce::DOMAIN_ADMIN_ROTATION),
                &u64::MAX,
            );
        });
        let new_admin = Address::generate(&env);

        assert_eq!(
            client.try_rotate_admin(&admin, &new_admin, &u64::MAX),
            Err(Ok(Error::Overflow))
        );
        assert_eq!(client.get_admin(), admin);
        assert_eq!(admin_nonce(&env, &client.address, &admin), u64::MAX);
    }

    #[test]
    fn cooldown_rejection_rolls_back_nonce_and_allows_retry_after_boundary() {
        let (env, client, admin) = setup();
        let next_admin = Address::generate(&env);
        let final_admin = Address::generate(&env);

        client.rotate_admin(&admin, &next_admin, &0);
        assert_eq!(client.get_admin(), next_admin);

        assert_eq!(
            client.try_rotate_admin(&next_admin, &final_admin, &0),
            Err(Ok(Error::CooldownActive))
        );
        assert_eq!(client.get_admin(), next_admin);
        assert_eq!(admin_nonce(&env, &client.address, &next_admin), 0);

        env.ledger().set_timestamp(1_000_000 + CONFIG_COOLDOWN_SECS);
        client.rotate_admin(&next_admin, &final_admin, &0);
        assert_eq!(client.get_admin(), final_admin);
        assert_eq!(admin_nonce(&env, &client.address, &next_admin), 1);
    }
}

// ── Schema migration ──────────────────────────────────────────────────────────

pub fn rewrite_subscriptions_for_ledger_expiration(env: &Env) -> u32 {
    let next_id: u32 = read_config(env, &DataKey::NextId).unwrap_or(0);
    let mut touched = 0u32;
    for id in 0..next_id {
        let key = DataKey::Sub(id);
        if let Some(sub) = env
            .storage()
            .persistent()
            .get::<_, crate::types::Subscription>(&key)
        {
            env.storage().persistent().set(&key, &sub);
            env.storage().persistent().extend_ttl(
                &key,
                SUB_TTL_THRESHOLD,
                SUB_TTL_EXTEND_TO,
            );
            touched = touched.saturating_add(1);
        }
    }
    touched
}

/// v4 → v5 migration: rewrite every `DataKey::Sub(id)` record so the new
/// `sub_account_label: Option<Symbol>` field deserializes cleanly for
/// subscriptions created before STORAGE_VERSION 5.  The in-memory struct
/// already carries `sub_account_label: None` after the deserialization
/// round-trip, so this just needs to read-write each record.
pub fn rewrite_subscriptions_for_sub_account_label(env: &Env) -> u32 {
    let next_id: u32 = read_config(env, &DataKey::NextId).unwrap_or(0);
    let mut touched = 0u32;
    for id in 0..next_id {
        let key = DataKey::Sub(id);
        if let Some(sub) = env
            .storage()
            .persistent()
            .get::<_, crate::types::Subscription>(&key)
        {
            // Round-trip: deserialise populates `sub_account_label: None`,
            // then write back so XDR encoding includes the new field.
            env.storage().persistent().set(&key, &sub);
            env.storage().persistent().extend_ttl(
                &key,
                SUB_TTL_THRESHOLD,
                SUB_TTL_EXTEND_TO,
            );
            touched = touched.saturating_add(1);
        }
    }
    touched
}

/// v5 → v6 migration: rewrite every `DataKey::Sub(id)` record so the new
/// trailing `arrears: i128` field deserializes cleanly for subscriptions
/// created before STORAGE_VERSION 6. The in-memory struct already carries
/// `arrears: 0` after the deserialization round-trip, so a read-write of each
/// record is sufficient to persist the new field in the XDR encoding.
pub fn rewrite_subscriptions_for_arrears(env: &Env) -> u32 {
    let next_id: u32 = read_config(env, &DataKey::NextId).unwrap_or(0);
    let mut touched = 0u32;
    for id in 0..next_id {
        let key = DataKey::Sub(id);
        if let Some(sub) = env
            .storage()
            .persistent()
            .get::<_, crate::types::Subscription>(&key)
        {
            env.storage().persistent().set(&key, &sub);
            env.storage().persistent().extend_ttl(
                &key,
                SUB_TTL_THRESHOLD,
                SUB_TTL_EXTEND_TO,
            );
            touched = touched.saturating_add(1);
        }
    }
    touched
}

pub fn do_migrate_config_to_persistent_internal(env: &Env) -> Result<(), Error> {
    let instance = env.storage().instance();
    let persistent = env.storage().persistent();

    // 1. Token
    if instance.has(&DataKey::Token) {
        let val: Address = instance.get(&DataKey::Token).unwrap();
        persistent.set(&DataKey::Token, &val);
        crate::subscription::maybe_extend_ttl(env, &DataKey::Token, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::Token);
    }

    // 2. Admin
    if instance.has(&DataKey::Admin) {
        let val: Address = instance.get(&DataKey::Admin).unwrap();
        persistent.set(&DataKey::Admin, &val);
        crate::subscription::maybe_extend_ttl(env, &DataKey::Admin, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::Admin);
    }

    // 3. MinTopup
    if instance.has(&DataKey::MinTopup) {
        let val: i128 = instance.get(&DataKey::MinTopup).unwrap();
        persistent.set(&DataKey::MinTopup, &val);
        crate::subscription::maybe_extend_ttl(env, &DataKey::MinTopup, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::MinTopup);
    }

    // 4. NextId
    if instance.has(&DataKey::NextId) {
        let val: u32 = instance.get(&DataKey::NextId).unwrap_or(0);
        persistent.set(&DataKey::NextId, &val);
        crate::subscription::maybe_extend_ttl(env, &DataKey::NextId, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::NextId);
    }

    // 5. EmergencyStop
    if instance.has(&DataKey::EmergencyStop) {
        let val: bool = instance.get(&DataKey::EmergencyStop).unwrap_or(false);
        persistent.set(&DataKey::EmergencyStop, &val);
        crate::subscription::maybe_extend_ttl(
            env,
            &DataKey::EmergencyStop,
            SUB_TTL_THRESHOLD,
            SUB_TTL_EXTEND_TO,
        );
        instance.remove(&DataKey::EmergencyStop);
    }

    // 6. Treasury
    if instance.has(&DataKey::Treasury) {
        let val: Address = instance.get(&DataKey::Treasury).unwrap();
        persistent.set(&DataKey::Treasury, &val);
        crate::subscription::maybe_extend_ttl(env, &DataKey::Treasury, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::Treasury);
    }

    // 7. FeeBps
    if instance.has(&DataKey::FeeBps) {
        let val: u32 = instance.get(&DataKey::FeeBps).unwrap_or(0);
        persistent.set(&DataKey::FeeBps, &val);
        crate::subscription::maybe_extend_ttl(env, &DataKey::FeeBps, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::FeeBps);
    }

    // 8. Operator
    if instance.has(&DataKey::Operator) {
        let val: Address = instance.get(&DataKey::Operator).unwrap();
        persistent.set(&DataKey::Operator, &val);
        crate::subscription::maybe_extend_ttl(env, &DataKey::Operator, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::Operator);
    }

    // 9. SchemaVersion
    if instance.has(&DataKey::SchemaVersion) {
        persistent.set(&DataKey::SchemaVersion, &3u32);
        crate::subscription::maybe_extend_ttl(
            env,
            &DataKey::SchemaVersion,
            SUB_TTL_THRESHOLD,
            SUB_TTL_EXTEND_TO,
        );
        instance.remove(&DataKey::SchemaVersion);
    } else {
        persistent.set(&DataKey::SchemaVersion, &3u32);
        crate::subscription::maybe_extend_ttl(
            env,
            &DataKey::SchemaVersion,
            SUB_TTL_THRESHOLD,
            SUB_TTL_EXTEND_TO,
        );
    }

    Ok(())
}

pub fn migrate_config_to_persistent(env: &Env, admin: Address) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;

    let stored_version = get_schema_version(env);
    if stored_version > 3 {
        return Err(Error::SchemaMigrationDowngrade);
    }

    do_migrate_config_to_persistent_internal(env)?;

    env.events().publish(
        (Symbol::new(env, "schema_migrated"),),
        crate::types::SchemaMigratedEvent {
            admin,
            from_version: stored_version,
            to_version: 3,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Execute a schema migration from the stored version to `STORAGE_VERSION`.
pub fn do_migrate(
    env: &Env,
    admin: Address,
    binary_version: u32,
) -> Result<(), crate::types::Error> {
    require_admin_auth(env, &admin)?;

    let stored_version = get_schema_version(env);

    if stored_version > binary_version {
        return Err(crate::types::Error::SchemaVersionMismatch);
    }

    if stored_version == binary_version {
        return Ok(());
    }

    let mut current = stored_version;
    while current < binary_version {
        match (current, binary_version) {
            (v, _) if v < 2 => {
                current = 2;
            }
            (2, _) => {
                do_migrate_config_to_persistent_internal(env)?;
                current = 3;
            }
            // v3 → v4: rewrite every `DataKey::Sub(id)` record so the new
            // `expires_at_ledger: Option<u32>` field deserializes cleanly
            // for subscriptions created before STORAGE_VERSION 4. Soroban's
            // `#[contracttype]` serialization is positional, so old records
            // would otherwise fail to deserialize and every `get_subscription`
            // call would panic after the binary upgrade.
            (3, _) => {
                rewrite_subscriptions_for_ledger_expiration(env);
                current = 4;
            }
            // v4 → v5: rewrite every `DataKey::Sub(id)` record so the new
            // `sub_account_label: Option<Symbol>` field deserializes cleanly
            // for subscriptions created before STORAGE_VERSION 5.
            (4, _) => {
                rewrite_subscriptions_for_sub_account_label(env);
                current = 5;
            }
            // v5 → v6: rewrite every `DataKey::Sub(id)` record so the new
            // trailing `arrears: i128` field deserializes cleanly for
            // subscriptions created before STORAGE_VERSION 6.
            (5, _) => {
                rewrite_subscriptions_for_arrears(env);
                current = 6;
            }
            _ => {
                current += 1;
            }
        }
    }

    if binary_version >= 3 {
        env.storage()
            .persistent()
            .set(&crate::types::DataKey::SchemaVersion, &binary_version);
        crate::subscription::maybe_extend_ttl(
            env,
            &crate::types::DataKey::SchemaVersion,
            SUB_TTL_THRESHOLD,
            SUB_TTL_EXTEND_TO,
        );
        env.storage()
            .instance()
            .remove(&crate::types::DataKey::SchemaVersion);
    } else {
        env.storage()
            .instance()
            .set(&crate::types::DataKey::SchemaVersion, &binary_version);
    }

    env.events().publish(
        (soroban_sdk::Symbol::new(env, "schema_migrated"),),
        crate::types::SchemaMigratedEvent {
            admin,
            from_version: stored_version,
            to_version: binary_version,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Adversarial coverage for [`rewrite_subscriptions_for_arrears`].
///
/// The migration helper is a pure read-write round-trip over every
/// `DataKey::Sub(id)` record in `0..NextId`: it must (a) rewrite exactly the
/// records that exist, (b) report a deterministic touched-count, (c) never
/// mutate the data it round-trips (including out-of-range `arrears` values it
/// is not responsible for validating), and (d) leave storage untouched when the
/// admin-gated entrypoint that drives it rejects the caller.
#[cfg(test)]
mod rewrite_subscriptions_for_arrears_tests {
    use super::*;
    use crate::test_utils::setup::TestEnv;
    use crate::types::{Subscription, SubscriptionStatus};
    use soroban_sdk::testutils::Address as _;

    /// Build a fully-populated subscription record so any field dropped by the
    /// round-trip fails the equality assertion.
    fn make_sub(env: &Env, arrears: i128) -> Subscription {
        Subscription {
            subscriber: Address::generate(env),
            merchant: Address::generate(env),
            token: Address::generate(env),
            amount: 10_000_000,
            interval_seconds: 2_592_000,
            last_payment_timestamp: 1_700_000_000,
            status: SubscriptionStatus::Active,
            prepaid_balance: 5_000_000,
            usage_enabled: false,
            lifetime_cap: Some(100_000_000),
            lifetime_charged: 3_000_000,
            start_time: 1_699_000_000,
            expires_at: Some(1_800_000_000),
            grace_start_timestamp: None,
            cancel_at: None,
            expires_at_ledger: None,
            sub_account_label: None,
            auto_renew: true,
            auto_renew_disabled_at: None,
            arrears,
        }
    }

    fn seed_sub(te: &TestEnv, id: u32, sub: &Subscription) {
        te.env.as_contract(&te.client.address, || {
            te.env.storage().persistent().set(&DataKey::Sub(id), sub);
        });
    }

    fn seed_next_id(te: &TestEnv, next_id: u32) {
        te.env.as_contract(&te.client.address, || {
            te.env
                .storage()
                .persistent()
                .set(&DataKey::NextId, &next_id);
        });
    }

    fn clear_next_id(te: &TestEnv) {
        te.env.as_contract(&te.client.address, || {
            te.env.storage().persistent().remove(&DataKey::NextId);
        });
    }

    fn read_sub(te: &TestEnv, id: u32) -> Option<Subscription> {
        te.env.as_contract(&te.client.address, || {
            te.env.storage().persistent().get(&DataKey::Sub(id))
        })
    }

    fn read_schema_version(te: &TestEnv) -> Option<u32> {
        te.env.as_contract(&te.client.address, || {
            te.env
                .storage()
                .persistent()
                .get(&DataKey::SchemaVersion)
        })
    }

    fn rewrite(te: &TestEnv) -> u32 {
        te.env.as_contract(&te.client.address, || {
            rewrite_subscriptions_for_arrears(&te.env)
        })
    }

    /// Valid call: every present record is rewritten in place and the return
    /// value equals the number of records visited.
    #[test]
    fn rewrite_arrears_rewrites_each_record_and_returns_count() {
        let te = TestEnv::default();
        let sub0 = make_sub(&te.env, 0);
        let sub1 = make_sub(&te.env, 1_000_000);
        let sub2 = make_sub(&te.env, i128::MAX);
        seed_sub(&te, 0, &sub0);
        seed_sub(&te, 1, &sub1);
        seed_sub(&te, 2, &sub2);
        seed_next_id(&te, 3);

        assert_eq!(rewrite(&te), 3);

        // Full-record equality proves the round-trip preserved every field,
        // not just `arrears`.
        assert_eq!(read_sub(&te, 0), Some(sub0));
        assert_eq!(read_sub(&te, 1), Some(sub1));
        assert_eq!(read_sub(&te, 2), Some(sub2));
    }

    /// Empty subscription set (both "NextId absent" and "NextId == 0") is a
    /// safe no-op: zero touched and no synthetic records written.
    #[test]
    fn rewrite_arrears_empty_set_returns_zero_and_creates_no_records() {
        let te = TestEnv::default();

        clear_next_id(&te);
        assert_eq!(rewrite(&te), 0);
        assert_eq!(read_sub(&te, 0), None);

        seed_next_id(&te, 0);
        assert_eq!(rewrite(&te), 0);
        assert_eq!(read_sub(&te, 0), None);
    }

    /// Non-existent ids inside `0..NextId` are skipped without panicking and
    /// without creating placeholder records.
    #[test]
    fn rewrite_arrears_skips_absent_ids_in_sparse_range() {
        let te = TestEnv::default();
        let sub1 = make_sub(&te.env, 7);
        let sub4 = make_sub(&te.env, 9);
        seed_sub(&te, 1, &sub1);
        seed_sub(&te, 4, &sub4);
        seed_next_id(&te, 5);

        assert_eq!(rewrite(&te), 2);
        assert_eq!(read_sub(&te, 0), None);
        assert_eq!(read_sub(&te, 1), Some(sub1));
        assert_eq!(read_sub(&te, 2), None);
        assert_eq!(read_sub(&te, 3), None);
        assert_eq!(read_sub(&te, 4), Some(sub4));
    }

    /// The loop bound is `NextId`: a record whose id is at or above the stored
    /// counter is not rewritten, but it is also not deleted or mutated.
    #[test]
    fn rewrite_arrears_ignores_ids_at_or_above_next_id() {
        let te = TestEnv::default();
        let sub0 = make_sub(&te.env, 1);
        let sub1 = make_sub(&te.env, 2);
        let sub2 = make_sub(&te.env, 3);
        seed_sub(&te, 0, &sub0);
        seed_sub(&te, 1, &sub1);
        seed_sub(&te, 2, &sub2);
        seed_next_id(&te, 2);

        assert_eq!(rewrite(&te), 2);
        assert_eq!(read_sub(&te, 0), Some(sub0));
        assert_eq!(read_sub(&te, 1), Some(sub1));
        assert_eq!(read_sub(&te, 2), Some(sub2));
    }

    /// A record that is already up to date (`arrears == 0`) is still rewritten
    /// and counted; running the migration twice is idempotent and stable.
    #[test]
    fn rewrite_arrears_is_idempotent_when_already_up_to_date() {
        let te = TestEnv::default();
        let sub = make_sub(&te.env, 0);
        seed_sub(&te, 0, &sub);
        seed_next_id(&te, 1);

        assert_eq!(rewrite(&te), 1);
        assert_eq!(read_sub(&te, 0), Some(sub.clone()));

        assert_eq!(rewrite(&te), 1);
        assert_eq!(read_sub(&te, 0), Some(sub));
    }

    /// Boundary values are round-tripped byte-for-byte: the helper is a
    /// migration, not a validator, so it must not clamp or normalise `arrears`.
    #[test]
    fn rewrite_arrears_preserves_extreme_values_without_clamping() {
        let te = TestEnv::default();
        let min = make_sub(&te.env, i128::MIN);
        let max = make_sub(&te.env, i128::MAX);
        seed_sub(&te, 0, &min);
        seed_sub(&te, 1, &max);
        seed_next_id(&te, 2);

        assert_eq!(rewrite(&te), 2);
        assert_eq!(read_sub(&te, 0).map(|s| s.arrears), Some(i128::MIN));
        assert_eq!(read_sub(&te, 1).map(|s| s.arrears), Some(i128::MAX));
    }

    /// The admin-gated `migrate` entrypoint drives the v5 → v6 arrears rewrite
    /// and advances the stored schema version to the binary's version.
    #[test]
    fn migrate_from_v5_rewrites_arrears_and_advances_schema() {
        let te = TestEnv::default();
        let sub0 = make_sub(&te.env, 0);
        let sub1 = make_sub(&te.env, 42);
        seed_sub(&te, 0, &sub0);
        seed_sub(&te, 1, &sub1);
        seed_next_id(&te, 2);
        te.env.as_contract(&te.client.address, || {
            te.env
                .storage()
                .persistent()
                .set(&DataKey::SchemaVersion, &5u32);
        });

        te.client.migrate(&te.admin);

        assert_eq!(read_schema_version(&te), Some(crate::STORAGE_VERSION));
        assert_eq!(read_sub(&te, 0), Some(sub0));
        assert_eq!(read_sub(&te, 1), Some(sub1));
    }

    /// A non-admin caller is authenticated (auth is mocked) but rejected with
    /// `Forbidden`, and storage is left exactly as it was — the rewrite never
    /// runs and the schema version is not advanced.
    #[test]
    fn migrate_rejects_non_admin_and_leaves_storage_untouched() {
        let te = TestEnv::default();
        let sub0 = make_sub(&te.env, 123);
        seed_sub(&te, 0, &sub0);
        seed_next_id(&te, 1);
        te.env.as_contract(&te.client.address, || {
            te.env
                .storage()
                .persistent()
                .set(&DataKey::SchemaVersion, &5u32);
        });

        let attacker = Address::generate(&te.env);
        assert_eq!(te.client.try_migrate(&attacker), Err(Ok(Error::Forbidden)));

        assert_eq!(read_schema_version(&te), Some(5u32));
        assert_eq!(read_sub(&te, 0), Some(sub0));
    }

    /// Same rejection observed through the panicking client path, pinning the
    /// contract error discriminant (#1002 `Forbidden`).
    #[test]
    #[should_panic(expected = "Error(Contract, #1002)")]
    fn migrate_panics_forbidden_for_non_admin_caller() {
        let te = TestEnv::default();
        let attacker = Address::generate(&te.env);
        te.client.migrate(&attacker);
    }
}
