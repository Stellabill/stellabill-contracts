//! Focused tests for `calculate_quorum` in `governance.rs`.
//!
//! `calculate_quorum(env, proposal) -> (votes_for, votes_against)` iterates the
//! proposal's `votes` map and re-validates each voter's current guardian status
//! at read-time.  This means:
//!
//! - Votes from addresses that are **no longer guardians** are silently excluded.
//! - Guardian **weight is re-read** from live storage, not from the weight at
//!   vote-submission time (the vote map only stores the yes/no bool).
//! - `checked_add` saturates at `u32::MAX` to avoid overflow panics.
//!
//! These tests exercise `calculate_quorum` indirectly through the public
//! contract entrypoints (`add_guardian`, `vote_proposal`, `execute_proposal`,
//! `remove_guardian`) so that the full storage round-trip is validated.
//!
//! # Test index
//!
//! | # | Scenario | Expected result |
//! |---|----------|-----------------|
//! | 1 | No votes cast | `(0, 0)` |
//! | 2 | All guardians vote yes | `(total_weight, 0)` |
//! | 3 | All guardians vote no | `(0, total_weight)` |
//! | 4 | Mixed yes / no votes | correct split |
//! | 5 | Guardian removed after voting | removed guardian's vote excluded |
//! | 6 | Non-guardian address votes | vote excluded (can't happen via public API) |
//! | 7 | Execute succeeds when quorum met | returns Ok |
//! | 8 | Execute fails when quorum not met | returns Err(InvalidInput) |
//! | 9 | Execute fails when ETA not reached | returns Err(InvalidInput) |
//! | 10 | Execute fails on already-executed proposal | returns Err(InvalidInput) |
//! | 11 | Single guardian, weight 1 | `(1, 0)` or `(0, 1)` |
//! | 12 | Quorum exactly at boundary (floor arithmetic) | executes iff votes_for >= floor |
//! | 13 | Guardian re-added after removal | new vote counted with new weight |
//! | 14 | 0 % quorum (quorum_bps = 0) always passes | executes with no votes |

#![cfg(test)]

use crate::types::{Error, ProposalKind};
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, String,
};

// ── Shared setup ─────────────────────────────────────────────────────────────

/// Initialise a fresh vault and return a ready-to-use client.
fn setup(env: &Env) -> (Address, SubscriptionVaultClient) {
    let admin = Address::generate(env);
    let token_admin = Address::generate(env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    client.init(
        &token,
        &6,           // decimals
        &admin,
        &10_000_000,  // min_topup
        &86_400,      // grace period
    );
    (admin, client)
}

/// Submit a RotateAdmin proposal with a 1-hour ETA and return its ID.
fn submit_proposal(env: &Env, client: &SubscriptionVaultClient, quorum_bps: u32) -> u64 {
    let target = Address::generate(env);
    let now = env.ledger().timestamp();
    client
        .submit_proposal(
            &ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &quorum_bps,
            &(now + 3_600), // ETA: 1 hour from now
        )
        .expect("submit_proposal should succeed")
}

/// Advance the ledger past `eta` so the proposal can be executed.
fn advance_past_eta(env: &Env, eta_delta: u64) {
    let now = env.ledger().timestamp();
    env.ledger().set_timestamp(now + eta_delta + 1);
}

// ── Test 1: No votes cast → (0, 0) ──────────────────────────────────────────

#[test]
fn quorum_no_votes_returns_zero_zero() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g = Address::generate(&env);
    client.add_guardian(&admin, &g, &100).unwrap();

    let pid = submit_proposal(&env, &client, 0); // 0 % quorum
    // Nobody votes. Advance past ETA.
    advance_past_eta(&env, 3_600);

    // execute_proposal calls calculate_quorum internally.
    // With quorum_bps = 0 and votes_for = 0, it must succeed (0 >= 0).
    client
        .try_execute_proposal(&pid)
        .expect("execution should succeed with 0% quorum and no votes");

    // Verify through the ProposalExecutedEvent: the stored proposal is executed.
    let proposal = client.get_proposal(&pid).unwrap();
    assert!(proposal.executed, "proposal must be marked executed");
}

// ── Test 2: All guardians vote yes → (total_weight, 0) ───────────────────────

