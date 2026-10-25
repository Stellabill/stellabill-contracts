//! Adversarial coverage for `do_respond_dispute` in `dispute.rs`.
//!
//! Exercises the full adversarial surface of the respond step of the dispute
//! lifecycle:
//!
//! * Authorization — only the stored admin may respond.
//! * Valid transitions — `Open -> Responded` succeeds.
//! * Invalid transitions — `Responded -> Responded` and resolved states are
//!   rejected.
//! * Missing dispute — unknown IDs are rejected.
//! * Evidence hash — both `Some` and `None` are accepted and persisted.
//! * Storage integrity — rejected calls leave dispute state unchanged.
//! * Event emission — the `dispute_responded` event is always emitted on
//!   success with the correct fields.
//! * Timing — `responded_at` is set to the current ledger timestamp.
//! * Post-rotation admin — after `rotate_admin` only the new admin may respond.

use crate::{
    test_utils::{fixtures, setup::TestEnv},
    DataKey, DisputeRespondedEvent, DisputeStatus, Error, SubscriptionStatus,
    DISPUTE_WINDOW_SECS,
};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    Address, BytesN, FromVal, Symbol,
};

// ── Constants ─────────────────────────────────────────────────────────────────

const DISPUTE_AMOUNT: i128 = 5_000_000;
const T0: u64 = 10_000;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Create an active subscription, seed the merchant balance, open a dispute
/// and return `(dispute_id, subscription_id)`.
fn open_dispute(t: &TestEnv) -> (u64, u32) {
    let (sub_id, subscriber, _) =
        fixtures::create_subscription(&t.env, &t.client, SubscriptionStatus::Active);
    let sub = t.client.get_subscription(&sub_id);

    // Seed merchant balance so the escrow deduction succeeds.
    t.env.as_contract(&t.client.address, || {
        t.env.storage().instance().set(
            &DataKey::MerchantBalance(sub.merchant.clone(), sub.token.clone()),
            &DISPUTE_AMOUNT,
        );
    });
    // Mint the same amount into the vault so token arithmetic holds.
    soroban_sdk::token::StellarAssetClient::new(&t.env, &t.token)
        .mint(&t.client.address, &DISPUTE_AMOUNT);

    let dispute_id = t.client.open_dispute(
        &subscriber,
        &sub_id,
        &DISPUTE_AMOUNT,
        &None::<BytesN<32>>,
    );

    (dispute_id, sub_id)
}

/// Return the on-chain [`crate::Dispute`] record for `dispute_id`.
fn read_dispute(t: &TestEnv, dispute_id: u64) -> crate::Dispute {
    t.client.get_dispute(&dispute_id)
}

/// Build a 32-byte evidence hash from a seed byte.
fn evidence_hash(seed: u8) -> Option<BytesN<32>> {
    Some(BytesN::from_array(&Env::default(), &[seed; 32]))
}

// ── Authorization tests ───────────────────────────────────────────────────────

#[test]
fn test_respond_dispute_admin_succeeds() {
    let t = TestEnv::default();
    t.env.ledger().with_mut(|l| l.timestamp = T0);
    let (dispute_id, _) = open_dispute(&t);

    t.client
        .respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>)
        .expect("admin respond must succeed");

    let dispute = read_dispute(&t, dispute_id);
    assert_eq!(dispute.status, DisputeStatus::Responded);
}

#[test]
fn test_respond_dispute_non_admin_rejected() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);
    let stranger = Address::generate(&t.env);

    // Force stranger auth — admin identity check must still reject it.
    t.env.set_auths(&[stranger.clone()]);
    let result = t
        .client
        .try_respond_dispute(&stranger, &dispute_id, &None::<BytesN<32>>);

    assert!(result.is_err(), "non-admin must be rejected");
}

#[test]
fn test_respond_dispute_wrong_admin_rejected() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);

    // A second address that has valid auth tokens but is not stored as admin.
    let wrong_admin = Address::generate(&t.env);
    let result = t
        .client
        .try_respond_dispute(&wrong_admin, &dispute_id, &None::<BytesN<32>>);

    assert!(result.is_err(), "wrong admin address must be rejected");
}

