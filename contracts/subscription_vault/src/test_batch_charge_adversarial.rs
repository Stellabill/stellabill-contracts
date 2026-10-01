//! Adversarial coverage for `batch_charge` / `do_batch_charge` (issue #995).
//!
//! `do_batch_charge` is the admin bulk-charge delegate in
//! `contracts/subscription_vault/src/admin.rs`, reached through the
//! `batch_charge` entrypoint. It had no directly associated test fixture:
//! `test_bulk_admin_ops.rs` covers the *pause/cancel* bulk tooling, the
//! interval suite covers the single-id path, and nothing pinned the batch
//! entrypoint's own auth, nonce, emergency-stop or result-mapping contract.
//!
//! What is exercised here:
//!
//! * the batch result vector is **positional** (`BatchChargeResult` carries no
//!   subscription id) and the order mirrors the input `Vec`;
//! * one bad id never aborts the batch — every failure mode is reported in
//!   place and the remaining ids are still charged;
//! * each `ChargeExecutionResult` variant maps to the documented
//!   `success` / `error_code` pair, including the two silent-skip outcomes
//!   (`ScheduledCancellation`, `Skipped`) which are reported as success;
//! * duplicate ids inside a single batch cannot double-charge;
//! * the `DOMAIN_BATCH_CHARGE` nonce is verified before any charge runs and is
//!   not shared with `DOMAIN_OPERATOR_BATCH_CHARGE`;
//! * a wrong or replayed nonce is rejected without consuming a nonce and
//!   without mutating subscription state;
//! * the nonce is consumed even when every id in the batch fails, so a batch
//!   cannot be retried under the same nonce;
//! * the emergency-stop guard runs *before* the auth/nonce checks, so an active
//!   stop neither charges nor burns a nonce (it can be retried once lifted).

use crate::nonce::{DOMAIN_BATCH_CHARGE, DOMAIN_OPERATOR_BATCH_CHARGE};
use crate::test_utils::setup::TestEnv;
use crate::types::{Error, SubscriptionStatus};
use soroban_sdk::{testutils::Address as _, vec, Address, Vec};

const AMOUNT: i128 = 1_000;
const INTERVAL: u64 = 24 * 60 * 60;
/// Vault minimum top-up configured by `TestEnv::default`.
const MIN_TOPUP: i128 = 1_000_000;
/// Comfortably covers my several interval charges.
const DEPOSIT: i128 = 5 * MIN_TOPUP;
/// The subscriber-creation rate limit window used by `enforce_creation_rate_limit`.
const SECONDS_IN_DAY: u64 = 86_400;

/// Create a subscription with an explicit `amount`, depositing nothing.
fn create_sub(te: &TestEnv, subscriber: &Address, amount: i128) -> u32 {
    let merchant = Address::generate(&te.env);
    te.client.create_subscription(
        subscriber,
        &merchant,
        &amount,
        &INTERVAL,
        &false,
        &None,
        &None,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    )
}

/// Create an `Active`, funded subscription with the standard `amount`.
fn funded_sub(te: &TestEnv, subscriber: &Address) -> u32 {
    let id = create_sub(te, subscriber, AMOUNT);
    te.stellar_token_client().mint(subscriber, &DEPOSIT);
    te.client
        .deposit_funds(&id, subscriber, &DEPOSIT, &None);
    id
}

/// Run `batch_charge` expecting a contract error, and return it.
fn batch_charge_err(te: &TestEnv, ids: &Vec<u32>, nonce: u64) -> Error {
    match te.client.try_batch_charge(ids, &nonce) {
        Err(Ok(e)) => e,
        other => panic!("expected a contract error, got {other:?}"),
    }
}

fn batch_nonce(te: &TestEnv) -> u64 {
    te.client.get_admin_nonce(&te.admin, &DOMAIN_BATCH_CHARGE)
}

fn charged(te: &TestEnv, id: &u32) -> i128 {
    te.client.get_subscription(id).lifetime_charged
}

// ── Happy path & result shape ───────────────────────────────────────────────

#[test]
fn batch_charge_charges_every_eligible_subscription_in_input_order() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber);
    let b = funded_sub(&te, &subscriber);
    te.jump(INTERVAL);

    let results = te.client.batch_charge(&vec![&te.env, a, b], &0u64);

    assert_eq!(results.len(), 2);
    assert!(results.get(0).unwrap().success);
    assert_eq!(results.get(0).unwrap().error_code, 0);
    assert!(results.get(1).unwrap().success);
    assert_eq!(results.get(1).unwrap().error_code, 0);
    assert_eq!(charged(&te, &a), AMOUNT);
    assert_eq!(charged(&te, &b), AMOUNT);
    // Exactly one nonce consumed for the whole batch.
    assert_eq!(batch_nonce(&te), 1);
}

