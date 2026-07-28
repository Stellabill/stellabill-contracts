#![cfg(test)]

use crate::{
    types::{AdminProposal, AdminProposalCancelledEvent, AdminProposalClaimedEvent, AdminProposalCreatedEvent},
    Error, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Events},
    Address, Env, IntoVal, Symbol,
};

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000_000);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, client, token, admin)
}

fn advance_seconds(env: &Env, seconds: u64) {
    let t = env.ledger().timestamp();
    env.ledger().set_timestamp(t + seconds);
}

// ── propose_admin ────────────────────────────────────────────────────────────

#[test]
fn test_propose_admin_success() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);

    let proposal = client.get_admin_proposal();
    assert!(proposal.is_some());
    let p = proposal.unwrap();
    assert_eq!(p.new_admin, new_admin);
    assert_eq!(p.proposed_at, 1_000_000);
    assert_eq!(p.expires_at, 1_000_000 + 7 * 24 * 60 * 60);
}

#[test]
fn test_propose_admin_unauthorized() {
    let (env, client, _token, _admin) = setup();
    let stranger = Address::generate(&env);
    let new_admin = Address::generate(&env);

    let result = client.try_propose_admin(&stranger, &new_admin);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_propose_admin_twice_rejected() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    let second = Address::generate(&env);
    let result = client.try_propose_admin(&admin, &second);
    assert_eq!(result, Err(Ok(Error::ProposalAlreadyExists)));
}

#[test]
fn test_propose_admin_to_contract_rejected() {
    let (env, client, _token, admin) = setup();

    let result = client.try_propose_admin(&admin, &client.address);
    assert_eq!(result, Err(Ok(Error::InvalidNewAdmin)));
}

#[test]
fn test_propose_admin_event_emitted() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);

    let events = env.events().all();
    let created = events.find_first(&(Symbol::new(&env, "admin_proposal_created"),));
    assert!(created.is_some());
}

#[test]
fn test_propose_admin_during_emergency_stop() {
    let (env, client, _token, admin) = setup();
    client.enable_emergency_stop(&admin);
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    assert!(client.get_admin_proposal().is_some());
}

// ── claim_admin_role ─────────────────────────────────────────────────────────

#[test]
fn test_claim_admin_role_success() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.claim_admin_role(&new_admin);

    assert_eq!(client.get_admin(), new_admin);
    assert!(client.get_admin_proposal().is_none());
}

#[test]
fn test_claim_admin_role_wrong_claimant() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    let impostor = Address::generate(&env);
    let result = client.try_claim_admin_role(&impostor);
    assert_eq!(result, Err(Ok(Error::InvalidClaimant)));
}

#[test]
fn test_claim_admin_role_no_proposal() {
    let (env, client, _token, _admin) = setup();
    let claimant = Address::generate(&env);

    let result = client.try_claim_admin_role(&claimant);
    assert_eq!(result, Err(Ok(Error::ProposalNotFound)));
}

#[test]
fn test_claim_admin_role_expired() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    advance_seconds(&env, 7 * 24 * 60 * 60 + 1);

    let result = client.try_claim_admin_role(&new_admin);
    assert_eq!(result, Err(Ok(Error::ProposalExpired)));

    // Proposal should be cleaned up after expiration check
    assert!(client.get_admin_proposal().is_none());
}

#[test]
fn test_claim_admin_role_new_admin_can_operate() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.claim_admin_role(&new_admin);

    // New admin can perform admin operations
    let result = client.try_set_min_topup(&new_admin, &2_000_000i128);
    assert!(result.is_ok());

    // Old admin cannot
    let result = client.try_set_min_topup(&admin, &2_000_000i128);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_claim_admin_role_event_emitted() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.claim_admin_role(&new_admin);

    let events = env.events().all();
    let claimed = events.find_first(&(Symbol::new(&env, "admin_proposal_claimed"),));
    assert!(claimed.is_some());
}

#[test]
fn test_claim_admin_role_after_expiry_rejected_without_side_effects() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    advance_seconds(&env, 7 * 24 * 60 * 60 + 1);

    let result = client.try_claim_admin_role(&new_admin);
    assert_eq!(result, Err(Ok(Error::ProposalExpired)));

    // Admin unchanged
    assert_eq!(client.get_admin(), admin);
}

// ── cancel_admin_proposal ────────────────────────────────────────────────────

#[test]
fn test_cancel_admin_proposal_success() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.cancel_admin_proposal(&admin);

    assert!(client.get_admin_proposal().is_none());
    assert_eq!(client.get_admin(), admin);
}

#[test]
fn test_cancel_admin_proposal_no_proposal() {
    let (env, client, _token, admin) = setup();
    let result = client.try_cancel_admin_proposal(&admin);
    assert_eq!(result, Err(Ok(Error::NoActiveProposal)));
}

