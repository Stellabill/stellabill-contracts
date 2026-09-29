//! Adversarial coverage for the `get_admin_proposal` view (issue #1103).
//!
//! `get_admin_proposal` is the read side of the two-step admin rotation flow
//! (`propose_admin` -> `claim_admin_role`, with `cancel_admin_proposal` as the
//! escape hatch). It is a pure instance-storage read, so the properties that
//! matter adversarially are:
//!
//! - It never requires authorization and never panics on empty storage.
//! - It is a *read*: repeated calls return byte-identical values and never
//!   clear, shorten or otherwise mutate the proposal.
//! - Rejected state-changing operations (`propose_admin`, `claim_admin_role`,
//!   `cancel_admin_proposal`) leave the observable proposal untouched.
//! - Expiry is enforced by the *claim* path, not by the getter: an expired but
//!   unclaimed proposal stays readable, and even a failed claim leaves it in
//!   place because Soroban rolls back the storage writes of an erroring frame.
//!   `cancel_admin_proposal` is therefore the only cleanup path, and a stale
//!   proposal keeps rejecting new ones until it is used.
//! - The window boundary (`now == expires_at`) is inclusive for claims, and the
//!   window arithmetic saturates instead of overflowing.
//!
//! Every test is deterministic: the ledger timestamp is pinned during setup and
//! only advanced explicitly.

#![cfg(test)]

use crate::{Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env,
};

/// Mirror of `admin::PROPOSAL_WINDOW_SECS` (7 days).
const WINDOW: u64 = 7 * 24 * 60 * 60;

/// Per-config-key admin cooldown (`admin::CONFIG_COOLDOWN_SECS`), needed to
/// perform two mutations of the same config key in one test.
const CONFIG_COOLDOWN: u64 = crate::admin::CONFIG_COOLDOWN_SECS;

/// Pinned starting ledger timestamp.
const T0: u64 = 1_000_000;

/// `(new_admin, proposed_at, expires_at)` snapshot of the active proposal.
type ProposalSnapshot = Option<(Address, u64, u64)>;

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, client, admin)
}

fn advance(env: &Env, seconds: u64) {
    let now = env.ledger().timestamp();
    env.ledger().set_timestamp(now + seconds);
}

fn snapshot(client: &SubscriptionVaultClient) -> ProposalSnapshot {
    client
        .get_admin_proposal()
        .map(|p| (p.new_admin, p.proposed_at, p.expires_at))
}

// ── §1 Valid calls ───────────────────────────────────────────────────────────

#[test]
fn test_get_admin_proposal_none_before_any_proposal() {
    let (_env, client, _admin) = setup();

    assert!(
        client.get_admin_proposal().is_none(),
        "freshly initialised contract must not expose a proposal"
    );
}

#[test]
fn test_get_admin_proposal_on_uninitialised_contract_is_none() {
    // Boundary: reading instance storage that was never written must return
    // `None` rather than panicking.
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    assert!(client.get_admin_proposal().is_none());
}

#[test]
fn test_get_admin_proposal_returns_exact_proposal_fields() {
    let (env, client, admin) = setup();
    let candidate = Address::generate(&env);

    client.propose_admin(&admin, &candidate);

    let p = client
        .get_admin_proposal()
        .expect("proposal must be readable after propose_admin");
    assert_eq!(p.new_admin, candidate);
    assert_eq!(p.proposed_at, T0);
    assert_eq!(p.expires_at, T0 + WINDOW);
}

#[test]
fn test_get_admin_proposal_is_a_pure_read() {
    let (env, client, admin) = setup();
    let candidate = Address::generate(&env);
    client.propose_admin(&admin, &candidate);

    let first = snapshot(&client);
    assert_eq!(first, Some((candidate.clone(), T0, T0 + WINDOW)));

    // Repeated reads are stable — the getter consumes nothing.
    for _ in 0..3 {
        assert_eq!(snapshot(&client), first);
    }

    // Advancing inside the window must not shorten or reset the proposal.
    advance(&env, WINDOW / 2);
    assert_eq!(snapshot(&client), first);

    // The active admin is untouched by any number of reads.
    assert_eq!(client.get_admin(), admin);
}

// ── §2 Authorization boundaries ──────────────────────────────────────────────

#[test]
fn test_get_admin_proposal_requires_no_authorization() {
    let (env, client, admin) = setup();
    let candidate = Address::generate(&env);
    client.propose_admin(&admin, &candidate);

    // Drop every mocked authorization: the view must still be callable.
    env.mock_auths(&[]);
    assert!(
        client.get_admin_proposal().is_some(),
        "read-only getter must not require authorization"
    );

    // …while the state-changing entrypoint on the same storage key is rejected
    // and leaves the proposal in place.
    assert!(
        client.try_cancel_admin_proposal(&admin).is_err(),
        "cancel without auth must be rejected"
    );
    env.mock_all_auths();
    assert_eq!(
        snapshot(&client),
        Some((candidate, T0, T0 + WINDOW)),
        "rejected cancel must not clear the proposal"
    );
}

