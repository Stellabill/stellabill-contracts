//! Adversarial unit tests for `push_key` in `idempotency.rs`.
//!
//! These tests exercise `push_key` **directly** — without going through the
//! contract client — so that every branch in the ring-buffer write path is
//! observable in isolation.
//!
//! Covered scenarios:
//! - Happy path: single insert into an empty buffer is detectable via `check_key`.
//! - Boundary subscription IDs: 0 and u32::MAX each get their own isolated ring.
//! - Boundary hash values: all-zeros and all-0xFF 32-byte hashes are accepted.
//! - Fill-to-capacity: inserting exactly IDEM_HISTORY keys fills the buffer; all
//!   keys remain detectable.
//! - Ring overwrite on overflow: the (IDEM_HISTORY + 1)-th insert overwrites the
//!   oldest slot; the evicted hash is no longer detectable.
//! - Cursor wrap-around: after IDEM_HISTORY * 2 inserts the cursor has wrapped
//!   twice; each eviction follows circular-buffer order.
//! - Duplicate insert (push is NOT a set): pushing the same hash twice records it
//!   in two slots; the ring still contains it after one further overwrite.
//! - Subscription isolation: keys inserted for subscription A are invisible to B.
//! - State invariant after check_key: calling `check_key` alone never mutates the
//!   ring; a subsequent `push_key` still advances state normally.

use crate::idempotency::{check_key, push_key, IDEM_HISTORY};
use soroban_sdk::{BytesN, Env};

// ─── helpers ────────────────────────────────────────────────────────────────

fn make_hash(env: &Env, val: u8) -> BytesN<32> {
    let mut arr = [0u8; 32];
    arr[31] = val;
    BytesN::from_array(env, &arr)
}

fn all_zeros(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[0u8; 32])
}

fn all_ff(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[0xFFu8; 32])
}

// ─── tests ──────────────────────────────────────────────────────────────────

/// Happy path: inserting a single hash into a fresh buffer makes it detectable.
#[test]
fn push_key_single_insert_is_detectable() {
    let env = Env::default();
    let sub_id: u32 = 1;
    let h = make_hash(&env, 42);

    assert!(
        !check_key(&env, sub_id, &h),
        "ring must be empty before first push"
    );

    push_key(&env, sub_id, &h);

    assert!(
        check_key(&env, sub_id, &h),
        "hash must be detectable after push"
    );
}

/// Boundary: subscription_id = 0 has its own isolated ring.
#[test]
fn push_key_subscription_id_zero() {
    let env = Env::default();
    let h = make_hash(&env, 1);

    push_key(&env, 0, &h);
    assert!(
        check_key(&env, 0, &h),
        "subscription_id=0 should store and detect the hash"
    );
}

/// Boundary: subscription_id = u32::MAX has its own isolated ring.
#[test]
fn push_key_subscription_id_u32_max() {
    let env = Env::default();
    let h = make_hash(&env, 2);

    push_key(&env, u32::MAX, &h);
    assert!(
        check_key(&env, u32::MAX, &h),
        "subscription_id=u32::MAX should store and detect the hash"
    );
}

/// Boundary hash: all-zeros 32-byte value is stored and detected.
#[test]
fn push_key_all_zeros_hash() {
    let env = Env::default();
    let h = all_zeros(&env);

    push_key(&env, 10, &h);
    assert!(
        check_key(&env, 10, &h),
        "all-zeros hash must be detectable after push"
    );
}

/// Boundary hash: all-0xFF 32-byte value is stored and detected.
#[test]
fn push_key_all_ff_hash() {
    let env = Env::default();
    let h = all_ff(&env);

    push_key(&env, 11, &h);
    assert!(
        check_key(&env, 11, &h),
        "all-0xFF hash must be detectable after push"
    );
}

