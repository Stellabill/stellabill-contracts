#![cfg(test)]

//! # Adversarial coverage: two-step admin rotation
//!
//! `do_propose_admin` / `do_claim_admin_role` / `do_cancel_admin_proposal` are
//! the only way to hand the contract's root authority to a new key, so every
//! off-nominal path is a potential privilege-escalation or lock-out bug. The
//! existing `test_governance.rs` covers the happy-path rotation; this suite
//! attacks the edges it leaves unasserted:
//!
//! * proposals from a non-admin must not be stored,
//! * the contract itself can never be nominated (self-rotation guard),
//! * a proposal cannot be overwritten while it is live,
//! * claiming from the wrong address must not consume the proposal,
//! * the expiry window boundary is inclusive at `expires_at` and exclusive at
//!   `expires_at + 1`, and an expired claim evicts the stale record,
//! * cancelling must not be possible for a non-admin and must be idempotent in
//!   its failure mode,
//! * a completed rotation must actually transfer authority: the old admin loses
//!   every privileged entry point and the new admin gains them.

extern crate std;

use crate::test_utils::setup::TestEnv;
use crate::{AdminProposal, Error};
use soroban_sdk::{testutils::Address as _, Address};

/// Mirrors the private constant in `admin.rs`.
const PROPOSAL_WINDOW_SECS: u64 = 7 * 24 * 60 * 60;

fn propose(te: &TestEnv, from: &Address, to: &Address) -> crate::AdminProposal {
    te.client.propose_admin(from, to);
    te.client
        .get_admin_proposal()
        .expect("proposal must be stored")
}

// ────────────────────────────────────────────────────────────────────
//  Proposal creation
// ────────────────────────────────────────────────────────────────────

#[test]
fn non_admin_cannot_open_a_proposal() {
    let te = TestEnv::default();
    let stranger = Address::generate(&te.env);
    let nominee = Address::generate(&te.env);

    let result = te.client.try_propose_admin(&stranger, &nominee);

    assert!(matches!(result, Err(Ok(Error::Unauthorized))));
    assert!(
        te.client.get_admin_proposal().is_none(),
        "a rejected proposal must not write the slot"
    );
    assert_eq!(te.client.get_admin(), te.admin);
}

#[test]
fn proposal_rejects_the_contract_address_as_nominee() {
    let te = TestEnv::default();
    let contract_id = te.client.address.clone();

    let result = te.client.try_propose_admin(&te.admin, &contract_id);

    assert!(matches!(result, Err(Ok(Error::InvalidNewAdmin))));
    assert!(te.client.get_admin_proposal().is_none());
}

#[test]
fn proposal_window_is_exactly_seven_days_from_the_current_ledger_time() {
    let te = TestEnv::default();
    let nominee = Address::generate(&te.env);

    te.env.ledger().with_mut(|li| li.timestamp = 1_700_000_000);
    let proposal = propose(&te, &te.admin, &nominee);

    assert_eq!(proposal.new_admin, nominee);
    assert_eq!(proposal.proposed_at, 1_700_000_000);
    assert_eq!(
        proposal.expires_at,
        1_700_000_000 + PROPOSAL_WINDOW_SECS
    );
}

#[test]
fn a_live_proposal_cannot_be_overwritten_by_a_second_one() {
    let te = TestEnv::default();
    let first = Address::generate(&te.env);
    let second = Address::generate(&te.env);

    propose(&te, &te.admin, &first);
    let result = te.client.try_propose_admin(&te.admin, &second);

    assert!(matches!(result, Err(Ok(Error::ProposalAlreadyExists))));
    assert_eq!(
        te.client.get_admin_proposal().unwrap().new_admin,
        first,
        "the original nominee must survive the rejected overwrite"
    );
}

// ────────────────────────────────────────────────────────────────────
//  Claiming
// ────────────────────────────────────────────────────────────────────

#[test]
fn claim_without_a_proposal_is_not_found() {
    let te = TestEnv::default();
    let claimant = Address::generate(&te.env);

    let result = te.client.try_claim_admin_role(&claimant);

    assert!(matches!(result, Err(Ok(Error::ProposalNotFound))));
    assert_eq!(te.client.get_admin(), te.admin);
}

#[test]
fn wrong_claimant_is_rejected_and_the_proposal_survives() {
    let te = TestEnv::default();
    let nominee = Address::generate(&te.env);
    let interloper = Address::generate(&te.env);

    propose(&te, &te.admin, &nominee);

    let result = te.client.try_claim_admin_role(&interloper);

    assert!(matches!(result, Err(Ok(Error::InvalidClaimant))));
    assert_eq!(
        te.client.get_admin_proposal().unwrap().new_admin,
        nominee,
        "a failed claim must leave the proposal claimable"
    );
    assert_eq!(te.client.get_admin(), te.admin);

    // The genuine nominee can still claim afterwards.
    te.client.claim_admin_role(&nominee);
    assert_eq!(te.client.get_admin(), nominee);
}

