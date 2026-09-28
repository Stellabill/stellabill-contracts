//! Focused tests for `governance::calculate_quorum`.
//!
//! `calculate_quorum(env, proposal) -> (votes_for, votes_against)` iterates
//! `proposal.votes` and re-validates each voter against the **current**
//! guardian set.  Votes from addresses that are no longer guardians are
//! silently dropped.  Each guardian's weight saturates at `u32::MAX` via
//! `checked_add(…).unwrap_or(u32::MAX)`.
//!
//! The function is not exposed directly on the contract client; we exercise it
//! through the `execute_proposal` path (integration style) so that every
//! combination of guardians, votes, weights, and removals is visible at the
//! contract boundary.  Where the integration path cannot reach a branch (e.g.
//! pure arithmetic saturation), we call `governance::calculate_quorum`
//! directly in white-box unit tests.
//!
//! ## Auth model note
//!
//! `do_vote_proposal` calls `require_stored_admin_auth` which reads the stored
//! admin address and calls `stored_admin.require_auth()`.  The returned address
//! is then checked for guardian weight.  In tests using `mock_all_auths()` the
//! auth passes but the returned address is the stored admin.  Therefore, to
//! actually record a vote, the admin address itself must be added as a guardian.
//!
//! ## Test inventory
//!
//! ### Integration tests (drive through contract client)
//! - `no_votes_quorum_zero_bps_executes_succeeds` — empty votes map, 0% quorum
//! - `no_votes_quorum_nonzero_bps_execute_fails` — empty votes, 1 bps quorum
//! - `all_yes_votes_single_guardian_executes` — unanimous single guardian
//! - `all_no_votes_single_guardian_execute_fails` — unanimous rejection
//! - `quorum_exactly_met_executes` — boundary: votes_for == required
//! - `removed_guardian_vote_dropped_breaks_quorum` — removal before execute
//! - `removed_non_voting_guardian_does_not_affect_outcome`
//! - `non_guardian_cannot_vote` — weight-0 address rejected
//! - `double_execute_rejected` — executed flag blocks re-execution
//! - `execute_before_eta_rejected` — ETA guard, state unchanged
//! - `multiple_proposals_independent_quorum` — proposals don't share vote state
//! - `quorum_bps_exceeds_max_rejected_at_submit` — 10001 invalid at submit
//! - `quorum_bps_at_max_boundary_is_valid` — 10000 accepted
//! - `quorum_bps_zero_is_valid` — 0 accepted
//!
//! ### White-box unit tests (call calculate_quorum directly)
//! - `unit_empty_votes_empty_guardians_returns_zero_zero`
//! - `unit_vote_from_non_guardian_is_dropped`
//! - `unit_removed_guardian_vote_dropped`
//! - `unit_votes_for_saturate_at_u32_max`
//! - `unit_votes_against_saturate_at_u32_max`
//! - `unit_mixed_votes_counted_into_correct_accumulators`
//! - `unit_single_guardian_weight_one_yes_vote`
//! - `unit_two_active_guardians_one_yes_one_no`

#![cfg(test)]

use crate::calculate_quorum;
use crate::types::{Proposal, ProposalKind};
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, Map,
};

// ── shared helpers ────────────────────────────────────────────────────────────

/// Register `SubscriptionVault`, call `init`, and return `(admin, client)`.
///
/// The admin address is returned so callers can add it as a guardian —
/// `do_vote_proposal` calls `require_stored_admin_auth` and the returned
/// address is the stored admin, so only the admin can accumulate votes via the
/// contract client with `mock_all_auths`.
fn init_vault<'a>(env: &'a Env) -> (Address, SubscriptionVaultClient<'a>) {
    let admin = Address::generate(env);
    let token_admin = Address::generate(env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    client.init(&token_address, &6, &admin, &10_000_000, &86400);
    (admin, client)
}

// ─────────────────────────────────────────────────────────────────────────────
// Integration tests — driven through the contract client
// ─────────────────────────────────────────────────────────────────────────────

/// Empty votes map with quorum_bps=0: required=0, votes_for=0 ≥ 0 → executes.
///
/// Verifies that calculate_quorum returns (0, 0) for an empty votes map.
#[test]
fn no_votes_quorum_zero_bps_executes_succeeds() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (admin, client) = init_vault(&env);
    let new_admin = Address::generate(&env);

    // No guardians, no votes; quorum_bps = 0 → required = 0
    let now = env.ledger().timestamp();
    let eta = now + 3_600;
    let proposal_id = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &0,
        &eta,
    );

    env.ledger().set_timestamp(eta + 1);
    let result = client.try_execute_proposal(&proposal_id);
    assert!(
        result.is_ok(),
        "quorum_bps=0 with no votes must succeed: {result:?}"
    );

    let _ = admin; // bound above, used via init_vault
}

