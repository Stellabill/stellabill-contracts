//! Adversarial coverage for the `bulk_cancel_subscriptions` entrypoint (#1109).
//!
//! `test_bulk_admin_ops.rs` covers the operator-hygiene happy paths for the bulk
//! tooling. This fixture targets the failure surface of the *cancel* batch
//! specifically, and asserts that a rejected call is observably inert:
//!
//! - **Nonce domain** — future, replayed, `u64::MAX`, and exhausted-counter
//!   nonces are rejected without cancelling, refunding, or advancing state.
//! - **Precheck ordering** — authorization runs before the size and nonce
//!   checks, and an oversized batch is rejected *without* burning a nonce.
//! - **Batch size boundaries** — `BATCH_MAX_SIZE` is accepted, `BATCH_MAX_SIZE + 1`
//!   is rejected wholesale.
//! - **Per-id failure isolation** — expired, missing (`0`, `u32::MAX`) and
//!   already-cancelled ids are reported per entry and never abort the batch.
//! - **Accounting** — each cancelled id opens exactly one cancellation escrow for
//!   its full prepaid balance (no double refund) and the refund reaches the
//!   subscriber exactly once after the escrow window.
//! - **Invariants** — a `Paused` id does not decrement the subscriber active
//!   count a second time, and the reentrancy guard is released on every path.
//! - **Revoked privilege** — a removed operator loses authorization while its own
//!   per-signer nonce counter is left untouched.

use crate::nonce::DOMAIN_OPERATOR_BATCH_CHARGE;
use crate::test_utils::setup::TestEnv;
use crate::types::{
    BulkCancelEvent, BulkSubscriptionResult, DataKey, Error, SubscriptionStatus, BATCH_MAX_SIZE,
    CANCELLATION_ESCROW_WINDOW_SECS,
};
use soroban_sdk::{
    testutils::Address as _, testutils::Events as _, vec, Address, FromVal, IntoVal, Symbol, Vec,
};

const AMOUNT: i128 = 1_000;
const INTERVAL: u64 = 24 * 60 * 60;
const DEPOSIT: i128 = 5_000_000; // >= init min_topup (1_000_000)

/// Create an `Active` subscription and return its id.
fn new_sub(te: &TestEnv, subscriber: &Address, merchant: &Address, expires_at: Option<u64>) -> u32 {
    te.client.create_subscription(
        subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &expires_at,
        &None::<u32>,
        &None::<Symbol>,
    )
}

/// Create an `Active` subscription holding `DEPOSIT` in prepaid balance.
fn funded_sub(te: &TestEnv, subscriber: &Address, merchant: &Address) -> u32 {
    let sub_id = new_sub(te, subscriber, merchant, None);
    te.stellar_token_client().mint(subscriber, &DEPOSIT);
    te.client
        .deposit_funds(&sub_id, subscriber, &DEPOSIT, &None);
    sub_id
}

fn token_balance(te: &TestEnv, who: &Address) -> i128 {
    soroban_sdk::token::Client::new(&te.env, &te.token).balance(who)
}

fn admin_nonce(te: &TestEnv) -> u64 {
    te.client
        .get_admin_nonce(&te.admin, &DOMAIN_OPERATOR_BATCH_CHARGE)
}

fn status(te: &TestEnv, sub_id: u32) -> SubscriptionStatus {
    te.client.get_subscription(&sub_id).status
}

fn changed(entry: &BulkSubscriptionResult) -> bool {
    entry.success && entry.changed
}

fn failed_with(entry: &BulkSubscriptionResult, error: Error) -> bool {
    !entry.success && !entry.changed && entry.error_code == error.to_code()
}

fn assert_no_escrow(te: &TestEnv, sub_id: u32) {
    assert_eq!(
        te.client.try_get_cancellation_escrow(&sub_id),
        Err(Ok(Error::EscrowNotFound)),
        "no refund may be escrowed for subscription {sub_id}"
    );
}

// ── Nonce domain ─────────────────────────────────────────────────────────────

#[test]
fn bulk_cancel_rejects_future_and_max_nonce_without_cancelling() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber, &merchant);
    let b = funded_sub(&te, &subscriber, &merchant);
    let before_a = te.client.get_subscription(&a);
    let before_b = te.client.get_subscription(&b);

    for nonce in [1u64, 7u64, u64::MAX] {
        let res = te
            .client
            .try_bulk_cancel_subscriptions(&te.admin, &vec![&te.env, a, b], &nonce);

        assert_eq!(res, Err(Ok(Error::NonceAlreadyUsed)));
        assert_eq!(admin_nonce(&te), 0, "rejected batch must not advance nonce");
        assert_eq!(te.client.get_subscription(&a), before_a);
        assert_eq!(te.client.get_subscription(&b), before_b);
        assert_eq!(token_balance(&te, &subscriber), 0);
        assert_no_escrow(&te, a);
        assert_no_escrow(&te, b);
    }
}

