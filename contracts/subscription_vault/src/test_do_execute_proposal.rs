//! Adversarial coverage for `governance::do_execute_proposal` (issue #1055).
//!
//! `do_execute_proposal` is the quorum-gated execution entry point for
//! governance proposals. It is stateful (marks the proposal executed and
//! applies a privileged action), time-sensitive (ETA timelock), and
//! arithmetic-sensitive (basis-point quorum over live guardian weights). It is
//! intentionally permissionless — anyone may trigger execution once the
//! timelock and quorum are satisfied — so there is no unauthorized-caller
//! path; these tests pin that property alongside every documented rejection
//! path, the ETA/quorum boundaries, and the "rejected operations leave state
//! unchanged" invariant.

use crate::types::{DataKey, Error, ProposalKind};
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    Address, Env, Symbol, TryFromVal,
};

fn init_vault<'a>(env: &'a Env, admin: &Address) -> (Address, SubscriptionVaultClient<'a>) {
    let token_admin = Address::generate(env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    assert!(client
        .try_init(&token_address, &6, admin, &10_000_000, &86400)
        .is_ok());
    (contract_id, client)
}

/// Submit a `RotateAdmin` proposal toward `target` with a 1-hour timelock.
fn submit_rotate(
    client: &SubscriptionVaultClient,
    env: &Env,
    target: &Address,
    quorum_bps: u32,
) -> u64 {
    let eta = env.ledger().timestamp() + 3_600;
    client.submit_proposal(
        &ProposalKind::RotateAdmin,
        target,
        &None,
        &0,
        &quorum_bps,
        &eta,
    )
}

/// Submit a `SetProtocolFee` proposal (`target3` = fee bps, `target2` = treasury).
fn submit_fee(
    client: &SubscriptionVaultClient,
    env: &Env,
    treasury: &Option<Address>,
    fee_bps: u32,
    quorum_bps: u32,
) -> u64 {
    let dummy = Address::generate(env);
    let eta = env.ledger().timestamp() + 3_600;
    client.submit_proposal(
        &ProposalKind::SetProtocolFee,
        &dummy,
        treasury,
        &fee_bps,
        &quorum_bps,
        &eta,
    )
}

/// Submit an `UpgradeContract` proposal (reserved kind; execution must reject).
fn submit_upgrade(client: &SubscriptionVaultClient, env: &Env, quorum_bps: u32) -> u64 {
    let target = Address::generate(env);
    let eta = env.ledger().timestamp() + 3_600;
    client.submit_proposal(
        &ProposalKind::UpgradeContract,
        &target,
        &None,
        &0,
        &quorum_bps,
        &eta,
    )
}

/// Vote YES as the stored admin (mocked auth) and fast-forward past the ETA.
fn vote_yes_and_mature(env: &Env, client: &SubscriptionVaultClient, id: u64) {
    client.try_vote_proposal(&id, &true).unwrap();
    advance_past_eta(env, client, id);
}

/// Advance past the ETA without casting an additional vote.
fn advance_past_eta(env: &Env, client: &SubscriptionVaultClient, id: u64) {
    let eta = client.get_proposal(&id).unwrap().eta;
    env.ledger().set_timestamp(eta + 1);
}

/// Read the raw treasury slot straight from contract storage (no view exists).
fn read_treasury(env: &Env, contract_id: &Address) -> Option<Address> {
    env.as_contract(contract_id, || {
        env.storage()
            .persistent()
            .get::<_, Address>(&DataKey::Treasury)
    })
}

/// Returns true when an event with topic `name` is visible. Callers must
/// invoke this before any further contract call, because the testenv only
/// exposes the most recent invocation's events.
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
fn rotate_admin_executes_at_exact_eta_and_marks_executed() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let target = Address::generate(&env);
    let id = submit_rotate(&client, &env, &target, 5_000);

    client.try_vote_proposal(&id, &true).unwrap();
    // Boundary: `now == eta` satisfies `now < eta` rejection guard's negation.
    let eta = client.get_proposal(&id).unwrap().eta;
    env.ledger().set_timestamp(eta);

    assert!(client.try_execute_proposal(&id).is_ok());
    assert_eq!(client.get_admin(), target);
    assert!(client.get_proposal(&id).unwrap().executed);
}