/// Empty votes map with quorum_bps=1 and a guardian present → execute fails.
///
/// total_weight=100, required = ceil(100 * 1 / 10000) = 1. votes_for=0 < 1.
#[test]
fn no_votes_quorum_nonzero_bps_execute_fails() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (admin, client) = init_vault(&env);
    let new_admin = Address::generate(&env);

    // Admin is the guardian (required so weight contributes to total_weight)
    client.add_guardian(&admin, &admin, &100);

    let now = env.ledger().timestamp();
    let eta = now + 3_600;
    let proposal_id = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &1, // 0.01% of 100 total weight → 1 required
        &eta,
    );

    // No votes cast
    env.ledger().set_timestamp(eta + 1);
    let result = client.try_execute_proposal(&proposal_id);
    assert!(
        result.is_err(),
        "no votes with quorum_bps>0 must fail to execute"
    );
}

/// Single guardian (admin) votes YES; quorum_bps=5000 (50% of 100 weight = 50 needed).
/// votes_for=100 ≥ 50 → execute succeeds.
#[test]
fn all_yes_votes_single_guardian_executes() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (admin, client) = init_vault(&env);
    let new_admin = Address::generate(&env);

    // Admin must be the guardian so require_stored_admin_auth returns a guardian
    client.add_guardian(&admin, &admin, &100);

    let now = env.ledger().timestamp();
    let eta = now + 3_600;
    let proposal_id = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &5_000,
        &eta,
    );

    client.vote_proposal(&proposal_id, &true);

    env.ledger().set_timestamp(eta + 1);
    let result = client.try_execute_proposal(&proposal_id);
    assert!(result.is_ok(), "single unanimous yes must execute: {result:?}");

    // Admin was rotated to new_admin
    assert_eq!(client.get_admin(), new_admin);
}

/// Single guardian (admin) votes NO; quorum_bps=5000.
/// votes_for=0 < 50 → execute fails.
#[test]
fn all_no_votes_single_guardian_execute_fails() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (admin, client) = init_vault(&env);
    let new_admin = Address::generate(&env);

    client.add_guardian(&admin, &admin, &100);

    let now = env.ledger().timestamp();
    let eta = now + 3_600;
    let proposal_id = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &5_000,
        &eta,
    );

    client.vote_proposal(&proposal_id, &false);

    env.ledger().set_timestamp(eta + 1);
    let result = client.try_execute_proposal(&proposal_id);
    assert!(
        result.is_err(),
        "unanimous no vote must not execute"
    );

    // State unchanged — proposal not executed
    let proposal = client.get_proposal(&proposal_id).unwrap();
    assert!(!proposal.executed);
}

/// Boundary: votes_for == required exactly → execute succeeds.
/// Admin guardian weight=100, quorum_bps=10000 → required=100; admin votes YES → votes_for=100.
#[test]
fn quorum_exactly_met_executes() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (admin, client) = init_vault(&env);
    let new_admin = Address::generate(&env);

    client.add_guardian(&admin, &admin, &100);

    let now = env.ledger().timestamp();
    let eta = now + 3_600;
    let proposal_id = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &10_000, // 100% → required = 100
        &eta,
    );

    client.vote_proposal(&proposal_id, &true);

    env.ledger().set_timestamp(eta + 1);
    let result = client.try_execute_proposal(&proposal_id);
    assert!(
        result.is_ok(),
        "votes_for == required must execute: {result:?}"
    );
}