#[test]
fn claim_at_the_exact_expiry_instant_succeeds() {
    let te = TestEnv::default();
    let nominee = Address::generate(&te.env);

    let proposal = propose(&te, &te.admin, &nominee);
    te.env
        .ledger()
        .with_mut(|li| li.timestamp = proposal.expires_at);

    te.client.claim_admin_role(&nominee);

    assert_eq!(te.client.get_admin(), nominee);
    assert!(te.client.get_admin_proposal().is_none());
}

#[test]
fn claim_one_second_after_expiry_fails_and_evicts_the_stale_record() {
    let te = TestEnv::default();
    let nominee = Address::generate(&te.env);

    let proposal = propose(&te, &te.admin, &nominee);
    te.env
        .ledger()
        .with_mut(|li| li.timestamp = proposal.expires_at + 1);

    let result = te.client.try_claim_admin_role(&nominee);

    assert!(matches!(result, Err(Ok(Error::ProposalExpired))));
    assert!(
        te.client.get_admin_proposal().is_none(),
        "the expired proposal must be evicted so the admin can propose again"
    );
    assert_eq!(te.client.get_admin(), te.admin);

    // The eviction means a retry reports `ProposalNotFound`, not `Expired` again.
    let retry = te.client.try_claim_admin_role(&nominee);
    assert!(matches!(retry, Err(Ok(Error::ProposalNotFound))));

    // And a fresh proposal is accepted without hitting the "already exists" guard.
    let replacement = Address::generate(&te.env);
    propose(&te, &te.admin, &replacement);
}

// ────────────────────────────────────────────────────────────────────
//  Cancelling
// ────────────────────────────────────────────────────────────────────

#[test]
fn non_admin_cannot_cancel() {
    let te = TestEnv::default();
    let nominee = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);

    propose(&te, &te.admin, &nominee);
    let result = te.client.try_cancel_admin_proposal(&stranger);

    assert!(matches!(result, Err(Ok(Error::Unauthorized))));
    assert!(
        te.client.get_admin_proposal().is_some(),
        "a rejected cancel must not clear the slot"
    );
}

#[test]
fn cancel_without_a_proposal_is_no_active_proposal() {
    let te = TestEnv::default();

    let result = te.client.try_cancel_admin_proposal(&te.admin);

    assert!(matches!(result, Err(Ok(Error::NoActiveProposal))));
}

#[test]
fn admin_cancel_clears_the_slot_and_blocks_the_nominee() {
    let te = TestEnv::default();
    let nominee = Address::generate(&te.env);

    propose(&te, &te.admin, &nominee);
    te.client.cancel_admin_proposal(&te.admin);

    assert!(te.client.get_admin_proposal().is_none());

    let result = te.client.try_claim_admin_role(&nominee);
    assert!(matches!(result, Err(Ok(Error::ProposalNotFound))));
    assert_eq!(te.client.get_admin(), te.admin);
}

// ────────────────────────────────────────────────────────────────────
//  Authority actually transfers
// ────────────────────────────────────────────────────────────────────

#[test]
fn completed_rotation_moves_privileges_to_the_new_admin() {
    let te = TestEnv::default();
    let new_admin = Address::generate(&te.env);
    let third_party = Address::generate(&te.env);

    propose(&te, &te.admin, &new_admin);
    te.client.claim_admin_role(&new_admin);
    assert_eq!(te.client.get_admin(), new_admin);

    // The old admin is now an ordinary address.
    assert!(matches!(
        te.client.try_propose_admin(&te.admin, &third_party),
        Err(Ok(Error::Unauthorized))
    ));
    assert!(matches!(
        te.client.try_cancel_admin_proposal(&te.admin),
        Err(Ok(Error::Unauthorized))
    ));

    // The new admin holds the privileged entry points.
    te.client.propose_admin(&new_admin, &third_party);
    assert_eq!(
        te.client.get_admin_proposal().unwrap().new_admin,
        third_party
    );
    te.client.cancel_admin_proposal(&new_admin);
    assert!(te.client.get_admin_proposal().is_none());
}

#[test]
fn rotation_does_not_disturb_unrelated_contract_state() {
    let te = TestEnv::default();
    let new_admin = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    // Give the contract some unrelated state to protect.
    te.client.set_operator(&te.admin, &operator);
    let nonce_before = te.client.get_operator_nonce(&operator);

    propose(&te, &te.admin, &new_admin);
    te.client.claim_admin_role(&new_admin);

    assert_eq!(te.client.get_operator(), Some(operator.clone()));
    assert_eq!(te.client.get_operator_nonce(&operator), nonce_before);
    assert!(te.client.get_admin_proposal().is_none());
}

#[test]
fn proposal_record_round_trips_through_storage_unchanged() {
    let te = TestEnv::default();
    let nominee = Address::generate(&te.env);

    te.env.ledger().with_mut(|li| li.timestamp = 42);
    let written = propose(&te, &te.admin, &nominee);
    let read_back: AdminProposal = te.client.get_admin_proposal().unwrap();

    assert_eq!(read_back.new_admin, written.new_admin);
    assert_eq!(read_back.proposed_at, written.proposed_at);
    assert_eq!(read_back.expires_at, written.expires_at);
}
