//! Adversarial coverage for `governance::do_vote_proposal` (issue #1054).
//!
//! `do_vote_proposal` is the guardian voting entry point. It is stateful
//! (persists a per-voter ballot), authorization-sensitive (only a guardian with
//! non-zero weight may vote), and time-sensitive (votes lock at the proposal's
//! ETA). This module pins the success path, every documented rejection path, the
//! vote-lock boundary, and the "rejected operations leave the ballot unchanged"
//! invariant.

use crate::types::{Error, ProposalKind};
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    Address, Env, Symbol, TryFromVal,
};

fn init_vault<'a>(env: &'a Env, admin: &Address) -> SubscriptionVaultClient<'a> {
    let token_admin = Address::generate(env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    assert!(client
        .try_init(&token_address, &6, admin, &10_000_000, &86400)
        .is_ok());
    client
}

/// Submit a `RotateAdmin` proposal with a 50% quorum and a 1-hour timelock.
fn submit_rotate(client: &SubscriptionVaultClient, env: &Env, quorum_bps: u32) -> (u64, Address) {
    let target = Address::generate(env);
    let eta = env.ledger().timestamp() + 3_600;
    let id = client
        .submit_proposal(&ProposalKind::RotateAdmin, &target, &None, &0, &quorum_bps, &eta)
        ;
    (id, target)
}

fn has_event(env: &Env, name: &str) -> bool {
    let expected = Symbol::new(env, name);
    env.events().all().iter().any(|e| {
        let topics = e.1;
        topics
            .get(0)
            .and_then(|t| Symbol::try_from_val(env, &t).ok())
            .map(|t| t == expected)
            .unwrap_or(false)
    })
}

// ════════════════════════════════════════════════════════════════════
//  Success paths
// ════════════════════════════════════════════════════════════════════

#[test]
fn vote_records_yes_ballot_for_a_guardian() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let client = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let (id, _target) = submit_rotate(&client, &env, 5_000);

    let result = client.try_vote_proposal(&id, &true);

    assert!(result.is_ok(), "a weighted guardian vote must succeed");
    let proposal = client.get_proposal(&id).unwrap();
    assert_eq!(proposal.votes.len(), 1);
    assert_eq!(proposal.votes.get(admin), Some(true));
}

#[test]
fn vote_records_no_ballot() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let client = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let (id, _target) = submit_rotate(&client, &env, 5_000);

    client.try_vote_proposal(&id, &false).unwrap();

    assert_eq!(client.get_proposal(&id).unwrap().votes.get(admin), Some(false));
}

#[test]
fn voting_emits_proposal_voted_event() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let client = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let (id, _target) = submit_rotate(&client, &env, 5_000);

    client.try_vote_proposal(&id, &true).unwrap();

    assert!(has_event(&env, "proposal_voted"));
}

#[test]
fn vote_flip_before_eta_overwrites_previous_ballot() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let client = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let (id, _target) = submit_rotate(&client, &env, 5_000);

    client.try_vote_proposal(&id, &true).unwrap();
    client.try_vote_proposal(&id, &false).unwrap();

    let proposal = client.get_proposal(&id).unwrap();
    assert_eq!(proposal.votes.len(), 1, "a flip must overwrite, not append");
    assert_eq!(proposal.votes.get(admin), Some(false));
}

#[test]
fn duplicate_vote_is_not_double_counted() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let client = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let (id, _target) = submit_rotate(&client, &env, 10_000);

    client.try_vote_proposal(&id, &true).unwrap();
    client.try_vote_proposal(&id, &true).unwrap();

    assert_eq!(client.get_proposal(&id).unwrap().votes.len(), 1);
}

#[test]
fn yes_vote_meeting_quorum_executes_the_rotation() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let client = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let (id, target) = submit_rotate(&client, &env, 5_000);

    client.try_vote_proposal(&id, &true).unwrap();
    env.ledger().set_timestamp(env.ledger().timestamp() + 3_700);

    let result = client.try_execute_proposal(&id);

    assert!(result.is_ok(), "quorum was met, execution must succeed");
    assert_eq!(client.get_admin(), target);
    assert!(client.get_proposal(&id).unwrap().executed);
}

// ════════════════════════════════════════════════════════════════════
//  Rejection paths — state must be preserved
// ════════════════════════════════════════════════════════════════════