/// A guardian that voted YES is removed before execution.
/// calculate_quorum drops their vote → votes_for=0 < required → execute fails.
/// State must be unchanged (proposal not executed).
///
/// Setup: admin weight=100 (votes YES), extra guardian weight=50 (no vote).
/// quorum_bps=10000 (100%): total_weight=50 after admin removed, required=50.
/// votes_for=0 (admin's YES dropped) < 50 → execute fails.
#[test]
fn removed_guardian_vote_dropped_breaks_quorum() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (admin, client) = init_vault(&env);
    let new_admin = Address::generate(&env);
    let extra_guardian = Address::generate(&env);

    // Admin (the voter) + extra guardian so total_weight > 0 after admin removed
    client.add_guardian(&admin, &admin, &100);
    client.add_guardian(&admin, &extra_guardian, &50);

    let now = env.ledger().timestamp();
    let eta = now + 3_600;
    // quorum_bps=10000: need 100% of total weight
    let proposal_id = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &10_000,
        &eta,
    );

    // Admin votes YES before the timelock
    client.vote_proposal(&proposal_id, &true);

    // Admin (guardian) is removed BEFORE execution.
    // After removal: total_weight=50 (only extra_guardian), required=50.
    // Admin's YES vote is dropped → votes_for=0 < 50.
    client.remove_guardian(&admin, &admin);
    assert_eq!(
        client.get_guardian_weight(&admin),
        0,
        "removed admin-guardian must have weight 0"
    );

    env.ledger().set_timestamp(eta + 1);
    let result = client.try_execute_proposal(&proposal_id);
    assert!(
        result.is_err(),
        "removed guardian's YES vote must not count toward quorum: {result:?}"
    );

    // Proposal state must be unchanged (not executed)
    let proposal = client.get_proposal(&proposal_id).unwrap();
    assert!(
        !proposal.executed,
        "proposal must not be marked executed after failed quorum check"
    );
}

/// Removing a non-participating guardian reduces total_weight but if the remaining
/// guardian's votes_for still meets the new required threshold, execute succeeds.
///
/// Setup: admin weight=100 (votes YES), extra guardian weight=50 (no vote).
/// Before removal: total=150, required=75 (50%), votes_for=100 ≥ 75 → ok.
/// After removing extra guardian: total=100, required=50, votes_for=100 ≥ 50 → ok.
#[test]
fn removed_non_voting_guardian_does_not_break_quorum() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (admin, client) = init_vault(&env);
    let new_admin = Address::generate(&env);
    let extra_guardian = Address::generate(&env);

    client.add_guardian(&admin, &admin, &100);
    client.add_guardian(&admin, &extra_guardian, &50);

    let now = env.ledger().timestamp();
    let eta = now + 3_600;
    let proposal_id = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &5_000, // 50%
        &eta,
    );

    // Only admin votes
    client.vote_proposal(&proposal_id, &true);

    // Remove the silent guardian
    client.remove_guardian(&admin, &extra_guardian);

    // After removal total_weight=100, required=50, votes_for=100 → ok
    env.ledger().set_timestamp(eta + 1);
    let result = client.try_execute_proposal(&proposal_id);
    assert!(
        result.is_ok(),
        "removing non-voting guardian must not prevent execution: {result:?}"
    );
}

/// A non-guardian address (weight=0) cannot vote on a proposal.
/// The contract returns Unauthorized and the votes map remains empty.
#[test]
fn non_guardian_cannot_vote() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (admin, client) = init_vault(&env);
    let new_admin = Address::generate(&env);

    // Admin is NOT added as a guardian → weight=0
    let now = env.ledger().timestamp();
    let eta = now + 3_600;
    let proposal_id = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &5_000,
        &eta,
    );

    // Vote attempt by admin (weight=0) must be rejected with Unauthorized
    let result = client.try_vote_proposal(&proposal_id, &true);
    assert!(
        result.is_err(),
        "address with weight=0 must not be allowed to vote: {result:?}"
    );

    // Proposal votes map must be empty
    let proposal = client.get_proposal(&proposal_id).unwrap();
    assert_eq!(
        proposal.votes.len(),
        0,
        "votes map must be empty after rejected vote"
    );

    let _ = admin;
}

/// Re-execution of an already-executed proposal must be rejected.
/// The `executed` flag blocks entry before calculate_quorum is reached.
#[test]
fn double_execute_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (admin, client) = init_vault(&env);
    let new_admin = Address::generate(&env);

    client.add_guardian(&admin, &admin, &100);

    let now = env.ledger().timestamp();
    let eta = now + 3_600;
    let proposal_id = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &5_000,
        &eta,
    );

    client.vote_proposal(&proposal_id, &true);
    env.ledger().set_timestamp(eta + 1);

    // First execute succeeds
    let first = client.try_execute_proposal(&proposal_id);
    assert!(first.is_ok(), "first execute must succeed: {first:?}");

    // Second execute must be rejected (executed flag set)
    let second = client.try_execute_proposal(&proposal_id);
    assert!(
        second.is_err(),
        "double-execute must be rejected: {second:?}"
    );
}