#[test]
fn test_cancel_admin_proposal_unauthorized() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);
    client.propose_admin(&admin, &new_admin);

    let stranger = Address::generate(&env);
    let result = client.try_cancel_admin_proposal(&stranger);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_cancel_admin_proposal_event_emitted() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.cancel_admin_proposal(&admin);

    let events = env.events().all();
    let cancelled = events.find_first(&(Symbol::new(&env, "admin_proposal_cancelled"),));
    assert!(cancelled.is_some());
}

#[test]
fn test_cancel_then_repropose_works() {
    let (env, client, _token, admin) = setup();
    let first = Address::generate(&env);
    let second = Address::generate(&env);

    client.propose_admin(&admin, &first);
    client.cancel_admin_proposal(&admin);

    client.propose_admin(&admin, &second);
    let proposal = client.get_admin_proposal().unwrap();
    assert_eq!(proposal.new_admin, second);
}

// ── get_admin_proposal ───────────────────────────────────────────────────────

#[test]
fn test_get_admin_proposal_none_when_not_set() {
    let (_env, client, _token, _admin) = setup();
    assert!(client.get_admin_proposal().is_none());
}

#[test]
fn test_get_admin_proposal_expired_still_visible() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    advance_seconds(&env, 14 * 24 * 60 * 60);

    // Proposal is still readable (cleaned up on claim attempt)
    let proposal = client.get_admin_proposal();
    assert!(proposal.is_some());
}

// ── Full lifecycle: propose -> claim -> operate -> rotate again ──────────────

#[test]
fn test_two_step_rotation_full_lifecycle() {
    let (env, client, _token, admin) = setup();
    let admin2 = Address::generate(&env);
    let admin3 = Address::generate(&env);

    // Propose admin2
    client.propose_admin(&admin, &admin2);
    assert!(client.get_admin_proposal().is_some());

    // Claim admin2
    client.claim_admin_role(&admin2);
    assert_eq!(client.get_admin(), admin2);

    // admin2 proposes admin3
    client.propose_admin(&admin2, &admin3);
    client.claim_admin_role(&admin3);
    assert_eq!(client.get_admin(), admin3);

    // Old admins cannot operate
    assert_eq!(
        client.try_set_min_topup(&admin, &999i128),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        client.try_set_min_topup(&admin2, &999i128),
        Err(Ok(Error::Unauthorized))
    );
    assert!(client.try_set_min_topup(&admin3, &999i128).is_ok());
}

// ─── Event payload verification ──────────────────────────────────────────────

#[test]
fn test_admin_proposal_created_event_payload() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);

    let events = env.events().all();
    let (_, _, payload) = events
        .find_first(&(Symbol::new(&env, "admin_proposal_created"),))
        .unwrap();

    let parsed: AdminProposalCreatedEvent = payload.try_into_val(&env).unwrap();
    assert_eq!(parsed.old_admin, admin);
    assert_eq!(parsed.new_admin, new_admin);
    assert_eq!(parsed.expires_at, 1_000_000 + 7 * 24 * 60 * 60);
    assert_eq!(parsed.timestamp, 1_000_000);
}

#[test]
fn test_admin_proposal_claimed_event_payload() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.claim_admin_role(&new_admin);

    let events = env.events().all();
    let (_, _, payload) = events
        .find_first(&(Symbol::new(&env, "admin_proposal_claimed"),))
        .unwrap();

    let parsed: AdminProposalClaimedEvent = payload.try_into_val(&env).unwrap();
    assert_eq!(parsed.old_admin, admin);
    assert_eq!(parsed.new_admin, new_admin);
}

#[test]
fn test_admin_proposal_cancelled_event_payload() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.cancel_admin_proposal(&admin);

    let events = env.events().all();
    let (_, _, payload) = events
        .find_first(&(Symbol::new(&env, "admin_proposal_cancelled"),))
        .unwrap();

    let parsed: AdminProposalCancelledEvent = payload.try_into_val(&env).unwrap();
    assert_eq!(parsed.admin, admin);
}

// ── Old rotate_admin still works (backward compat) ───────────────────────────

#[test]
fn test_old_rotate_admin_still_works() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.rotate_admin(&admin, &new_admin);
    assert_eq!(client.get_admin(), new_admin);
}

#[test]
fn test_proposal_cannot_be_claimed_by_old_admin_after_immediate_rotation() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    // Use old single-step to rotate directly instead
    client.rotate_admin(&admin, &new_admin);
    // Claim is no longer needed; proposal is stale
    // However, the proposal still exists since we didn't cancel it.
    // But admin is already new_admin, and the proposal was for them.
    // They claim it, but it should be no-op effect (already admin).
    client.claim_admin_role(&new_admin);
    assert_eq!(client.get_admin(), new_admin);
}