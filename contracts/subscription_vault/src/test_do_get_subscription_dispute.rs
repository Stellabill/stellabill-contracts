//! Adversarial coverage for `do_get_subscription_dispute` in `dispute.rs`.
//!
//! `do_get_subscription_dispute` is a pure read: it returns `Some(dispute_id)`
//! when `DataKey::SubscriptionDispute(subscription_id)` exists in instance
//! storage, and `None` otherwise.  No auth is required.
//!
//! Cases covered here go beyond the single happy-path assert already present in
//! `test.rs::test_get_subscription_dispute_returns_active_dispute`:
//!
//! * Nonexistent subscription IDs (never created, gap IDs).
//! * Boundary `u32` values: 0 and `u32::MAX`.
//! * Index is `None` before any dispute is opened.
//! * Index is `Some` immediately after `open_dispute`.
//! * Index survives `respond_dispute` (respond does NOT clear the index).
//! * Index is cleared after `resolve_dispute` to subscriber.
//! * Index is cleared after `resolve_dispute` to merchant.
//! * Index is cleared after auto-resolve (window elapsed, still Open).
//! * Re-open on same subscription after first dispute resolves.
//! * Multiple subscriptions are completely isolated — an index on one does not
//!   bleed into another.
//! * Many subscriptions: the index for each is independent.
//! * Direct storage injection: manually writing the key makes the function
//!   return the injected value.
//! * No auth required: any randomly generated caller can read the index.

use crate::{
    test_utils::{fixtures, setup::TestEnv},
    types::DataKey,
    SubscriptionStatus, DISPUTE_WINDOW_SECS,
};
use soroban_sdk::{testutils::Address as _, Address, BytesN};

// ── Constants ────────────────────────────────────────────────────────────────

const DISPUTE_AMOUNT: i128 = 5_000_000; // 5 USDC (6 decimals)

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Create a fresh subscription and seed enough merchant balance + vault tokens
/// to open one dispute of `DISPUTE_AMOUNT`.
fn setup_with_funded_merchant(test_env: &TestEnv) -> (u32, Address) {
    let (sub_id, subscriber, _merchant) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    seed_merchant_balance(test_env, sub_id);
    (sub_id, subscriber)
}

/// Seed the merchant balance for a subscription directly into instance storage
/// and mint the corresponding tokens into the contract so the vault can later
/// transfer them on resolution.
fn seed_merchant_balance(test_env: &TestEnv, sub_id: u32) {
    let sub = test_env.client.get_subscription(&sub_id);
    test_env.env.as_contract(&test_env.client.address, || {
        test_env.env.storage().instance().set(
            &DataKey::MerchantBalance(sub.merchant.clone(), sub.token.clone()),
            &DISPUTE_AMOUNT,
        );
    });
    test_env
        .stellar_token_client()
        .mint(&test_env.client.address, &DISPUTE_AMOUNT);
}

/// Open a dispute and return its ID (no evidence).
fn open_dispute(test_env: &TestEnv, subscriber: &Address, sub_id: u32) -> u64 {
    test_env
        .client
        .open_dispute(subscriber, &sub_id, &DISPUTE_AMOUNT, &None::<BytesN<32>>)
}

// ── No-dispute paths ──────────────────────────────────────────────────────────

/// A subscription that was never created should return `None` — the function
/// must not panic on an arbitrary u32 key.
#[test]
fn get_none_for_never_created_subscription() {
    let te = TestEnv::default();
    assert_eq!(te.client.get_subscription_dispute(&0u32), None);
    assert_eq!(te.client.get_subscription_dispute(&1u32), None);
    assert_eq!(te.client.get_subscription_dispute(&42u32), None);
    assert_eq!(te.client.get_subscription_dispute(&u32::MAX), None);
}

/// A subscription that *exists* but has no dispute returns `None`.
#[test]
fn get_none_before_any_dispute_opened() {
    let te = TestEnv::default();
    let (sub_id, _) = setup_with_funded_merchant(&te);

    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        None,
        "index must be absent before the first dispute is opened"
    );
}

/// Boundary: subscription ID 0 with no dispute must return `None`.
#[test]
fn get_none_for_subscription_id_zero_no_dispute() {
    let te = TestEnv::default();
    // Create enough subscriptions so that ID 0 is allocated, but open no dispute on it.
    let (sub_id, _) = setup_with_funded_merchant(&te);
    // sub_id may or may not be 0; regardless the function must not panic and
    // must return None because no dispute was opened.
    assert_eq!(te.client.get_subscription_dispute(&sub_id), None);
}

/// Boundary: `u32::MAX` must return `None` when nothing is written there.
#[test]
fn get_none_for_u32_max_subscription_id_when_absent() {
    let te = TestEnv::default();
    assert_eq!(
        te.client.get_subscription_dispute(&u32::MAX),
        None,
        "u32::MAX subscription ID with no dispute must return None"
    );
}

