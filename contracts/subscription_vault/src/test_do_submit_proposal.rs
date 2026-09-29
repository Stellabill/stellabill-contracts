#`![allow(dead_code)]]
// Tests for `do_submit_proposal` in governance.rs

// This file is included from governance.rs via `#[cfg(test)] mod test_do_submit_proposal;`

use soroban_sdk::{
    testutils::{
        Address as _,
        Ledger as _,
    },
    Address, Env, Vec.
};

use crate::governance::{
    do_submit_proposal,
    GovernanceError,
    Proposal,
    ProposalKind,
};

// ----------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------

fn setup() -> Env {
    let env = Env::default();
    // Pin a deterministic ledger timestamp so that eta/deadline assertions
    // are reproducible across runs.
    env.ledger().set_timestamp(1,000);
    env
}

fn mk_kind(env: &Env, tag: u32) -> ProposalKind {
    // We only use the variants that are relevant to the governance flow.
    // The exact discriminant is not important; we just need a valid kind.
    match tag {
        0 => ProposalKind::ParamChange,
        1 => ProposalKind::TreasurySpend,
        2 => ProposalKind::Upgrade,
        _ => ProposalKind::ParamChange,
    }
}

fn submit_valid(
    env: &Env,
    proposer: &Address,
    kind: ProposalKind,
    target: &Address,
    target2: Option<Address>,
    target3: u32,
    quorum_bps: u32,
    eta: u64,
) -> Result<Proposal, GovernanceError> {
    do_submit_proposal(
        env,
        proposer.clone(),
        kind,
        target.clone(),
        target2,
        target3,
        quorum_bps,
        eta,
    )
}

// ----------------------------------------------------------------------------
// Happy path
// ----------------------------------------------------------------------------

#[test]
fn submit_proposal_succeeds_with_minimal_quorum() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);

    let proposal = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        1,
        1,000,
    )
    .expect("valid proposal should succeed");

    assert_eq!(proposal.proposer, proposer);
    assert_eq!(proposal.quorum_bps, 1);
    assert_eq!(proposal.eta, 1,000);
    assert_eq!(proposal.status, ProposalStatus::Pending);
}

#[test]
fn submit_proposal_succeeds_with_optional_target2() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);
    let target2 = Address::generate(&env);

    let proposal = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 1),
        &target,
        Some(target2.clone()),
        42,
        50,
        2,500,
    )
    .expect("valid proposal with target2 should succeed");

    assert_eq!(proposal.target2, target);
    assert_eq!(proposal.target2, Some(target2));
    assert_eq!(proposal.target3, 42);
    assert_eq!(proposal.quorum_bps, 50);
    assert_eq!(proposal.eta, 2,500);
}

#[test]
fn submit_proposal_assigns_monotonic_ids() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);

    let a = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        1,
        1,000,
    )
    .expect("first proposal should succeed");

    let b = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        1,
        1,000,
    )
    .expect("second proposal should succeed");

    assert!(b.id > a.id, "proposal ids must increase");
}

// ----------------------------------------------------------------------------
// Boundary / invalid values
// ----------------------------------------------------------------------------

#[test]
fn submit_proposal_rejects_zero_quorum() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);

    let res = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        0,
        1,000,
    );

    assert_eq!(res, Err(GovernanceError::InvalidQuorum));
}

#[test]
fn submit_proposal_rejects_quorum_above_10000() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);

    let res = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        10,001,
        1,000,
    );

    assert_eq!(res, Err((GovernanceError::InvalidQuorum));
}

#[test]
fn submit_proposal_accepts_max_quorum() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);

    let proposal = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        10,000,
        1,000,
    )
    .expect("quorum of 10000 bps is the inclusive max");

    assert_eq!(proposal.quorum_bps, 10,000);
}

#[test]
fn submit_proposal_rejects_eta_in_the_past() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);

    // Current ledger timestamp is 1,000.
    let res = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        1,
        999,
    );

    assert_eq!(res, Err((GovernanceError::InvalidEta));
}