#[test]
fn test_respond_dispute_new_admin_after_rotation_succeeds() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);
    let new_admin = Address::generate(&t.env);

    // Rotate admin to `new_admin`.
    t.client
        .rotate_admin(&t.admin, &new_admin, &0u64)
        .expect("rotation must succeed");

    // New admin can respond.
    t.client
        .respond_dispute(&new_admin, &dispute_id, &None::<BytesN<32>>)
        .expect("new admin must be able to respond after rotation");

    assert_eq!(
        read_dispute(&t, dispute_id).status,
        DisputeStatus::Responded
    );
}

#[test]
fn test_respond_dispute_old_admin_rejected_after_rotation() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);
    let new_admin = Address::generate(&t.env);

    t.client
        .rotate_admin(&t.admin, &new_admin, &0u64)
        .expect("rotation must succeed");

    // Old admin is now a stranger.
    let old_admin = t.admin.clone();
    let result = t
        .client
        .try_respond_dispute(&old_admin, &dispute_id, &None::<BytesN<32>>);

    assert!(result.is_err(), "old admin must be rejected after rotation");
}

// ── State-transition / invalid-status tests ───────────────────────────────────

#[test]
fn test_respond_dispute_already_responded_rejected() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);

    // First response succeeds.
    t.client
        .respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>)
        .expect("first respond must succeed");

    // Second response must fail.
    let result = t
        .client
        .try_respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>);
    assert_eq!(result, Err(Ok(Error::DisputeAlreadyResponded)));
}

#[test]
fn test_respond_dispute_after_resolve_to_merchant_rejected() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);

    t.client
        .respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>)
        .unwrap();
    t.client
        .resolve_dispute(&t.admin, &dispute_id, &false)
        .unwrap();

    let result = t
        .client
        .try_respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>);
    assert_eq!(result, Err(Ok(Error::DisputeAlreadyResponded)));
}

#[test]
fn test_respond_dispute_after_resolve_to_subscriber_rejected() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);

    t.client
        .respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>)
        .unwrap();
    t.client
        .resolve_dispute(&t.admin, &dispute_id, &true)
        .unwrap();

    let result = t
        .client
        .try_respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>);
    assert_eq!(result, Err(Ok(Error::DisputeAlreadyResponded)));
}

#[test]
fn test_respond_dispute_nonexistent_id_rejected() {
    let t = TestEnv::default();

    let result = t
        .client
        .try_respond_dispute(&t.admin, &9_999_999u64, &None::<BytesN<32>>);
    assert_eq!(result, Err(Ok(Error::DisputeNotFound)));
}

#[test]
fn test_respond_dispute_id_zero_when_no_disputes_rejected() {
    let t = TestEnv::default();

    // Dispute ID 0 has never been created.
    let result = t
        .client
        .try_respond_dispute(&t.admin, &0u64, &None::<BytesN<32>>);
    assert_eq!(result, Err(Ok(Error::DisputeNotFound)));
}

#[test]
fn test_respond_dispute_max_u64_id_rejected() {
    let t = TestEnv::default();

    let result = t
        .client
        .try_respond_dispute(&t.admin, &u64::MAX, &None::<BytesN<32>>);
    assert_eq!(result, Err(Ok(Error::DisputeNotFound)));
}

// ── Evidence hash tests ───────────────────────────────────────────────────────

#[test]
fn test_respond_dispute_with_none_evidence_hash() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);

    t.client
        .respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>)
        .unwrap();

    let dispute = read_dispute(&t, dispute_id);
    assert_eq!(dispute.admin_evidence_hash, None);
}

#[test]
fn test_respond_dispute_with_some_evidence_hash() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);

    let hash = evidence_hash(0xAB);
    t.client
        .respond_dispute(&t.admin, &dispute_id, &hash)
        .unwrap();

    let dispute = read_dispute(&t, dispute_id);
    assert_eq!(
        dispute.admin_evidence_hash, hash,
        "evidence hash must be stored"
    );
}

#[test]
fn test_respond_dispute_evidence_hash_all_zeros() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);

    let hash = evidence_hash(0x00);
    t.client
        .respond_dispute(&t.admin, &dispute_id, &hash)
        .unwrap();

    assert_eq!(read_dispute(&t, dispute_id).admin_evidence_hash, hash);
}

#[test]
fn test_respond_dispute_evidence_hash_all_ff() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);

    let hash = evidence_hash(0xFF);
    t.client
        .respond_dispute(&t.admin, &dispute_id, &hash)
        .unwrap();

    assert_eq!(read_dispute(&t, dispute_id).admin_evidence_hash, hash);
}