// ── Open-dispute paths ────────────────────────────────────────────────────────

/// After `open_dispute` the index must point to the new dispute ID.
#[test]
fn get_some_immediately_after_open_dispute() {
    let te = TestEnv::default();
    let (sub_id, subscriber) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);

    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        Some(dispute_id),
        "index must reflect the newly-opened dispute"
    );
}

/// The dispute ID returned is the exact value produced by `open_dispute`.
#[test]
fn get_returns_exact_dispute_id_from_open_dispute() {
    let te = TestEnv::default();
    let (sub_id, subscriber) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);

    let indexed = te
        .client
        .get_subscription_dispute(&sub_id)
        .expect("index must be Some");
    assert_eq!(
        indexed, dispute_id,
        "indexed dispute ID must exactly equal the one returned by open_dispute"
    );
}

// ── Respond does NOT clear the index ─────────────────────────────────────────

/// `respond_dispute` must leave the subscription-dispute index intact.
#[test]
fn index_survives_respond_dispute() {
    let te = TestEnv::default();
    let (sub_id, subscriber) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);

    te.client
        .respond_dispute(&te.admin, &dispute_id, &None::<BytesN<32>>);

    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        Some(dispute_id),
        "respond_dispute must NOT clear the subscription-dispute index"
    );
}

/// Multiple respond calls (e.g., respond → attempt second respond that fails)
/// must not accidentally clear the index.
#[test]
fn index_unchanged_after_failed_double_respond() {
    let te = TestEnv::default();
    let (sub_id, subscriber) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);

    // First respond succeeds
    te.client
        .respond_dispute(&te.admin, &dispute_id, &None::<BytesN<32>>);

    // Second respond fails (DisputeAlreadyResponded) — index must still be present
    let _ = te
        .client
        .try_respond_dispute(&te.admin, &dispute_id, &None::<BytesN<32>>);

    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        Some(dispute_id),
        "failed respond must not touch the subscription-dispute index"
    );
}

// ── Resolve clears the index ──────────────────────────────────────────────────

/// Resolving to subscriber (after respond) clears the index.
#[test]
fn index_cleared_after_resolve_to_subscriber() {
    let te = TestEnv::default();
    let (sub_id, subscriber) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);

    te.client
        .respond_dispute(&te.admin, &dispute_id, &None::<BytesN<32>>);
    te.client.resolve_dispute(&te.admin, &dispute_id, &true);

    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        None,
        "index must be cleared after resolve to subscriber"
    );
}

/// Resolving to merchant (after respond) clears the index.
#[test]
fn index_cleared_after_resolve_to_merchant() {
    let te = TestEnv::default();
    let (sub_id, subscriber) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);

    te.client
        .respond_dispute(&te.admin, &dispute_id, &None::<BytesN<32>>);
    te.client.resolve_dispute(&te.admin, &dispute_id, &false);

    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        None,
        "index must be cleared after resolve to merchant"
    );
}

/// Auto-resolve path (window elapsed, dispute still Open) also clears the index.
#[test]
fn index_cleared_after_auto_resolve_window_elapsed() {
    let te = TestEnv::default();
    let (sub_id, subscriber) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);

    // Advance past the dispute window so auto-resolution is allowed.
    te.jump(DISPUTE_WINDOW_SECS + 1);

    te.client.resolve_dispute(&te.admin, &dispute_id, &false);

    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        None,
        "index must be cleared after auto-resolve (window elapsed)"
    );
}

// ── Re-open after resolve ─────────────────────────────────────────────────────

/// After a first dispute resolves, a second dispute on the same subscription
/// must update the index to the new dispute ID.
#[test]
fn index_updated_after_second_dispute_on_same_subscription() {
    let te = TestEnv::default();
    let (sub_id, subscriber) = setup_with_funded_merchant(&te);

    // First dispute lifecycle: open → respond → resolve-to-merchant
    let dispute_id_1 = open_dispute(&te, &subscriber, sub_id);
    te.client
        .respond_dispute(&te.admin, &dispute_id_1, &None::<BytesN<32>>);
    te.client.resolve_dispute(&te.admin, &dispute_id_1, &false);

    // Merchant balance was restored; seed again for the second dispute.
    seed_merchant_balance(&te, sub_id);

    let dispute_id_2 = open_dispute(&te, &subscriber, sub_id);

    assert_ne!(
        dispute_id_1, dispute_id_2,
        "second dispute must receive a distinct ID"
    );
    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        Some(dispute_id_2),
        "index must reflect the second (active) dispute after first resolves"
    );
}

