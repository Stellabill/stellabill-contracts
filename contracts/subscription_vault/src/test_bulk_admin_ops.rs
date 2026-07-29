//! Tests for the bulk admin/operator pause & cancel tooling (issue #497).
//!
//! Coverage targets the operational-hygiene requirements:
//! - admin **and** operator are both accepted authorizers; anyone else is rejected;
//! - the batch is partial-failure tolerant (already-paused/cancelled/missing ids
//!   are skipped or reported, never aborting the batch);
//! - empty list is a no-op that consumes no nonce and emits no envelope event;
//! - duplicate ids inside one batch are safe (no double pause / double refund);
//! - batches larger than `BATCH_MAX_SIZE` are rejected wholesale;
//! - the per-batch nonce advances and rejects wrong/replayed values.

use crate::nonce::DOMAIN_OPERATOR_BATCH_CHARGE;
use crate::test_utils::setup::TestEnv;
use crate::types::{BulkSubscriptionResult, Error, SubscriptionStatus, BATCH_MAX_SIZE};
use soroban_sdk::{testutils::Address as _, vec, Address, Vec};

const AMOUNT: i128 = 1_000;
const INTERVAL: u64 = 24 * 60 * 60;
const DEPOSIT: i128 = 5_000_000; // >= init min_topup (1_000_000)

/// Create an `Active`, fully-funded subscription and return its id.
fn funded_sub(te: &TestEnv, subscriber: &Address, merchant: &Address) -> u32 {
    let sub_id = te.client.create_subscription(
        subscriber, merchant, &AMOUNT, &INTERVAL, &false, &None, &None,
        &None::<u32>,
);
    te.stellar_token_client().mint(subscriber, &DEPOSIT);
    te.client
        .deposit_funds(&sub_id, subscriber, &DEPOSIT, &None);
    sub_id
}

fn token_balance(te: &TestEnv, who: &Address) -> i128 {
    soroban_sdk::token::Client::new(&te.env, &te.token).balance(who)
}

fn status(te: &TestEnv, sub_id: u32) -> SubscriptionStatus {
    te.client.get_subscription(&sub_id).status
}

// ── Bulk pause: happy paths ─────────────────────────────────────────────────

#[test]
fn admin_bulk_pause_pauses_all_active_subscriptions() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    let a = funded_sub(&te, &subscriber, &merchant);
    let b = funded_sub(&te, &subscriber, &merchant);

    let results = te
        .client
        .bulk_pause_subscriptions(&te.admin, &vec![&te.env, a, b], &0u64);

    assert_eq!(results.len(), 2);
    assert!(results.get(0).unwrap().success && results.get(0).unwrap().changed);
    assert!(results.get(1).unwrap().success && results.get(1).unwrap().changed);
    assert_eq!(status(&te, a), SubscriptionStatus::Paused);
    assert_eq!(status(&te, b), SubscriptionStatus::Paused);
}

#[test]
fn operator_is_authorized_for_bulk_pause() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);

    let a = funded_sub(&te, &subscriber, &merchant);

    let results = te
        .client
        .bulk_pause_subscriptions(&operator, &vec![&te.env, a], &0u64);

    assert!(results.get(0).unwrap().changed);
    assert_eq!(status(&te, a), SubscriptionStatus::Paused);
}

#[test]
fn unauthorized_caller_rejected_for_bulk_pause() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);

    let a = funded_sub(&te, &subscriber, &merchant);

    let res = te
        .client
        .try_bulk_pause_subscriptions(&stranger, &vec![&te.env, a], &0u64);
    assert_eq!(res, Err(Ok(Error::Unauthorized)));
    // State untouched.
    assert_eq!(status(&te, a), SubscriptionStatus::Active);
}

// ── Bulk pause: edge cases ──────────────────────────────────────────────────

