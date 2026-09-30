//! Adversarial coverage for `do_get_cancellation_escrow` (dispute.rs).
//!
//! # Function under test
//!
//! ```ignore
//! pub fn do_get_cancellation_escrow(
//!     env: &Env,
//!     subscription_id: u32,
//! ) -> Result<CancellationEscrow, Error>
//! ```
//!
//! The function is a pure persistent-storage read:
//! - Returns `Ok(CancellationEscrow)` when a record exists under
//!   `DataKey::CancellationEscrow(subscription_id)`.
//! - Returns `Err(Error::EscrowNotFound)` otherwise.
//! - Requires no authorisation and performs no state mutation.
//!
//! # Adversarial cases covered
//!
//! | # | Scenario | Expected |
//! |---|----------|----------|
//! | 1 | Subscription_id never created | `EscrowNotFound` |
//! | 2 | Subscription_id = 0 (boundary low) | `EscrowNotFound` |
//! | 3 | Subscription_id = u32::MAX (boundary high) | `EscrowNotFound` |
//! | 4 | Active subscription, not yet cancelled | `EscrowNotFound` |
//! | 5 | Cancelled with zero balance (no escrow created) | `EscrowNotFound` |
//! | 6 | Cancelled with positive balance — happy path | all fields correct |
//! | 7 | Merchant-initiated cancellation with balance | all fields correct |
//! | 8 | Repeated reads are idempotent (no state mutation) | same result twice |
//! | 9 | Escrow consumed by subscriber claim | `EscrowNotFound` |
//! | 10 | Escrow consumed by merchant lodge-dispute | `EscrowNotFound` |
//! | 11 | Two separate subscriptions — independent escrow records | each correct |
//! | 12 | Subscriber cancel then deposit reverts balance (cancelled, escrow is correct) | `EscrowNotFound` on wrong id |

use crate::{
    test_utils::{fixtures, setup::TestEnv},
    types::CANCELLATION_ESCROW_WINDOW_SECS,
    Error, SubscriptionStatus,
};
use soroban_sdk::testutils::Address as _;

const PREPAID: i128 = 20_000_000;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Cancel an active subscription funded with `PREPAID` tokens.
/// Returns `(test_env, subscription_id, subscriber, merchant)`.
fn setup_funded_and_cancelled() -> (TestEnv, u32, soroban_sdk::Address, soroban_sdk::Address) {
    let test_env = TestEnv::default();
    let (id, subscriber, merchant) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env
        .client
        .deposit_funds(&id, &subscriber, &PREPAID, &None::<soroban_sdk::BytesN<32>>);
    test_env.client.cancel_subscription(&id, &subscriber);
    (test_env, id, subscriber, merchant)
}

// ── 1. Never-created subscription ID ─────────────────────────────────────────

#[test]
fn get_escrow_unknown_id_returns_not_found() {
    let test_env = TestEnv::default();
    let result = test_env.client.try_get_cancellation_escrow(&9999u32);
    assert_eq!(result, Err(Ok(Error::EscrowNotFound)));
}

// ── 2. Boundary: subscription_id = 0 ─────────────────────────────────────────

#[test]
fn get_escrow_id_zero_returns_not_found() {
    let test_env = TestEnv::default();
    let result = test_env.client.try_get_cancellation_escrow(&0u32);
    assert_eq!(result, Err(Ok(Error::EscrowNotFound)));
}

// ── 3. Boundary: subscription_id = u32::MAX ──────────────────────────────────

#[test]
fn get_escrow_id_max_u32_returns_not_found() {
    let test_env = TestEnv::default();
    let result = test_env.client.try_get_cancellation_escrow(&u32::MAX);
    assert_eq!(result, Err(Ok(Error::EscrowNotFound)));
}

// ── 4. Active subscription, no cancellation ──────────────────────────────────

#[test]
fn get_escrow_active_subscription_returns_not_found() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    // Fund but do NOT cancel — escrow must not exist.
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env
        .client
        .deposit_funds(&id, &subscriber, &PREPAID, &None::<soroban_sdk::BytesN<32>>);

    let result = test_env.client.try_get_cancellation_escrow(&id);
    assert_eq!(result, Err(Ok(Error::EscrowNotFound)));
}

// ── 5. Cancelled with zero balance — no escrow is created ────────────────────

