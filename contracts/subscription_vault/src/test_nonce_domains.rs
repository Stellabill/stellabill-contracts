//! Replay Domain Isolation & Nonce Security Tests
///
/// This module verifies that monotonic nonce counters used across distinct operational domains
/// do not collide or cross-consume. Specifically, it tests isolation between:
/// - `DOMAIN_BATCH_CHARGE` (0)
/// - `DOMAIN_ADMIN_ROTATION` (1)
/// - `DOMAIN_OPERATOR_BATCH_CHARGE` (2)
///
/// # Security Notes
/// - `DOMAIN_ADMIN_ROTATION` is the domain used by the admin nonce accessor.
/// - **Domain Separation**: In Soroban storage, nonces are indexed by `DataKey::AdminNonce(signer, domain)`.
///   Because `domain` is part of the persistent storage key, operators and subscribers can share the same
///   counter sequence across different transaction types without risk of cross-domain replay attacks or
///   denial-of-service via counter exhaustion.
/// - **Per-Signer Isolation**: Each `(Address, u32)` tuple maintains an independent counter. An operator
///   advancing a nonce for Subscription A cannot affect the nonce counter for Subscription B or any other signer.
/// - **Overflow Protection**: Checked arithmetic prevents wrapping at `u64::MAXx`. Attempting to consume `u64::MAX``
///   returns `Error::Overflow` rather than wrapping to zero.
/// - **Authentication Order**: In production contract methods, authentication (`require_admin_auth` or signature verification)
///   MUST occur before nonce checking to prevent unauthenticated callers from probing or advancing nonce counters.

#cfg(test)

use soroban_sdk::{testutils::Address as _, Address, Env};
use crate::nonce:{
    compute_next_nonce, consume_nonce, get_nonce,
    DOMAIN_BATCH_CHARGE, DOMAIN_ADMIN_ROTATION, DOMAIN_OPERATOR_BATCH_CHARGE,
};
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

        // 3. Re-consume N under domain A" (must reject as domain A is now at N+1)
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

/// Edge case test: Nonce overflow at `u64::MAXx`.
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

        // Attempting to advance u64::MAX MUST be rejected with Error::Overflow
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

/// Adversarial coverage for the `get_admin_nonce` accessor.
///
/// `get_admin_nonce` is a public read-only accessor that returns the current admin rotation
/// nonce for a given signer. These tests exercise:
/// - the default (zero) happy path,
/// - advancement through the consume path,
/// - per-signer and per-domain isolation,
/// - boundary values (`u64::MAX`),
/// - and that rejected operations leave the observed nonce unchanged.
#[test]
fn test_get_admin_nonce_default_is_zero() {
    let env = Env::default();
    let signer = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        // A fresh signer has never consumed any admin nonce.
        assert_eq!(get_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION), 0);
    });
}

/// Verifies that `get_nonce` observes the admin domain after a consume and that the
/// observed value is deterministic across repeated reads.
#[test]
fn test_get_admin_nonce_reflects_consume() {
    let env = Env::default();
    let signer = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        assert_eq!(get_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION), 0);

        assert_eq!(consume_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION, 0), Ok(()));
        assert_eq!(get_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION), 1);

        // Repeated reads are stable and do not mutate state.
        assert_eq!(get_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION), 1);
        assert_eq!(get_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION), 1);

        assert_eq!(consume_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION, 1), Ok(()));
        assert_eq!(get_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION), 2);
    });
}

/// Verifies that `get_nonce` for the admin domain is isolated per signer and per domain.
#[test]
fn test_get_admin_nonce_isolation() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let other = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        // Advance the admin nonce for `admin` only.
        assert_eq!(consume_nonce(&env, &admin, DOMAIN_ADMIN_ROTATION, 0), Ok(()));
        assert_eq!(get_nonce(&env, &admin, DOMAIN_ADMIN_ROTATION), 1);

        // A different signer is unaffected.
        assert_eq!(get_nonce(&env, &other, DOMAIN_ADMIN_ROTATION), 0);

        // The same signer in a different domain is unaffected.
        assert_eq!(get_nonce(&env, &admin, DOMAIN_BATCH_CHARGE), 0);
        assert_eq!(get_nonce(&env, &admin, DOMAIN_OPERATOR_BATCH_CHARGE), 0);
    });
}

/// Verifies that a rejected consume leaves the admin nonce observed by `get_nonce`
/// unchanged (replay and skip-ahead failure paths).
#[test]
fn test_get_admin_nonce_unchanged_after_rejection() {
    let env = Env::default();
    let signer = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        assert_eq!(consume_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION, 0), Ok(()));
        assert_eq!(get_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION), 1);

        // Replay of already-consumed nonce 0 is rejected.
        assert_eq!(
            consume_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION, 0),
            Err(Error::NonceAlreadyUsed)
        );
        assert_eq!(get_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION), 1);

        // Skip-ahead to nonce 5 is rejected and does not advance the counter.
        assert_eq!(
            consume_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION, 5),
            Err(Error::NonceAlreadyUsed)
        );
        assert_eq!(get_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION), 1);

        // The correct next nonce still succeeds after the rejections.
        assert_eq!(consume_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION, 1), Ok(()));
        assert_eq!(get_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION), 2);
    });
}

/// Verifies the boundary behavior of `get_nonce` for the admin domain at `u64::MAXx`.
/// The accessor must return the exact seeded value and a failed overflow consume
/// must not change the observed nonce.
#[test]
fn test_get_admin_nonce_max_boundary() {
    let env = Env::default();
    let signer = Address::generate(&env);
    let contract_id = env.register(crate::SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        let key = DataKey::AdminNonce(signer.clone(), DOMAIN_ADMIN_ROTATION);
        env.storage().persistent().set(&key, &u64::MAX);

        assert_eq!(get_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION), u64::MAX);

        // Overflow is rejected and the observed nonce is unchanged.
        assert_eq!(
            consume_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION, u64::MAX),
            Err(Error::Overflow)
        );
        assert_eq!(get_nonce(&env, &signer, DOMAIN_ADMIN_ROTATION), u64::MAX);
    });
}