#[test]
fn rotate_admin_executes_after_eta_and_emits_event() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let target = Address::generate(&env);
    let id = submit_rotate(&client, &env, &target, 5_000);
    vote_yes_and_mature(&env, &client, id);

    assert!(client.try_execute_proposal(&id).is_ok());
    // NOTE: `env.events().all()` only exposes the most recent invocation's
    // events, so the emission must be asserted before any further contract
    // call (e.g. `get_admin`) overwrites the visible event window.
    assert!(has_event(&env, "proposal_executed"));
    assert_eq!(client.get_admin(), target);
    assert!(client.get_proposal(&id).unwrap().executed);
}

#[test]
fn set_protocol_fee_updates_fee_and_treasury() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let (contract_id, client) = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    assert_eq!(client.get_protocol_fee_bps(), 0);
    assert_eq!(read_treasury(&env, &contract_id), None);

    let treasury = Address::generate(&env);
    let id = submit_fee(&client, &env, &Some(treasury.clone()), 250, 5_000);
    vote_yes_and_mature(&env, &client, id);

    assert!(client.try_execute_proposal(&id).is_ok());
    assert_eq!(client.get_protocol_fee_bps(), 250);
    assert_eq!(read_treasury(&env, &contract_id), Some(treasury));
    assert!(client.get_proposal(&id).unwrap().executed);
}

#[test]
fn set_protocol_fee_without_treasury_preserves_existing_treasury() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let (contract_id, client) = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();

    // Seed a treasury via a first proposal.
    let treasury = Address::generate(&env);
    let seed = submit_fee(&client, &env, &Some(treasury.clone()), 100, 5_000);
    vote_yes_and_mature(&env, &client, seed);
    client.try_execute_proposal(&seed).unwrap();
    assert_eq!(read_treasury(&env, &contract_id), Some(treasury.clone()));

    // A follow-up proposal with `target2 == None` must leave it untouched.
    let id = submit_fee(&client, &env, &None, 300, 5_000);
    vote_yes_and_mature(&env, &client, id);
    assert!(client.try_execute_proposal(&id).is_ok());
    assert_eq!(client.get_protocol_fee_bps(), 300);
    assert_eq!(read_treasury(&env, &contract_id), Some(treasury));
}

#[test]
fn zero_quorum_executes_without_any_votes() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let target = Address::generate(&env);
    // quorum_bps = 0 → required = 0, so even an empty ballot satisfies quorum.
    let id = submit_rotate(&client, &env, &target, 0);
    let eta = client.get_proposal(&id).unwrap().eta;
    env.ledger().set_timestamp(eta + 1);

    assert!(client.try_execute_proposal(&id).is_ok());
    assert_eq!(client.get_admin(), target);
}

#[test]
fn empty_guardian_set_executes_when_required_votes_are_zero() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);
    // No guardians at all: total weight 0 → required 0 → 0 >= 0 holds.
    let target = Address::generate(&env);
    let id = submit_rotate(&client, &env, &target, 5_000);
    let eta = client.get_proposal(&id).unwrap().eta;
    env.ledger().set_timestamp(eta + 1);

    assert!(
        client.try_execute_proposal(&id).is_ok(),
        "total=0/required=0 must deterministically execute"
    );
    assert_eq!(client.get_admin(), target);
}

#[test]
fn exact_quorum_threshold_succeeds() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let other = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);
    // Total 200. quorum 5000 → required exactly 100; the admin's 100 meets it.
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    client.try_add_guardian(&admin, &other, &100).unwrap();
    let target = Address::generate(&env);
    let id = submit_rotate(&client, &env, &target, 5_000);
    vote_yes_and_mature(&env, &client, id);

    assert!(client.try_execute_proposal(&id).is_ok());
    assert_eq!(client.get_admin(), target);
}