/// Execution before ETA must be rejected regardless of quorum.
/// State must be unchanged.
#[test]
fn execute_before_eta_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (admin, client) = init_vault(&env);
    let new_admin = Address::generate(&env);

    client.add_guardian(&admin, &admin, &100);

    let now = env.ledger().timestamp();
    let eta = now + 3_600;
    let proposal_id = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &5_000,
        &eta,
    );

    client.vote_proposal(&proposal_id, &true);

    // Do NOT advance past ETA
    let result = client.try_execute_proposal(&proposal_id);
    assert!(
        result.is_err(),
        "execute before ETA must be rejected: {result:?}"
    );

    // Proposal must still be unexecuted
    let proposal = client.get_proposal(&proposal_id).unwrap();
    assert!(!proposal.executed, "proposal must not be executed before ETA");
}

/// Two proposals are independent: votes on proposal A do not affect quorum
/// for proposal B, even when the same guardian is involved.
#[test]
fn multiple_proposals_independent_quorum() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (admin, client) = init_vault(&env);
    let new_admin_a = Address::generate(&env);
    let new_admin_b = Address::generate(&env);

    client.add_guardian(&admin, &admin, &100);

    let now = env.ledger().timestamp();
    let eta = now + 3_600;

    // Submit two proposals
    let proposal_a = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin_a,
        &None,
        &0,
        &5_000,
        &eta,
    );
    let proposal_b = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin_b,
        &None,
        &0,
        &5_000,
        &eta,
    );

    // Vote only on proposal_a
    client.vote_proposal(&proposal_a, &true);
    // proposal_b has no votes

    env.ledger().set_timestamp(eta + 1);

    // Proposal A should execute (quorum met)
    let result_a = client.try_execute_proposal(&proposal_a);
    assert!(result_a.is_ok(), "proposal A with votes must execute: {result_a:?}");

    // Proposal B has no votes → quorum not met
    let result_b = client.try_execute_proposal(&proposal_b);
    assert!(
        result_b.is_err(),
        "proposal B without votes must fail: {result_b:?}"
    );
}

/// quorum_bps = 10001 must be rejected at submit time.
/// (One above the valid range 0..=10000.)
#[test]
fn quorum_bps_exceeds_max_rejected_at_submit() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (admin, client) = init_vault(&env);
    let new_admin = Address::generate(&env);

    let now = env.ledger().timestamp();
    let eta = now + 3_600;

    let result = client.try_submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &10_001,
        &eta,
    );
    assert!(
        result.is_err(),
        "quorum_bps > 10000 must be rejected at submit: {result:?}"
    );

    let _ = admin;
}

/// quorum_bps = 10000 (100%) is the maximum valid value.
#[test]
fn quorum_bps_at_max_boundary_is_valid() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (admin, client) = init_vault(&env);
    let new_admin = Address::generate(&env);

    let now = env.ledger().timestamp();
    let eta = now + 3_600;

    let result = client.try_submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &10_000,
        &eta,
    );
    assert!(
        result.is_ok(),
        "quorum_bps = 10000 must be accepted: {result:?}"
    );

    let _ = admin;
}

/// quorum_bps = 0 is the minimum valid value.
/// Any number of yes votes (including 0) satisfies the quorum when required=0.
#[test]
fn quorum_bps_zero_is_valid() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (admin, client) = init_vault(&env);
    let new_admin = Address::generate(&env);

    let now = env.ledger().timestamp();
    let eta = now + 3_600;

    let result = client.try_submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &0,
        &eta,
    );
    assert!(
        result.is_ok(),
        "quorum_bps = 0 must be accepted: {result:?}"
    );

    let _ = admin;
}

// ─────────────────────────────────────────────────────────────────────────────
// White-box unit tests — call governance::calculate_quorum directly
// ─────────────────────────────────────────────────────────────────────────────

/// Baseline: empty votes map, empty guardian set → (0, 0).
#[test]
fn unit_empty_votes_empty_guardians_returns_zero_zero() {
    let env = Env::default();
    env.mock_all_auths();

    let (admin, client) = init_vault(&env);
    let target = Address::generate(&env);
    env.ledger().set_timestamp(1_000);

    let proposal = Proposal {
        id: 0,
        kind: ProposalKind::RotateAdmin,
        target: target.clone(),
        target2: None,
        target3: 0,
        quorum_bps: 5_000,
        votes: Map::new(&env),
        eta: 5_000,
        submitted_at: 1_000,
        executed: false,
    };

    let (votes_for, votes_against) = env.as_contract(&client.address, || {
        calculate_quorum(&env, &proposal)
    });

    assert_eq!(votes_for, 0, "votes_for must be 0 for empty votes map");
    assert_eq!(votes_against, 0, "votes_against must be 0 for empty votes map");

    let _ = admin;
}