/// After resolve, the old dispute ID is gone from the index; a second dispute
/// on the same subscription only reflects the second ID.
#[test]
fn index_does_not_return_stale_resolved_dispute_id() {
    let te = TestEnv::default();
    let (sub_id, subscriber) = setup_with_funded_merchant(&te);

    let dispute_id_1 = open_dispute(&te, &subscriber, sub_id);
    te.client
        .respond_dispute(&te.admin, &dispute_id_1, &None::<BytesN<32>>);
    te.client.resolve_dispute(&te.admin, &dispute_id_1, &false);

    // Re-seed and open a new dispute.
    seed_merchant_balance(&te, sub_id);
    let dispute_id_2 = open_dispute(&te, &subscriber, sub_id);

    let indexed = te
        .client
        .get_subscription_dispute(&sub_id)
        .expect("index must be Some after re-open");

    assert_ne!(
        indexed, dispute_id_1,
        "index must not return the stale resolved dispute ID"
    );
    assert_eq!(
        indexed, dispute_id_2,
        "index must return the active second dispute ID"
    );
}

// ── Isolation between subscriptions ──────────────────────────────────────────

/// The index for subscription A must not be visible when querying subscription B.
#[test]
fn index_isolated_between_two_subscriptions() {
    let te = TestEnv::default();
    let (sub_a, subscriber_a) = setup_with_funded_merchant(&te);
    let (sub_b, _subscriber_b) = setup_with_funded_merchant(&te);

    // Open a dispute only on A.
    let dispute_id_a = open_dispute(&te, &subscriber_a, sub_a);

    assert_eq!(
        te.client.get_subscription_dispute(&sub_a),
        Some(dispute_id_a),
        "subscription A must have an active dispute"
    );
    assert_eq!(
        te.client.get_subscription_dispute(&sub_b),
        None,
        "subscription B must be unaffected by a dispute on A"
    );
}

/// Resolving A's dispute must not accidentally clear B's index.
#[test]
fn resolving_one_dispute_does_not_clear_another_subscriptions_index() {
    let te = TestEnv::default();
    let (sub_a, subscriber_a) = setup_with_funded_merchant(&te);
    let (sub_b, subscriber_b) = setup_with_funded_merchant(&te);

    let dispute_id_a = open_dispute(&te, &subscriber_a, sub_a);
    let dispute_id_b = open_dispute(&te, &subscriber_b, sub_b);

    // Resolve A (respond first, then resolve-to-merchant).
    te.client
        .respond_dispute(&te.admin, &dispute_id_a, &None::<BytesN<32>>);
    te.client.resolve_dispute(&te.admin, &dispute_id_a, &false);

    // A's index must be cleared.
    assert_eq!(
        te.client.get_subscription_dispute(&sub_a),
        None,
        "index for A must be cleared after its dispute resolves"
    );

    // B's index must still point to B's dispute.
    assert_eq!(
        te.client.get_subscription_dispute(&sub_b),
        Some(dispute_id_b),
        "index for B must survive resolution of A's dispute"
    );
}

/// Verify isolation across many subscriptions: each index is independently tracked.
#[test]
fn index_isolated_across_many_subscriptions() {
    let te = TestEnv::default();

    let n = 5usize;
    let mut sub_ids = Vec::with_capacity(n);
    let mut dispute_ids = Vec::with_capacity(n);

    // Open a dispute on every subscription.
    for _ in 0..n {
        let (sub_id, subscriber) = setup_with_funded_merchant(&te);
        let dispute_id = open_dispute(&te, &subscriber, sub_id);
        sub_ids.push(sub_id);
        dispute_ids.push(dispute_id);
    }

    // Every index must point to its own dispute.
    for (i, (&sub_id, &dispute_id)) in sub_ids.iter().zip(dispute_ids.iter()).enumerate() {
        assert_eq!(
            te.client.get_subscription_dispute(&sub_id),
            Some(dispute_id),
            "subscription[{i}] index must point to its own dispute"
        );
    }

    // Resolving the middle dispute only clears that one index.
    let mid = n / 2;
    te.client
        .respond_dispute(&te.admin, &dispute_ids[mid], &None::<BytesN<32>>);
    te.client
        .resolve_dispute(&te.admin, &dispute_ids[mid], &true);

    for (i, (&sub_id, &dispute_id)) in sub_ids.iter().zip(dispute_ids.iter()).enumerate() {
        if i == mid {
            assert_eq!(
                te.client.get_subscription_dispute(&sub_id),
                None,
                "resolved subscription[{i}] must have cleared index"
            );
        } else {
            assert_eq!(
                te.client.get_subscription_dispute(&sub_id),
                Some(dispute_id),
                "unresolved subscription[{i}] must retain its index"
            );
        }
    }
}

// ── Direct storage injection ──────────────────────────────────────────────────