#[test]
fn execution_is_permissionless_and_never_returns_unauthorized() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    // `do_execute_proposal` performs no authorization check: any caller may
    // trigger execution once timelock + quorum hold. With mocked auth every
    // caller passes, so the observable property pinned here is that the
    // outcome is never an identity rejection (Unauthorized/Forbidden).
    let target = Address::generate(&env);
    let id = submit_rotate(&client, &env, &target, 5_000);
    vote_yes_and_mature(&env, &client, id);

    let result = client.try_execute_proposal(&id);
    assert!(result.is_ok());
    assert_ne!(result, Err(Ok(Error::Unauthorized)));
    assert_ne!(result, Err(Ok(Error::Forbidden)));
    assert_eq!(client.get_admin(), target);
}

// ════════════════════════════════════════════════════════════════════
//  Rejection paths — state must be unchanged
// ════════════════════════════════════════════════════════════════════

#[test]
fn execute_missing_proposal_returns_not_found_and_changes_nothing() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let (contract_id, client) = init_vault(&env, &admin);
    let admin_before = client.get_admin();
    let fee_before = client.get_protocol_fee_bps();

    for missing in [0u64, 999, u64::MAX] {
        assert_eq!(
            client.try_execute_proposal(&missing),
            Err(Ok(Error::NotFound)),
            "id {missing} was never allocated"
        );
        assert!(client.get_proposal(&missing).is_none());
    }

    assert_eq!(client.get_admin(), admin_before);
    assert_eq!(client.get_protocol_fee_bps(), fee_before);
    assert_eq!(read_treasury(&env, &contract_id), None);
}

#[test]
fn premature_execution_is_rejected_and_leaves_state_unchanged() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let target = Address::generate(&env);
    let id = submit_rotate(&client, &env, &target, 5_000);
    client.try_vote_proposal(&id, &true).unwrap();

    let eta = client.get_proposal(&id).unwrap().eta;
    let admin_before = client.get_admin();

    // Immediately after submission and one second before the ETA.
    for now in [env.ledger().timestamp(), eta - 1] {
        env.ledger().set_timestamp(now);
        assert_eq!(
            client.try_execute_proposal(&id),
            Err(Ok(Error::InvalidInput)),
            "execution at {now} (eta {eta}) must be rejected"
        );
    }

    let proposal = client.get_proposal(&id).unwrap();
    assert!(
        !proposal.executed,
        "rejected execute must not mark executed"
    );
    assert_eq!(proposal.votes.len(), 1, "ballot must be preserved");
    assert_eq!(client.get_admin(), admin_before);
}

#[test]
fn double_execute_is_rejected_and_keeps_first_effect() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let target = Address::generate(&env);
    let id = submit_rotate(&client, &env, &target, 5_000);
    vote_yes_and_mature(&env, &client, id);

    assert!(client.try_execute_proposal(&id).is_ok());
    assert_eq!(
        client.try_execute_proposal(&id),
        Err(Ok(Error::InvalidInput)),
        "re-execution must be rejected"
    );
    // First effect stands; the second call changes nothing further.
    assert_eq!(client.get_admin(), target);
    assert!(client.get_proposal(&id).unwrap().executed);
}

#[test]
fn cancelled_proposal_cannot_execute() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let target = Address::generate(&env);
    let id = submit_rotate(&client, &env, &target, 5_000);
    client.try_vote_proposal(&id, &true).unwrap();

    let reason = soroban_sdk::String::from_str(&env, "superseded");
    client.try_cancel_proposal(&id, &reason).unwrap();

    advance_past_eta(&env, &client, id);
    assert_eq!(
        client.try_execute_proposal(&id),
        Err(Ok(Error::InvalidInput)),
        "cancelled (executed=true) proposals must not execute"
    );
    assert_eq!(client.get_admin(), admin, "admin must be unchanged");
}