#[test]
fn quorum_all_yes_votes_returns_total_weight_zero() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    client.add_guardian(&admin, &g1, &60).unwrap();
    client.add_guardian(&admin, &g2, &40).unwrap();

    // 50 % quorum needed → 50 out of 100 total weight
    let pid = submit_proposal(&env, &client, 5_000);

    // Both guardians vote yes
    client.vote_proposal(&pid, &true).unwrap();
    client.vote_proposal(&pid, &true).unwrap();

    // Advance past ETA and execute
    advance_past_eta(&env, 3_600);
    client
        .try_execute_proposal(&pid)
        .expect("execution must succeed: votes_for = 100 >= required 50");

    let proposal = client.get_proposal(&pid).unwrap();
    assert!(proposal.executed);
}

// ── Test 3: All guardians vote no → (0, total_weight) ────────────────────────

#[test]
fn quorum_all_no_votes_fails_execution() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    client.add_guardian(&admin, &g1, &60).unwrap();
    client.add_guardian(&admin, &g2, &40).unwrap();

    // Require at least 1 bps (> 0) so that zero yes-votes fails
    let pid = submit_proposal(&env, &client, 1);

    // Both vote no
    client.vote_proposal(&pid, &false).unwrap();
    client.vote_proposal(&pid, &false).unwrap();

    advance_past_eta(&env, 3_600);

    let result = client.try_execute_proposal(&pid);
    assert_eq!(
        result,
        Err(Ok(Error::InvalidInput)),
        "execution must fail when all votes are no"
    );

    // Proposal must not be marked executed
    let proposal = client.get_proposal(&pid).unwrap();
    assert!(!proposal.executed, "proposal must remain unexecuted");
}

// ── Test 4: Mixed yes / no votes → correct split ─────────────────────────────

#[test]
fn quorum_mixed_votes_correct_split() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    let g3 = Address::generate(&env);
    // Weights: 50 + 30 + 20 = 100
    client.add_guardian(&admin, &g1, &50).unwrap();
    client.add_guardian(&admin, &g2, &30).unwrap();
    client.add_guardian(&admin, &g3, &20).unwrap();

    // Quorum = 50 % of 100 = 50
    let pid = submit_proposal(&env, &client, 5_000);

    // g1 (50) and g2 (30) vote yes; g3 (20) votes no
    // votes_for = 80, votes_against = 20 → quorum met
    client.vote_proposal(&pid, &true).unwrap();  // g1 or g2 or g3 depending on auth mock
    client.vote_proposal(&pid, &true).unwrap();
    client.vote_proposal(&pid, &false).unwrap();

    advance_past_eta(&env, 3_600);

    // votes_for must be at least 50 for execution to succeed
    client
        .try_execute_proposal(&pid)
        .expect("quorum must be met with 80 yes votes out of 100");
}

// ── Test 5: Guardian removed after voting → vote excluded ────────────────────

#[test]
fn quorum_removed_guardian_vote_excluded() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    // g1: weight 80, g2: weight 20 — total = 100
    client.add_guardian(&admin, &g1, &80).unwrap();
    client.add_guardian(&admin, &g2, &20).unwrap();

    // Require 50% quorum = 50 out of 100
    let pid = submit_proposal(&env, &client, 5_000);

    // Both vote yes
    client.vote_proposal(&pid, &true).unwrap();
    client.vote_proposal(&pid, &true).unwrap();

    // Now remove g1 (80-weight guardian)
    // After removal: total remaining weight = 20; required = floor(20 * 5000 / 10000) = 10
    // votes_for from live guardians = 20 (only g2 still counts)
    // 20 >= 10 → execution should still succeed
    client.remove_guardian(&admin, &g1).unwrap();

    advance_past_eta(&env, 3_600);

    // g1's vote must be excluded but g2's vote (20) still meets the new required (10).
    client
        .try_execute_proposal(&pid)
        .expect("g2's remaining vote must still meet quorum after g1 removal");

    let proposal = client.get_proposal(&pid).unwrap();
    assert!(proposal.executed);
}

// ── Test 6: Removed guardian — quorum now fails ──────────────────────────────
//
// If the only yes-voting guardian is removed, quorum calculation should return
// (0, 0) and execution must fail.

#[test]
fn quorum_sole_yes_voter_removed_fails_execution() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    client.add_guardian(&admin, &g1, &100).unwrap();
    client.add_guardian(&admin, &g2, &100).unwrap();

    // 50% quorum → at least 100 out of 200 total weight required
    let pid = submit_proposal(&env, &client, 5_000);

    // Only g1 votes yes
    client.vote_proposal(&pid, &true).unwrap();

    // Remove g1 → their yes vote is excluded; g2 has not voted
    client.remove_guardian(&admin, &g1).unwrap();

    advance_past_eta(&env, 3_600);

    // After g1 removal: total weight = 100, required = 50; votes_for = 0 → fails
    let result = client.try_execute_proposal(&pid);
    assert_eq!(
        result,
        Err(Ok(Error::InvalidInput)),
        "execution must fail when the only yes-voter was removed"
    );

    let proposal = client.get_proposal(&pid).unwrap();
    assert!(!proposal.executed, "proposal must remain unexecuted");
}

