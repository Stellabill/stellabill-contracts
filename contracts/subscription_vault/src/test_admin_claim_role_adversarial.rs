#![cfg(test)]

//! Adversarial coverage for `admin::do_claim_admin_role` — the second step of
//! the two-step admin rotation (`propose_admin` → `claim_admin_role`).
//!
//! The happy path is short: the current admin proposes an address, and that
//! exact address claims within a seven-day window. Everything a hostile or
//! merely unlucky caller can do around that path is what this module pins
//! down:
//!
//! * claiming with no proposal, with a proposal addressed to someone else, or
//!   as the sitting admin when the proposal names a third party;
//! * replaying a claim that already succeeded (the proposal must not be
//!   reusable and the new admin must not be disturbed);
//! * the expiry boundary — `expires_at` is inclusive, so a claim at exactly
//!   `expires_at` succeeds and one second later is swept and rejected;
//! * rejection ordering when a proposal is both expired *and* the caller is
//!   not the designated claimant;
//! * missing authorization (`require_auth` must trap, not fall through);
//! * the invariant that a rejected claim leaves `DataKey::Admin` and the
//!   pending proposal exactly as they were, and emits no claim event;
//! * two cross-entrypoint boundaries: a self-addressed proposal, and a
//!   pending proposal that outlives a single-step `rotate_admin`.

use crate::{
    types::AdminProposalClaimedEvent,
    Error, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    Address, Env, Symbol, TryFromVal, Val, Vec,
};

/// The proposal window hard-coded in `admin::PROPOSAL_WINDOW_SECS`.
const SEVEN_DAYS: u64 = 7 * 24 * 60 * 60;
/// Fixed starting ledger timestamp so `proposed_at`/`expires_at` are exact.
const START_TS: u64 = 1_000_000;

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(START_TS);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &SEVEN_DAYS);

    (env, client, admin)
}

fn advance_seconds(env: &Env, seconds: u64) {
    env.ledger().set_timestamp(env.ledger().timestamp() + seconds);
}

/// `soroban_sdk::Val` does not implement `PartialEq`, so an event topic is
/// compared by decoding the stored `Val` back into a `Symbol`.
fn topic_matches(env: &Env, topics: &Vec<Val>, topic: &Symbol) -> bool {
    match topics.get(0) {
        Some(val) => match Symbol::try_from_val(env, &val) {
            Ok(sym) => sym == *topic,
            Err(_) => false,
        },
        None => false,
    }
}

/// Return the payload of the first event whose first topic equals `topic`.
fn find_event_data(env: &Env, topic: &Symbol) -> Option<Val> {
    let all = env.events().all();
    for i in 0..all.len() {
        let (_, topics, data): (Address, Vec<Val>, Val) = all.get(i).unwrap();
        if topic_matches(env, &topics, topic) {
            return Some(data);
        }
    }
    None
}

/// Number of emitted events whose first topic equals `topic`.
fn event_count(env: &Env, topic: &Symbol) -> u32 {
    let all = env.events().all();
    let mut count = 0u32;
    for i in 0..all.len() {
        let (_, topics, _): (Address, Vec<Val>, Val) = all.get(i).unwrap();
        if topic_matches(env, &topics, topic) {
            count += 1;
        }
    }
    count
}

fn claimed_topic(env: &Env) -> Symbol {
    Symbol::new(env, "admin_proposal_claimed")
}

// ── No proposal ──────────────────────────────────────────────────────────────

#[test]
fn claim_without_any_proposal_is_rejected_and_leaves_admin_unchanged() {
    let (env, client, admin) = setup();
    let orphan = Address::generate(&env);

    let result = client.try_claim_admin_role(&orphan);
    assert_eq!(result, Err(Ok(Error::ProposalNotFound)));

    assert_eq!(client.get_admin(), admin);
    assert!(client.get_admin_proposal().is_none());
    assert_eq!(event_count(&env, &claimed_topic(&env)), 0);
}

// ── Wrong claimant ───────────────────────────────────────────────────────────

