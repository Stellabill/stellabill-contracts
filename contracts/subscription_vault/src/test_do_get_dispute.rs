#![cfg(test)]

//! Adversarial coverage for `do_get_dispute` in `dispute.rs` (issue #1045).
//!
//! `do_get_dispute` is the read-side entrypoint of the dispute workflow
//! (`DataKey::Dispute(dispute_id)` in persistent storage). It is a pure read:
//! it performs no auth, touches no other key, and returns either the stored
//! [`Dispute`] or [`Error::DisputeNotFound`].
//!
//! The existing unit tests in `dispute.rs` and `test.rs` call `get_dispute`
//! only incidentally while asserting resolution *behaviour*. This file pins
//! the reader itself:
//!
//! * Full field fidelity on open (with and without `evidence_hash`).
//! * Every lifecycle state is observable through the reader
//!   (`Open` → `Responded` → resolved, plus auto-resolve).
//! * Unknown / boundary IDs (`0`, gap IDs, `u64::MAX`) return the typed
//!   `DisputeNotFound` error instead of panicking.
//! * The record is *retained* after resolution even though the
//!   subscription-dispute index is cleared.
//! * Rejected operations (`respond` twice, resolve-before-response,
//!   resolve twice, under-funded open) leave the stored record byte-for-byte
//!   unchanged.
//! * The reader is pure: escrow, index and record are untouched by any number
//!   of reads.
//! * Direct-storage injection/removal round-trips, proving the exact key is
//!   read (and that the reader does not fall back to the index).
//! * Cross-subscription isolation and concurrency of multiple disputes.
//! * No auth is required — a caller with no signatures can still read.

use crate::dispute::do_get_dispute;
use crate::test_utils::{fixtures, setup::TestEnv};
use crate::types::{DataKey, Dispute, DisputeStatus, Error, DISPUTE_WINDOW_SECS};
use crate::SubscriptionStatus;
use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, Address, BytesN};

// ── Constants ─────────────────────────────────────────────────────────────────

/// 5 USDC (6 decimals) — small enough to keep arithmetic obvious in asserts.
const DISPUTE_AMOUNT: i128 = 5_000_000;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Create a fresh subscription and seed enough merchant balance + vault tokens
/// to open one dispute of [`DISPUTE_AMOUNT`].
fn setup_with_funded_merchant(test_env: &TestEnv) -> (u32, Address, Address) {
    let (sub_id, subscriber, merchant) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    seed_merchant_balance(test_env, sub_id);
    (sub_id, subscriber, merchant)
}

/// Seed the merchant balance for a subscription directly into instance storage
/// and mint matching tokens into the vault so resolution transfers can settle.
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

/// Open a dispute with no evidence and return its ID.
fn open_dispute(test_env: &TestEnv, subscriber: &Address, sub_id: u32) -> u64 {
    test_env
        .client
        .open_dispute(subscriber, &sub_id, &DISPUTE_AMOUNT, &None::<BytesN<32>>)
}

/// Compare every field of two dispute records.
///
/// `Dispute` does not derive `PartialEq`, so equality is asserted explicitly;
/// listing the fields also means a newly-added field forces a compile-time
/// review of this helper rather than silently weakening the check.
#[track_caller]
fn assert_same_dispute(a: &Dispute, b: &Dispute) {
    assert_eq!(a.id, b.id, "id changed");
    assert_eq!(
        a.subscription_id, b.subscription_id,
        "subscription_id changed"
    );
    assert_eq!(a.subscriber, b.subscriber, "subscriber changed");
    assert_eq!(a.merchant, b.merchant, "merchant changed");
    assert_eq!(a.amount, b.amount, "amount changed");
    assert_eq!(a.opened_at, b.opened_at, "opened_at changed");
    assert_eq!(a.status, b.status, "status changed");
    assert_eq!(a.evidence_hash, b.evidence_hash, "evidence_hash changed");
    assert_eq!(a.responded_at, b.responded_at, "responded_at changed");
    assert_eq!(
        a.admin_evidence_hash, b.admin_evidence_hash,
        "admin_evidence_hash changed"
    );
}

// ── Full-field fidelity on open ───────────────────────────────────────────────