#[test]
fn batch_charge_is_partial_failure_tolerant_and_stays_positional() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let first = funded_sub(&te, &subscriber);
    let missing = 4_242u32; // never created
    let last = funded_sub(&te, &subscriber);
    te.jump(INTERVAL);

    let results = te.client.batch_charge(&vec![&te.env, first, missing, last], &0u64);

    assert_eq!(results.len(), 3, "one result per input id, in order");
    // Index 0 -> first (charged).
    assert!(results.get(0).unwrap().success);
    assert_eq!(results.get(0).unwrap().error_code, 0);
    // Index 1 -> missing (reported, not fatal).
    assert!(!results.get(1).unwrap().success);
    assert_eq!(results.get(1).unwrap().error_code, Error::NotFound.to_code());
    // Index 2 -> last (still charged despite the failure at index 1).
    assert!(results.get(2).unwrap().success);
    assert_eq!(charged(&te, &last), AMOUNT);
}

#[test]
fn batch_charge_reports_insufficient_balance_without_reverting_the_batch() {
    let te = TestEnv::default();

    // A subscription whose interval charge cannot be covered by its deposit.
    let broke = Address::generate(&te.env);
    let underfunded = create_sub(&te, &broke, 2 * MIN_TOPUP);
    te.stellar_token_client().mint(&broke, &MIN_TOPUP);
    te.client
        .deposit_funds(&underfunded, &broke, &MIN_TOPUP, &None);

    // A healthy subscription in the same batch must still be charged.
    let healthy = Address::generate(&te.env);
    let ok = funded_sub(&te, &healthy);
    te.jump(INTERVAL);

    let results = te.client.batch_charge(&vec![&te.env, underfunded, ok], &0u64);

    assert!(!results.get(0).unwrap().success);
    assert_eq!(
        results.get(0).unwrap().error_code,
        Error::InsufficientBalance.to_code()
    );
    assert_eq!(charged(&te, &underfunded), 0);
    assert!(results.get(1).unwrap().success);
    assert_eq!(charged(&te, &ok), AMOUNT);
}

#[test]
fn batch_charge_reports_a_reached_lifetime_cap_as_non_fatal() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    let id = te.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &Some(AMOUNT), // lifetime cap exactly one charge
        &None,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    te.stellar_token_client().mint(&subscriber, &DEPOSIT);
    te.client
        .deposit_funds(&id, &subscriber, &DEPOSIT, &None);

    // First charge consumes the whole cap.
    te.jump(INTERVAL);
    let first = te.client.try_batch_charge(&vec![&te.env, id], &0u64);
    assert!(first.is_ok());
    assert!(first.unwrap().unwrap().get(0).unwrap().success);
    assert_eq!(charged(&te, &id), AMOUNT);

    // Second batch charge reports the cap instead of aborting the batch.
    let results = te.client.batch_charge(&vec![&te.env, id], &1u64);
    assert!(!results.get(0).unwrap().success);
    assert_eq!(
        results.get(0).unwrap().error_code,
        Error::LifetimeCapReached.to_code()
    );
    assert_eq!(charged(&te, &id), AMOUNT, "no extra funds were moved");
    assert_eq!(
        te.client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}

#[test]
fn batch_charge_reports_scheduled_cancellation_as_success() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let id = funded_sub(&te, &subscriber);

    // Arm a future cancellation, then let it mature.
    te.client.schedule_cancel(&id, &subscriber, &(INTERVAL + 1));
    te.jump(INTERVAL + 2);

    let results = te.client.batch_charge(&vec![&te.env, id], &0u64);

    // A matured scheduled cancellation is a *successful* terminal outcome.
    assert!(results.get(0).unwrap().success);
    assert_eq!(results.get(0).unwrap().error_code, 0);
    assert_eq!(
        te.client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}

#[test]
fn batch_charge_skips_a_non_renewing_elapsed_subscription_without_error() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let id = funded_sub(&te, &subscriber);

    te.client.set_auto_renew(&id, &subscriber, &false);
    te.jump(INTERVAL);

    let results = te.client.batch_charge(&vec![&te.env, id], &0u64);

    // `Skipped` is reported as success with error_code 0 — but nothing moved.
    assert!(results.get(0).unwrap().success);
    assert_eq!(results.get(0).unwrap().error_code, 0);
    assert_eq!(charged(&te, &id), 0, "a skipped charge must not bill");
    assert_eq!(
        te.client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );
}

#[test]
fn batch_charge_reports_an_expired_subscription_as_failure() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    let expired = te.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &Some(INTERVAL + 1), // expires one interval from now
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    te.stellar_token_client().mint(&subscriber, &DEPOSIT);
    te.client
        .deposit_funds(&expired, &subscriber, &DEPOSIT, &None);

    let still_ok = funded_sub(&te, &subscriber);
    te.jump(INTERVAL + 2);

    let results = te.client.batch_charge(&vec![&te.env, expired, still_ok], &0u64);

    assert!(!results.get(0).unwrap().success);
    assert_eq!(
        results.get(0).unwrap().error_code,
        Error::SubscriptionExpired.to_code()
    );
    assert!(results.get(1).unwrap().success);
    assert_eq!(charged(&te, &still_ok), AMOUNT);
}