/// A vote from an address not in the current guardian set must be dropped.
#[test]
fn unit_vote_from_non_guardian_is_dropped() {
    let env = Env::default();
    env.mock_all_auths();

    let (admin, client) = init_vault(&env);
    let not_a_guardian = Address::generate(&env);
    let target = Address::generate(&env);
    env.ledger().set_timestamp(1_000);

    // not_a_guardian is never added to the guardian set
    let mut votes = Map::new(&env);
    votes.set(not_a_guardian.clone(), true);

    let proposal = Proposal {
        id: 0,
        kind: ProposalKind::RotateAdmin,
        target,
        target2: None,
        target3: 0,
        quorum_bps: 5_000,
        votes,
        eta: 5_000,
        submitted_at: 1_000,
        executed: false,
    };

    let (votes_for, votes_against) = env.as_contract(&client.address, || {
        calculate_quorum(&env, &proposal)
    });

    assert_eq!(
        votes_for, 0,
        "vote from non-guardian must be dropped"
    );
    assert_eq!(votes_against, 0);

    let _ = admin;
}

/// A vote from a removed guardian is dropped.
/// Guardian existed at vote time but was removed before calculate_quorum.
#[test]
fn unit_removed_guardian_vote_dropped() {
    let env = Env::default();
    env.mock_all_auths();

    let (admin, client) = init_vault(&env);
    let guardian = Address::generate(&env);
    let target = Address::generate(&env);
    env.ledger().set_timestamp(1_000);

    // Add, then remove the guardian
    client.add_guardian(&admin, &guardian, &200);
    client.remove_guardian(&admin, &guardian);
    assert_eq!(client.get_guardian_weight(&guardian), 0);

    // Build a proposal with the (now-removed) guardian's vote still in the map
    let mut votes = Map::new(&env);
    votes.set(guardian.clone(), true);

    let proposal = Proposal {
        id: 0,
        kind: ProposalKind::RotateAdmin,
        target,
        target2: None,
        target3: 0,
        quorum_bps: 5_000,
        votes,
        eta: 5_000,
        submitted_at: 1_000,
        executed: false,
    };

    let (votes_for, votes_against) = env.as_contract(&client.address, || {
        calculate_quorum(&env, &proposal)
    });

    assert_eq!(
        votes_for, 0,
        "removed guardian's vote must not contribute to votes_for"
    );
    assert_eq!(votes_against, 0);
}

/// votes_for saturates at u32::MAX (not wraps/overflows).
/// Two guardians both with weight=u32::MAX both vote YES.
#[test]
fn unit_votes_for_saturate_at_u32_max() {
    let env = Env::default();
    env.mock_all_auths();

    let (admin, client) = init_vault(&env);
    let guardian_a = Address::generate(&env);
    let guardian_b = Address::generate(&env);
    let target = Address::generate(&env);
    env.ledger().set_timestamp(1_000);

    client.add_guardian(&admin, &guardian_a, &u32::MAX);
    client.add_guardian(&admin, &guardian_b, &u32::MAX);

    let mut votes = Map::new(&env);
    votes.set(guardian_a.clone(), true);
    votes.set(guardian_b.clone(), true);

    let proposal = Proposal {
        id: 0,
        kind: ProposalKind::RotateAdmin,
        target,
        target2: None,
        target3: 0,
        quorum_bps: 5_000,
        votes,
        eta: 5_000,
        submitted_at: 1_000,
        executed: false,
    };

    let (votes_for, votes_against) = env.as_contract(&client.address, || {
        calculate_quorum(&env, &proposal)
    });

    assert_eq!(
        votes_for,
        u32::MAX,
        "votes_for must saturate at u32::MAX, not overflow"
    );
    assert_eq!(votes_against, 0);
}