/// Fill exactly IDEM_HISTORY entries: every inserted hash is still detectable.
#[test]
fn push_key_fill_to_capacity_all_detectable() {
    let env = Env::default();
    let sub_id: u32 = 20;

    for i in 0..IDEM_HISTORY {
        let h = make_hash(&env, i as u8);
        push_key(&env, sub_id, &h);
    }

    for i in 0..IDEM_HISTORY {
        let h = make_hash(&env, i as u8);
        assert!(
            check_key(&env, sub_id, &h),
            "hash {i} must still be detectable at capacity"
        );
    }
}

/// Ring overwrite: the (IDEM_HISTORY + 1)-th push evicts the oldest entry (index 0).
#[test]
fn push_key_overwrite_evicts_oldest() {
    let env = Env::default();
    let sub_id: u32 = 30;

    // Fill to capacity.
    for i in 0..IDEM_HISTORY {
        push_key(&env, sub_id, &make_hash(&env, i as u8));
    }

    // One more insert: slot 0 is overwritten by the new hash (0xFF).
    let new_hash = all_ff(&env);
    push_key(&env, sub_id, &new_hash);

    // New hash must be detectable.
    assert!(
        check_key(&env, sub_id, &new_hash),
        "the new hash must be detectable after overwrite"
    );

    // Hash at original slot 0 (make_hash(0)) must have been evicted.
    let evicted = make_hash(&env, 0);
    assert!(
        !check_key(&env, sub_id, &evicted),
        "hash at slot 0 must be evicted after the first overwrite"
    );

    // Hashes at slots 1..IDEM_HISTORY-1 must still be present.
    for i in 1..IDEM_HISTORY {
        let h = make_hash(&env, i as u8);
        assert!(
            check_key(&env, sub_id, &h),
            "hash {i} must not have been evicted yet"
        );
    }
}

/// Cursor wrap-around: after inserting IDEM_HISTORY * 2 distinct keys, the
/// overwrite order follows the circular cursor — slot 0 is overwritten first,
/// then slot 1, and so on.
#[test]
fn push_key_cursor_wraps_twice() {
    let env = Env::default();
    let sub_id: u32 = 40;

    // Insert 2 * IDEM_HISTORY unique hashes. The first IDEM_HISTORY fill the
    // buffer; the second IDEM_HISTORY overwrite each slot in order.
    for i in 0..(IDEM_HISTORY * 2) {
        push_key(&env, sub_id, &make_hash(&env, i as u8));
    }

    // After 2 * IDEM_HISTORY inserts the ring contains indices
    // [IDEM_HISTORY .. 2*IDEM_HISTORY - 1].
    for i in 0..IDEM_HISTORY {
        let evicted = make_hash(&env, i as u8);
        assert!(
            !check_key(&env, sub_id, &evicted),
            "first-round hash {i} must be evicted after full second pass"
        );

        let live = make_hash(&env, (IDEM_HISTORY + i) as u8);
        assert!(
            check_key(&env, sub_id, &live),
            "second-round hash {} must still be present",
            IDEM_HISTORY + i
        );
    }
}

/// Duplicate push: pushing the same hash TWICE occupies two slots.
/// After the ring evicts the first occurrence, the second occurrence keeps
/// the hash detectable; only after both slots are overwritten does it vanish.
#[test]
fn push_key_duplicate_occupies_two_slots() {
    let env = Env::default();
    let sub_id: u32 = 50;

    let dup = make_hash(&env, 77);

    // Push the duplicate hash into slot 0 and slot 1.
    push_key(&env, sub_id, &dup); // slot 0
    push_key(&env, sub_id, &dup); // slot 1

    // Fill slots 2..IDEM_HISTORY-1 with distinct hashes to reach capacity.
    for i in 2..IDEM_HISTORY {
        push_key(&env, sub_id, &make_hash(&env, i as u8));
    }

    // Now overwrite slot 0 with a fresh hash. The duplicate in slot 1 should
    // keep the hash detectable.
    let fresh_a = make_hash(&env, 200);
    push_key(&env, sub_id, &fresh_a); // overwrites slot 0

    assert!(
        check_key(&env, sub_id, &dup),
        "dup hash must still be detectable via slot 1 after slot 0 is overwritten"
    );

    // Overwrite slot 1 as well.
    let fresh_b = make_hash(&env, 201);
    push_key(&env, sub_id, &fresh_b); // overwrites slot 1

    assert!(
        !check_key(&env, sub_id, &dup),
        "dup hash must NOT be detectable after both slots are overwritten"
    );
}

