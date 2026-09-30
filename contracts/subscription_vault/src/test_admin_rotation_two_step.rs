#![cfg(test)]

use crate::{
    types::{
        AdminProposalCancelledEvent, AdminProposalClaimedEvent, AdminProposalCreatedEvent,
    },
    Error, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    Address, Env, IntoVal, Symbol, TryFromVal, Val, Vec,
};

fn find_event_data(env: &Env, topic: &Symbol) -> Option<Val> {
    let all = env.events().all();
    let topic_val: Val = topic.clone().into_val(env);
    for i in 0..all.len() {
        let (_, topics, data): (Address, Vec<Val>, Val) = all.get(i).unwrap();
        let first_topic = topics.get(0);
        if first_topic == Some(topic_val.clone()) {
            return Some(data);
        }
    }
    None
}

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
    let (_env, client, _token, admin) = setup();

    let result = client.try_propose_admin(&admin, &client.address);
    assert_eq!(result, Err(Ok(Error::InvalidNewAdmin)));
}

#[test]
fn test_propose_admin_event_emitted() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);

    assert!(find_event_data(&env, &Symbol::new(&env, "admin_proposal_created")).is_some());
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
    assert_eq!(result, Err(Ok(Error::Forbidden)));
}

#[test]
fn test_claim_admin_role_event_emitted() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.claim_admin_role(&new_admin);

    assert!(find_event_data(&env, &Symbol::new(&env, "admin_proposal_claimed")).is_some());
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
    let (_env, client, _token, admin) = setup();
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

    assert!(find_event_data(&env, &Symbol::new(&env, "admin_proposal_cancelled")).is_some());
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

// ── cancel_admin_proposal — additional adversarial and boundary coverage ─────

/// Double-cancel: after the first cancel removes the proposal, a second
/// cancel by the same admin must return `NoActiveProposal` and must not
/// alter any state.
#[test]
fn test_cancel_admin_proposal_double_cancel_rejected() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.cancel_admin_proposal(&admin);

    let result = client.try_cancel_admin_proposal(&admin);
    assert_eq!(result, Err(Ok(Error::NoActiveProposal)));
    // Admin is still the original admin and no proposal exists.
    assert_eq!(client.get_admin(), admin);
    assert!(client.get_admin_proposal().is_none());
}

/// The pending `new_admin` is not the stored admin, so they cannot cancel —
/// only the current admin holds that privilege.
#[test]
fn test_cancel_admin_proposal_proposed_address_cannot_cancel() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);

    // `new_admin` is the proposed address but is NOT the current admin.
    let result = client.try_cancel_admin_proposal(&new_admin);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));

    // Proposal is still live.
    assert!(client.get_admin_proposal().is_some());
}

/// Cancelling a proposal whose window has already elapsed is still valid —
/// the function must not check expiry, only whether a proposal exists.
#[test]
fn test_cancel_admin_proposal_after_window_elapsed_succeeds() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    // Advance past the 7-day window.
    advance_seconds(&env, 7 * 24 * 60 * 60 + 1);

    // Cancel must succeed even though the proposal is expired.
    client.cancel_admin_proposal(&admin);

    assert!(client.get_admin_proposal().is_none());
    assert_eq!(client.get_admin(), admin);
}

/// After cancellation, the intended claimant can no longer claim the role
/// and the admin address remains unchanged.
#[test]
fn test_cancel_admin_proposal_blocks_subsequent_claim() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.cancel_admin_proposal(&admin);

    let result = client.try_claim_admin_role(&new_admin);
    assert_eq!(result, Err(Ok(Error::ProposalNotFound)));
    assert_eq!(client.get_admin(), admin);
}

/// State invariant: a successful cancel must leave the stored admin address
/// exactly as it was before the proposal was created.
#[test]
fn test_cancel_admin_proposal_admin_address_unchanged() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    let admin_before = client.get_admin();
    client.propose_admin(&admin, &new_admin);
    client.cancel_admin_proposal(&admin);
    let admin_after = client.get_admin();

    assert_eq!(admin_before, admin_after);
}

/// The event payload must carry the correct admin address and a timestamp
/// equal to the ledger time at the moment of cancellation.
#[test]
fn test_cancel_admin_proposal_event_payload() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    // Advance time so the cancel timestamp differs from proposal time.
    advance_seconds(&env, 3600);
    client.cancel_admin_proposal(&admin);

    let payload =
        find_event_data(&env, &Symbol::new(&env, "admin_proposal_cancelled")).unwrap();
    let parsed = AdminProposalCancelledEvent::try_from_val(&env, &payload).unwrap();
    assert_eq!(parsed.admin, admin);
    assert_eq!(parsed.timestamp, 1_000_000 + 3600);
}