#[test]
fn bulk_cancel_fails_closed_when_the_nonce_counter_cannot_advance() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber, &merchant);
    let before = te.client.get_subscription(&a);

    // Exhaust the per-signer batch counter so `stored + 1` would overflow.
    te.env.as_contract(&te.client.address, || {
        te.env.storage().persistent().set(
            &DataKey::AdminNonce(te.admin.clone(), DOMAIN_OPERATOR_BATCH_CHARGE),
            &u64::MAX,
        );
    });

    let res = te
        .client
        .try_bulk_cancel_subscriptions(&te.admin, &vec![&te.env, a], &u64::MAX);

    assert_eq!(res, Err(Ok(Error::Overflow)));
    assert_eq!(admin_nonce(&te), u64::MAX);
    assert_eq!(te.client.get_subscription(&a), before);
    assert_eq!(status(&te, a), SubscriptionStatus::Active);
    assert_eq!(token_balance(&te, &subscriber), 0);
    assert_no_escrow(&te, a);
}

#[test]
fn bulk_cancel_releases_the_guard_after_rejected_and_successful_calls() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber, &merchant);
    let b = funded_sub(&te, &subscriber, &merchant);

    // Rejected call first: the guard must not stay held (Error::Reentrancy).
    assert_eq!(
        te.client
            .try_bulk_cancel_subscriptions(&te.admin, &vec![&te.env, a], &9u64),
        Err(Ok(Error::NonceAlreadyUsed))
    );
    let first = te
        .client
        .bulk_cancel_subscriptions(&te.admin, &vec![&te.env, a], &0u64);
    assert!(changed(&first.get(0).unwrap()));

    // And again after a successful call, with the advanced nonce.
    let second = te
        .client
        .bulk_cancel_subscriptions(&te.admin, &vec![&te.env, b], &1u64);
    assert!(changed(&second.get(0).unwrap()));
    assert_eq!(admin_nonce(&te), 2);
}

// ── Precheck ordering and size boundaries ────────────────────────────────────

#[test]
fn bulk_cancel_rejects_unauthorized_callers_before_size_and_nonce_checks() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber, &merchant);
    let before = te.client.get_subscription(&a);

    let oversized = BATCH_MAX_SIZE + 1;
    let mut batch: Vec<u32> = Vec::new(&te.env);
    for i in 0..oversized {
        batch.push_back(i);
    }

    // Subscriber and merchant own the subscriptions but hold no bulk-cancel role.
    for caller in [&stranger, &subscriber, &merchant] {
        // Wrong nonce *and* oversized batch: authorization must win, so the error
        // is deterministic and no nonce is consumed.
        let res = te
            .client
            .try_bulk_cancel_subscriptions(caller, &batch, &7u64);
        assert_eq!(res, Err(Ok(Error::Unauthorized)));
        assert_eq!(admin_nonce(&te), 0);
    }

    assert_eq!(te.client.get_subscription(&a), before);
    assert_eq!(status(&te, a), SubscriptionStatus::Active);
    assert_eq!(token_balance(&te, &subscriber), 0);
    assert_no_escrow(&te, a);
}

#[test]
fn bulk_cancel_oversized_batch_is_rejected_without_burning_a_nonce() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber, &merchant);
    let before = te.client.get_subscription(&a);

    let mut oversized: Vec<u32> = Vec::new(&te.env);
    for i in 0..(BATCH_MAX_SIZE + 1) {
        oversized.push_back(i);
    }

    assert_eq!(
        te.client
            .try_bulk_cancel_subscriptions(&te.admin, &oversized, &0u64),
        Err(Ok(Error::BatchTooLarge))
    );
    assert_eq!(
        admin_nonce(&te),
        0,
        "size rejection must not consume a nonce"
    );
    assert_eq!(te.client.get_subscription(&a), before);
    assert_no_escrow(&te, a);

    // The untouched nonce is still usable, proving nothing was burned.
    let results = te
        .client
        .bulk_cancel_subscriptions(&te.admin, &vec![&te.env, a], &0u64);
    assert!(changed(&results.get(0).unwrap()));
    assert_eq!(admin_nonce(&te), 1);
}

