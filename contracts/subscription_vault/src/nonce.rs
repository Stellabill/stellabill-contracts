//! Nonce-based replay protection for privileged admin operations.
//!
//! Each `(signer, domain)` pair maintains an independent monotonic counter stored
//! in persistent contract storage. A privileged call must supply a `nonce` equal
//! to the currently stored value; on success the counter advances by one.
//!
//! This prevents:
//!
//! 1. **Replay attacks** — resubmitting a previously captured transaction.
//! 2. **Cross-domain collisions** — a nonce valid for one operation class
//!    cannot be replayed for a different one.
//! 3. **Out-of-order execution** — callers must consume nonces sequentially;
//!    skipping or repeating a value is rejected.
//!
//! # Usage
//!
//! ```text
//! off-chain: nonce = get_admin_nonce(signer, domain)   // read current value
//! on-chain:  privileged_fn(... , nonce)                // must match stored
//! contract:  check_and_advance(env, &signer, domain, nonce)?;
//! ```
//!
//! # Storage layout
//!
//! Key:   `DataKey::AdminNonce(signer: Address, domain: u32)`
//! Value: `u64`  — next expected nonce (starts at `0`; first valid call passes `0`)
//! Tier:  `persistent` — survives ledger TTL bumps

#![allow(dead_code)]

use crate::types::{DataKey, Error, NonceConsumedEvent};
use soroban_sdk::{Address, Env, Symbol};

// ── Domain constants ──────────────────────────────────────────────────────────

/// Domain tag for `batch_charge` admin operations.
///
/// All batch charge calls share this domain so their nonces form a single
/// monotonic sequence, giving full ordering to the admin's batch history.
pub const DOMAIN_BATCH_CHARGE: u32 = 0;

/// Domain tag for `rotate_admin` operations.
///
/// Separating admin rotation from other operations ensures that capturing a
/// batch-charge nonce cannot be leveraged to replay a rotate-admin call.
pub const DOMAIN_ADMIN_ROTATION: u32 = 1;

/// Domain tag for `operator_batch_charge` operations.
///
/// Operators have their own nonce sequence independent of the admin's
/// batch-charge counter, so a captured operator nonce cannot be replayed
/// as an admin batch-charge (or vice-versa).
pub const DOMAIN_OPERATOR_BATCH_CHARGE: u32 = 2;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns a human-readable `Symbol` label for a domain tag.
///
/// Used exclusively in events so that indexers can display domain names
/// without needing to hard-code the numeric constants.
fn domain_label(env: &Env, domain: u32) -> Symbol {
    match domain {
        DOMAIN_BATCH_CHARGE => Symbol::new(env, "batch"),
        DOMAIN_ADMIN_ROTATION => Symbol::new(env, "adm_rot"),
        DOMAIN_OPERATOR_BATCH_CHARGE => Symbol::new(env, "op_batch"),
        _ => Symbol::new(env, "unknown"),
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Returns the current (next-expected) nonce for the given `(signer, domain)` pair.
///
/// Returns `0` when no nonce has been consumed yet.
///
/// Off-chain callers should read this value, then pass it unchanged to the
/// privileged function that calls [`check_and_advance`].
pub fn get_nonce(env: &Env, signer: &Address, domain: u32) -> u64 {
    env.storage()
        .persistent()
        .get(&DataKey::AdminNonce(signer.clone(), domain))
        .unwrap_or(0u64)
}

/// Validates `provided_nonce` and advances the counter for `(signer, domain)`.
///
/// The contract checks `provided_nonce == stored_nonce`, then atomically
/// writes `stored_nonce + 1` before emitting a [`NonceConsumedEvent`].
///
/// # Replay / out-of-order protection
///
/// - **Old nonce (replay):** `provided < stored` → `Error::NonceAlreadyUsed`
/// - **Future nonce (skip):**  `provided > stored` → `Error::NonceAlreadyUsed`
/// - **Correct nonce:**        `provided == stored` → advances to `stored + 1`
///
/// # Panics
///
/// Panics (via `checked_add` overflow) if `stored_nonce` is `u64::MAX`.
/// In practice a nonce will never reach this value.
pub fn check_and_advance(
    env: &Env,
    signer: &Address,
    domain: u32,
    provided_nonce: u64,
) -> Result<(), Error> {
    let stored = get_nonce(env, signer, domain);

    if provided_nonce != stored {
        return Err(Error::NonceAlreadyUsed);
    }

    // Advance before emitting — effects before interactions pattern.
    let next = stored.checked_add(1).expect("nonce overflow");
    env.storage()
        .persistent()
        .set(&DataKey::AdminNonce(signer.clone(), domain), &next);

    env.events().publish(
        (
            Symbol::new(env, "nonce_consumed"),
            signer.clone(),
            domain_label(env, domain),
        ),
        NonceConsumedEvent {
            signer: signer.clone(),
            domain,
            nonce: provided_nonce,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(())
}