#[test]
fn batch_charge_duplicate_ids_cannot_double_charge() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let id = funded_sub(&te, &subscriber);
    te.jump(INTERVAL);

    let results = te.client.batch_charge(&vec![&te.env, id, id], &0u64);

    assert_eq!(results.len(), 2);
    assert!(results.get(0).unwrap().success);
    assert_eq!(results.get(0).unwrap().error_code, 0);
    // The second occurrence of the same id is rejected by the same-period
    // replay guard, so duplicates inside one batch cannot bill twice.
    assert!(!results.get(1).unwrap().success);
    assert_eq!(results.get(1).unwrap().error_code, Error::Replay.to_code());
    assert_eq!(charged(&te, &id), AMOUNT);
}

// ── Empty batch ─────────────────────────────────────────────────────────────

#[test]
fn batch_charge_empty_batch_charges_nothing_but_still_consumes_a_nonce() {
    let te = TestEnv::default();
    let before = te.client.get_subscription_count();
    let empty: Vec<u32> = Vec::new(&te.env);

    let results = te.client.batch_charge(&empty, &0u64);

    assert_eq!(results.len(), 0);
    assert_eq!(te.client.get_subscription_count(), before);
    // Unlike the bulk pause/cancel tooling, the batch-charge nonce is consumed
    // before the loop runs, so an empty batch still burns one nonce.
    assert_eq!(batch_nonce(&te), 1);
    let replay = batch_charge_err(&te, &empty, 0);
    assert_eq!(replay, Error::NonceAlreadyUsed);
}

// ── Nonce contract ──────────────────────────────────────────────────────────

#[test]
fn batch_charge_rejects_a_wrong_nonce_without_consuming_one() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let id = funded_sub(&te, &subscriber);
    te.jump(INTERVAL);

    let err = batch_charge_err(&te, &vec![&te.env, id], 7);

    assert_eq!(err, Error::NonceAlreadyUsed);
    // Neither the nonce nor the subscription moved.
    assert_eq!(batch_nonce(&te), 0);
    assert_eq!(charged(&te, &id), 0);
}

#[test]
fn batch_charge_rejects_a_replayed_nonce() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let a = funded_sub(&te, &subscriber);
    let b = funded_sub(&te, &subscriber);
    te.jump(INTERVAL);

    te.client.batch_charge(&vec![&te.env, a], &0u64);
    let err = batch_charge_err(&te, &vec![&te.env, b], 0);

    assert_eq!(err, Error::NonceAlreadyUsed);
    assert_eq!(charged(&te, &b), 0, "the replayed batch must not charge");
    assert_eq!(batch_nonce(&te), 1);
}

#[test]
fn batch_charge_consumes_the_nonce_even_when_every_id_fails() {
    let te = TestEnv::default();
    let missing: Vec<u32> = vec![&te.env, 10_001u32, 10_002u32];

    let results = te.client.batch_charge(&missing, &0u64);

    assert_eq!(results.len(), 2);
    assert!(!results.get(0).unwrap().success);
    assert!(!results.get(1).unwrap().success);
    // The batch is non-empty, so its nonce is spent even though nothing charged.
    assert_eq!(batch_nonce(&te), 1);
}

#[test]
fn batch_charge_nonce_domain_is_independent_of_the_operator_domain() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let paused = funded_sub(&te, &subscriber);
    let charged_id = funded_sub(&te, &subscriber);

    // `bulk_pause_subscriptions` consumes DOMAIN_OPERATOR_BATCH_CHARGE for the
    // admin signer; it must not touch the batch-charge counter.
    te.client
        .bulk_pause_subscriptions(&te.admin, &vec![&te.env, paused], &0u64);

    assert_eq!(
        te.client
            .get_admin_nonce(&te.admin, &DOMAIN_OPERATOR_BATCH_CHARGE),
        1
    );
    assert_eq!(batch_nonce(&te), 0, "batch-charge nonce must be untouched");

    // Nonce 0 is therefore still valid in the batch-charge domain.
    te.jump(INTERVAL);
    let results = te.client.batch_charge(&vec![&te.env, charged_id], &0u64);
    assert!(results.get(0).unwrap().success);
    assert_eq!(batch_nonce(&te), 1);
}

// ── Emergency stop ──────────────────────────────────────────────────────────

#[test]
fn batch_charge_is_blocked_by_an_emergency_stop_without_burning_a_nonce() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let id = funded_sub(&te, &subscriber);
    te.jump(INTERVAL);

    te.client.enable_emergency_stop(&te.admin);

    let err = batch_charge_err(&te, &vec![&te.env, id], 0);
    assert_eq!(err, Error::EmergencyStopActive);
    // The emergency-stop guard runs before the nonce check, so nothing is spent
    // and the same nonce works once the stop is lifted.
    assert_eq!(batch_nonce(&te), 0);
    assert_eq!(charged(&te, &id), 0);

    // `disable_emergency_stop` shares the per-key admin config cooldown
    // (`CONFIG_COOLDOWN_SECS`, 6h) with the enable call, so wait it out first.
    te.jump(6 * 60 * 60 + 1);
    te.client.disable_emergency_stop(&te.admin);
    let results = te.client.batch_charge(&vec![&te.env, id], &0u64);
    assert!(results.get(0).unwrap().success);
    assert_eq!(batch_nonce(&te), 1);
}