#[test]
fn empty_bulk_pause_is_a_noop_and_consumes_no_nonce() {
    let te = TestEnv::default();

    let empty: Vec<u32> = Vec::new(&te.env);
    let results = te.client.bulk_pause_subscriptions(&te.admin, &empty, &0u64);
    assert_eq!(results.len(), 0);

    // Nonce was NOT consumed — a real batch can still use nonce 0.
    assert_eq!(
        te.client
            .get_admin_nonce(&te.admin, &DOMAIN_OPERATOR_BATCH_CHARGE),
        0u64
    );
}

#[test]
fn bulk_pause_skips_already_paused_without_aborting() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    let active = funded_sub(&te, &subscriber, &merchant);
    let already = funded_sub(&te, &subscriber, &merchant);
    // Pre-pause `already` via the merchant (single-id path).
    te.client.pause_subscription(&already, &merchant);

    let missing = 9_999u32; // never created

    let results = te.client.bulk_pause_subscriptions(
        &te.admin,
        &vec![&te.env, active, already, missing],
        &0u64,
    );

    // active -> changed
    assert_eq!(
        results.get(0).unwrap(),
        BulkSubscriptionResult {
            subscription_id: active,
            success: true,
            changed: true,
            error_code: 0,
        }
    );
    // already paused -> skipped (success, not changed)
    assert_eq!(
        results.get(1).unwrap(),
        BulkSubscriptionResult {
            subscription_id: already,
            success: true,
            changed: false,
            error_code: 0,
        }
    );
    // missing -> failed with NotFound
    assert_eq!(
        results.get(2).unwrap(),
        BulkSubscriptionResult {
            subscription_id: missing,
            success: false,
            changed: false,
            error_code: Error::NotFound.to_code(),
        }
    );

    assert_eq!(status(&te, active), SubscriptionStatus::Paused);
}

#[test]
fn bulk_pause_with_duplicate_ids_pauses_once_then_skips() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber, &merchant);

    let results = te
        .client
        .bulk_pause_subscriptions(&te.admin, &vec![&te.env, a, a, a], &0u64);

    assert!(results.get(0).unwrap().changed);
    assert!(!results.get(1).unwrap().changed && results.get(1).unwrap().success);
    assert!(!results.get(2).unwrap().changed && results.get(2).unwrap().success);
    assert_eq!(status(&te, a), SubscriptionStatus::Paused);
}

#[test]
fn bulk_pause_reports_expired_as_failure() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    let now = te.env.ledger().timestamp();
    let sub_id = te.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &Some(now + 1_000),
        &None::<u32>,
);
    te.stellar_token_client().mint(&subscriber, &DEPOSIT);
    te.client
        .deposit_funds(&sub_id, &subscriber, &DEPOSIT, &None);

    te.jump(2_000); // past expires_at

    let results = te
        .client
        .bulk_pause_subscriptions(&te.admin, &vec![&te.env, sub_id], &0u64);
    assert_eq!(
        results.get(0).unwrap().error_code,
        Error::SubscriptionExpired.to_code()
    );
    assert!(!results.get(0).unwrap().success);
}

#[test]
fn bulk_pause_rejects_oversized_batch() {
    let te = TestEnv::default();

    let mut ids: Vec<u32> = Vec::new(&te.env);
    for i in 0..(BATCH_MAX_SIZE + 1) {
        ids.push_back(i);
    }

    let res = te
        .client
        .try_bulk_pause_subscriptions(&te.admin, &ids, &0u64);
    assert_eq!(res, Err(Ok(Error::BatchTooLarge)));
    // Oversized batch must not burn the nonce.
    assert_eq!(
        te.client
            .get_admin_nonce(&te.admin, &DOMAIN_OPERATOR_BATCH_CHARGE),
        0u64
    );
}

#[test]
fn bulk_pause_at_max_batch_size_is_accepted() {
    let te = TestEnv::default();

    let mut ids: Vec<u32> = Vec::new(&te.env);
    for i in 0..BATCH_MAX_SIZE {
        ids.push_back(i); // ids don't need to exist; they report NotFound
    }

    // Exactly BATCH_MAX_SIZE is allowed (does not return BatchTooLarge).
    let results = te.client.bulk_pause_subscriptions(&te.admin, &ids, &0u64);
    assert_eq!(results.len(), BATCH_MAX_SIZE);
}