#[test]
fn test_respond_dispute_overwrite_subscriber_evidence_hash() {
    // When subscriber opened with evidence, admin evidence is a separate field.
    let t = TestEnv::default();
    let (sub_id, subscriber, _) =
        fixtures::create_subscription(&t.env, &t.client, SubscriptionStatus::Active);
    let sub = t.client.get_subscription(&sub_id);

    t.env.as_contract(&t.client.address, || {
        t.env.storage().instance().set(
            &DataKey::MerchantBalance(sub.merchant.clone(), sub.token.clone()),
            &DISPUTE_AMOUNT,
        );
    });
    soroban_sdk::token::StellarAssetClient::new(&t.env, &t.token)
        .mint(&t.client.address, &DISPUTE_AMOUNT);

    let sub_hash = evidence_hash(0x11);
    let dispute_id = t.client.open_dispute(&subscriber, &sub_id, &DISPUTE_AMOUNT, &sub_hash);

    let admin_hash = evidence_hash(0x22);
    t.client
        .respond_dispute(&t.admin, &dispute_id, &admin_hash)
        .unwrap();

    let dispute = read_dispute(&t, dispute_id);
    // Subscriber's original evidence must be preserved.
    assert_eq!(dispute.evidence_hash, sub_hash, "subscriber evidence unchanged");
    // Admin evidence is stored in its own field.
    assert_eq!(dispute.admin_evidence_hash, admin_hash, "admin evidence stored");
}

// ── Timing / timestamp tests ──────────────────────────────────────────────────

#[test]
fn test_respond_dispute_responded_at_timestamp_set() {
    let t = TestEnv::default();
    t.env.ledger().with_mut(|l| l.timestamp = T0);
    let (dispute_id, _) = open_dispute(&t);

    let respond_ts = T0 + 1_000;
    t.env.ledger().with_mut(|l| l.timestamp = respond_ts);

    t.client
        .respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>)
        .unwrap();

    let dispute = read_dispute(&t, dispute_id);
    assert_eq!(
        dispute.responded_at,
        Some(respond_ts),
        "responded_at must equal ledger timestamp at time of call"
    );
}

#[test]
fn test_respond_dispute_valid_within_window() {
    let t = TestEnv::default();
    t.env.ledger().with_mut(|l| l.timestamp = T0);
    let (dispute_id, _) = open_dispute(&t);

    // Respond just before the dispute window closes.
    t.jump(DISPUTE_WINDOW_SECS - 1);

    t.client
        .respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>)
        .expect("respond within window must succeed");

    assert_eq!(
        read_dispute(&t, dispute_id).status,
        DisputeStatus::Responded
    );
}

#[test]
fn test_respond_dispute_after_window_elapsed_succeeds() {
    // The respond step has no time-gate; it remains valid even after the
    // dispute window elapses (resolve is the time-sensitive step).
    let t = TestEnv::default();
    t.env.ledger().with_mut(|l| l.timestamp = T0);
    let (dispute_id, _) = open_dispute(&t);

    t.jump(DISPUTE_WINDOW_SECS + 1);

    t.client
        .respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>)
        .expect("respond after window must still succeed");

    assert_eq!(
        read_dispute(&t, dispute_id).status,
        DisputeStatus::Responded
    );
}

#[test]
fn test_respond_dispute_exactly_at_window_boundary() {
    let t = TestEnv::default();
    t.env.ledger().with_mut(|l| l.timestamp = T0);
    let (dispute_id, _) = open_dispute(&t);

    t.jump(DISPUTE_WINDOW_SECS);

    t.client
        .respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>)
        .expect("respond at exact window boundary must succeed");
}

// ── State-isolation tests (rejected calls leave state unchanged) ──────────────

#[test]
fn test_respond_dispute_rejected_leaves_status_unchanged() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);

    // Capture state before the failing call.
    let before = read_dispute(&t, dispute_id);
    assert_eq!(before.status, DisputeStatus::Open);

    // Try to respond with wrong admin — must fail.
    let wrong = Address::generate(&t.env);
    let _ = t
        .client
        .try_respond_dispute(&wrong, &dispute_id, &None::<BytesN<32>>);

    let after = read_dispute(&t, dispute_id);
    assert_eq!(
        after.status,
        DisputeStatus::Open,
        "status must remain Open after rejected call"
    );
    assert_eq!(
        after.responded_at, None,
        "responded_at must remain None after rejected call"
    );
    assert_eq!(
        after.admin_evidence_hash, None,
        "admin_evidence_hash must remain None after rejected call"
    );
}