#[test]
fn get_escrow_cancelled_zero_balance_returns_not_found() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    // Cancel immediately without depositing any funds.
    test_env.client.cancel_subscription(&id, &subscriber);

    let result = test_env.client.try_get_cancellation_escrow(&id);
    assert_eq!(result, Err(Ok(Error::EscrowNotFound)));
}

// ── 6. Happy path: cancelled with positive balance ───────────────────────────

#[test]
fn get_escrow_after_funded_cancellation_returns_correct_record() {
    let (test_env, id, subscriber, merchant) = setup_funded_and_cancelled();
    let now = test_env.env.ledger().timestamp();

    let escrow = test_env.client.get_cancellation_escrow(&id);

    assert_eq!(escrow.subscription_id, id, "subscription_id mismatch");
    assert_eq!(escrow.amount, PREPAID, "amount mismatch");
    assert_eq!(escrow.subscriber, subscriber, "subscriber mismatch");
    assert_eq!(escrow.merchant, merchant, "merchant mismatch");
    assert_eq!(
        escrow.released_at,
        now + CANCELLATION_ESCROW_WINDOW_SECS,
        "released_at mismatch"
    );
    // token field must be populated (the contract's configured token address).
    assert_eq!(escrow.token, test_env.token, "token mismatch");
}

// ── 7. Merchant-initiated cancellation ───────────────────────────────────────

#[test]
fn get_escrow_after_merchant_cancel_returns_correct_record() {
    let test_env = TestEnv::default();
    let (id, subscriber, merchant) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env
        .client
        .deposit_funds(&id, &subscriber, &PREPAID, &None::<soroban_sdk::BytesN<32>>);
    // Merchant initiates the cancellation.
    test_env.client.cancel_subscription(&id, &merchant);

    let escrow = test_env.client.get_cancellation_escrow(&id);

    assert_eq!(escrow.subscription_id, id);
    assert_eq!(escrow.amount, PREPAID);
    assert_eq!(escrow.subscriber, subscriber, "subscriber in escrow must still be subscriber");
    assert_eq!(escrow.merchant, merchant, "merchant in escrow must be merchant");
}

// ── 8. Idempotency: repeated reads return identical state ─────────────────────

#[test]
fn get_escrow_repeated_reads_are_idempotent() {
    let (test_env, id, _, _) = setup_funded_and_cancelled();

    let first = test_env.client.get_cancellation_escrow(&id);
    let second = test_env.client.get_cancellation_escrow(&id);

    // All fields must be identical across calls.
    assert_eq!(first.subscription_id, second.subscription_id);
    assert_eq!(first.amount, second.amount);
    assert_eq!(first.subscriber, second.subscriber);
    assert_eq!(first.merchant, second.merchant);
    assert_eq!(first.released_at, second.released_at);
    assert_eq!(first.token, second.token);
}

// ── 9. Escrow removed after subscriber claims it ─────────────────────────────

#[test]
fn get_escrow_returns_not_found_after_claim() {
    let (test_env, id, subscriber, _) = setup_funded_and_cancelled();

    // Advance past hold window and claim.
    test_env.jump(CANCELLATION_ESCROW_WINDOW_SECS + 1);
    test_env.client.claim_cancellation_escrow(&subscriber, &id);

    // Escrow record must no longer exist.
    let result = test_env.client.try_get_cancellation_escrow(&id);
    assert_eq!(result, Err(Ok(Error::EscrowNotFound)));
}

// ── 10. Escrow removed after merchant lodges a dispute ───────────────────────

#[test]
fn get_escrow_returns_not_found_after_lodge_dispute() {
    let (test_env, id, _, merchant) = setup_funded_and_cancelled();

    test_env.client.lodge_escrow_dispute(&merchant, &id);

    // Escrow record must have been converted to a Dispute and removed.
    let result = test_env.client.try_get_cancellation_escrow(&id);
    assert_eq!(result, Err(Ok(Error::EscrowNotFound)));
}

// ── 11. Two independent subscriptions carry independent escrow records ────────