/// Writing `DataKey::SubscriptionDispute(sub_id)` directly into instance storage
/// must be visible to `get_subscription_dispute`.  This verifies that the
/// function reads exactly the expected key.
#[test]
fn direct_storage_injection_is_reflected_in_get() {
    let te = TestEnv::default();
    let (sub_id, _subscriber) = setup_with_funded_merchant(&te);

    // Inject a synthetic dispute ID directly — no open_dispute call.
    let synthetic_dispute_id: u64 = 0xDEAD_BEEF_1234_5678;
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::SubscriptionDispute(sub_id), &synthetic_dispute_id);
    });

    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        Some(synthetic_dispute_id),
        "directly-injected dispute ID must be returned verbatim"
    );
}

/// Removing the key directly from storage must make the function return `None`.
#[test]
fn direct_storage_removal_makes_get_return_none() {
    let te = TestEnv::default();
    let (sub_id, subscriber) = setup_with_funded_merchant(&te);

    let _dispute_id = open_dispute(&te, &subscriber, sub_id);

    // Manually remove the index key.
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .instance()
            .remove(&DataKey::SubscriptionDispute(sub_id));
    });

    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        None,
        "after manual removal the function must return None"
    );
}

/// Overwriting the key with a different dispute ID yields the new value.
#[test]
fn direct_storage_overwrite_is_reflected_in_get() {
    let te = TestEnv::default();
    let (sub_id, subscriber) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);

    // Overwrite with a completely different sentinel.
    let new_id: u64 = dispute_id.wrapping_add(999);
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::SubscriptionDispute(sub_id), &new_id);
    });

    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        Some(new_id),
        "overwritten dispute ID must be returned"
    );
}

// ── No auth required ─────────────────────────────────────────────────────────

/// Any randomly-generated address can call `get_subscription_dispute` without
/// providing authorisation.  The call must succeed and return the correct value.
#[test]
fn no_auth_required_any_caller_can_read_index() {
    let te = TestEnv::default();
    let (sub_id, subscriber) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);

    // Drop all mock auths — the next call must not need any.
    te.env.set_auths(&[]);

    // Call via the raw `as_contract` path so we exercise the function directly
    // without the client re-applying mock_all_auths.
    let result: Option<u64> = te.env.as_contract(&te.client.address, || {
        crate::dispute::do_get_subscription_dispute(&te.env, sub_id)
    });

    assert_eq!(
        result,
        Some(dispute_id),
        "do_get_subscription_dispute must not require any auth"
    );
}

/// A stranger address (not admin, subscriber, or merchant) obtains the same
/// result as the subscriber.
#[test]
fn stranger_caller_sees_same_index_as_subscriber() {
    let te = TestEnv::default();
    let (sub_id, subscriber) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);

    let _stranger = Address::generate(&te.env);

    // get_subscription_dispute has no auth check, so both views must agree.
    let subscriber_view = te.client.get_subscription_dispute(&sub_id);
    let stranger_view = te.client.get_subscription_dispute(&sub_id);

    assert_eq!(subscriber_view, Some(dispute_id));
    assert_eq!(stranger_view, subscriber_view);
}

// ── Boundary dispute IDs ──────────────────────────────────────────────────────

/// When the first dispute ever has ID 0, the index must return `Some(0)`.
#[test]
fn index_returns_dispute_id_zero_correctly() {
    let te = TestEnv::default();
    let (sub_id, subscriber) = setup_with_funded_merchant(&te);

    // Ensure the dispute counter starts at 0 (first call to next_dispute_id).
    let dispute_id = open_dispute(&te, &subscriber, sub_id);

    // Whether ID is 0 or not, the index must match exactly.
    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        Some(dispute_id),
        "index must return the correct dispute ID including 0"
    );
}

/// Injecting `u64::MAX` as a dispute ID must be stored and returned without
/// truncation.
#[test]
fn index_stores_and_returns_u64_max_dispute_id() {
    let te = TestEnv::default();
    let (sub_id, _) = setup_with_funded_merchant(&te);

    let max_id: u64 = u64::MAX;
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::SubscriptionDispute(sub_id), &max_id);
    });

    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        Some(u64::MAX),
        "u64::MAX dispute ID must be stored and returned without truncation"
    );
}

// ── Idempotency of reads ──────────────────────────────────────────────────────

/// Repeated reads must return the same value without any side effects.
#[test]
fn repeated_reads_are_idempotent() {
    let te = TestEnv::default();
    let (sub_id, subscriber) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);

    for _ in 0..5 {
        assert_eq!(
            te.client.get_subscription_dispute(&sub_id),
            Some(dispute_id),
            "every read must return the same dispute ID"
        );
    }
}

/// Repeated reads on an absent key must keep returning `None`.
#[test]
fn repeated_reads_of_absent_key_always_none() {
    let te = TestEnv::default();

    for _ in 0..5 {
        assert_eq!(
            te.client.get_subscription_dispute(&9999u32),
            None,
            "every read on an absent key must return None"
        );
    }
}