// ── Bulk pause: nonce semantics ─────────────────────────────────────────────

#[test]
fn bulk_pause_advances_nonce() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber, &merchant);

    te.client
        .bulk_pause_subscriptions(&te.admin, &vec![&te.env, a], &0u64);
    assert_eq!(
        te.client
            .get_admin_nonce(&te.admin, &DOMAIN_OPERATOR_BATCH_CHARGE),
        1u64
    );
}

#[test]
fn bulk_pause_wrong_nonce_rejected() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber, &merchant);

    let res = te
        .client
        .try_bulk_pause_subscriptions(&te.admin, &vec![&te.env, a], &1u64);
    assert_eq!(res, Err(Ok(Error::NonceAlreadyUsed)));
}

#[test]
fn bulk_pause_replay_rejected() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber, &merchant);
    let b = funded_sub(&te, &subscriber, &merchant);

    te.client
        .bulk_pause_subscriptions(&te.admin, &vec![&te.env, a], &0u64);
    // Replaying nonce 0 must fail.
    let res = te
        .client
        .try_bulk_pause_subscriptions(&te.admin, &vec![&te.env, b], &0u64);
    assert_eq!(res, Err(Ok(Error::NonceAlreadyUsed)));
}

#[test]
fn admin_and_operator_have_independent_nonce_sequences() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);

    let a = funded_sub(&te, &subscriber, &merchant);
    let b = funded_sub(&te, &subscriber, &merchant);

    // Both start at nonce 0 independently (keyed per signer address).
    te.client
        .bulk_pause_subscriptions(&te.admin, &vec![&te.env, a], &0u64);
    te.client
        .bulk_pause_subscriptions(&operator, &vec![&te.env, b], &0u64);

    assert_eq!(
        te.client
            .get_admin_nonce(&te.admin, &DOMAIN_OPERATOR_BATCH_CHARGE),
        1u64
    );
    assert_eq!(te.client.get_operator_nonce(&operator), 1u64);
}

// ── Bulk cancel: happy paths & refunds ──────────────────────────────────────

#[test]
fn admin_bulk_cancel_cancels_and_refunds() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    let a = funded_sub(&te, &subscriber, &merchant);
    let b = funded_sub(&te, &subscriber, &merchant);
    // Both deposits are in the contract; subscriber holds nothing right now.
    assert_eq!(token_balance(&te, &subscriber), 0);

    let results = te
        .client
        .bulk_cancel_subscriptions(&te.admin, &vec![&te.env, a, b], &0u64);

    assert!(results.get(0).unwrap().changed && results.get(1).unwrap().changed);
    assert_eq!(status(&te, a), SubscriptionStatus::Cancelled);
    assert_eq!(status(&te, b), SubscriptionStatus::Cancelled);
    // Full prepaid balance of both subscriptions refunded.
    assert_eq!(token_balance(&te, &subscriber), DEPOSIT * 2);
}

#[test]
fn operator_is_authorized_for_bulk_cancel() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);

    let a = funded_sub(&te, &subscriber, &merchant);
    te.client
        .bulk_cancel_subscriptions(&operator, &vec![&te.env, a], &0u64);
    assert_eq!(status(&te, a), SubscriptionStatus::Cancelled);
}

#[test]
fn unauthorized_caller_rejected_for_bulk_cancel() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber, &merchant);

    let res = te
        .client
        .try_bulk_cancel_subscriptions(&stranger, &vec![&te.env, a], &0u64);
    assert_eq!(res, Err(Ok(Error::Unauthorized)));
    assert_eq!(status(&te, a), SubscriptionStatus::Active);
}

// ── Bulk cancel: edge cases ─────────────────────────────────────────────────