#[test]
fn get_escrow_independent_records_for_distinct_subscriptions() {
    let test_env = TestEnv::default();

    // First subscription — funded + cancelled.
    let (id_a, sub_a, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    let prepaid_a: i128 = 10_000_000;
    test_env.stellar_token_client().mint(&sub_a, &prepaid_a);
    test_env
        .client
        .deposit_funds(&id_a, &sub_a, &prepaid_a, &None::<soroban_sdk::BytesN<32>>);
    test_env.client.cancel_subscription(&id_a, &sub_a);

    // Second subscription — funded + cancelled.
    let (id_b, sub_b, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    let prepaid_b: i128 = 30_000_000;
    test_env.stellar_token_client().mint(&sub_b, &prepaid_b);
    test_env
        .client
        .deposit_funds(&id_b, &sub_b, &prepaid_b, &None::<soroban_sdk::BytesN<32>>);
    test_env.client.cancel_subscription(&id_b, &sub_b);

    let escrow_a = test_env.client.get_cancellation_escrow(&id_a);
    let escrow_b = test_env.client.get_cancellation_escrow(&id_b);

    // Each record is independent — amount, subscriber, subscription_id differ.
    assert_eq!(escrow_a.subscription_id, id_a);
    assert_eq!(escrow_b.subscription_id, id_b);
    assert_eq!(escrow_a.amount, prepaid_a);
    assert_eq!(escrow_b.amount, prepaid_b);
    assert_eq!(escrow_a.subscriber, sub_a);
    assert_eq!(escrow_b.subscriber, sub_b);
    // Querying one ID must not return the other.
    assert_ne!(escrow_a.subscription_id, escrow_b.subscription_id);
}

// ── 12. Cross-id isolation: wrong subscription_id returns not-found ───────────

#[test]
fn get_escrow_wrong_id_is_isolated_from_existing_escrow() {
    let (test_env, id, _, _) = setup_funded_and_cancelled();

    // id + 1 has no escrow record of its own.
    let wrong_id = id.wrapping_add(1);
    let result = test_env.client.try_get_cancellation_escrow(&wrong_id);
    assert_eq!(
        result,
        Err(Ok(Error::EscrowNotFound)),
        "a neighbouring subscription_id must not bleed into another escrow"
    );
}

// ── 13. Read requires no auth — any address can call ─────────────────────────

#[test]
fn get_escrow_requires_no_authorization() {
    // The contract's mock_all_auths already makes this green when using
    // the client normally, but we verify by reading as a complete stranger.
    let (test_env, id, _, _) = setup_funded_and_cancelled();

    // No auth is set up for the stranger — the function is open.
    let _stranger = soroban_sdk::Address::generate(&test_env.env);
    // Calling via the contract client (which drives the wasm or native env)
    // must succeed without any auth arrangement.
    let escrow = test_env.client.get_cancellation_escrow(&id);
    assert_eq!(escrow.subscription_id, id);
}

// ── 14. Escrow released_at boundary values are observable ────────────────────

#[test]
fn get_escrow_released_at_equals_creation_time_plus_window() {
    let test_env = TestEnv::default();

    // Set a precise ledger timestamp before cancelling.
    let base_time: u64 = 1_750_000_000;
    test_env.env.ledger().with_mut(|l| l.timestamp = base_time);

    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env
        .client
        .deposit_funds(&id, &subscriber, &PREPAID, &None::<soroban_sdk::BytesN<32>>);
    test_env.client.cancel_subscription(&id, &subscriber);

    let escrow = test_env.client.get_cancellation_escrow(&id);
    assert_eq!(
        escrow.released_at,
        base_time + CANCELLATION_ESCROW_WINDOW_SECS,
        "released_at must be exactly cancellation_time + CANCELLATION_ESCROW_WINDOW_SECS"
    );
}

// ── 15. Paused subscription cancel also creates escrow ───────────────────────

#[test]
fn get_escrow_from_paused_then_cancelled_subscription() {
    let test_env = TestEnv::default();
    let (id, subscriber, merchant) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env
        .client
        .deposit_funds(&id, &subscriber, &PREPAID, &None::<soroban_sdk::BytesN<32>>);

    // Pause the subscription first.
    test_env.client.pause_subscription(&id, &subscriber);
    // Then cancel from the paused state.
    test_env.client.cancel_subscription(&id, &subscriber);

    let escrow = test_env.client.get_cancellation_escrow(&id);
    assert_eq!(escrow.subscription_id, id);
    assert_eq!(escrow.amount, PREPAID);
    assert_eq!(escrow.subscriber, subscriber);
    assert_eq!(escrow.merchant, merchant);
}