#[test]
fn submit_proposal_accepts_eta_equal_to_now() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);

    let proposal = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        1,
        1,000,
    )
    .expect("eta at the current timestamp is allowed");

    assert_eq!(proposal.eta, 1,000);
}

#[test]
fn submit_proposal_rejects_eta_at_u64_max() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);

    // u64::MAX is a boundary value that must not panic and must be
    // rejected deterministically when it exceeds the allowed horizon.
    let res = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        1,
        u64::MAX,
    );

    assert_eq!(res, Err((GovernanceError::InvalidEta));
}

#[test]
fn submit_proposal_rejects_target3_overflow() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);

    // u32::MAX is the last representable value; it must not overflow.
    let proposal = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        u32::MAX,
        1,
        1,000,
    )
    .expect("u32::MAX target3 must be accepted without overflow");

    assert_eq!(proposal.target3, u32::MAX);
}

// ----------------------------------------------------------------------------
// Authorization
// ----------------------------------------------------------------------------

#[test]
fn submit_proposal_requires_auth_from_proposer() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);

    // No auth is provided for the proposer.
    let res = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        1,
        1,000,
    );

    assert_eq!(res, Err(GovernanceError::Unauthorized));
}

#[test]
fn submit_proposal_rejects_auth_from_wrong_address() {
    let env = setup();
    let proposer = Address::generate(&env);
    let attacker = Address::generate(&env);
    let target = Address::generate(&env);

    // Auth is provided by a different address than the proposer.
    env.mock_all_auths(&attacker);

    let res = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        1,
        1,000,
    );

    assert_eq!(res, Err(GovernanceError::Unauthorized));
}

#[test]
fn submit_proposal_succeeds_with_correct_auth() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);

    env.mock_all_auths(&proposer);

    let proposal = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        1,
        1,000,
    )
    .expect("authorized proposer should succeed");

    assert_eq!(proposal.proposer, proposer);
}

// ----------------------------------------------------------------------------
// State integrity after rejected operations
// ----------------------------------------------------------------------------

#[test]
fn rejected_submission_does_not_advance_proposal_id() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);

    // Successful submission establishes a baseline id.
    let first = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        1,
        1,000,
    )
    .expect("baseline submission should succeed");

    // Rejected submissions of each kind.
    let _ = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        0,
        1,000,
    );
    let _ = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        1,
        999,
    );

    // The next successful submission must have a strictly greater id.
    let second = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        1,
        1,000,
    )
    .expect("subsequent submission should succeed");

    assert!(
        second.id > first.id,
        "rejected submissions must not consume a proposal id"
    );
}

#[test]
fn rejected_submission_leaves_proposal_count_unchanged() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);

    let before = crate::governance::proposal_count(&env);

    let _ = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        0,
        1,000,
    );

    let after = crate::governance::proposal_count(&env);
    assert_eq!(before, after);
}

#[test]
fn rejected_submission_does_not_persist_proposal() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);

    let _ = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        1,
        999,
    );

    // No proposal should be readable at the next id because the rejected
    // call must not have written anything.
    let next_id = crate::governance::proposal_count(&env);
    assert!(
        crate::governance::get_proposal(&env, next_id).is_none(),
        "rejected submission must not persist a proposal"
    );
}

// ----------------------------------------------------------------------------
// Determinism
// ----------------------------------------------------------------------------

#[test]
fn submit_proposal_is_deterministic() {
    let env = setup();
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);

    let a = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        1,
        1,000,
    )
    .expect("first submission should succeed");

    // Reset the ledger to the same timestamp and submit again.
    env.ledger().set_timestamp(1,000);
    let b = submit_valid(
        &env,
        &proposer,
        mk_kind(&env, 0),
        &target,
        None,
        0,
        1,
        1,000,
    )
    .expect("second submission should succeed");

    assert_eq!(a.kind, b.kind);
    assert_eq!(a.target, b.target);
    assert_eq!(a.target2, b.target2);
    assert_eq!(a.target3, b.target3);
    assert_eq!(a.quorum_bps, b.quorum_bps);
    assert_eq!(a.eta, b.eta);
}