#[test]
fn test_respond_dispute_double_respond_leaves_state_unchanged() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);

    t.client
        .respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>)
        .unwrap();
    let after_first = read_dispute(&t, dispute_id);

    // Second respond must fail and leave state from first respond intact.
    let _ = t
        .client
        .try_respond_dispute(&t.admin, &dispute_id, &evidence_hash(0xDE));

    let after_second = read_dispute(&t, dispute_id);
    assert_eq!(after_second.status, after_first.status);
    assert_eq!(after_second.responded_at, after_first.responded_at);
    assert_eq!(
        after_second.admin_evidence_hash,
        after_first.admin_evidence_hash
    );
}

#[test]
fn test_respond_dispute_nonexistent_does_not_create_dispute() {
    let t = TestEnv::default();

    let _ = t
        .client
        .try_respond_dispute(&t.admin, &42u64, &None::<BytesN<32>>);

    // No dispute with id 42 should have been created.
    let result = t.client.try_get_dispute(&42u64);
    assert!(result.is_err(), "phantom dispute must not exist after rejected respond");
}

// ── Event-emission tests ──────────────────────────────────────────────────────

#[test]
fn test_respond_dispute_emits_dispute_responded_event() {
    let t = TestEnv::default();
    t.env.ledger().with_mut(|l| l.timestamp = T0);
    let (dispute_id, sub_id) = open_dispute(&t);

    let respond_ts = T0 + 500;
    t.env.ledger().with_mut(|l| l.timestamp = respond_ts);

    let hash = evidence_hash(0xCC);
    t.client
        .respond_dispute(&t.admin, &dispute_id, &hash)
        .unwrap();

    let events = t.env.events().all();
    let event = events
        .iter()
        .find(|e| {
            Symbol::from_val(&t.env, &e.1.get(0).unwrap())
                == Symbol::new(&t.env, "dispute_responded")
        })
        .expect("dispute_responded event must be emitted");

    let data: DisputeRespondedEvent = event.2.clone().into_val(&t.env);
    assert_eq!(data.dispute_id, dispute_id, "event dispute_id mismatch");
    assert_eq!(data.subscription_id, sub_id, "event subscription_id mismatch");
    assert_eq!(data.admin_evidence_hash, hash, "event evidence hash mismatch");
    assert_eq!(data.timestamp, respond_ts, "event timestamp mismatch");
}

#[test]
fn test_respond_dispute_rejected_emits_no_event() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);
    let event_count_before = t.env.events().all().len();

    let wrong = Address::generate(&t.env);
    let _ = t
        .client
        .try_respond_dispute(&wrong, &dispute_id, &None::<BytesN<32>>);

    let new_events = t.env.events().all().len();
    assert_eq!(
        new_events, event_count_before,
        "no events must be emitted on rejection"
    );
}

#[test]
fn test_respond_dispute_with_none_hash_event_field_is_none() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);

    t.client
        .respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>)
        .unwrap();

    let events = t.env.events().all();
    let event = events
        .iter()
        .find(|e| {
            Symbol::from_val(&t.env, &e.1.get(0).unwrap())
                == Symbol::new(&t.env, "dispute_responded")
        })
        .expect("dispute_responded event must be emitted");

    let data: DisputeRespondedEvent = event.2.clone().into_val(&t.env);
    assert_eq!(
        data.admin_evidence_hash, None,
        "event evidence hash must be None"
    );
}

// ── Multi-dispute isolation tests ─────────────────────────────────────────────

#[test]
fn test_respond_dispute_responds_only_targeted_dispute() {
    let t = TestEnv::default();

    // Open two disputes on separate subscriptions.
    let (dispute_id_1, _) = open_dispute(&t);
    let (dispute_id_2, _) = open_dispute(&t);

    // Respond only to the first.
    t.client
        .respond_dispute(&t.admin, &dispute_id_1, &None::<BytesN<32>>)
        .unwrap();

    assert_eq!(
        read_dispute(&t, dispute_id_1).status,
        DisputeStatus::Responded,
        "first dispute must be Responded"
    );
    assert_eq!(
        read_dispute(&t, dispute_id_2).status,
        DisputeStatus::Open,
        "second dispute must remain Open"
    );
}

