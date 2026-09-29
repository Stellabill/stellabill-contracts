//! Replay Domain Isolation & Nonce Security Tests
//!
//! This module verifies that monotonic nonce counters used across distinct operational domains
//! do not collide or cross-consume. Specifically, it tests isolation between:
//! - `DOMAIN_BATCH_CHARGE` (0)
//! - `DOMAIN_ADMIN_ROTATION` (1)
//! - `DOMAIN_OPERATOR_BATCH_CHARGE` (2)
//!
//! # Security Notes
//! - **Domain Separation**: In Soroban storage, nonces are indexed by `DataKey::AdminNonce(signer, domain)`.
//!   Because `domain` is part of the persistent storage key, operators and subscribers can share the same
//!   counter sequence across different transaction types without risk of cross-domain replay attacks or
//!   denial-of-service via counter exhaustion.
//! - **Per-Signer Isolation**: Each `(Address, u32)` tuple maintains an independent counter. An operator
//!   advancing a nonce for Subscription A cannot affect the nonce counter for Subscription B or any other signer.
//! - **Overflow Protection**: Checked arithmetic prevents wrapping at `u64::MAX`. Attempting to consume `u64::MAX`
//!   returns `Error::Overflow` rather than wrapping to zero.
//! - **Authentication Order**: In production contract methods, authentication (`require_admin_auth` or signature verification)
//!   MUST occur before nonce checking to prevent unauthenticated callers from probing or advancing nonce counters.
//! - **Admin Nonce Accessor**: `get_admin_nonce` is a read-only accessor over the same persistent
//!   `DataKey::AdminNonce(signer, domain)` storage. It must never mutate state, must be deterministic
//!   for uninitialized keys (returning 0), and must reflect writes performed by `consume_nonce`.

#![cfg(test)]

use soroban_sdk::{testutils::Address as _, Address, Env};
use crate::nonce::{
    compute_next_nonce, consume_nonce, get_nonce,
    DOMAIN_BATCH_CHARGE, DOMAIN_ADMIN_ROTATION, DOMAIN_OPERATOR_BATCH_CHARGE,
};
use crate::get_admin_nonce;
use crate::types::{DataKey, Error};

/// Verifies core domain isolation as specified in issue #603:
/// 1. Consume nonce N under domain A.
/// 2. Consume nonce N under domain B (must succeed).
/// 3. Re-consume N under domain A (must reject).
#[test]
fn test_nonce_domain_isolation_core() {
    let env = Env::default();
    let signer = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        let domain_a = DOMAIN_BATCH_CHARGE;
        let domain_b = DOMAIN_ADMIN_ROTATION;
        let domain_c = DOMAIN_OPERATOR_BATCH_CHARGE;
        let nonce_n = 0u64;

        // Verify initial state across all domains is 0
        assert_eq!(get_nonce(&env, &signer, domain_a), nonce_n);
        assert_eq!(get_nonce(&env, &signer, domain_b), nonce_n);
        assert_eq!(get_nonce(&env, &signer, domain_c), nonce_n);

        // 1. Consume nonce N under domain A
        assert_eq!(consume_nonce(&env, &signer, domain_a, nonce_n), Ok(()));
        assert_eq!(get_nonce(&env, &signer, domain_a), nonce_n + 1);
        // Ensure domain B and C remain untouched
        assert_eq!(get_nonce(&env, &signer, domain_b), nonce_n);
        assert_eq!(get_nonce(&env, &signer, domain_c), nonce_n);

        // 2. Consume nonce N under domain B (must succeed despite N being consumed in domain A)
        assert_eq!(consume_nonce(&env, &signer, domain_b, nonce_n), Ok(()));
        assert_eq!(get_nonce(&env, &signer, domain_b), nonce_n + 1);
        assert_eq!(get_nonce(&env, &signer, domain_c), nonce_n);

        // 3. Re-consume N under domain A (must reject as domain A is now at N+1)
        assert_eq!(
            consume_nonce(&env, &signer, domain_a, nonce_n),
            Err(Error::NonceAlreadyUsed)
        );
        // Ensure failed re-consumption did not advance the counter
        assert_eq!(get_nonce(&env, &signer, domain_a), nonce_n + 1);

        // Verify we can still consume nonce N under domain C without collision
        assert_eq!(consume_nonce(&env, &signer, domain_c, nonce_n), Ok(()));
        assert_eq!(get_nonce(&env, &signer, domain_c), nonce_n + 1);
    });
}