/// After a single-step rotation the former admin is no longer the stored
/// admin and must be rejected when they attempt to cancel a proposal that
/// the new admin created.
#[test]
fn test_cancel_admin_proposal_former_admin_rejected_after_rotation() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);
    let third = Address::generate(&env);

    // Rotate directly (single-step) so `admin` loses the role.
    client.rotate_admin(&admin, &new_admin, &0);
    assert_eq!(client.get_admin(), new_admin);

    // New admin creates a proposal.
    client.propose_admin(&new_admin, &third);

    // Former admin attempts to cancel — must be rejected.
    let result = client.try_cancel_admin_proposal(&admin);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));

    // Proposal is still intact.
    assert!(client.get_admin_proposal().is_some());
}

/// Cancel → repropose → cancel again: the contract must handle repeated
/// create/cancel cycles without accumulating stale state.
#[test]
fn test_cancel_admin_proposal_multiple_cycles() {
    let (env, client, _token, admin) = setup();

    for _ in 0..3 {
        let candidate = Address::generate(&env);
        client.propose_admin(&admin, &candidate);
        assert!(client.get_admin_proposal().is_some());
        client.cancel_admin_proposal(&admin);
        assert!(client.get_admin_proposal().is_none());
    }

    // Admin is unchanged after three full cycles.
    assert_eq!(client.get_admin(), admin);
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
        Err(Ok(Error::Forbidden))
    );
    assert_eq!(
        client.try_set_min_topup(&admin2, &999i128),
        Err(Ok(Error::Forbidden))
    );
    assert!(client.try_set_min_topup(&admin3, &999i128).is_ok());
}

// ─── Event payload verification ──────────────────────────────────────────────

#[test]
fn test_admin_proposal_created_event_payload() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);

    let payload = find_event_data(&env, &Symbol::new(&env, "admin_proposal_created")).unwrap();
    let parsed = AdminProposalCreatedEvent::try_from_val(&env, &payload).unwrap();
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

    let payload = find_event_data(&env, &Symbol::new(&env, "admin_proposal_claimed")).unwrap();
    let parsed = AdminProposalClaimedEvent::try_from_val(&env, &payload).unwrap();
    assert_eq!(parsed.old_admin, admin);
    assert_eq!(parsed.new_admin, new_admin);
}

#[test]
fn test_admin_proposal_cancelled_event_payload() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.cancel_admin_proposal(&admin);

    let payload = find_event_data(&env, &Symbol::new(&env, "admin_proposal_cancelled")).unwrap();
    let parsed = AdminProposalCancelledEvent::try_from_val(&env, &payload).unwrap();
    assert_eq!(parsed.admin, admin);
}

// ── Old rotate_admin still works (backward compat) ───────────────────────────

#[test]
fn test_old_rotate_admin_still_works() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.rotate_admin(&admin, &new_admin, &0);
    assert_eq!(client.get_admin(), new_admin);
}

#[test]
fn test_rotate_admin_rejects_skipped_nonce() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    let result = client.try_rotate_admin(&admin, &new_admin, &1);
    assert_eq!(result, Err(Ok(Error::NonceAlreadyUsed)));
    assert_eq!(client.get_admin(), admin);
}

#[test]
fn test_rotate_admin_accepts_current_nonce_after_rejected_skip() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    let result = client.try_rotate_admin(&admin, &new_admin, &1);
    assert_eq!(result, Err(Ok(Error::NonceAlreadyUsed)));

    client.rotate_admin(&admin, &new_admin, &0);
    assert_eq!(client.get_admin(), new_admin);
}

#[test]
fn test_rotate_admin_nonce_is_per_signer() {
    let (env, client, _token, admin) = setup();
    let admin2 = Address::generate(&env);
    let admin3 = Address::generate(&env);

    client.rotate_admin(&admin, &admin2, &0);
    client.rotate_admin(&admin2, &admin3, &0);
    assert_eq!(client.get_admin(), admin3);
}

#[test]
fn test_proposal_cannot_be_claimed_by_old_admin_after_immediate_rotation() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    // Use old single-step to rotate directly instead
    client.rotate_admin(&admin, &new_admin, &0);
    // Claim is no longer needed; proposal is stale
    // However, the proposal still exists since we didn't cancel it.
    // But admin is already new_admin, and the proposal was for them.
    // They claim it, but it should be no-op effect (already admin).
    client.claim_admin_role(&new_admin);
    assert_eq!(client.get_admin(), new_admin);
}