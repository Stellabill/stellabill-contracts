//! Admin and config: init, min_topup, batch_charge.
//!
//! **PRs that only change admin or batch behavior should edit this file only.**

#![allow(dead_code)]

use crate::types::{
    AcceptedToken, AdminRotatedEvent, BatchChargeResult, DataKey, Error, RecoveryEvent,
    RecoveryReason,
};
use crate::{charge_core::charge_one, ChargeExecutionResult};
use soroban_sdk::{token, Address, Env, String, Symbol, Vec};

fn accepted_tokens_key(env: &Env) -> Symbol {
    Symbol::new(env, "accepted_tokens")
}

fn accepted_token_decimals_key(env: &Env, token: &Address) -> (Symbol, Address) {
    (Symbol::new(env, "token_decimals"), token.clone())
}

pub fn do_init(
    env: &Env,
    token: Address,
    token_decimals: u32,
    admin: Address,
    min_topup: i128,
    grace_period: u64,
) -> Result<(), Error> {
    let instance = env.storage().instance();
    if instance.has(&Symbol::new(env, "token")) || instance.has(&Symbol::new(env, "admin")) {
        return Err(Error::AlreadyInitialized);
    }
    if min_topup < 0 {
        return Err(Error::InvalidAmount);
    }

    instance.set(&Symbol::new(env, "token"), &token);
    instance.set(&accepted_token_decimals_key(env, &token), &token_decimals);
    let mut tokens = Vec::new(env);
    tokens.push_back(token.clone());
    instance.set(&accepted_tokens_key(env), &tokens);
    instance.set(&Symbol::new(env, "admin"), &admin);
    instance.set(&Symbol::new(env, "min_topup"), &min_topup);
    instance.set(&Symbol::new(env, "grace_period"), &grace_period);
    instance.set(&DataKey::SchemaVersion, &1u32);
    env.events().publish(
        (Symbol::new(env, "initialized"),),
        (token, admin, min_topup, grace_period),
    );
    Ok(())
}

pub fn require_admin(env: &Env) -> Result<Address, Error> {
    env.storage()
        .instance()
        .get(&Symbol::new(env, "admin"))
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
    env.storage()
        .instance()
        .set(&Symbol::new(env, "min_topup"), &min_topup);
    env.events()
        .publish((Symbol::new(env, "min_topup_updated"),), min_topup);
    Ok(())
}

pub fn get_min_topup(env: &Env) -> Result<i128, Error> {
    env.storage()
        .instance()
        .get(&Symbol::new(env, "min_topup"))
        .ok_or(Error::NotInitialized)
}

pub fn do_set_grace_period(env: &Env, admin: Address, grace_period: u64) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;
    env.storage()
        .instance()
        .set(&Symbol::new(env, "grace_period"), &grace_period);
    Ok(())
}

pub fn get_grace_period(env: &Env) -> Result<u64, Error> {
    Ok(env
        .storage()
        .instance()
        .get(&Symbol::new(env, "grace_period"))
        .unwrap_or(0))
}

pub fn get_token(env: &Env) -> Result<Address, Error> {
    env.storage()
        .instance()
        .get(&Symbol::new(env, "token"))
        .ok_or(Error::NotFound)
}

pub fn get_token_decimals(env: &Env, token: &Address) -> Result<u32, Error> {
    env.storage()
        .instance()
        .get(&accepted_token_decimals_key(env, token))
        .ok_or(Error::NotFound)
}

pub fn is_token_accepted(env: &Env, token: &Address) -> bool {
    env.storage()
        .instance()
        .has(&accepted_token_decimals_key(env, token))
}

pub fn add_accepted_token(
    env: &Env,
    admin: Address,
    token: Address,
    decimals: u32,
) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;

    let storage = env.storage().instance();
    if !storage.has(&accepted_token_decimals_key(env, &token)) {
        let mut tokens: Vec<Address> = storage
            .get(&accepted_tokens_key(env))
            .unwrap_or(Vec::new(env));
        tokens.push_back(token.clone());
        storage.set(&accepted_tokens_key(env), &tokens);
    }
    storage.set(&accepted_token_decimals_key(env, &token), &decimals);
    Ok(())
}

