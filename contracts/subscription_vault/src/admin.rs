//! Admin and config: init, min_topup, batch_charge, single charge.
//!
//! **PRs that only change admin or batch behavior should edit this file only.**

#![allow(dead_code)]

use crate::types::{
    AcceptedToken, AdminRotatedEvent, BatchChargeResult, DataKey, Error, RecoveryEvent,
    RecoveryReason, SUB_TTL_EXTEND_TO, SUB_TTL_THRESHOLD,
};
use crate::{charge_core::{charge_one, charge_usage_one}, ChargeExecutionResult};
use soroban_sdk::{token, Address, Env, String, Symbol, Vec};

pub fn get_schema_version(env: &Env) -> u32 {
    if let Some(v) = env.storage().persistent().get::<_, u32>(&DataKey::SchemaVersion) {
        v
    } else if let Some(v) = env.storage().instance().get::<_, u32>(&DataKey::SchemaVersion) {
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
        env.storage().persistent().extend_ttl(key, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
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
    env.storage().persistent().set(&DataKey::SchemaVersion, &crate::STORAGE_VERSION);
    env.storage().persistent().extend_ttl(&DataKey::SchemaVersion, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);

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
    read_config(env, &DataKey::Admin)
        .ok_or(Error::NotInitialized)
}

pub fn require_admin_auth(env: &Env, admin: &Address) -> Result<(), Error> {
    admin.require_auth();
    let stored_admin = require_admin(env)?;
    if admin != &stored_admin {
        return Err(Error::Unauthorized);
    }
    Ok(())
}

pub fn require_stored_admin_auth(env: &Env) -> Result<Address, Error> {
    let stored_admin = require_admin(env)?;
    stored_admin.require_auth();
    Ok(stored_admin)
}

pub fn do_set_min_topup(env: &Env, admin: Address, min_topup: i128) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;
    if min_topup <= 0 {
        return Err(Error::InvalidAmount);
    }
    write_config(env, &DataKey::MinTopup, &min_topup);
    env.events()
        .publish((Symbol::new(env, "min_topup_updated"),), min_topup);
    Ok(())
}

pub fn get_min_topup(env: &Env) -> Result<i128, Error> {
    read_config(env, &DataKey::MinTopup)
        .ok_or(Error::NotInitialized)
}

pub fn do_set_grace_period(env: &Env, admin: Address, grace_period: u64) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;
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

pub fn get_token(env: &Env) -> Result<Address, Error> {
    read_config(env, &DataKey::Token)
        .ok_or(Error::NotFound)
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
        let mut tokens: Vec<Address> = storage
            .get(&accepted_tokens_key())
            .unwrap_or(Vec::new(env));
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

    let storage = env.storage().instance();
    storage.remove(&accepted_token_decimals_key(&token));

    let tokens: Vec<Address> = storage
        .get(&accepted_tokens_key())
        .unwrap_or(Vec::new(env));
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
    let tokens: Vec<Address> = storage
        .get(&accepted_tokens_key())
        .unwrap_or(Vec::new(env));
    let mut out = Vec::new(env);
    for token in tokens.iter() {
        if let Some(decimals) = storage.get::<_, u32>(&accepted_token_decimals_key(&token)) {
            out.push_back(AcceptedToken { token, decimals });
        }
    }
    out
}

/// Execute the core batch-charge loop without any auth or nonce checks.
///
/// Called by both `do_batch_charge` (admin path) and
/// `operator::do_operator_batch_charge` (operator path) after their respective
/// auth/nonce guards have been satisfied.
pub(crate) fn execute_batch_charge(env: &Env, subscription_ids: &Vec<u32>) -> Vec<BatchChargeResult> {
    let now = env.ledger().timestamp();
    let mut results = Vec::new(env);
    for id in subscription_ids.iter() {
        let r = charge_one(env, id, now, None);
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
pub fn do_charge_subscription(
    env: &Env,
    subscription_id: u32,
) -> Result<ChargeExecutionResult, Error> {
    let _admin = require_stored_admin_auth(env)?;

    let now = env.ledger().timestamp();
    charge_one(env, subscription_id, now, None)
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
    read_config(env, &DataKey::Admin)
        .ok_or(Error::NotInitialized)
}

pub fn do_rotate_admin(env: &Env, current_admin: Address, new_admin: Address, nonce: u64) -> Result<(), Error> {
    require_admin_auth(env, &current_admin)?;

    // Consume nonce for this domain before any other state mutation.
    crate::nonce::check_and_advance(env, &current_admin, crate::nonce::DOMAIN_ADMIN_ROTATION, nonce)?;

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
        (Symbol::new(env, "recovery"), admin.clone()),
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
pub fn set_protocol_fee(
    env: &Env,
    admin: Address,
    treasury: Address,
    fee_bps: u32,
) -> Result<(), crate::types::Error> {
    admin.require_auth();
    let stored = require_admin(env)?;
    if admin != stored {
        return Err(crate::types::Error::Unauthorized);
    }
    if fee_bps > 10_000 {
        return Err(crate::types::Error::InvalidInput);
    }
    write_config(env, &DataKey::FeeBps, &fee_bps);
    write_config(env, &DataKey::Treasury, &treasury);
    env.events().publish(
        (Symbol::new(env, "protocol_fee_configured"),),
        crate::types::ProtocolFeeConfiguredEvent {
            admin: admin.clone(),
            treasury,
            fee_bps,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );
    Ok(())
}

/// Return the configured protocol fee in basis points (0 = disabled).
pub fn get_protocol_fee_bps(env: &Env) -> u32 {
    read_config(env, &DataKey::FeeBps).unwrap_or(0u32)
}

/// Return the configured treasury address, or None if not set.
pub fn get_treasury(env: &Env) -> Option<Address> {
    read_config(env, &DataKey::Treasury)
}

// ── Schema migration ──────────────────────────────────────────────────────────

pub fn do_migrate_config_to_persistent_internal(env: &Env) -> Result<(), Error> {
    let instance = env.storage().instance();
    let persistent = env.storage().persistent();

    // 1. Token
    if instance.has(&DataKey::Token) {
        let val: Address = instance.get(&DataKey::Token).unwrap();
        persistent.set(&DataKey::Token, &val);
        persistent.extend_ttl(&DataKey::Token, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::Token);
    }

    // 2. Admin
    if instance.has(&DataKey::Admin) {
        let val: Address = instance.get(&DataKey::Admin).unwrap();
        persistent.set(&DataKey::Admin, &val);
        persistent.extend_ttl(&DataKey::Admin, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::Admin);
    }

    // 3. MinTopup
    if instance.has(&DataKey::MinTopup) {
        let val: i128 = instance.get(&DataKey::MinTopup).unwrap();
        persistent.set(&DataKey::MinTopup, &val);
        persistent.extend_ttl(&DataKey::MinTopup, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::MinTopup);
    }

    // 4. NextId
    if instance.has(&DataKey::NextId) {
        let val: u32 = instance.get(&DataKey::NextId).unwrap_or(0);
        persistent.set(&DataKey::NextId, &val);
        persistent.extend_ttl(&DataKey::NextId, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::NextId);
    }

    // 5. EmergencyStop
    if instance.has(&DataKey::EmergencyStop) {
        let val: bool = instance.get(&DataKey::EmergencyStop).unwrap_or(false);
        persistent.set(&DataKey::EmergencyStop, &val);
        persistent.extend_ttl(&DataKey::EmergencyStop, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::EmergencyStop);
    }

    // 6. Treasury
    if instance.has(&DataKey::Treasury) {
        let val: Address = instance.get(&DataKey::Treasury).unwrap();
        persistent.set(&DataKey::Treasury, &val);
        persistent.extend_ttl(&DataKey::Treasury, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::Treasury);
    }

    // 7. FeeBps
    if instance.has(&DataKey::FeeBps) {
        let val: u32 = instance.get(&DataKey::FeeBps).unwrap_or(0);
        persistent.set(&DataKey::FeeBps, &val);
        persistent.extend_ttl(&DataKey::FeeBps, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::FeeBps);
    }

    // 8. Operator
    if instance.has(&DataKey::Operator) {
        let val: Address = instance.get(&DataKey::Operator).unwrap();
        persistent.set(&DataKey::Operator, &val);
        persistent.extend_ttl(&DataKey::Operator, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::Operator);
    }

    // 9. SchemaVersion
    if instance.has(&DataKey::SchemaVersion) {
        persistent.set(&DataKey::SchemaVersion, &3u32);
        persistent.extend_ttl(&DataKey::SchemaVersion, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        instance.remove(&DataKey::SchemaVersion);
    } else {
        persistent.set(&DataKey::SchemaVersion, &3u32);
        persistent.extend_ttl(&DataKey::SchemaVersion, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
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

    // Emit event
    env.events().publish(
        (Symbol::new(env, "schema_migrated"),),
        crate::types::SchemaMigratedEvent {
            admin,
            from_version: stored_version,
            to_version: 3,
            timestamp: env.ledger().timestamp(),
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
    // Auth first — no state reads before the caller is verified.
    require_admin_auth(env, &admin)?;

    let stored_version = get_schema_version(env);

    // Downgrade guard: reject if on-chain version is newer than the binary.
    if stored_version > binary_version {
        return Err(crate::types::Error::SchemaMigrationDowngrade);
    }

    // Idempotent no-op: already at the target version.
    if stored_version == binary_version {
        return Ok(());
    }

    // ── Forward upgrade ladder ────────────────────────────────────────────────
    let mut current = stored_version;
    while current < binary_version {
        match (current, binary_version) {
            // v0/v1 → v2: SchemaVersion key was not written by early init
            // calls. No data-shape changes needed; writing the key is enough.
            (v, _) if v < 2 => {
                current = 2;
            }
            // v2 → v3: config keys migration
            (2, _) => {
                do_migrate_config_to_persistent_internal(env)?;
                current = 3;
            }
            _ => {
                current += 1;
            }
        }
    }

    // Commit the new version atomically after all upgrade steps succeed.
    if binary_version >= 3 {
        env.storage().persistent().set(&crate::types::DataKey::SchemaVersion, &binary_version);
        env.storage().persistent().extend_ttl(&crate::types::DataKey::SchemaVersion, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
        env.storage().instance().remove(&crate::types::DataKey::SchemaVersion);
    } else {
        env.storage().instance().set(&crate::types::DataKey::SchemaVersion, &binary_version);
    }

    // Emit audit event.
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