/// The record read back after `open_dispute` must reproduce *every* field that
/// `do_open_dispute` wrote — including the evidence hash and the two `None`
/// response fields.
#[test]
fn get_returns_full_record_after_open_with_evidence() {
    let te = TestEnv::default();
    let (sub_id, subscriber, merchant) = setup_with_funded_merchant(&te);

    let evidence = BytesN::from_array(&te.env, &[0xAB; 32]);
    let opened_at = te.env.ledger().timestamp();
    let dispute_id = te.client.open_dispute(
        &subscriber,
        &sub_id,
        &DISPUTE_AMOUNT,
        &Some(evidence.clone()),
    );

    let got = te.client.get_dispute(&dispute_id);

    assert_eq!(got.id, dispute_id);
    assert_eq!(got.subscription_id, sub_id);
    assert_eq!(got.subscriber, subscriber);
    assert_eq!(got.merchant, merchant);
    assert_eq!(got.amount, DISPUTE_AMOUNT);
    assert_eq!(got.opened_at, opened_at);
    assert_eq!(got.status, DisputeStatus::Open);
    assert_eq!(got.evidence_hash, Some(evidence));
    assert_eq!(got.responded_at, None);
    assert_eq!(got.admin_evidence_hash, None);
}

/// Opening without evidence must round-trip `evidence_hash == None` (not a
/// zero-filled `Some`).
#[test]
fn get_returns_none_evidence_when_opened_without_evidence() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);
    let got = te.client.get_dispute(&dispute_id);

    assert_eq!(got.evidence_hash, None);
    assert_eq!(got.status, DisputeStatus::Open);
}

/// The very first dispute is allocated ID `0`; the reader must return it
/// (guarding against any `0`-as-sentinel confusion).
#[test]
fn get_returns_dispute_id_zero() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);

    // The counter starts at 0 on a fresh env, so the first dispute is ID 0.
    assert_eq!(dispute_id, 0, "first dispute must be allocated ID 0");
    assert_eq!(te.client.get_dispute(&dispute_id).id, 0);
}

// ── Lifecycle observability ───────────────────────────────────────────────────

/// `Responded` transition (status, `responded_at`, `admin_evidence_hash`) is
/// visible through the reader.
#[test]
fn get_reflects_responded_transition() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);

    let responded_at = te.env.ledger().timestamp() + 123;
    te.env.ledger().set_timestamp(responded_at);
    let admin_evidence = BytesN::from_array(&te.env, &[0x11; 32]);
    te.client
        .respond_dispute(&te.admin, &dispute_id, &Some(admin_evidence.clone()));

    let got = te.client.get_dispute(&dispute_id);
    assert_eq!(got.status, DisputeStatus::Responded);
    assert_eq!(got.responded_at, Some(responded_at));
    assert_eq!(got.admin_evidence_hash, Some(admin_evidence));
    // Open-time fields must be untouched by the response.
    assert_eq!(got.amount, DISPUTE_AMOUNT);
    assert_eq!(got.evidence_hash, None);
}

/// Resolution to the merchant is observable and the record is retained.
#[test]
fn get_reflects_resolution_to_merchant() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);
    te.client
        .respond_dispute(&te.admin, &dispute_id, &None::<BytesN<32>>);
    te.client.resolve_dispute(&te.admin, &dispute_id, &false);

    let got = te.client.get_dispute(&dispute_id);
    assert_eq!(got.status, DisputeStatus::ResolvedToMerchant);
}

/// Resolution to the subscriber is observable and the record is retained.
#[test]
fn get_reflects_resolution_to_subscriber() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);
    te.client
        .respond_dispute(&te.admin, &dispute_id, &None::<BytesN<32>>);
    te.client.resolve_dispute(&te.admin, &dispute_id, &true);

    let got = te.client.get_dispute(&dispute_id);
    assert_eq!(got.status, DisputeStatus::ResolvedToSubscriber);
}

/// Auto-resolution (window elapsed while still `Open`) lands on
/// `ResolvedToSubscriber` and remains readable.
#[test]
fn get_reflects_auto_resolution_after_window() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);
    te.jump(DISPUTE_WINDOW_SECS + 1);
    te.client.resolve_dispute(&te.admin, &dispute_id, &false);

    let got = te.client.get_dispute(&dispute_id);
    assert_eq!(got.status, DisputeStatus::ResolvedToSubscriber);
}