#[test]
fn wrong_claimant_is_rejected_and_proposal_is_preserved_for_designated_claimant() {
    let (env, client, admin) = setup();
    let designated = Address::generate(&env);
    let impostor = Address::generate(&env);

    client.propose_admin(&admin, &designated);

    let result = client.try_claim_admin_role(&impostor);
    assert_eq!(result, Err(Ok(Error::InvalidClaimant)));

    // Neither the admin nor the pending proposal is mutated by the rejection.
    assert_eq!(client.get_admin(), admin);
    let proposal = client
        .get_admin_proposal()
        .expect("rejected claim must not consume the proposal");
    assert_eq!(proposal.new_admin, designated);
    assert_eq!(proposal.proposed_at, START_TS);
    assert_eq!(proposal.expires_at, START_TS + SEVEN_DAYS);
    assert_eq!(event_count(&env, &claimed_topic(&env)), 0);

    // Because the proposal survived, the designated address can still claim.
    client.claim_admin_role(&designated);
    assert_eq!(client.get_admin(), designated);
    assert!(client.get_admin_proposal().is_none());
}

#[test]
fn sitting_admin_cannot_claim_a_proposal_addressed_to_someone_else() {
    let (env, client, admin) = setup();
    let designated = Address::generate(&env);

    client.propose_admin(&admin, &designated);

    // The current admin's auth is valid, but the claimant must be the proposed
    // address — being admin confers no right to short-circuit the handshake.
    let result = client.try_claim_admin_role(&admin);
    assert_eq!(result, Err(Ok(Error::InvalidClaimant)));

    assert_eq!(client.get_admin(), admin);
    assert!(client.get_admin_proposal().is_some());
    assert_eq!(event_count(&env, &claimed_topic(&env)), 0);
}

#[test]
fn repeated_rejected_claims_never_mutate_admin_or_consume_the_proposal() {
    let (env, client, admin) = setup();
    let designated = Address::generate(&env);
    client.propose_admin(&admin, &designated);

    for _ in 0..4 {
        let impostor = Address::generate(&env);
        let result = client.try_claim_admin_role(&impostor);
        assert_eq!(result, Err(Ok(Error::InvalidClaimant)));
        assert_eq!(client.get_admin(), admin);
    }

    assert!(client.get_admin_proposal().is_some());
    assert_eq!(event_count(&env, &claimed_topic(&env)), 0);

    client.claim_admin_role(&designated);
    assert_eq!(client.get_admin(), designated);
}

// ── Replay of a completed claim ──────────────────────────────────────────────