/// Edge case test: Cross-subscription / cross-signer isolation for the same nonce and domain.
/// Verifies that two different operators/subscribers using the exact same nonce under the exact same
/// replay domain do not collide or interfere with each other's execution flow.
#[test]
fn test_cross_subscription_same_nonce() {
    let env = Env::default();
    let subscriber1 = Address::generate(&env);
    let subscriber2 = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        let domain = DOMAIN_BATCH_CHARGE;

        // Both subscribers start at nonce 0
        assert_eq!(get_nonce(&env, &subscriber1, domain), 0);
        assert_eq!(get_nonce(&env, &subscriber2, domain), 0);

        // Subscriber 1 advances nonces 0, 1, 2
        assert_eq!(consume_nonce(&env, &subscriber1, domain, 0), Ok(()));
        assert_eq!(consume_nonce(&env, &subscriber1, domain, 1), Ok(()));
        assert_eq!(consume_nonce(&env, &subscriber1, domain, 2), Ok(()));
        assert_eq!(get_nonce(&env, &subscriber1, domain), 3);

        // Subscriber 2 consuming nonce 0 under the same domain MUST succeed
        assert_eq!(consume_nonce(&env, &subscriber2, domain, 0), Ok(()));
        assert_eq!(get_nonce(&env, &subscriber2, domain), 1);

        // Subscriber 2 consuming nonce 1 MUST succeed
        assert_eq!(consume_nonce(&env, &subscriber2, domain, 1), Ok(()));
        assert_eq!(get_nonce(&env, &subscriber2, domain), 2);

        // Verify Subscriber 1's nonce counter was not altered
        assert_eq!(get_nonce(&env, &subscriber1, domain), 3);
    });
}

/// Edge case: skip-ahead nonces are rejected and do not advance the stored counter.
#[test]
fn test_skip_ahead_nonce_rejected() {
    let env = Env::default();
    let signer = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        let domain = DOMAIN_BATCH_CHARGE;

        assert_eq!(consume_nonce(&env, &signer, domain, 0), Ok(()));
        assert_eq!(get_nonce(&env, &signer, domain), 1);

        // Skip-ahead is a replay-protection violation.
        assert_eq!(
            consume_nonce(&env, &signer, domain, 2),
            Err(Error::NonceAlreadyUsed)
        );

        // The rejected attempt does not lock the counter; the correct next nonce still succeeds.
        assert_eq!(consume_nonce(&env, &signer, domain, 1), Ok(()));
        assert_eq!(get_nonce(&env, &signer, domain), 2);
    });
}

/// Edge case test: Nonce zero consumption and initialization across all defined domains.
/// Verifies that nonce 0 is uniformly accepted as the starting state and increments cleanly to 1.
#[test]
fn test_nonce_zero_consumption_all_domains() {
    let env = Env::default();
    let signer = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    let all_domains = [
        DOMAIN_BATCH_CHARGE,
        DOMAIN_ADMIN_ROTATION,
        DOMAIN_OPERATOR_BATCH_CHARGE,
    ];

    env.as_contract(&contract_id, || {
        for domain in all_domains {
            // Initial nonce must be 0
            assert_eq!(get_nonce(&env, &signer, domain), 0);

            // Consuming nonce 0 must succeed
            assert_eq!(consume_nonce(&env, &signer, domain, 0), Ok(()));
            assert_eq!(get_nonce(&env, &signer, domain), 1);

            // Re-consuming nonce 0 must fail
            assert_eq!(
                consume_nonce(&env, &signer, domain, 0),
                Err(Error::NonceAlreadyUsed)
            );
        }
    });
}