/// The dispute record survives resolution even though the subscription-level
/// index is cleared — the reader must not be built on top of the index.
#[test]
fn get_still_returns_record_after_index_is_cleared() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);
    te.client
        .respond_dispute(&te.admin, &dispute_id, &None::<BytesN<32>>);
    te.client.resolve_dispute(&te.admin, &dispute_id, &false);

    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        None,
        "subscription index must be cleared on resolution"
    );
    // ...but the historical record is still readable by its own ID.
    let got = te.client.get_dispute(&dispute_id);
    assert_eq!(got.id, dispute_id);
    assert_eq!(got.status, DisputeStatus::ResolvedToMerchant);
}

// ── Unknown and boundary IDs ──────────────────────────────────────────────────

/// Assert the reader reports `DisputeNotFound` for `id`.
///
/// `Dispute` does not implement `PartialEq`, so the error is matched rather
/// than compared as part of a `Result`.
#[track_caller]
fn assert_not_found(test_env: &TestEnv, id: u64) {
    match test_env.client.try_get_dispute(&id) {
        Err(Ok(e)) => assert_eq!(e, Error::DisputeNotFound),
        _other => panic!("expected DisputeNotFound for id {id}, got a different result"),
    }
}

/// Unknown IDs must produce the typed error, never a panic.
#[test]
fn get_returns_dispute_not_found_for_unknown_ids() {
    let te = TestEnv::default();
    assert_not_found(&te, 0);
    assert_not_found(&te, 1);
    assert_not_found(&te, 42);
    assert_not_found(&te, u64::MAX);
}

/// Gap IDs: after opening exactly one dispute (ID 0), neighbors stay absent.
#[test]
fn get_returns_not_found_for_gap_ids() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);
    assert_eq!(dispute_id, 0);

    assert_not_found(&te, 1);
    assert_not_found(&te, u64::MAX);
}

/// `u64::MAX` is a valid key: a record injected there must be returned intact.
#[test]
fn get_returns_injected_record_at_u64_max_id() {
    let te = TestEnv::default();
    let (sub_id, _subscriber, merchant) = setup_with_funded_merchant(&te);

    let injected = Dispute {
        id: u64::MAX,
        subscription_id: sub_id,
        subscriber: Address::generate(&te.env),
        merchant,
        amount: 1i128,
        opened_at: 7,
        status: DisputeStatus::Responded,
        evidence_hash: None,
        responded_at: Some(8),
        admin_evidence_hash: None,
    };
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .persistent()
            .set(&DataKey::Dispute(u64::MAX), &injected);
    });

    let got = te.client.get_dispute(&u64::MAX);
    assert_eq!(got.id, u64::MAX);
    assert_eq!(got.opened_at, 7);
    assert_eq!(got.responded_at, Some(8));
}

// ── Purity / no state mutation ────────────────────────────────────────────────

/// Reading repeatedly must not change the record, the escrow ledger or the
/// subscription index.
#[test]
fn get_is_a_pure_read() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);
    let before = te.client.get_dispute(&dispute_id);

    for _ in 0..5 {
        let got = te.client.get_dispute(&dispute_id);
        assert_same_dispute(&before, &got);
    }

    // Escrow untouched.
    te.env.as_contract(&te.client.address, || {
        let ledger = te
            .env
            .storage()
            .instance()
            .get::<_, crate::types::DisputeEscrowLedger>(&DataKey::DisputeEscrow(dispute_id))
            .expect("escrow must still be present after reads");
        assert_eq!(ledger.original_amount, DISPUTE_AMOUNT);
        assert_eq!(ledger.total_disbursed, 0);
    });

    // Subscription index untouched.
    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        Some(dispute_id)
    );
}

/// Reading an absent key repeatedly keeps returning the same error.
#[test]
fn get_of_absent_key_is_deterministic() {
    let te = TestEnv::default();
    for _ in 0..5 {
        assert_not_found(&te, 99);
    }
}