#[test]
fn test_respond_dispute_both_disputes_independently() {
    let t = TestEnv::default();

    let (dispute_id_1, _) = open_dispute(&t);
    let (dispute_id_2, _) = open_dispute(&t);

    let hash1 = evidence_hash(0x01);
    let hash2 = evidence_hash(0x02);

    t.client.respond_dispute(&t.admin, &dispute_id_1, &hash1).unwrap();
    t.client.respond_dispute(&t.admin, &dispute_id_2, &hash2).unwrap();

    assert_eq!(read_dispute(&t, dispute_id_1).admin_evidence_hash, hash1);
    assert_eq!(read_dispute(&t, dispute_id_2).admin_evidence_hash, hash2);
}

#[test]
fn test_respond_dispute_sequential_id_disputes() {
    // Verify the dispute ID counter increments correctly and each dispute
    // can be targeted independently.
    let t = TestEnv::default();

    let (id0, _) = open_dispute(&t);
    let (id1, _) = open_dispute(&t);
    let (id2, _) = open_dispute(&t);

    // IDs must be sequential.
    assert_eq!(id1, id0 + 1);
    assert_eq!(id2, id0 + 2);

    // Respond to the middle one only.
    t.client
        .respond_dispute(&t.admin, &id1, &None::<BytesN<32>>)
        .unwrap();

    assert_eq!(read_dispute(&t, id0).status, DisputeStatus::Open);
    assert_eq!(read_dispute(&t, id1).status, DisputeStatus::Responded);
    assert_eq!(read_dispute(&t, id2).status, DisputeStatus::Open);
}

// ── Full lifecycle integration tests ─────────────────────────────────────────

#[test]
fn test_respond_then_resolve_to_merchant_full_lifecycle() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);

    t.client
        .respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>)
        .unwrap();
    t.client
        .resolve_dispute(&t.admin, &dispute_id, &false)
        .expect("resolve to merchant must succeed after respond");

    let dispute = read_dispute(&t, dispute_id);
    assert_eq!(dispute.status, DisputeStatus::ResolvedToMerchant);
}

#[test]
fn test_respond_then_resolve_to_subscriber_full_lifecycle() {
    let t = TestEnv::default();
    let (dispute_id, _) = open_dispute(&t);

    t.client
        .respond_dispute(&t.admin, &dispute_id, &None::<BytesN<32>>)
        .unwrap();
    t.client
        .resolve_dispute(&t.admin, &dispute_id, &true)
        .expect("resolve to subscriber must succeed after respond");

    let dispute = read_dispute(&t, dispute_id);
    assert_eq!(dispute.status, DisputeStatus::ResolvedToSubscriber);
}

#[test]
fn test_respond_dispute_resolve_without_respond_before_window_rejected() {
    // Before window elapses, resolution without a prior respond must fail.
    let t = TestEnv::default();
    t.env.ledger().with_mut(|l| l.timestamp = T0);
    let (dispute_id, _) = open_dispute(&t);

    // No respond call — try to resolve.
    let result = t.client.try_resolve_dispute(&t.admin, &dispute_id, &true);
    assert_eq!(
        result,
        Err(Ok(Error::DisputeNotResponded)),
        "resolve without respond before window must be rejected"
    );
}

#[test]
fn test_respond_dispute_idempotency_check_after_respond() {
    // After a successful respond, the dispute fields must be immutable.
    let t = TestEnv::default();
    t.env.ledger().with_mut(|l| l.timestamp = T0);
    let (dispute_id, _) = open_dispute(&t);

    let ts1 = T0 + 100;
    t.env.ledger().with_mut(|l| l.timestamp = ts1);
    t.client
        .respond_dispute(&t.admin, &dispute_id, &evidence_hash(0x11))
        .unwrap();

    let snapshot = read_dispute(&t, dispute_id);

    // Advance time and attempt another respond — must fail.
    t.env.ledger().with_mut(|l| l.timestamp = T0 + 200);
    let _ = t
        .client
        .try_respond_dispute(&t.admin, &dispute_id, &evidence_hash(0x22));

    let after = read_dispute(&t, dispute_id);
    assert_eq!(after.status, snapshot.status);
    assert_eq!(after.responded_at, snapshot.responded_at);
    assert_eq!(after.admin_evidence_hash, snapshot.admin_evidence_hash);
}