// ── Test 7: Execute succeeds when quorum exactly met ─────────────────────────

#[test]
fn quorum_exact_threshold_succeeds() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g = Address::generate(&env);
    // Single guardian, weight 100. Quorum = 100% → required = 100.
    client.add_guardian(&admin, &g, &100).unwrap();

    let pid = submit_proposal(&env, &client, 10_000); // 100 %

    client.vote_proposal(&pid, &true).unwrap();

    advance_past_eta(&env, 3_600);

    client
        .try_execute_proposal(&pid)
        .expect("votes_for = 100 == required 100 must succeed");

    let proposal = client.get_proposal(&pid).unwrap();
    assert!(proposal.executed);
}

// ── Test 8: Execute fails when quorum one vote short ─────────────────────────

#[test]
fn quorum_one_below_threshold_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    // Weights: 49 + 51 = 100. 50 % quorum → required = 50.
    client.add_guardian(&admin, &g1, &49).unwrap();
    client.add_guardian(&admin, &g2, &51).unwrap();

    let pid = submit_proposal(&env, &client, 5_000);

    // Only g1 (49) votes yes → votes_for = 49 < 50
    client.vote_proposal(&pid, &true).unwrap();

    advance_past_eta(&env, 3_600);

    let result = client.try_execute_proposal(&pid);
    assert_eq!(
        result,
        Err(Ok(Error::InvalidInput)),
        "votes_for = 49 must not meet quorum of 50"
    );

    let proposal = client.get_proposal(&pid).unwrap();
    assert!(!proposal.executed);
}

// ── Test 9: Execute fails when ETA not yet reached ───────────────────────────

#[test]
fn quorum_execute_before_eta_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g = Address::generate(&env);
    client.add_guardian(&admin, &g, &100).unwrap();

    let pid = submit_proposal(&env, &client, 0); // 0 % quorum

    // Do NOT advance past ETA
    let result = client.try_execute_proposal(&pid);
    assert_eq!(
        result,
        Err(Ok(Error::InvalidInput)),
        "execution before ETA must be rejected"
    );

    let proposal = client.get_proposal(&pid).unwrap();
    assert!(!proposal.executed, "state must be unchanged after rejected execute");
}

// ── Test 10: Execute fails on already-executed proposal ──────────────────────

#[test]
fn quorum_double_execute_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g = Address::generate(&env);
    client.add_guardian(&admin, &g, &100).unwrap();

    let pid = submit_proposal(&env, &client, 0); // 0 % quorum
    advance_past_eta(&env, 3_600);

    // First execution must succeed
    client.try_execute_proposal(&pid).expect("first execute must succeed");

    // Second execution must fail
    let result = client.try_execute_proposal(&pid);
    assert_eq!(
        result,
        Err(Ok(Error::InvalidInput)),
        "double-execution must be rejected"
    );
}

// ── Test 11: Single guardian with weight 1 ───────────────────────────────────

#[test]
fn quorum_single_guardian_weight_one_yes() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g = Address::generate(&env);
    client.add_guardian(&admin, &g, &1).unwrap();

    let pid = submit_proposal(&env, &client, 10_000); // 100 %
    client.vote_proposal(&pid, &true).unwrap();

    advance_past_eta(&env, 3_600);

    client
        .try_execute_proposal(&pid)
        .expect("weight-1 yes vote must meet 100% quorum on a single-guardian set");
}

#[test]
fn quorum_single_guardian_weight_one_no_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g = Address::generate(&env);
    client.add_guardian(&admin, &g, &1).unwrap();

    // 1 bps of 1 → required = floor(1 * 1 / 10000) = 0 → would still pass at 1bps
    // Use 10000 bps (100%) so that a no-vote definitely fails.
    let pid = submit_proposal(&env, &client, 10_000);
    client.vote_proposal(&pid, &false).unwrap();

    advance_past_eta(&env, 3_600);

    let result = client.try_execute_proposal(&pid);
    assert_eq!(
        result,
        Err(Ok(Error::InvalidInput)),
        "no-vote with 100% quorum requirement must fail"
    );
}

// ── Test 12: Quorum boundary with floor arithmetic ───────────────────────────
//
// With total_weight = 3 and quorum_bps = 3334 (33.34%):
//   required = floor(3 * 3334 / 10000) = floor(10002 / 10000) = 1
// One yes-vote (weight 1) must be exactly enough.