pub fn remove_accepted_token(env: &Env, admin: Address, token: Address) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;

    let default_token = get_token(env)?;
    if token == default_token {
        return Err(Error::InvalidInput);
    }

    let storage = env.storage().instance();
    storage.remove(&accepted_token_decimals_key(env, &token));

    let tokens: Vec<Address> = storage
        .get(&accepted_tokens_key(env))
        .unwrap_or(Vec::new(env));
    let mut next = Vec::new(env);
    for t in tokens.iter() {
        if t != token {
            next.push_back(t);
        }
    }
    storage.set(&accepted_tokens_key(env), &next);
    Ok(())
}

pub fn list_accepted_tokens(env: &Env) -> Vec<AcceptedToken> {
    let storage = env.storage().instance();
    let tokens: Vec<Address> = storage
        .get(&accepted_tokens_key(env))
        .unwrap_or(Vec::new(env));
    let mut out = Vec::new(env);
    for token in tokens.iter() {
        if let Some(decimals) = storage.get::<_, u32>(&accepted_token_decimals_key(env, &token)) {
            out.push_back(AcceptedToken { token, decimals });
        }
    }
    out
}

pub fn do_batch_charge(
    env: &Env,
    subscription_ids: &Vec<u32>,
) -> Result<Vec<BatchChargeResult>, Error> {
    let _admin = require_stored_admin_auth(env)?;

    let now = env.ledger().timestamp();
    let mut results = Vec::new(env);
    for id in subscription_ids.iter() {
        let r = charge_one(env, id, now, None);
        let res = match &r {
            Ok(ChargeExecutionResult::Charged) => BatchChargeResult {
                success: true,
                error_code: 0,
            },
            Ok(ChargeExecutionResult::InsufficientBalance) => BatchChargeResult {
                success: false,
                error_code: Error::InsufficientBalance.to_code(),
            },
            Err(e) => BatchChargeResult {
                success: false,
                error_code: e.to_code(),
            },
        };
        results.push_back(res);
    }
    Ok(results)
}

pub fn do_get_admin(env: &Env) -> Result<Address, Error> {
    env.storage()
        .instance()
        .get(&Symbol::new(env, "admin"))
        .ok_or(Error::NotInitialized)
}

pub fn do_rotate_admin(env: &Env, current_admin: Address, new_admin: Address) -> Result<(), Error> {
    require_admin_auth(env, &current_admin)?;

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
    env.storage()
        .instance()
        .set(&Symbol::new(env, "admin"), &new_admin);

    env.events().publish(
        (Symbol::new(env, "admin_rotated"),),
        AdminRotatedEvent {
            old_admin: current_admin,
            new_admin,
            timestamp: env.ledger().timestamp(),
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
    
    let recoverable = contract_balance.checked_sub(accounted_balance).ok_or(Error::Underflow)?;
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
        return Err(crate::types::Error::Forbidden);
    }
    if fee_bps > 10_000 {
        return Err(crate::types::Error::InvalidInput);
    }
    let storage = env.storage().instance();
    storage.set(&Symbol::new(env, "fee_bps"), &fee_bps);
    storage.set(&Symbol::new(env, "treasury"), &treasury);
    env.events().publish(
        (Symbol::new(env, "protocol_fee_configured"),),
        crate::types::ProtocolFeeConfiguredEvent {
            treasury: Some(treasury),
            fee_bps,
            timestamp: env.ledger().timestamp(),
        },
    );
    Ok(())
}

/// Return the configured protocol fee in basis points (0 = disabled).
pub fn get_protocol_fee_bps(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&Symbol::new(env, "fee_bps"))
        .unwrap_or(0u32)
}

/// Return the configured treasury address, or None if not set.
pub fn get_treasury(env: &Env) -> Option<Address> {
    env.storage()
        .instance()
        .get(&Symbol::new(env, "treasury"))
}