#[test]
fn bulk_cancel_skips_already_cancelled_no_double_refund() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    // `funded_sub` is net-zero on the subscriber's wallet (mint DEPOSIT, then
    // deposit DEPOSIT into the contract).
    let already = funded_sub(&te, &subscriber, &merchant);
    // Cancel once via the merchant single-id path — refunds DEPOSIT already.
    te.client.cancel_subscription(&already, &merchant);
    assert_eq!(token_balance(&te, &subscriber), DEPOSIT);

    // Creating a second funded sub is net-zero, so the refund above is untouched.
    let active = funded_sub(&te, &subscriber, &merchant);
    assert_eq!(token_balance(&te, &subscriber), DEPOSIT);

    let results =
        te.client
            .bulk_cancel_subscriptions(&te.admin, &vec![&te.env, already, active], &0u64);

    // already-cancelled -> skipped, no second refund
    assert_eq!(
        results.get(0).unwrap(),
        BulkSubscriptionResult {
            subscription_id: already,
            success: true,
            changed: false,
            error_code: 0,
        }
    );
    assert!(results.get(1).unwrap().changed);
    // Only `active`'s DEPOSIT is refunded here, on top of the earlier refund.
    assert_eq!(token_balance(&te, &subscriber), DEPOSIT * 2);
}

#[test]
fn bulk_cancel_duplicate_ids_refunds_once() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber, &merchant);
    assert_eq!(token_balance(&te, &subscriber), 0);

    let results = te
        .client
        .bulk_cancel_subscriptions(&te.admin, &vec![&te.env, a, a], &0u64);

    assert!(results.get(0).unwrap().changed);
    assert!(!results.get(1).unwrap().changed && results.get(1).unwrap().success);
    // Refund happened exactly once.
    assert_eq!(token_balance(&te, &subscriber), DEPOSIT);
}

#[test]
fn bulk_cancel_mixed_valid_cancelled_missing() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    let valid = funded_sub(&te, &subscriber, &merchant);
    let cancelled = funded_sub(&te, &subscriber, &merchant);
    te.client.cancel_subscription(&cancelled, &merchant);
    let missing = 4_242u32;

    let results = te.client.bulk_cancel_subscriptions(
        &te.admin,
        &vec![&te.env, valid, cancelled, missing],
        &0u64,
    );

    assert!(results.get(0).unwrap().changed);
    assert!(!results.get(1).unwrap().changed && results.get(1).unwrap().success);
    assert_eq!(
        results.get(2).unwrap().error_code,
        Error::NotFound.to_code()
    );
    assert!(!results.get(2).unwrap().success);
}

#[test]
fn empty_bulk_cancel_is_a_noop_and_consumes_no_nonce() {
    let te = TestEnv::default();
    let empty: Vec<u32> = Vec::new(&te.env);
    let results = te
        .client
        .bulk_cancel_subscriptions(&te.admin, &empty, &0u64);
    assert_eq!(results.len(), 0);
    assert_eq!(
        te.client
            .get_admin_nonce(&te.admin, &DOMAIN_OPERATOR_BATCH_CHARGE),
        0u64
    );
}

#[test]
fn bulk_cancel_rejects_oversized_batch() {
    let te = TestEnv::default();
    let mut ids: Vec<u32> = Vec::new(&te.env);
    for i in 0..(BATCH_MAX_SIZE + 1) {
        ids.push_back(i);
    }
    let res = te
        .client
        .try_bulk_cancel_subscriptions(&te.admin, &ids, &0u64);
    assert_eq!(res, Err(Ok(Error::BatchTooLarge)));
}

#[test]
fn bulk_cancel_replay_rejected() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber, &merchant);
    let b = funded_sub(&te, &subscriber, &merchant);

    te.client
        .bulk_cancel_subscriptions(&te.admin, &vec![&te.env, a], &0u64);
    let res = te
        .client
        .try_bulk_cancel_subscriptions(&te.admin, &vec![&te.env, b], &0u64);
    assert_eq!(res, Err(Ok(Error::NonceAlreadyUsed)));
}