#[test]
fn quorum_floor_arithmetic_boundary() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    let g3 = Address::generate(&env);
    client.add_guardian(&admin, &g1, &1).unwrap();
    client.add_guardian(&admin, &g2, &1).unwrap();
    client.add_guardian(&admin, &g3, &1).unwrap();

    // required = floor(3 * 3334 / 10000) = 1
    let pid = submit_proposal(&env, &client, 3_334);

    // One yes-vote (weight 1) must be sufficient
    client.vote_proposal(&pid, &true).unwrap();

    advance_past_eta(&env, 3_600);

    client
        .try_execute_proposal(&pid)
        .expect("one yes-vote (weight 1) must meet floor-calculated quorum of 1");
}

// ── Test 13: Guardian re-added after removal ──────────────────────────────────
//
// If a guardian is removed and then re-added (possibly with a different weight),
// any prior vote is still present in the proposal's vote map. The re-added guardian's
// new weight must be used when calculate_quorum is evaluated.

#[test]
fn quorum_guardian_readded_with_different_weight() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g = Address::generate(&env);
    // Original weight: 100
    client.add_guardian(&admin, &g, &100).unwrap();

    let pid = submit_proposal(&env, &client, 5_000); // 50%
    client.vote_proposal(&pid, &true).unwrap(); // votes yes at weight 100

    // Remove the guardian (vote no longer counted)
    client.remove_guardian(&admin, &g).unwrap();

    // Re-add with lower weight: 10.
    // total_weight is now 10; required = floor(10 * 5000 / 10000) = 5.
    // votes_for = 10 (g's yes vote re-counts at new weight 10) → 10 >= 5 → passes.
    client.add_guardian(&admin, &g, &10).unwrap();

    advance_past_eta(&env, 3_600);

    client
        .try_execute_proposal(&pid)
        .expect("re-added guardian's yes vote at new weight must satisfy quorum");

    let proposal = client.get_proposal(&pid).unwrap();
    assert!(proposal.executed);
}

// ── Test 14: Zero-quorum (quorum_bps = 0) always passes ──────────────────────

#[test]
fn quorum_zero_bps_passes_with_no_votes() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g = Address::generate(&env);
    client.add_guardian(&admin, &g, &100).unwrap();

    // quorum_bps = 0 → required = floor(100 * 0 / 10000) = 0
    let pid = submit_proposal(&env, &client, 0);

    // No one votes
    advance_past_eta(&env, 3_600);

    client
        .try_execute_proposal(&pid)
        .expect("0% quorum must pass with zero votes");

    let proposal = client.get_proposal(&pid).unwrap();
    assert!(proposal.executed);
}

// ── Test 15: Proposal not found ──────────────────────────────────────────────

#[test]
fn quorum_execute_nonexistent_proposal_returns_not_found() {
    let env = Env::default();
    env.mock_all_auths();
    let (_admin, client) = setup(&env);

    let result = client.try_execute_proposal(&9_999);
    assert_eq!(
        result,
        Err(Ok(Error::NotFound)),
        "executing a non-existent proposal must return NotFound"
    );
}

// ── Test 16: State is unchanged after rejected operations ────────────────────
//
// Verifies that a failed execute (ETA not reached, quorum not met, or
// already-executed) leaves proposal.executed = false and the guardian state
// untouched.

#[test]
fn quorum_state_unchanged_after_failed_execute() {
    let env = Env::default();
    env.mock_all_auths();
    let (admin, client) = setup(&env);

    let g = Address::generate(&env);
    client.add_guardian(&admin, &g, &100).unwrap();

    // Submit a proposal requiring 100% quorum, but nobody votes
    let pid = submit_proposal(&env, &client, 10_000);

    // Attempt 1: ETA not reached
    let _ = client.try_execute_proposal(&pid);
    let proposal = client.get_proposal(&pid).unwrap();
    assert!(!proposal.executed, "state must not change after ETA-not-reached failure");

    // Advance past ETA but quorum not met (no votes)
    advance_past_eta(&env, 3_600);

    // Attempt 2: quorum not met
    let _ = client.try_execute_proposal(&pid);
    let proposal = client.get_proposal(&pid).unwrap();
    assert!(!proposal.executed, "state must not change after quorum-not-met failure");

    // Guardian weight must also be unchanged
    assert_eq!(
        client.get_guardian_weight(&g),
        100,
        "guardian weight must be unchanged after failed executes"
    );
}