// ── §3 State is unchanged after rejected operations ──────────────────────────

#[test]
fn test_get_admin_proposal_stays_none_when_propose_is_rejected() {
    let (env, client, admin) = setup();
    let stranger = Address::generate(&env);
    let candidate = Address::generate(&env);

    // Unauthorized proposer is rejected and leaves no residue.
    assert_eq!(
        client.try_propose_admin(&stranger, &candidate),
        Err(Ok(Error::Unauthorized))
    );
    assert!(client.get_admin_proposal().is_none());

    // Invalid target (the contract itself) is rejected and leaves no residue.
    assert_eq!(
        client.try_propose_admin(&admin, &client.address),
        Err(Ok(Error::InvalidNewAdmin))
    );
    assert!(client.get_admin_proposal().is_none());

    assert_eq!(client.get_admin(), admin);
}

#[test]
fn test_get_admin_proposal_unchanged_by_rejected_second_propose() {
    let (env, client, admin) = setup();
    let first = Address::generate(&env);
    let second = Address::generate(&env);
    client.propose_admin(&admin, &first);

    assert_eq!(
        client.try_propose_admin(&admin, &second),
        Err(Ok(Error::ProposalAlreadyExists))
    );

    assert_eq!(
        snapshot(&client),
        Some((first, T0, T0 + WINDOW)),
        "a rejected re-proposal must leave the original proposal byte-identical"
    );
}

#[test]
fn test_get_admin_proposal_unchanged_by_rejected_claim() {
    let (env, client, admin) = setup();
    let candidate = Address::generate(&env);
    client.propose_admin(&admin, &candidate);

    let impostor = Address::generate(&env);
    assert_eq!(
        client.try_claim_admin_role(&impostor),
        Err(Ok(Error::InvalidClaimant))
    );

    assert_eq!(
        snapshot(&client),
        Some((candidate, T0, T0 + WINDOW)),
        "a rejected claim must not consume the proposal"
    );
    assert_eq!(client.get_admin(), admin);
}

#[test]
fn test_get_admin_proposal_unchanged_by_rejected_cancel() {
    let (env, client, admin) = setup();
    let candidate = Address::generate(&env);
    client.propose_admin(&admin, &candidate);

    let stranger = Address::generate(&env);
    assert_eq!(
        client.try_cancel_admin_proposal(&stranger),
        Err(Ok(Error::Unauthorized))
    );

    assert_eq!(
        snapshot(&client),
        Some((candidate, T0, T0 + WINDOW)),
        "a rejected cancel must not clear the proposal"
    );
    assert_eq!(client.get_admin(), admin);
}

// ── §4 Clearing paths ────────────────────────────────────────────────────────

#[test]
fn test_get_admin_proposal_cleared_by_successful_claim() {
    let (env, client, admin) = setup();
    let candidate = Address::generate(&env);

    client.propose_admin(&admin, &candidate);
    client.claim_admin_role(&candidate);

    assert!(
        client.get_admin_proposal().is_none(),
        "a claimed proposal must not remain readable"
    );
    assert_eq!(client.get_admin(), candidate);

    // The window is consumed: a second claim finds nothing.
    assert_eq!(
        client.try_claim_admin_role(&candidate),
        Err(Ok(Error::ProposalNotFound))
    );
    assert!(client.get_admin_proposal().is_none());
    assert_eq!(client.get_admin(), candidate);
}

#[test]
fn test_get_admin_proposal_cleared_by_cancel_and_absent_afterwards() {
    let (env, client, admin) = setup();
    let candidate = Address::generate(&env);

    // Cancelling with no proposal is rejected and reports nothing pending.
    assert_eq!(
        client.try_cancel_admin_proposal(&admin),
        Err(Ok(Error::NoActiveProposal))
    );
    assert!(client.get_admin_proposal().is_none());

    client.propose_admin(&admin, &candidate);
    client.cancel_admin_proposal(&admin);

    assert!(client.get_admin_proposal().is_none());
    assert_eq!(client.get_admin(), admin);

    // After cancellation the proposal cannot be cancelled again, nor claimed.
    assert_eq!(
        client.try_cancel_admin_proposal(&admin),
        Err(Ok(Error::NoActiveProposal))
    );
    assert_eq!(
        client.try_claim_admin_role(&candidate),
        Err(Ok(Error::ProposalNotFound))
    );
    assert!(client.get_admin_proposal().is_none());
}

// ── §5 Expiry boundaries ─────────────────────────────────────────────────────