// ── State unchanged after rejected operations ─────────────────────────────────

/// A second `respond_dispute` is rejected and must leave the record as it was
/// after the first (successful) response.
#[test]
fn rejected_double_respond_leaves_record_unchanged() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);
    te.client
        .respond_dispute(&te.admin, &dispute_id, &None::<BytesN<32>>);
    let before = te.client.get_dispute(&dispute_id);

    let res = te
        .client
        .try_respond_dispute(&te.admin, &dispute_id, &None::<BytesN<32>>);
    assert_eq!(res, Err(Ok(Error::DisputeAlreadyResponded)));

    let after = te.client.get_dispute(&dispute_id);
    assert_same_dispute(&before, &after);
    assert_eq!(after.status, DisputeStatus::Responded);
}

/// Resolving before the admin responds (window not elapsed) is rejected and
/// must not perturb the record.
#[test]
fn rejected_resolve_before_response_leaves_record_unchanged() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);
    let before = te.client.get_dispute(&dispute_id);

    let res = te.client.try_resolve_dispute(&te.admin, &dispute_id, &true);
    assert_eq!(res, Err(Ok(Error::DisputeNotResponded)));

    let after = te.client.get_dispute(&dispute_id);
    assert_same_dispute(&before, &after);
    assert_eq!(after.status, DisputeStatus::Open);
    assert_eq!(after.responded_at, None);
}

/// A second resolution is rejected and must not flip the terminal status.
#[test]
fn rejected_double_resolve_leaves_record_unchanged() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);
    te.client
        .respond_dispute(&te.admin, &dispute_id, &None::<BytesN<32>>);
    te.client.resolve_dispute(&te.admin, &dispute_id, &false);
    let before = te.client.get_dispute(&dispute_id);

    let res = te.client.try_resolve_dispute(&te.admin, &dispute_id, &true);
    assert_eq!(res, Err(Ok(Error::DisputeAlreadyResolved)));

    let after = te.client.get_dispute(&dispute_id);
    assert_same_dispute(&before, &after);
    assert_eq!(after.status, DisputeStatus::ResolvedToMerchant);
}

/// An under-funded `open_dispute` is rejected, allocates no ID and leaves no
/// record behind.
#[test]
fn rejected_open_leaves_no_record_and_no_id_gap() {
    let te = TestEnv::default();
    // Create a subscription but do *not* seed the merchant balance.
    let (sub_id, subscriber, _) =
        fixtures::create_subscription(&te.env, &te.client, SubscriptionStatus::Active);

    let res =
        te.client
            .try_open_dispute(&subscriber, &sub_id, &DISPUTE_AMOUNT, &None::<BytesN<32>>);
    assert_eq!(res, Err(Ok(Error::InsufficientBalance)));

    // No dispute record exists...
    assert_not_found(&te, 0);
    // ...and no index was written.
    assert_eq!(te.client.get_subscription_dispute(&sub_id), None);

    // The rejected attempt did not consume an ID: the next successful open
    // still gets ID 0.
    seed_merchant_balance(&te, sub_id);
    let dispute_id = open_dispute(&te, &subscriber, sub_id);
    assert_eq!(dispute_id, 0);
}

// ── Exact-key semantics ───────────────────────────────────────────────────────

/// Injecting a fully synthetic record under a specific ID must be returned
/// verbatim — the reader must not consult the subscription index or mutate the
/// injected value.
#[test]
fn direct_storage_injection_is_returned_verbatim() {
    let te = TestEnv::default();
    let (sub_id, _subscriber, merchant) = setup_with_funded_merchant(&te);

    let injected = Dispute {
        id: 0xDEAD_BEEF,
        subscription_id: sub_id,
        subscriber: Address::generate(&te.env),
        merchant,
        amount: 123,
        opened_at: 456,
        status: DisputeStatus::Open,
        evidence_hash: Some(BytesN::from_array(&te.env, &[9; 32])),
        responded_at: None,
        admin_evidence_hash: None,
    };
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .persistent()
            .set(&DataKey::Dispute(0xDEAD_BEEF), &injected);
    });

    let got = te.client.get_dispute(&0xDEAD_BEEF);
    assert_eq!(got.id, 0xDEAD_BEEF);
    assert_eq!(got.amount, 123);
    assert_eq!(got.opened_at, 456);
    assert_eq!(
        got.evidence_hash,
        Some(BytesN::from_array(&te.env, &[9; 32]))
    );
}