#[test]
fn replay_of_a_completed_claim_is_rejected_and_new_admin_stays_in_place() {
    let (env, client, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.claim_admin_role(&new_admin);
    assert_eq!(client.get_admin(), new_admin);
    assert!(client.get_admin_proposal().is_none());

    // A second claim by the same address must not resurrect the proposal nor
    // re-apply any rotation.
    let replay = client.try_claim_admin_role(&new_admin);
    assert_eq!(replay, Err(Ok(Error::ProposalNotFound)));
    assert_eq!(client.get_admin(), new_admin);
    assert!(client.get_admin_proposal().is_none());
    // Exactly one claim event for the one real claim.
    assert_eq!(event_count(&env, &claimed_topic(&env)), 1);
}

// ── Expiry boundary ──────────────────────────────────────────────────────────

#[test]
fn claim_at_exactly_the_deadline_is_accepted_boundary() {
    let (env, client, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    // `do_claim_admin_role` rejects only when `now > expires_at`, so the
    // deadline itself is inclusive.
    advance_seconds(&env, SEVEN_DAYS);

    client.claim_admin_role(&new_admin);
    assert_eq!(client.get_admin(), new_admin);
    assert!(client.get_admin_proposal().is_none());
}

#[test]
fn claim_one_second_past_the_deadline_is_rejected_and_proposal_is_swept() {
    let (env, client, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    advance_seconds(&env, SEVEN_DAYS + 1);

    let result = client.try_claim_admin_role(&new_admin);
    assert_eq!(result, Err(Ok(Error::ProposalExpired)));

    // The expiry path deliberately sweeps the stale proposal...
    assert!(client.get_admin_proposal().is_none());
    // ...but a rejected claim never rotates the admin.
    assert_eq!(client.get_admin(), admin);
    assert_eq!(event_count(&env, &claimed_topic(&env)), 0);
}

#[test]
fn expired_proposal_reports_expired_before_checking_the_claimant() {
    let (env, client, admin) = setup();
    let designated = Address::generate(&env);
    let impostor = Address::generate(&env);

    client.propose_admin(&admin, &designated);
    advance_seconds(&env, SEVEN_DAYS + 1);

    // Expiry is checked before the claimant identity, so even an impostor sees
    // `ProposalExpired` rather than `InvalidClaimant`.
    let result = client.try_claim_admin_role(&impostor);
    assert_eq!(result, Err(Ok(Error::ProposalExpired)));

    assert!(client.get_admin_proposal().is_none());
    assert_eq!(client.get_admin(), admin);
}

// ── Cancelled / competing paths ──────────────────────────────────────────────

#[test]
fn claim_after_the_admin_cancels_the_proposal_is_rejected_and_admin_unchanged() {
    let (env, client, admin) = setup();
    let designated = Address::generate(&env);

    client.propose_admin(&admin, &designated);
    client.cancel_admin_proposal(&admin);

    let result = client.try_claim_admin_role(&designated);
    assert_eq!(result, Err(Ok(Error::ProposalNotFound)));

    assert_eq!(client.get_admin(), admin);
    assert!(client.get_admin_proposal().is_none());
}

// ── Authorization ────────────────────────────────────────────────────────────

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn claim_without_authorization_traps_before_looking_at_the_proposal() {
    let (env, client, admin) = setup();
    let designated = Address::generate(&env);
    client.propose_admin(&admin, &designated);

    // Turn off the blanket auth mock: `claimant.require_auth()` must now trap,
    // even though a valid proposal exists for this exact address.
    env.mock_auths(&[]);

    let _ = client.claim_admin_role(&designated);
}

// ── State invariants on success ──────────────────────────────────────────────

#[test]
fn successful_claim_preserves_unrelated_admin_configuration() {
    let (env, client, admin) = setup();
    let new_admin = Address::generate(&env);

    client.set_min_topup(&admin, &2_500_000i128);
    client.propose_admin(&admin, &new_admin);
    client.claim_admin_role(&new_admin);

    assert_eq!(client.get_admin(), new_admin);
    // The claim rotates the admin only; other config keys are untouched.
    assert_eq!(client.get_min_topup(), 2_500_000i128);
}

#[test]
fn claimed_event_carries_old_and_new_admin_and_ledger_timestamp() {
    let (env, client, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    advance_seconds(&env, 3600);
    client.claim_admin_role(&new_admin);

    let payload = find_event_data(&env, &claimed_topic(&env))
        .expect("a successful claim must emit admin_proposal_claimed");
    let parsed = AdminProposalClaimedEvent::try_from_val(&env, &payload).unwrap();
    assert_eq!(parsed.old_admin, admin);
    assert_eq!(parsed.new_admin, new_admin);
    assert_eq!(parsed.timestamp, START_TS + 3600);
    assert_eq!(event_count(&env, &claimed_topic(&env)), 1);
}

// ── Cross-entrypoint boundaries ──────────────────────────────────────────────

#[test]
fn self_addressed_proposal_claims_as_a_noop_rotation_and_clears_the_proposal() {
    let (env, client, admin) = setup();

    // Unlike `rotate_admin`, which rejects a self-rotation with `SelfRotation`,
    // `propose_admin` has no such guard — so this boundary is reachable.
    client.propose_admin(&admin, &admin);
    assert_eq!(client.get_admin_proposal().unwrap().new_admin, admin);

    client.claim_admin_role(&admin);

    assert_eq!(client.get_admin(), admin);
    assert!(client.get_admin_proposal().is_none());
}

#[test]
fn pending_proposal_outlives_a_single_step_rotation_and_can_retake_admin() {
    let (env, client, admin) = setup();
    let proposed = Address::generate(&env);
    let interim = Address::generate(&env);

    client.propose_admin(&admin, &proposed);

    // A single-step `rotate_admin` to a *different* address does not clear the
    // pending two-step proposal.
    client.rotate_admin(&admin, &interim, &0u64);
    assert_eq!(client.get_admin(), interim);
    assert!(client.get_admin_proposal().is_some());

    // Consequently the still-pending claimant can complete the claim and take
    // admin from the interim admin. This test documents the current
    // cross-entrypoint behaviour (see the PR notes) so that any future change
    // to invalidation semantics is deliberate and visible.
    client.claim_admin_role(&proposed);
    assert_eq!(client.get_admin(), proposed);
    assert!(client.get_admin_proposal().is_none());
}