#[test]
fn quorum_not_met_without_votes_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let target = Address::generate(&env);
    let id = submit_rotate(&client, &env, &target, 5_000);
    let admin_before = client.get_admin();

    let eta = client.get_proposal(&id).unwrap().eta;
    env.ledger().set_timestamp(eta + 1);

    assert_eq!(
        client.try_execute_proposal(&id),
        Err(Ok(Error::InvalidInput))
    );
    assert!(!client.get_proposal(&id).unwrap().executed);
    assert_eq!(client.get_admin(), admin_before);
}

#[test]
fn opposing_vote_cannot_satisfy_quorum() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let target = Address::generate(&env);
    let id = submit_rotate(&client, &env, &target, 5_000);
    client.try_vote_proposal(&id, &false).unwrap();
    let eta = client.get_proposal(&id).unwrap().eta;
    env.ledger().set_timestamp(eta + 1);

    assert_eq!(
        client.try_execute_proposal(&id),
        Err(Ok(Error::InvalidInput))
    );
    assert!(!client.get_proposal(&id).unwrap().executed);
    assert_eq!(client.get_admin(), admin);
}

#[test]
fn just_below_quorum_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let other = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);
    // Total 200. quorum 5100 → required = 200*5100/10000 = 102 > admin's 100.
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    client.try_add_guardian(&admin, &other, &100).unwrap();
    let target = Address::generate(&env);
    let id = submit_rotate(&client, &env, &target, 5_100);
    vote_yes_and_mature(&env, &client, id);

    assert_eq!(
        client.try_execute_proposal(&id),
        Err(Ok(Error::InvalidInput)),
        "100 < 102 required must be rejected"
    );
    assert!(!client.get_proposal(&id).unwrap().executed);
    assert_eq!(client.get_admin(), admin);
}

#[test]
fn guardian_removal_invalidates_prior_vote() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let other = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    client.try_add_guardian(&admin, &other, &100).unwrap();
    let target = Address::generate(&env);
    let id = submit_rotate(&client, &env, &target, 5_000);
    client.try_vote_proposal(&id, &true).unwrap();

    // Removing the admin's guardian weight drops votes_for to 0 while total
    // stays 100 → required 50 is no longer met.
    client.try_remove_guardian(&admin, &admin).unwrap();
    assert_eq!(client.get_guardian_weight(&admin), 0);
    advance_past_eta(&env, &client, id);

    assert_eq!(
        client.try_execute_proposal(&id),
        Err(Ok(Error::InvalidInput)),
        "a removed guardian's vote must not count"
    );
    assert!(!client.get_proposal(&id).unwrap().executed);
    assert_eq!(client.get_admin(), admin);
}

#[test]
fn upgrade_contract_kind_is_always_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    let admin_before = client.get_admin();

    let id = submit_upgrade(&client, &env, 5_000);
    vote_yes_and_mature(&env, &client, id);

    assert_eq!(
        client.try_execute_proposal(&id),
        Err(Ok(Error::InvalidInput)),
        "reserved UpgradeContract kind must be rejected even with quorum + ETA"
    );
    let proposal = client.get_proposal(&id).unwrap();
    assert!(!proposal.executed, "rejected kind must not mark executed");
    assert_eq!(client.get_admin(), admin_before);
}

#[test]
fn rejected_fee_execute_leaves_fee_and_treasury_unchanged() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let (contract_id, client) = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &admin, &100).unwrap();

    // No votes cast: quorum (50%) cannot be met → InvalidInput.
    let treasury = Address::generate(&env);
    let id = submit_fee(&client, &env, &Some(treasury), 500, 5_000);
    let eta = client.get_proposal(&id).unwrap().eta;
    env.ledger().set_timestamp(eta + 1);

    assert_eq!(
        client.try_execute_proposal(&id),
        Err(Ok(Error::InvalidInput))
    );
    assert_eq!(client.get_protocol_fee_bps(), 0);
    assert_eq!(read_treasury(&env, &contract_id), None);
    assert!(!client.get_proposal(&id).unwrap().executed);
}