/// votes_against saturates at u32::MAX (symmetric saturation path).
#[test]
fn unit_votes_against_saturate_at_u32_max() {
    let env = Env::default();
    env.mock_all_auths();

    let (admin, client) = init_vault(&env);
    let guardian_a = Address::generate(&env);
    let guardian_b = Address::generate(&env);
    let target = Address::generate(&env);
    env.ledger().set_timestamp(1_000);

    client.add_guardian(&admin, &guardian_a, &u32::MAX);
    client.add_guardian(&admin, &guardian_b, &u32::MAX);

    let mut votes = Map::new(&env);
    votes.set(guardian_a.clone(), false);
    votes.set(guardian_b.clone(), false);

    let proposal = Proposal {
        id: 0,
        kind: ProposalKind::RotateAdmin,
        target,
        target2: None,
        target3: 0,
        quorum_bps: 5_000,
        votes,
        eta: 5_000,
        submitted_at: 1_000,
        executed: false,
    };

    let (votes_for, votes_against) = env.as_contract(&client.address, || {
        calculate_quorum(&env, &proposal)
    });

    assert_eq!(votes_for, 0);
    assert_eq!(
        votes_against,
        u32::MAX,
        "votes_against must saturate at u32::MAX, not overflow"
    );
}

/// Mixed YES/NO votes are counted into the correct accumulators.
/// guardian_yes weight=30 (YES), guardian_no weight=70 (NO) → (30, 70).
#[test]
fn unit_mixed_votes_counted_into_correct_accumulators() {
    let env = Env::default();
    env.mock_all_auths();

    let (admin, client) = init_vault(&env);
    let guardian_yes = Address::generate(&env);
    let guardian_no = Address::generate(&env);
    let target = Address::generate(&env);
    env.ledger().set_timestamp(1_000);

    client.add_guardian(&admin, &guardian_yes, &30);
    client.add_guardian(&admin, &guardian_no, &70);

    let mut votes = Map::new(&env);
    votes.set(guardian_yes.clone(), true);
    votes.set(guardian_no.clone(), false);

    let proposal = Proposal {
        id: 0,
        kind: ProposalKind::RotateAdmin,
        target,
        target2: None,
        target3: 0,
        quorum_bps: 5_000,
        votes,
        eta: 5_000,
        submitted_at: 1_000,
        executed: false,
    };

    let (votes_for, votes_against) = env.as_contract(&client.address, || {
        calculate_quorum(&env, &proposal)
    });

    assert_eq!(votes_for, 30, "YES vote weight must be 30");
    assert_eq!(votes_against, 70, "NO vote weight must be 70");
}

/// Single guardian with weight=1 voting YES → (1, 0).
/// Minimum non-zero weight boundary.
#[test]
fn unit_single_guardian_weight_one_yes_vote() {
    let env = Env::default();
    env.mock_all_auths();

    let (admin, client) = init_vault(&env);
    let guardian = Address::generate(&env);
    let target = Address::generate(&env);
    env.ledger().set_timestamp(1_000);

    client.add_guardian(&admin, &guardian, &1);

    let mut votes = Map::new(&env);
    votes.set(guardian.clone(), true);

    let proposal = Proposal {
        id: 0,
        kind: ProposalKind::RotateAdmin,
        target,
        target2: None,
        target3: 0,
        quorum_bps: 5_000,
        votes,
        eta: 5_000,
        submitted_at: 1_000,
        executed: false,
    };

    let (votes_for, votes_against) = env.as_contract(&client.address, || {
        calculate_quorum(&env, &proposal)
    });

    assert_eq!(votes_for, 1);
    assert_eq!(votes_against, 0);
}

/// Two active guardians, one YES and one NO; insertion order must not affect results.
/// guardian_no (weight=60) inserted first, guardian_yes (weight=40) second.
/// Expected: (40, 60).
#[test]
fn unit_two_active_guardians_one_yes_one_no_order_independent() {
    let env = Env::default();
    env.mock_all_auths();

    let (admin, client) = init_vault(&env);
    let g_yes = Address::generate(&env);
    let g_no = Address::generate(&env);
    let target = Address::generate(&env);
    env.ledger().set_timestamp(1_000);

    client.add_guardian(&admin, &g_yes, &40);
    client.add_guardian(&admin, &g_no, &60);

    let mut votes = Map::new(&env);
    votes.set(g_no.clone(), false);  // NO inserted first
    votes.set(g_yes.clone(), true);  // YES inserted second

    let proposal = Proposal {
        id: 0,
        kind: ProposalKind::RotateAdmin,
        target,
        target2: None,
        target3: 0,
        quorum_bps: 5_000,
        votes,
        eta: 5_000,
        submitted_at: 1_000,
        executed: false,
    };

    let (votes_for, votes_against) = env.as_contract(&client.address, || {
        calculate_quorum(&env, &proposal)
    });

    assert_eq!(votes_for, 40, "votes_for must equal g_yes weight regardless of insertion order");
    assert_eq!(votes_against, 60, "votes_against must equal g_no weight");
}
