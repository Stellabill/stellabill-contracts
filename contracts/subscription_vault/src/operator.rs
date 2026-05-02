//! Least-privilege operator role for billing operations.
//!
//! An **operator** is an address set by the admin that may execute charge
//! operations (batch charge, single interval charge, usage charge) but has no
//! access to governance, fund withdrawal, or high-risk configuration.
//!
//! ## Privilege boundaries
//!
//! | Capability | Admin | Operator |
//! |:-----------|:-----:|:--------:|
//! | `operator_batch_charge` | — | ✓ |
//! | `operator_charge_subscription` | — | ✓ |
//! | `operator_charge_usage` / `_with_reference` | — | ✓ |
//! | `set_operator` / `remove_operator` | ✓ | ✗ |
//! | All governance ops (rotate_admin, set_min_topup …) | ✓ | ✗ |
//! | Fund withdrawal / recovery | ✓ | ✗ |
//!
//! ## Lifecycle
//!
//! 1. Admin calls `set_operator(admin, operator)` — stores the address and emits
//!    `OperatorSetEvent`.
//! 2. Operator signs `operator_batch_charge` / `operator_charge_*` transactions.
//! 3. Admin calls `remove_operator(admin)` — clears the key and emits
//!    `OperatorRemovedEvent`. The operator loses access **immediately**.
//!
//! ## Admin rotation policy
//!
//! Operator storage (`DataKey::Operator`) is **not** touched during `rotate_admin`.
//! The existing operator key persists after rotation. The new admin may call
//! `remove_operator` or `set_operator` to update it.
//!
//! ## Replay protection
//!
//! `operator_batch_charge` requires a monotonic nonce under
//! `DOMAIN_OPERATOR_BATCH_CHARGE = 2`, independent of the admin's own
//! `DOMAIN_BATCH_CHARGE = 0` counter. This prevents cross-role replay.

#![allow(dead_code)]

use crate::admin::require_admin_auth;
use crate::charge_core::{charge_one, charge_usage_one};
use crate::types::{
    BatchChargeResult, ChargeExecutionResult, DataKey, Error, OperatorRemovedEvent,
    OperatorSetEvent, UsageChargeResult,
};
use soroban_sdk::{Address, Env, String, Symbol, Vec};

// ── Storage helpers ───────────────────────────────────────────────────────────

/// Return the stored operator address, or `None` if none is set.
pub fn get_operator(env: &Env) -> Option<Address> {
    env.storage().instance().get(&DataKey::Operator)
}

/// Require that `operator` has signed the transaction and matches the stored
/// operator address. Returns `Error::Unauthorized` in all failure cases:
/// - no operator is set
/// - the provided address does not match the stored one
/// - the host-level auth check fails (no valid signature)
pub fn require_operator_auth(env: &Env, operator: &Address) -> Result<(), Error> {
    operator.require_auth();
    let stored = get_operator(env).ok_or(Error::Unauthorized)?;
    if operator != &stored {
        return Err(Error::Unauthorized);
    }
    Ok(())
}

// ── Admin-managed operator CRUD ───────────────────────────────────────────────

/// Set the operator address. Admin only.
///
/// Replaces any previously stored operator. Emits [`OperatorSetEvent`].
///
/// # Errors
/// - [`Error::Unauthorized`] — `admin` is not the stored admin.
/// - [`Error::InvalidInput`] — `operator` is the contract's own address (would
///   permanently lock operator privileges since the contract cannot sign).
pub fn do_set_operator(env: &Env, admin: Address, operator: Address) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;

    if operator == env.current_contract_address() {
        return Err(Error::InvalidInput);
    }

    env.storage().instance().set(&DataKey::Operator, &operator);

    env.events().publish(
        (Symbol::new(env, "operator_set"),),
        OperatorSetEvent {
            admin,
            operator,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(())
}

/// Remove the operator. Admin only.
///
/// Clears the stored operator address. Any in-flight operator transactions
/// will fail immediately after this call. Emits [`OperatorRemovedEvent`].
///
/// Calling this when no operator is set is a no-op (returns `Ok`).
///
/// # Errors
/// - [`Error::Unauthorized`] — `admin` is not the stored admin.
pub fn do_remove_operator(env: &Env, admin: Address) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;

    if !env.storage().instance().has(&DataKey::Operator) {
        return Ok(());
    }

    env.storage().instance().remove(&DataKey::Operator);

    env.events().publish(
        (Symbol::new(env, "operator_removed"),),
        OperatorRemovedEvent {
            admin,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(())
}

// ── Operator charge operations ────────────────────────────────────────────────

/// Batch charge by an operator. Operator only.
///
/// Requires a monotonic nonce under `DOMAIN_OPERATOR_BATCH_CHARGE` (domain 2).
/// This is independent of the admin's batch-charge nonce (domain 0) so that
/// capturing one cannot be replayed as the other.
///
/// Returns a per-subscription result vector identical in shape to
/// [`do_batch_charge`](crate::admin::do_batch_charge).
///
/// # Errors
/// - [`Error::Unauthorized`] — caller is not the stored operator.
/// - [`Error::NonceAlreadyUsed`] — nonce mismatch.
/// - [`Error::EmergencyStopActive`] — checked by the lib.rs entrypoint.
pub fn do_operator_batch_charge(
    env: &Env,
    operator: Address,
    subscription_ids: &Vec<u32>,
    nonce: u64,
) -> Result<Vec<BatchChargeResult>, Error> {
    require_operator_auth(env, &operator)?;

    crate::nonce::check_and_advance(
        env,
        &operator,
        crate::nonce::DOMAIN_OPERATOR_BATCH_CHARGE,
        nonce,
    )?;

    Ok(crate::admin::execute_batch_charge(env, subscription_ids))
}

/// Single interval charge by an operator. Operator only.
///
/// # Errors
/// - [`Error::Unauthorized`] — caller is not the stored operator.
/// - [`Error::EmergencyStopActive`] — checked by the lib.rs entrypoint.
pub fn do_operator_charge_subscription(
    env: &Env,
    operator: Address,
    subscription_id: u32,
) -> Result<ChargeExecutionResult, Error> {
    require_operator_auth(env, &operator)?;

    let now = env.ledger().timestamp();
    charge_one(env, subscription_id, now, None)
}

/// Metered usage charge by an operator. Operator only.
///
/// # Errors
/// - [`Error::Unauthorized`] — caller is not the stored operator.
/// - [`Error::EmergencyStopActive`] — checked by the lib.rs entrypoint.
pub fn do_operator_charge_usage(
    env: &Env,
    operator: Address,
    subscription_id: u32,
    usage_amount: i128,
) -> Result<UsageChargeResult, Error> {
    require_operator_auth(env, &operator)?;

    charge_usage_one(
        env,
        subscription_id,
        usage_amount,
        String::from_str(env, "usage"),
    )
}

/// Metered usage charge with an explicit reference string. Operator only.
///
/// # Errors
/// - [`Error::Unauthorized`] — caller is not the stored operator.
/// - [`Error::EmergencyStopActive`] — checked by the lib.rs entrypoint.
pub fn do_operator_charge_usage_with_reference(
    env: &Env,
    operator: Address,
    subscription_id: u32,
    usage_amount: i128,
    reference: String,
) -> Result<UsageChargeResult, Error> {
    require_operator_auth(env, &operator)?;

    charge_usage_one(env, subscription_id, usage_amount, reference)
}