#[test]
fn test_get_admin_proposal_visible_at_exact_expiry_boundary() {
    let (env, client, admin) = setup();
    let candidate = Address::generate(&env);
    client.propose_admin(&admin, &candidate);

    // Landing exactly on `expires_at` must keep the proposal alive: the window
    // is inclusive, expiry only triggers once `now > expires_at`.
    advance(&env, WINDOW);
    let p = client
        .get_admin_proposal()
        .expect("proposal must remain readable at the exact expiry boundary");
    assert_eq!(p.expires_at, env.ledger().timestamp());

    client.claim_admin_role(&candidate);
    assert_eq!(client.get_admin(), candidate);
    assert!(client.get_admin_proposal().is_none());
}

#[test]
fn test_get_admin_proposal_still_readable_after_window_expires() {
    let (env, client, admin) = setup();
    let candidate = Address::generate(&env);
    client.propose_admin(&admin, &candidate);

    advance(&env, WINDOW + 1);

    // The getter is a pure read: it exposes the expired proposal and performs
    // no cleanup of its own.
    let p = client
        .get_admin_proposal()
        .expect("an expired but unclaimed proposal stays readable");
    assert_eq!(p.proposed_at + WINDOW, p.expires_at);
    assert!(p.expires_at < env.ledger().timestamp());

    // The claim path is what enforces expiry: it returns `ProposalExpired`.
    assert_eq!(
        client.try_claim_admin_role(&candidate),
        Err(Ok(Error::ProposalExpired))
    );

    // That same path also removes the stale proposal before returning the error,
    // but Soroban rolls back every storage write of an erroring frame — so the
    // removal never survives and the expired proposal is still observable.
    // Nothing garbage-collects it on read.
    assert_eq!(
        snapshot(&client),
        Some((candidate.clone(), T0, T0 + WINDOW)),
        "a reverted claim must leave the expired proposal byte-identical"
    );
    assert_eq!(client.get_admin(), admin);

    // Consequence worth pinning down: the stale proposal keeps blocking new
    // proposals even though it can never be claimed.
    let next_candidate = Address::generate(&env);
    assert_eq!(
        client.try_propose_admin(&admin, &next_candidate),
        Err(Ok(Error::ProposalAlreadyExists))
    );

    // `cancel_admin_proposal` does not inspect expiry, so it is the only escape
    // hatch; once used, a fresh proposal is accepted.
    client.cancel_admin_proposal(&admin);
    assert!(client.get_admin_proposal().is_none());

    client.propose_admin(&admin, &next_candidate);
    let now = T0 + WINDOW + 1;
    assert_eq!(
        snapshot(&client),
        Some((next_candidate.clone(), now, now + WINDOW))
    );
}

#[test]
fn test_get_admin_proposal_window_saturates_at_u64_max() {
    let (env, client, admin) = setup();
    let candidate = Address::generate(&env);

    env.ledger().set_timestamp(u64::MAX - 1);
    client.propose_admin(&admin, &candidate);

    let p = client
        .get_admin_proposal()
        .expect("proposal at the extreme timestamp must be readable");
    assert_eq!(p.proposed_at, u64::MAX - 1);
    assert_eq!(
        p.expires_at,
        u64::MAX,
        "window arithmetic must saturate instead of overflowing"
    );

    // `now` is not greater than `expires_at`, so the proposal is still claimable.
    client.claim_admin_role(&candidate);
    assert_eq!(client.get_admin(), candidate);
    assert!(client.get_admin_proposal().is_none());
}

// ── §6 Freshness / isolation ─────────────────────────────────────────────────

#[test]
fn test_get_admin_proposal_reflects_fresh_state_after_rotation() {
    let (env, client, admin) = setup();
    let admin2 = Address::generate(&env);
    let admin3 = Address::generate(&env);

    client.propose_admin(&admin, &admin2);
    client.claim_admin_role(&admin2);
    assert!(client.get_admin_proposal().is_none());

    advance(&env, 1_000);
    client.propose_admin(&admin2, &admin3);

    let p = client
        .get_admin_proposal()
        .expect("new admin must be able to open a fresh proposal");
    assert_eq!(p.new_admin, admin3);
    assert_eq!(
        p.proposed_at,
        T0 + 1_000,
        "timestamps must come from the new proposal, not the claimed one"
    );
    assert_eq!(p.expires_at, T0 + 1_000 + WINDOW);
}

#[test]
fn test_get_admin_proposal_unchanged_by_unrelated_privileged_ops() {
    let (env, client, admin) = setup();
    let candidate = Address::generate(&env);
    client.propose_admin(&admin, &candidate);
    let before = snapshot(&client);

    // Unrelated admin operations must not disturb the pending proposal.
    client.set_min_topup(&admin, &2_000_000i128);
    client.enable_emergency_stop(&admin);
    assert_eq!(snapshot(&client), before);

    // Flipping the circuit breaker back off is rate-limited by the per-config
    // cooldown, so the clock has to move past it first — another
    // proposal-agnostic mutation that must not touch the proposal either.
    advance(&env, CONFIG_COOLDOWN);
    client.disable_emergency_stop(&admin);

    assert_eq!(snapshot(&client), before);
    assert_eq!(client.get_admin(), admin);
}