/// Directly removing the record makes the reader report `DisputeNotFound`.
#[test]
fn direct_storage_removal_makes_get_not_found() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);
    assert!(te.client.get_dispute(&dispute_id).status == DisputeStatus::Open);

    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .persistent()
            .remove(&DataKey::Dispute(dispute_id));
    });

    assert_not_found(&te, dispute_id);
}

/// The index entry alone must not satisfy the reader: an ID pointed to by the
/// subscription index but missing its record must still be `DisputeNotFound`.
#[test]
fn index_without_record_does_not_satisfy_get() {
    let te = TestEnv::default();
    let (sub_id, _subscriber, _) = setup_with_funded_merchant(&te);

    // Write only the index key, never the record.
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::SubscriptionDispute(sub_id), &7u64);
    });

    assert_eq!(te.client.get_subscription_dispute(&sub_id), Some(7u64));
    assert_not_found(&te, 7);
}

// ── Isolation & multiplicity ──────────────────────────────────────────────────

/// Two subscriptions' disputes are addressed independently by ID.
#[test]
fn disputes_for_different_subscriptions_are_isolated() {
    let te = TestEnv::default();
    let (sub_a, subscriber_a, merchant_a) = setup_with_funded_merchant(&te);
    let (sub_b, subscriber_b, merchant_b) = setup_with_funded_merchant(&te);

    let id_a = open_dispute(&te, &subscriber_a, sub_a);
    let id_b = open_dispute(&te, &subscriber_b, sub_b);
    assert_ne!(id_a, id_b);

    let a = te.client.get_dispute(&id_a);
    let b = te.client.get_dispute(&id_b);

    assert_eq!(a.id, id_a);
    assert_eq!(a.subscription_id, sub_a);
    assert_eq!(a.subscriber, subscriber_a);
    assert_eq!(a.merchant, merchant_a);

    assert_eq!(b.id, id_b);
    assert_eq!(b.subscription_id, sub_b);
    assert_eq!(b.subscriber, subscriber_b);
    assert_eq!(b.merchant, merchant_b);
}

/// Many disputes: every ID maps to its own distinct record.
#[test]
fn many_disputes_are_individually_addressable() {
    let te = TestEnv::default();

    let n = 5usize;
    let mut ids = Vec::with_capacity(n);
    let mut subs = Vec::with_capacity(n);

    for _ in 0..n {
        let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);
        let id = open_dispute(&te, &subscriber, sub_id);
        ids.push(id);
        subs.push(sub_id);
    }

    for (i, (&id, &sub_id)) in ids.iter().zip(subs.iter()).enumerate() {
        let got = te.client.get_dispute(&id);
        assert_eq!(got.id, id, "record[{i}] id mismatch");
        assert_eq!(
            got.subscription_id, sub_id,
            "record[{i}] subscription_id mismatch"
        );
    }
}

// ── No auth required ──────────────────────────────────────────────────────────

/// `do_get_dispute` performs no auth: with *all* mock auths cleared, an
/// internal call still returns the record.
#[test]
fn no_auth_required_for_read() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);

    // Drop every mock auth — the next call must not need any signature.
    te.env.set_auths(&[]);

    let res = te
        .env
        .as_contract(&te.client.address, || do_get_dispute(&te.env, dispute_id));
    assert_eq!(res.map(|d| d.id), Ok(dispute_id));
}

/// A stranger's view equals the subscriber's view (no caller-dependent
/// filtering).
#[test]
fn stranger_sees_same_record_as_subscriber() {
    let te = TestEnv::default();
    let (sub_id, subscriber, _) = setup_with_funded_merchant(&te);

    let dispute_id = open_dispute(&te, &subscriber, sub_id);
    let _stranger = Address::generate(&te.env);

    let direct = te.client.get_dispute(&dispute_id);
    let again = te.client.get_dispute(&dispute_id);
    assert_same_dispute(&direct, &again);
}