#[test]
fn bulk_cancel_accepts_exactly_batch_max_size() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber, &merchant);

    // One real id plus BATCH_MAX_SIZE - 1 missing ids: the boundary batch is
    // accepted, keeps request order, and reports each entry independently.
    let mut batch: Vec<u32> = Vec::new(&te.env);
    for i in 1..BATCH_MAX_SIZE {
        batch.push_back(10_000 + i);
    }
    batch.push_back(a);

    let results = te
        .client
        .bulk_cancel_subscriptions(&te.admin, &batch, &0u64);

    assert_eq!(results.len(), BATCH_MAX_SIZE);
    for i in 0..(BATCH_MAX_SIZE - 1) {
        let entry = results.get(i).unwrap();
        assert_eq!(entry.subscription_id, 10_000 + i + 1);
        assert!(failed_with(&entry, Error::NotFound));
    }
    let last = results.get(BATCH_MAX_SIZE - 1).unwrap();
    assert_eq!(last.subscription_id, a);
    assert!(changed(&last));
    assert_eq!(status(&te, a), SubscriptionStatus::Cancelled);
    assert_eq!(admin_nonce(&te), 1);
}

// ── Per-id failure isolation ─────────────────────────────────────────────────

#[test]
fn bulk_cancel_reports_expired_and_missing_ids_without_aborting_the_batch() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let expires_at = te.env.ledger().timestamp() + INTERVAL;

    let expiring = new_sub(&te, &subscriber, &merchant, Some(expires_at));
    te.stellar_token_client().mint(&subscriber, &DEPOSIT);
    te.client
        .deposit_funds(&expiring, &subscriber, &DEPOSIT, &None);
    let live = funded_sub(&te, &subscriber, &merchant);

    te.jump(INTERVAL + 1);
    let expiring_before = te.client.get_subscription(&expiring);

    let results = te.client.bulk_cancel_subscriptions(
        &te.admin,
        &vec![&te.env, 0u32, expiring, u32::MAX, live],
        &0u64,
    );

    assert_eq!(results.len(), 4);
    // Request order is preserved: every entry maps back to its requested id.
    let missing = results.get(0).unwrap();
    assert_eq!(missing.subscription_id, 0u32);
    assert!(failed_with(&missing, Error::NotFound));
    let expired = results.get(1).unwrap();
    assert_eq!(expired.subscription_id, expiring);
    assert!(failed_with(&expired, Error::SubscriptionExpired));
    let absent = results.get(2).unwrap();
    assert_eq!(absent.subscription_id, u32::MAX);
    assert!(failed_with(&absent, Error::NotFound));
    let cancelled = results.get(3).unwrap();
    assert_eq!(cancelled.subscription_id, live);
    assert!(changed(&cancelled));

    // The failed id is untouched: still active, still holding its deposit, and
    // no refund was escrowed for it.
    assert_eq!(te.client.get_subscription(&expiring), expiring_before);
    assert_eq!(status(&te, expiring), SubscriptionStatus::Active);
    assert_no_escrow(&te, expiring);
    assert_eq!(
        te.client.get_subscription(&live).status,
        SubscriptionStatus::Cancelled
    );
    assert_no_escrow(&te, 0);
    assert_eq!(admin_nonce(&te), 1);
}

#[test]
fn bulk_cancel_counts_envelope_for_a_mixed_batch() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let cancelled = funded_sub(&te, &subscriber, &merchant);
    let already = funded_sub(&te, &subscriber, &merchant);
    let live = funded_sub(&te, &subscriber, &merchant);
    te.client.cancel_subscription(&already, &merchant);

    te.client.bulk_cancel_subscriptions(
        &te.admin,
        &vec![&te.env, cancelled, already, u32::MAX, live],
        &0u64,
    );

    let events = te.env.events().all();
    let envelope = events
        .iter()
        .find(|e| {
            Symbol::from_val(&te.env, &e.1.get(0).unwrap())
                == Symbol::new(&te.env, "bulk_cancelled")
        })
        .expect("missing bulk_cancelled envelope");
    let data: BulkCancelEvent = envelope.2.clone().into_val(&te.env);

    assert_eq!(data.caller, te.admin);
    assert_eq!(data.requested, 4);
    assert_eq!(data.cancelled, 2);
    assert_eq!(data.skipped, 1);
    assert_eq!(data.failed, 1);
    assert_eq!(data.nonce, 0);
}

// ── Accounting ───────────────────────────────────────────────────────────────