/// Subscription isolation: inserting a hash for subscription A does not affect
/// the ring for subscription B.
#[test]
fn push_key_subscriptions_are_isolated() {
    let env = Env::default();
    let h = make_hash(&env, 55);

    push_key(&env, 100, &h);

    assert!(
        check_key(&env, 100, &h),
        "hash must be in ring for sub 100"
    );
    assert!(
        !check_key(&env, 101, &h),
        "hash must NOT be in ring for sub 101"
    );
    assert!(
        !check_key(&env, 0, &h),
        "hash must NOT be in ring for sub 0"
    );
}

/// State invariant: calling `check_key` alone never mutates the ring.
/// A subsequent `push_key` still works correctly after a read-only check.
#[test]
fn check_key_does_not_mutate_ring() {
    let env = Env::default();
    let sub_id: u32 = 60;
    let absent = make_hash(&env, 88);

    // Checking an absent key on an empty ring must return false without
    // mutating storage; a later push must still be observable.
    let result = check_key(&env, sub_id, &absent);
    assert!(!result, "absent hash must not be detected in empty ring");

    let h = make_hash(&env, 99);
    push_key(&env, sub_id, &h);
    assert!(
        check_key(&env, sub_id, &h),
        "push after a no-op check must be detectable"
    );

    // The ring holds exactly one entry; `absent` must still be absent.
    assert!(
        !check_key(&env, sub_id, &absent),
        "unrelated hash must not appear after an unrelated push"
    );
}

/// Determinism: inserting the same sequence of hashes twice on two independent
/// Env instances produces identical visibility for every hash.
#[test]
fn push_key_deterministic_across_envs() {
    let hashes_to_push: u32 = IDEM_HISTORY + 2; // intentionally exceeds capacity
    let sub_id: u32 = 70;

    let env_a = Env::default();
    let env_b = Env::default();

    for i in 0..hashes_to_push {
        push_key(&env_a, sub_id, &make_hash(&env_a, i as u8));
        push_key(&env_b, sub_id, &make_hash(&env_b, i as u8));
    }

    // Both envs must agree on which hashes are present or absent.
    for i in 0..hashes_to_push {
        let h_a = make_hash(&env_a, i as u8);
        let h_b = make_hash(&env_b, i as u8);
        assert_eq!(
            check_key(&env_a, sub_id, &h_a),
            check_key(&env_b, sub_id, &h_b),
            "hash visibility must be deterministic at index {i}"
        );
    }
}

/// Cursor position after N pushes: cursor always equals N % IDEM_HISTORY.
/// We verify this indirectly by confirming which slot gets overwritten on
/// the (IDEM_HISTORY + k)-th push for several values of k.
#[test]
fn push_key_cursor_position_matches_eviction_order() {
    let env = Env::default();
    let sub_id: u32 = 80;

    // Phase 1: fill the buffer with hashes 0..IDEM_HISTORY-1.
    for i in 0..IDEM_HISTORY {
        push_key(&env, sub_id, &make_hash(&env, i as u8));
    }

    // Phase 2: each additional push should evict the slot at index k
    // (i.e. the hash inserted k steps back, where k = 0, 1, 2, …).
    for k in 0..IDEM_HISTORY {
        // Insert a fresh hash that takes slot k (wrapping cursor).
        let fresh = make_hash(&env, (100 + k) as u8);
        push_key(&env, sub_id, &fresh);

        // The original hash at position k must now be evicted.
        let evicted = make_hash(&env, k as u8);
        assert!(
            !check_key(&env, sub_id, &evicted),
            "original hash at slot {k} must be evicted after push #{k}"
        );

        // The fresh hash must be present.
        assert!(
            check_key(&env, sub_id, &fresh),
            "fresh hash at slot {k} must be detectable"
        );
    }
}