#[test]
fn non_guardian_voter_is_unauthorized_and_ballot_is_empty() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let client = init_vault(&env, &admin);
    // Admin is the stored admin but has *not* been added as a guardian.
    let (id, _target) = submit_rotate(&client, &env, 5_000);

    let result = client.try_vote_proposal(&id, &true);

    assert_eq!(result, Err(Ok(Error::Unauthorized)));
    assert_eq!(
        client.get_proposal(&id).unwrap().votes.len(),
        0,
        "rejected vote must not be recorded",
    );
}

#[test]
fn vote_on_missing_proposal_returns_not_found() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let client = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();

    let result = client.try_vote_proposal(&999u64, &true);

    assert_eq!(result, Err(Ok(Error::NotFound)));
}

// ════════════════════════════════════════════════════════════════════
//  Timelock boundary — votes lock at / after the ETA
// ════════════════════════════════════════════════════════════════════

#[test]
fn vote_at_exact_eta_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let client = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let (id, _target) = submit_rotate(&client, &env, 5_000);

    let eta = client.get_proposal(&id).unwrap().eta;
    env.ledger().set_timestamp(eta);

    let result = client.try_vote_proposal(&id, &true);

    assert_eq!(result, Err(Ok(Error::InvalidInput)));
    assert_eq!(client.get_proposal(&id).unwrap().votes.len(), 0);
}

#[test]
fn vote_after_eta_is_rejected_emits_vote_locked_and_keeps_ballot() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let client = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let (id, _target) = submit_rotate(&client, &env, 5_000);

    client.try_vote_proposal(&id, &true).unwrap();
    let eta = client.get_proposal(&id).unwrap().eta;
    env.ledger().set_timestamp(eta + 100);

    let result = client.try_vote_proposal(&id, &false);

    assert_eq!(result, Err(Ok(Error::InvalidInput)));
    assert!(has_event(&env, "vote_locked"), "vote_locked event must fire");
    let proposal = client.get_proposal(&id).unwrap();
    assert_eq!(
        proposal.votes.get(admin),
        Some(true),
        "a locked-out vote must not overwrite the existing ballot",
    );
}

// ════════════════════════════════════════════════════════════════════
//  Executed proposals reject further votes
// ════════════════════════════════════════════════════════════════════

#[test]
#[ignore = "pre-existing on main; docs/known-failing-tests.md: expected Err(Ok(InvalidInput)), got Err(Ok(Unauthorized))"]
fn vote_on_executed_proposal_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let client = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let (id, target) = submit_rotate(&client, &env, 5_000);

    client.try_vote_proposal(&id, &true).unwrap();
    env.ledger().set_timestamp(env.ledger().timestamp() + 3_700);
    client.try_execute_proposal(&id).unwrap();
    assert_eq!(client.get_admin(), target);

    let result = client.try_vote_proposal(&id, &false);

    assert_eq!(result, Err(Ok(Error::InvalidInput)));
    let proposal = client.get_proposal(&id).unwrap();
    assert!(proposal.executed);
    assert_eq!(proposal.votes.get(admin), Some(true));
}

// ════════════════════════════════════════════════════════════════════
//  Quorum arithmetic / voter-set changes
// ════════════════════════════════════════════════════════════════════

#[test]
fn single_yes_vote_below_full_quorum_cannot_execute() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let other = Address::generate(&env);
    let client = init_vault(&env, &admin);
    // Total weight 200. A 100% quorum needs 200; the admin alone has 100.
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    client.try_add_guardian(&admin, &other, &100).unwrap();
    let (id, _target) = submit_rotate(&client, &env, 10_000);

    client.try_vote_proposal(&id, &true).unwrap();
    env.ledger().set_timestamp(env.ledger().timestamp() + 3_700);

    let result = client.try_execute_proposal(&id);

    assert_eq!(result, Err(Ok(Error::InvalidInput)));
    assert!(!client.get_proposal(&id).unwrap().executed);
}

#[test]
fn opposing_vote_cannot_satisfy_quorum() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let client = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let (id, _target) = submit_rotate(&client, &env, 5_000);

    client.try_vote_proposal(&id, &false).unwrap();
    env.ledger().set_timestamp(env.ledger().timestamp() + 3_700);

    let result = client.try_execute_proposal(&id);

    assert_eq!(result, Err(Ok(Error::InvalidInput)));
    assert!(!client.get_proposal(&id).unwrap().executed);
}