#[test]
fn bulk_cancel_escrows_each_prepaid_balance_once_and_refunds_exactly_once() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber, &merchant);
    let b = funded_sub(&te, &subscriber, &merchant);
    assert_eq!(token_balance(&te, &subscriber), 0);

    // Duplicate `a` inside the batch: the second occurrence is skipped, so the
    // refund can never be escrowed twice.
    let results = te
        .client
        .bulk_cancel_subscriptions(&te.admin, &vec![&te.env, a, b, a], &0u64);

    assert!(changed(&results.get(0).unwrap()));
    assert!(changed(&results.get(1).unwrap()));
    assert!(!results.get(2).unwrap().changed && results.get(2).unwrap().success);

    for sub_id in [a, b] {
        assert_eq!(te.client.get_cancellation_escrow(&sub_id).amount, DEPOSIT);
        assert_eq!(te.client.get_subscription(&sub_id).prepaid_balance, 0);
    }
    // Refunds are time-locked, so no tokens have moved yet.
    assert_eq!(token_balance(&te, &subscriber), 0);

    te.jump(CANCELLATION_ESCROW_WINDOW_SECS + 1);
    assert_eq!(
        te.client.claim_cancellation_escrow(&subscriber, &a),
        DEPOSIT
    );
    assert_eq!(
        te.client.claim_cancellation_escrow(&subscriber, &b),
        DEPOSIT
    );
    assert_eq!(token_balance(&te, &subscriber), DEPOSIT * 2);

    // A second claim finds nothing: the refund is not replayable.
    assert_eq!(
        te.client.try_claim_cancellation_escrow(&subscriber, &a),
        Err(Ok(Error::EscrowNotFound))
    );
    assert_eq!(token_balance(&te, &subscriber), DEPOSIT * 2);
}

#[test]
fn bulk_cancel_of_a_paused_subscription_keeps_the_active_count_consistent() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let paused = funded_sub(&te, &subscriber, &merchant);
    let other = funded_sub(&te, &subscriber, &merchant);
    assert_eq!(
        te.client.get_subscriber_active_count(&subscriber),
        2,
        "both subscriptions start Active"
    );

    // Pausing consumes the same batch nonce, so the cancel must use nonce 1.
    te.client
        .bulk_pause_subscriptions(&te.admin, &vec![&te.env, paused], &0u64);
    assert_eq!(te.client.get_subscriber_active_count(&subscriber), 1);

    let results =
        te.client
            .bulk_cancel_subscriptions(&te.admin, &vec![&te.env, paused, other], &1u64);

    assert!(changed(&results.get(0).unwrap()));
    assert!(changed(&results.get(1).unwrap()));
    assert_eq!(status(&te, paused), SubscriptionStatus::Cancelled);
    assert_eq!(status(&te, other), SubscriptionStatus::Cancelled);
    // `other` decrements (it was Active); `paused` must not decrement a second
    // time or the subscriber's counter would be wrong.
    assert_eq!(te.client.get_subscriber_active_count(&subscriber), 0);
    assert_eq!(admin_nonce(&te), 2);
}

// ── Revoked operator ─────────────────────────────────────────────────────────

#[test]
fn bulk_cancel_by_a_removed_operator_is_rejected_with_state_intact() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let operator = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber, &merchant);
    let b = funded_sub(&te, &subscriber, &merchant);

    te.client.set_operator(&te.admin, &operator);
    let first = te
        .client
        .bulk_cancel_subscriptions(&operator, &vec![&te.env, a], &0u64);
    assert!(changed(&first.get(0).unwrap()));
    assert_eq!(te.client.get_operator_nonce(&operator), 1);

    let before = te.client.get_subscription(&b);
    te.client.remove_operator(&te.admin);
    assert_eq!(te.client.get_operator(), None);

    // The operator's own nonce counter is per-signer, and the revoked role must
    // not be able to advance it or cancel anything.
    let res = te
        .client
        .try_bulk_cancel_subscriptions(&operator, &vec![&te.env, b], &1u64);
    assert_eq!(res, Err(Ok(Error::Unauthorized)));
    assert_eq!(te.client.get_operator_nonce(&operator), 1);
    assert_eq!(te.client.get_subscription(&b), before);
    assert_eq!(status(&te, b), SubscriptionStatus::Active);
    assert_no_escrow(&te, b);
    assert_eq!(token_balance(&te, &subscriber), 0);
}

#[test]
fn bulk_cancel_fails_closed_on_an_uninitialized_vault() {
    let env = soroban_sdk::Env::default();
    env.mock_all_auths();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let client = crate::SubscriptionVaultClient::new(&env, &contract_id);
    let caller = Address::generate(&env);

    let res = client.try_bulk_cancel_subscriptions(&caller, &vec![&env, 1u32], &0u64);
    assert_eq!(res, Err(Ok(Error::NotInitialized)));
}