/// Happy path: `get_admin_nonce` returns 0 for an uninitialized `(signer, domain)` pair.
#[test]
fn test_get_admin_nonce_uninitialized_returns_zero() {
    let env = Env::default();
    let signer = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        assert_eq!(get_admin_nonce(env.clone(), signer.clone(), DOMAIN_BATCH_CHARGE), 0);
        assert_eq!(get_admin_nonce(env.clone(), signer.clone(), DOMAIN_ADMIN_ROTATION), 0);
        assert_eq!(
            get_admin_nonce(env.clone(), signer.clone(), DOMAIN_OPERATOR_BATCH_CHARGE),
            0
        );
    });
}

/// Happy path: `get_admin_nonce` reflects the counter after `consume_nonce` advances it.
#[test]
fn test_get_admin_nonce_reflects_consumed_nonce() {
    let env = Env::default();
    let signer = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        let domain = DOMAIN_ADMIN_ROTATION;
        assert_eq!(get_admin_nonce(env.clone(), signer.clone(), domain), 0);

        assert_eq!(consume_nonce(&env, &signer, domain, 0), Ok(()));
        assert_eq!(get_admin_nonce(env.clone(), signer.clone(), domain), 1);

        assert_eq!(consume_nonce(&env, &signer, domain, 1), Ok(()));
        assert_eq!(get_admin_nonce(env.clone(), signer.clone(), domain), 2);

        // Cross-check with the internal accessor.
        assert_eq!(get_nonce(&env, &signer, domain), 2);
    });
}

/// Boundary: `get_admin_nonce` is a pure read and must not mutate state across repeated calls.
#[test]
fn test_get_admin_nonce_is_read_only() {
    let env = Env::default();
    let signer = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        let domain = DOMAIN_BATCH_CHARGE;
        let key = DataKey::AdminNonce(signer.clone(), domain);

        // Uninitialized: repeated reads stay at 0 and do not create the key.
        for _ in 0..5 {
            assert_eq!(get_admin_nonce(env.clone(), signer.clone(), domain), 0);
        }
        assert!(!env.storage().persistent().has(&key));

        // After a write, repeated reads return the stored value without advancing it.
        assert_eq!(consume_nonce(&env, &signer, domain, 0), Ok(()));
        for _ in 0..5 {
            assert_eq!(get_admin_nonce(env.clone(), signer.clone(), domain), 1);
        }
        assert_eq!(get_nonce(&env, &signer, domain), 1);
    });
}

/// Boundary: `get_admin_nonce` at `u64::MAX` returns the stored value without overflow.
#[test]
fn test_get_admin_nonce_at_u64_max() {
    let env = Env::default();
    let signer = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        let domain = DOMAIN_OPERATOR_BATCH_CHARGE;
        let key = DataKey::AdminNonce(signer.clone(), domain);
        env.storage().persistent().set(&key, &u64::MAX);

        assert_eq!(get_admin_nonce(env.clone(), signer.clone(), domain), u64::MAX);
        // Reading must not mutate the saturated counter.
        assert_eq!(get_admin_nonce(env.clone(), signer.clone(), domain), u64::MAX);
        assert_eq!(get_nonce(&env, &signer, domain), u64::MAX);
    });
}

/// Isolation: `get_admin_nonce` is scoped per `(signer, domain)` and does not leak across either axis.
#[test]
fn test_get_admin_nonce_signer_and_domain_isolation() {
    let env = Env::default();
    let signer_a = Address::generate(&env);
    let signer_b = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        // Advance signer_a under DOMAIN_BATCH_CHARGE only.
        assert_eq!(consume_nonce(&env, &signer_a, DOMAIN_BATCH_CHARGE, 0), Ok(()));
        assert_eq!(consume_nonce(&env, &signer_a, DOMAIN_BATCH_CHARGE, 1), Ok(()));

        // signer_a / DOMAIN_BATCH_CHARGE is advanced.
        assert_eq!(
            get_admin_nonce(env.clone(), signer_a.clone(), DOMAIN_BATCH_CHARGE),
            2
        );
        // Other domains for signer_a remain untouched.
        assert_eq!(
            get_admin_nonce(env.clone(), signer_a.clone(), DOMAIN_ADMIN_ROTATION),
            0
        );
        assert_eq!(
            get_admin_nonce(env.clone(), signer_a.clone(), DOMAIN_OPERATOR_BATCH_CHARGE),
            0
        );
        // Other signers remain untouched across all domains.
        assert_eq!(
            get_admin_nonce(env.clone(), signer_b.clone(), DOMAIN_BATCH_CHARGE),
            0
        );
        assert_eq!(
            get_admin_nonce(env.clone(), signer_b.clone(), DOMAIN_ADMIN_ROTATION),
            0
        );
        assert_eq!(
            get_admin_nonce(env.clone(), signer_b.clone(), DOMAIN_OPERATOR_BATCH_CHARGE),
            0
        );
    });
}

