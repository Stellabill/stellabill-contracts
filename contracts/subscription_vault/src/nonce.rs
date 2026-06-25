//! Nonce: replay-protection counters for privileged operations.
//!
//! This module implements persistent, domain-separated monotonic nonce counters
//! that prevent replay attacks on sensitive operations like `batch_charge` and
//! `rotate_admin`. Each `(signer, domain)` pair maintains an independent counter
//! stored in persistent ledger storage, ensuring correctness across contract upgrades
//! and ledger TTL extensions.
//!
//! # Design
//!
//! - **Monotonic**: Nonces increment by exactly 1 on each successful consumption.
//! - **Domain-separated**: Each operation type (batch_charge, rotate_admin, operator_batch_charge)
//!   uses a distinct domain constant to prevent cross-domain replay.
//! - **Per-signer**: Each caller maintains its own independent counter.
//! - **Persistent**: Stored in ledger persistent storage, surviving upgrades.
//! - **Bounded storage**: Exactly one `u64` per `(signer, domain)` pair.
//!
//! # Security
//!
//! - Auth check (`require_admin_auth`) runs **before** nonce check to reject invalid signers early.
//! - Nonce overflow is prevented by Rust's checked arithmetic (panics rather than wraps).
//! - Cross-domain collision is impossible: domain is part of the storage key.
//! - Out-of-order submission is rejected: only the exact stored value is accepted.

use soroban_sdk::{Address, Env};
use crate::types::{DataKey, Error, NonceConsumedEvent};

/// Domain constant for batch charge operations.
/// Prevents replay of batch_charge nonces into rotate_admin and vice versa.
pub const DOMAIN_BATCH_CHARGE: u32 = 0;

/// Domain constant for admin rotation operations.
pub const DOMAIN_ADMIN_ROTATION: u32 = 1;

/// Domain constant for operator batch charge operations.
pub const DOMAIN_OPERATOR_BATCH_CHARGE: u32 = 2;


/// Retrieve the current (next-expected) nonce for a `(signer, domain)` pair.
///
/// Returns `0` when no nonce has been consumed yet for this combination (first call).
///
/// # Arguments
///
/// * `env` — Soroban environment (for storage access).
/// * `signer` — The address consuming nonces in this domain.
/// * `domain` — The operation domain (e.g., `DOMAIN_BATCH_CHARGE`).
///
/// # Returns
///
/// The next expected nonce value (starting at 0).
pub fn get_nonce(env: &Env, signer: &Address, domain: u32) -> u64 {
    env.storage()
        .persistent()
        .get::<DataKey, u64>(&DataKey::AdminNonce(signer.clone(), domain))
        .unwrap_or(0)
}

/// Consume a nonce, verifying it matches the current expected value and incrementing for the next call.
///
/// This function implements the core replay-protection logic:
/// 1. Reads the stored nonce (default 0 if absent).
/// 2. Asserts `expected == stored`.
/// 3. Increments and persists `stored + 1`.
/// 4. Emits `NonceConsumedEvent` for audit.
///
/// # Arguments
///
/// * `env` — Soroban environment.
/// * `signer` — The address that consumed this nonce (must already be auth'd).
/// * `domain` — The operation domain (DOMAIN_BATCH_CHARGE, etc.).
/// * `expected` — The nonce value caller believes is current. Must equal stored exactly.
///
/// # Errors
///
/// * [`Error::NonceAlreadyUsed`] — `expected != stored`. Nonce has already been consumed,
///   or caller skipped ahead, or is reusing an old nonce.
///
/// # Panics
///
/// Panics if `stored.checked_add(1)` overflows (u64::MAX reached). The transaction aborts
/// rather than wrapping to 0, preventing accidental nonce reuse.
///
/// # Security
///
/// Auth check **must** run before calling this function. Invalid signers are rejected
/// before the nonce counter is touched, preventing auth bypass via nonce manipulation.
pub fn check_and_advance(
    env: &Env,
    signer: &Address,
    domain: u32,
    expected: u64,
) -> Result<(), Error> {
    let key = DataKey::AdminNonce(signer.clone(), domain);
    let stored = env.storage().persistent().get::<DataKey, u64>(&key).unwrap_or(0);

    // Reject if expected does not match stored exactly.
    if expected != stored {
        return Err(Error::NonceAlreadyUsed);
    }

    // Increment the counter atomically. Returns Error::Overflow on overflow (u64::MAX).
    let next = stored
        .checked_add(1)
        .ok_or(Error::Overflow)?;

    // Persist the incremented nonce before emitting event (effects-before-interactions).
    env.storage().persistent().set(&key, &next);

    // Emit audit event with current timestamp.
    env.events().publish(
        (soroban_sdk::Symbol::new(env, "nonce_consumed"), signer.clone(), domain),
        NonceConsumedEvent {
            signer: signer.clone(),
            domain,
            nonce: stored,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    /// Mock test to verify constant values are correct.
    #[test]
    fn test_domain_constants() {
        assert_eq!(DOMAIN_BATCH_CHARGE, 0);
        assert_eq!(DOMAIN_ADMIN_ROTATION, 1);
        assert_eq!(DOMAIN_OPERATOR_BATCH_CHARGE, 2);
    }

    #[test]
    fn test_check_and_advance_overflow() {
        let env = Env::default();
        let signer = Address::generate(&env);
        let domain = DOMAIN_BATCH_CHARGE;
        let contract_id = env.register(crate::SubscriptionVault, ());

        let res = env.as_contract(&contract_id, || {
            let key = DataKey::AdminNonce(signer.clone(), domain);
            // Seed with u64::MAX
            env.storage().persistent().set(&key, &u64::MAX);

            // Try to advance it, it should return Err(Error::Overflow)
            check_and_advance(&env, &signer, domain, u64::MAX)
        });
        assert_eq!(res, Err(Error::Overflow));
    }
}