/// Failure path: a rejected `consume_nonce` (replay) must leave `get_admin_nonce` unchanged.
#[test]
fn test_get_admin_nonce_unchanged_after_rejected_consume() {
    let env = Env::default();
    let signer = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        let domain = DOMAIN_ADMIN_ROTATION;
        assert_eq!(consume_nonce(&env, &signer, domain, 0), Ok(()));
        assert_eq!(get_admin_nonce(env.clone(), signer.clone(), domain), 1);

        // Replay of nonce 0 must be rejected.
        assert_eq!(
            consume_nonce(&env, &signer, domain, 0),
            Err(Error::NonceAlreadyUsed)
        );
        assert_eq!(get_admin_nonce(env.clone(), signer.clone(), domain), 1);

        // Skip-ahead must also be rejected and leave the counter untouched.
        assert_eq!(
            consume_nonce(&env, &signer, domain, 5),
            Err(Error::NonceAlreadyUsed)
        );
        assert_eq!(get_admin_nonce(env.clone(), signer.clone(), domain), 1);

        // Overflow at u64::MAX must be rejected and leave the counter untouched.
        let key = DataKey::AdminNonce(signer.clone(), domain);
        env.storage().persistent().set(&key, &u64::MAX);
        assert_eq!(
            consume_nonce(&env, &signer, domain, u64::MAX),
            Err(Error::Overflow)
        );
        assert_eq!(get_admin_nonce(env.clone(), signer.clone(), domain), u64::MAX);
    });
}

/// Edge case test: Nonce overflow at `u64::MAX`.
/// Verifies that when a nonce reaches `u64::MAX`, any attempt to consume it is rejected with `Error::Overflow`
/// rather than wrapping to zero and reopening replay vulnerabilities.
#[test]
fn test_nonce_max_overflow_domain_isolation() {
    let env = Env::default();
    let signer = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        let domain = DOMAIN_OPERATOR_BATCH_CHARGE;
        let key = DataKey::AdminNonce(signer.clone(), domain);

        // Artificially seed storage with u64::MAX
        env.storage().persistent().set(&key, &u64::MAX);
        assert_eq!(get_nonce(&env, &signer, domain), u64::MAX);

        // Attempting to advance u64::MAX must return Error::Overflow
        assert_eq!(
            consume_nonce(&env, &signer, domain, u64::MAX),
            Err(Error::Overflow)
        );

        // Verify counter did not wrap around to 0
        assert_eq!(get_nonce(&env, &signer, domain), u64::MAX);

        // Verify pure helper math rejects overflow
        assert_eq!(compute_next_nonce(u64::MAX, u64::MAX), Err(Error::Overflow));
    });
}

/// Verifies total independence across all 3 domain constants simultaneously.
#[test]
fn test_all_domains_mutual_independence() {
    let env = Env::default();
    let signer = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    let all_domains = [
        DOMAIN_BATCH_CHARGE,
        DOMAIN_ADMIN_ROTATION,
        DOMAIN_OPERATOR_BATCH_CHARGE,
    ];

    env.as_contract(&contract_id, || {
        // Step 1: Advance each domain by a different number of steps (domain index + 1 times)
        for (idx, &domain) in all_domains.iter().enumerate() {
            for step in 0..=(idx as u64) {
                assert_eq!(consume_nonce(&env, &signer, domain, step), Ok(()));
            }
        }

        // Step 2: Verify each domain has its exact expected counter value (idx + 1)
        for (idx, &domain) in all_domains.iter().enumerate() {
            assert_eq!(get_nonce(&env, &signer, domain), (idx as u64) + 1);
        }
    });
}
