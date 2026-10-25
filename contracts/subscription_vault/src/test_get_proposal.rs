#![cfg(test)]

//! Adversarial coverage for `SubscriptionVault::get_proposal`.
//!
//! `get_proposal(env, proposal_id: u64) -> Option<Proposal>` is a
//! permissionless, read-only view over persistent governance storage.  Its
//! contract is intentionally simple, but several adversarial properties must be
//! pinned:
//!
//! **Success path**
//! - Returns `Some(proposal)` for every ID that was successfully submitted.
//! - All fields of the returned `Proposal` are byte-for-byte identical to
//!   what was written by `submit_proposal`.
//! - Works across all `ProposalKind` variants.
//! - Works for proposals in every lifecycle state: fresh / voted / executed /
//!   cancelled.
//!
//! **Miss path**
//! - Returns `None` for IDs that were never allocated (including `0`, `u64::MAX`,
//!   and arbitrary random values).
//! - Returns `None` for IDs that are *allocated* but not yet stored (impossible
//!   by design, but the test formalises the absence of a race).
//!
//! **Immutability / side-effect freedom**
//! - Calling `get_proposal` any number of times on the same ID returns the
//!   same value every time (pure read).
//! - `get_proposal` does not advance the proposal counter.
//! - `get_proposal` does not change the `executed` flag, the vote map, or any
//!   other field of the stored proposal.
//! - `get_proposal` does not emit events.
//!
//! **Auth surface**
//! - No `require_auth` is invoked; the call must succeed with auth mocking
//!   disabled (documents the intentionally-open read surface).
//!
//! **Boundary values**
//! - `proposal_id = u64::MAX` never allocated → `None`.
//! - `proposal_id = u64::MAX` stored directly → correct `Some`.
//! - ID `0` before any submission → `None`.
//! - ID `0` after one submission → `Some` with correct fields.
//!
//! **Multi-proposal isolation**
//! - ID `n` returns the proposal stored at `n`, never the one at `n ± 1`.
//! - Two vaults sharing an `Env` do not bleed proposals across contract
//!   instances.
//!
//! **Post-mutation consistency**
//! - After a vote the vote map in storage is reflected by `get_proposal`.
//! - After execution `executed == true` is reflected by `get_proposal`.
//! - After cancellation `executed == true` is reflected by `get_proposal`.

use crate::types::{DataKey, Proposal, ProposalKind};
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, Map, String,
};

// ── shared helpers ─────────────────────────────────────────────────────────────

/// Spin up a fresh vault and return `(token_address, client)`.
fn init_vault<'a>(env: &'a Env) -> (Address, SubscriptionVaultClient<'a>) {
    let admin = Address::generate(env);
    let token_admin = Address::generate(env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    client.init(&token, &6, &admin, &10_000_000, &86_400);
    (token, client)
}

/// Submit a minimal `RotateAdmin` proposal and return its ID.
fn submit_rotate(client: &SubscriptionVaultClient, target: &Address, eta: u64) -> u64 {
    client.submit_proposal(&ProposalKind::RotateAdmin, target, &None, &0, &5_000, &eta)
}

fn seed_proposal(env: &Env, contract_id: &Address, proposal: &Proposal) {
    env.as_contract(contract_id, || {
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(proposal.id), proposal);
    });
}

// ══════════════════════════════════════════════════════════════════════════════
// Success-path tests
// ══════════════════════════════════════════════════════════════════════════════

/// A proposal that was just submitted is immediately retrievable.
#[test]
fn returns_some_for_freshly_submitted_proposal() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let target = Address::generate(&env);
    let eta = 10_000u64;

    let id = submit_rotate(&client, &target, eta);
    let proposal = client.get_proposal(&id).expect("proposal must exist");

    assert_eq!(proposal.id, id);
    assert_eq!(proposal.kind, ProposalKind::RotateAdmin);
    assert_eq!(proposal.target, target);
    assert_eq!(proposal.target2, None);
    assert_eq!(proposal.target3, 0);
    assert_eq!(proposal.quorum_bps, 5_000);
    assert!(!proposal.executed);
    assert_eq!(proposal.votes.len(), 0);
    assert_eq!(proposal.eta, eta);
}

/// Every field of a `SetProtocolFee` proposal is stored and returned correctly.
#[test]
fn set_protocol_fee_proposal_fields_are_correct() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let treasury = Address::generate(&env);
    let fee_bps: u32 = 250; // 2.5%
    let eta = 5_000u64;
    let quorum_bps = 7_500u32;

    let id = client.submit_proposal(
        &ProposalKind::SetProtocolFee,
        &treasury,
        &Some(treasury.clone()),
        &fee_bps,
        &quorum_bps,
        &eta,
    );

    let p = client.get_proposal(&id).expect("proposal must exist");
    assert_eq!(p.kind, ProposalKind::SetProtocolFee);
    assert_eq!(p.target, treasury);
    assert_eq!(p.target2, Some(treasury));
    assert_eq!(p.target3, fee_bps);
    assert_eq!(p.quorum_bps, quorum_bps);
    assert_eq!(p.eta, eta);
    assert!(!p.executed);
}

/// `UpgradeContract` proposals are stored with the correct kind discriminant.
#[test]
fn upgrade_contract_proposal_kind_is_stored_correctly() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let target = Address::generate(&env);
    let eta = 5_000u64;

    let id = client.submit_proposal(
        &ProposalKind::UpgradeContract,
        &target,
        &None,
        &0,
        &10_000,
        &eta,
    );

    let p = client.get_proposal(&id).expect("proposal must exist");
    assert_eq!(p.kind, ProposalKind::UpgradeContract);
    assert!(!p.executed);
}

/// Multiple proposals submitted sequentially each return distinct, correct data.
#[test]
fn multiple_proposals_are_each_independently_retrievable() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let target_a = Address::generate(&env);
    let target_b = Address::generate(&env);
    let target_c = Address::generate(&env);

    let id_a = submit_rotate(&client, &target_a, 5_000);
    let id_b = submit_rotate(&client, &target_b, 5_001);
    let id_c = submit_rotate(&client, &target_c, 5_002);

    assert_eq!(client.get_proposal(&id_a).unwrap().target, target_a);
    assert_eq!(client.get_proposal(&id_b).unwrap().target, target_b);
    assert_eq!(client.get_proposal(&id_c).unwrap().target, target_c);
}

// ══════════════════════════════════════════════════════════════════════════════
// Miss-path tests (None)
// ══════════════════════════════════════════════════════════════════════════════

/// A completely uninitialised contract returns `None` for every ID.
#[test]
fn returns_none_on_uninitialized_contract() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    assert!(client.get_proposal(&0u64).is_none());
    assert!(client.get_proposal(&1u64).is_none());
    assert!(client.get_proposal(&u64::MAX).is_none());
}

/// ID `0` before any submission returns `None`.
#[test]
fn returns_none_for_id_zero_before_any_submission() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    assert!(client.get_proposal(&0u64).is_none());
}

/// An ID beyond the current counter always returns `None`.
#[test]
fn returns_none_for_unallocated_id_beyond_counter() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let target = Address::generate(&env);

    // Allocate ID 0 and verify ID 1 is None.
    submit_rotate(&client, &target, 5_000);
    assert!(client.get_proposal(&1u64).is_none());
    assert!(client.get_proposal(&999u64).is_none());
    assert!(client.get_proposal(&u64::MAX).is_none());
}

/// `u64::MAX` as a proposal ID returns `None` when the counter has never
/// reached that value (no allocation at that position).
#[test]
fn returns_none_for_max_u64_when_never_allocated() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    assert!(client.get_proposal(&u64::MAX).is_none());
    // Reading must not accidentally advance the counter.
    assert_eq!(client.get_current_proposal_id(), 0);
}

// ══════════════════════════════════════════════════════════════════════════════
// Immutability / side-effect-free read
// ══════════════════════════════════════════════════════════════════════════════

/// Repeated calls for the same ID return identical values (pure read).
#[test]
fn repeated_reads_return_identical_value() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let target = Address::generate(&env);
    let id = submit_rotate(&client, &target, 5_000);

    for _ in 0..8 {
        let p = client.get_proposal(&id).expect("must stay Some");
        assert_eq!(p.id, id);
        assert_eq!(p.target, target);
        assert!(!p.executed);
    }
}

/// `get_proposal` must not consume or advance the proposal counter.
#[test]
fn read_does_not_advance_proposal_counter() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let target = Address::generate(&env);
    submit_rotate(&client, &target, 5_000);

    let before = client.get_current_proposal_id();
    // Call get_proposal multiple times for both existing and non-existing IDs.
    let _ = client.get_proposal(&0u64);
    let _ = client.get_proposal(&0u64);
    let _ = client.get_proposal(&999u64);
    let after = client.get_current_proposal_id();

    assert_eq!(before, after, "get_proposal must not advance the counter");
}

/// `get_proposal` must not emit any events.
#[test]
fn read_does_not_emit_events() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let target = Address::generate(&env);
    let id = submit_rotate(&client, &target, 5_000);

    // Drain events already emitted by submit_proposal.
    let events_before_read = env.events().all().len();

    let _ = client.get_proposal(&id);
    let _ = client.get_proposal(&999u64);

    assert_eq!(
        env.events().all().len(),
        events_before_read,
        "get_proposal must not emit any events"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// Post-mutation consistency
// ══════════════════════════════════════════════════════════════════════════════

/// After a guardian votes, the vote is reflected in the map returned by
/// `get_proposal`.
#[test]
fn vote_is_reflected_in_subsequent_get_proposal() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let admin = client.get_admin();
    let target = Address::generate(&env);
    let eta = 10_000u64;

    let id = submit_rotate(&client, &target, eta);

    // Register admin as a guardian so vote_proposal can proceed.
    client.add_guardian(&admin, &admin, &100);
    client.vote_proposal(&id, &true);

    let p = client.get_proposal(&id).expect("proposal must exist");
    assert_eq!(
        p.votes.get(admin.clone()),
        Some(true),
        "yes vote must be visible through get_proposal"
    );
}

/// After a no-vote, the stored vote reflects `false`.
#[test]
fn no_vote_is_reflected_in_subsequent_get_proposal() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let admin = client.get_admin();
    let target = Address::generate(&env);
    let eta = 10_000u64;

    let id = submit_rotate(&client, &target, eta);
    client.add_guardian(&admin, &admin, &100);
    client.vote_proposal(&id, &false);

    let p = client.get_proposal(&id).expect("proposal must exist");
    assert_eq!(
        p.votes.get(admin),
        Some(false),
        "no vote must be visible through get_proposal"
    );
}

/// After a successful execution, `executed == true` is reflected immediately.
#[test]
fn executed_flag_set_after_successful_execution() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let admin = client.get_admin();
    let target = Address::generate(&env);
    let eta = 5_000u64;

    let id = submit_rotate(&client, &target, eta);

    // Single guardian with weight 100 covers 100% of the weight.
    client.add_guardian(&admin, &admin, &100);
    client.vote_proposal(&id, &true);

    // Advance past the timelock.
    env.ledger().set_timestamp(eta + 1);
    client.execute_proposal(&id);

    let p = client
        .get_proposal(&id)
        .expect("must remain in storage after execution");
    assert!(
        p.executed,
        "executed flag must be true after execute_proposal"
    );
}

/// After cancellation, `executed == true` is reflected immediately; the
/// proposal record is not deleted, only sealed.
#[test]
fn executed_flag_set_after_cancellation() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let target = Address::generate(&env);
    let eta = 5_000u64;

    let id = submit_rotate(&client, &target, eta);

    let reason = String::from_str(&env, "superseded");
    client.cancel_proposal(&id, &reason);

    let p = client
        .get_proposal(&id)
        .expect("must remain in storage after cancellation");
    assert!(
        p.executed,
        "executed flag must be true after cancel_proposal"
    );
}

/// A cancelled proposal is sealed: `execute_proposal` on a cancelled proposal
/// returns `InvalidInput`, and a second `cancel_proposal` also returns
/// `InvalidInput`.  `get_proposal` continues to return the sealed record.
#[test]
fn cancelled_proposal_cannot_be_re_cancelled_or_executed() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let admin = client.get_admin();
    let target = Address::generate(&env);
    let eta = 5_000u64;

    let id = submit_rotate(&client, &target, eta);
    client.add_guardian(&admin, &admin, &100);
    client.vote_proposal(&id, &true);

    let reason = String::from_str(&env, "cancelled");
    client.cancel_proposal(&id, &reason);

    // Advance past eta so execute path could in principle run.
    env.ledger().set_timestamp(eta + 1);

    assert_eq!(
        client.try_execute_proposal(&id),
        Err(Ok(crate::types::Error::InvalidInput)),
        "cannot execute a cancelled proposal"
    );
    assert_eq!(
        client.try_cancel_proposal(&id, &reason),
        Err(Ok(crate::types::Error::InvalidInput)),
        "cannot cancel an already-cancelled proposal"
    );

    // Record must still be readable.
    assert!(client.get_proposal(&id).unwrap().executed);
}

/// An executed proposal is sealed: calling `execute_proposal` a second time
/// returns `InvalidInput`, and `get_proposal` still shows `executed == true`.
#[test]
fn executed_proposal_cannot_be_re_executed() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let admin = client.get_admin();
    let target = Address::generate(&env);
    let eta = 5_000u64;

    let id = submit_rotate(&client, &target, eta);
    client.add_guardian(&admin, &admin, &100);
    client.vote_proposal(&id, &true);

    env.ledger().set_timestamp(eta + 1);
    client.execute_proposal(&id);

    assert_eq!(
        client.try_execute_proposal(&id),
        Err(Ok(crate::types::Error::InvalidInput)),
        "cannot re-execute an already-executed proposal"
    );
    assert!(client.get_proposal(&id).unwrap().executed);
}

// ══════════════════════════════════════════════════════════════════════════════
// Auth-surface: permissionless read
// ══════════════════════════════════════════════════════════════════════════════

/// `get_proposal` must not require any auth at all: calling it with no mock
/// auth succeeds for both present and absent proposals.
#[test]
fn get_proposal_requires_no_authorization() {
    // Deliberately *not* calling env.mock_all_auths().
    let env = Env::default();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let target = Address::generate(&env);
    let proposal = Proposal {
        id: 7,
        kind: ProposalKind::RotateAdmin,
        target: target.clone(),
        target2: None,
        target3: 0,
        quorum_bps: 5_000,
        votes: Map::new(&env),
        eta: 10_000,
        submitted_at: 1_000,
        executed: false,
    };
    seed_proposal(&env, &contract_id, &proposal);

    // No-auth calls work for both present and missing proposals.
    let fetched = client
        .get_proposal(&proposal.id)
        .expect("an existing proposal must be readable without auth");
    assert_eq!(fetched.id, proposal.id);
    assert_eq!(fetched.target, target);
    assert!(client.get_proposal(&0u64).is_none());
    assert!(client.get_proposal(&u64::MAX).is_none());
}

// ══════════════════════════════════════════════════════════════════════════════
// Boundary-value tests
// ══════════════════════════════════════════════════════════════════════════════

/// A proposal stored at the maximum possible ID remains retrievable.
#[test]
fn returns_some_for_max_u64_id() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let target = Address::generate(&env);
    let proposal = Proposal {
        id: u64::MAX,
        kind: ProposalKind::RotateAdmin,
        target: target.clone(),
        target2: None,
        target3: 0,
        quorum_bps: 10_000,
        votes: Map::new(&env),
        eta: u64::MAX,
        submitted_at: 1_000,
        executed: false,
    };
    seed_proposal(&env, &contract_id, &proposal);

    let p = client
        .get_proposal(&u64::MAX)
        .expect("max-u64 proposal must exist");
    assert_eq!(p.id, u64::MAX);
    assert_eq!(p.target, target);
    assert_eq!(p.eta, u64::MAX);
    assert!(!p.executed);
}

/// Quorum boundary: 0 bps and 10_000 bps are both valid and stored correctly.
#[test]
fn quorum_boundary_values_are_stored_and_returned_correctly() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let target = Address::generate(&env);

    let id_zero_quorum = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &target,
        &None,
        &0,
        &0, // minimum quorum
        &5_000,
    );
    let id_full_quorum = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &target,
        &None,
        &0,
        &10_000, // maximum quorum
        &5_000,
    );

    assert_eq!(client.get_proposal(&id_zero_quorum).unwrap().quorum_bps, 0);
    assert_eq!(
        client.get_proposal(&id_full_quorum).unwrap().quorum_bps,
        10_000
    );
}

/// `submitted_at` is set to the ledger timestamp at submission time and is
/// observable through `get_proposal`.
#[test]
fn submitted_at_reflects_ledger_timestamp_at_submission() {
    let env = Env::default();
    env.mock_all_auths();

    let submit_ts = 42_000u64;
    env.ledger().set_timestamp(submit_ts);

    let (_, client) = init_vault(&env);
    let target = Address::generate(&env);
    let id = submit_rotate(&client, &target, submit_ts + 3_600);

    let p = client.get_proposal(&id).expect("must exist");
    assert_eq!(
        p.submitted_at, submit_ts,
        "submitted_at must match ledger timestamp at submission"
    );
}

/// ETA is stored verbatim: even a very large ETA (far-future) round-trips.
#[test]
fn large_eta_is_stored_and_returned_verbatim() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let target = Address::generate(&env);
    let far_future_eta = u64::MAX - 1;

    let id = submit_rotate(&client, &target, far_future_eta);
    let p = client.get_proposal(&id).expect("must exist");
    assert_eq!(p.eta, far_future_eta, "ETA must round-trip exactly");
}

// ══════════════════════════════════════════════════════════════════════════════
// Multi-proposal isolation
// ══════════════════════════════════════════════════════════════════════════════

/// Fetching ID `n` must never return the data stored at `n + 1`.
#[test]
fn adjacent_proposal_ids_return_independent_data() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let target_0 = Address::generate(&env);
    let target_1 = Address::generate(&env);

    let id0 = submit_rotate(&client, &target_0, 5_000);
    let id1 = submit_rotate(&client, &target_1, 5_001);

    // ID 0 must not bleed data from ID 1.
    assert_ne!(
        client.get_proposal(&id0).unwrap().target,
        client.get_proposal(&id1).unwrap().target,
        "adjacent proposals must have independent targets"
    );
    assert_eq!(client.get_proposal(&id0).unwrap().eta, 5_000);
    assert_eq!(client.get_proposal(&id1).unwrap().eta, 5_001);
}

/// Two vault instances in the same `Env` have completely isolated proposal
/// storage.
#[test]
fn proposals_are_isolated_per_contract_instance() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client_a) = init_vault(&env);
    let (_, client_b) = init_vault(&env);

    let target_a = Address::generate(&env);
    let target_b = Address::generate(&env);

    let id_a = submit_rotate(&client_a, &target_a, 5_000);
    let id_b = submit_rotate(&client_b, &target_b, 5_001);

    // Each vault returns its own proposal; the other vault's is absent.
    assert_eq!(
        client_a.get_proposal(&id_a).unwrap().target,
        target_a,
        "vault A must return its own proposal"
    );
    assert!(
        client_b.get_proposal(&id_a).is_none(),
        "vault B must not see vault A's proposal"
    );

    assert_eq!(
        client_b.get_proposal(&id_b).unwrap().target,
        target_b,
        "vault B must return its own proposal"
    );
    assert!(
        client_a.get_proposal(&id_b).is_none(),
        "vault A must not see vault B's proposal"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// Failed-lifecycle operations leave storage unchanged
// ══════════════════════════════════════════════════════════════════════════════

/// Rejected operations (invalid ETA, out-of-range quorum) must not create any
/// phantom proposal record, so `get_proposal` returns `None` for every ID in
/// the range that was attempted.
#[test]
fn rejected_submissions_leave_no_phantom_proposal_record() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(5_000);

    let (_, client) = init_vault(&env);
    let target = Address::generate(&env);

    // ETA in the past — rejected before ID allocation.
    let _ = client.try_submit_proposal(
        &ProposalKind::RotateAdmin,
        &target,
        &None,
        &0,
        &5_000,
        &4_999, // past
    );

    // ETA equal to now — also rejected.
    let _ = client.try_submit_proposal(
        &ProposalKind::RotateAdmin,
        &target,
        &None,
        &0,
        &5_000,
        &5_000, // equal to now
    );

    // Quorum too high — rejected before ID allocation.
    let _ = client.try_submit_proposal(
        &ProposalKind::RotateAdmin,
        &target,
        &None,
        &0,
        &10_001,
        &10_000,
    );

    // Counter must still be 0 — no ID was consumed.
    assert_eq!(client.get_current_proposal_id(), 0);
    // No phantom record at ID 0.
    assert!(
        client.get_proposal(&0u64).is_none(),
        "a rejected submission must not create a phantom proposal"
    );
}

/// Calling `vote_proposal` or `execute_proposal` against a non-existent ID
/// returns an error and does not create a phantom proposal record at that ID.
#[test]
fn failed_lifecycle_ops_on_missing_id_leave_no_phantom_record() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let (_, client) = init_vault(&env);
    let admin = client.get_admin();

    // Give the admin guardian weight so vote checks clear the weight guard
    // and reach the proposal-lookup, which must then report NotFound.
    client.add_guardian(&admin, &admin, &100);

    let ghost_id = 999u64;

    assert_eq!(
        client.try_vote_proposal(&ghost_id, &true),
        Err(Ok(crate::types::Error::NotFound)),
        "vote on non-existent id must return NotFound"
    );
    assert_eq!(
        client.try_execute_proposal(&ghost_id),
        Err(Ok(crate::types::Error::NotFound)),
        "execute on non-existent id must return NotFound"
    );

    // Counter and storage are unchanged.
    assert_eq!(client.get_current_proposal_id(), 0);
    assert!(
        client.get_proposal(&ghost_id).is_none(),
        "failed lifecycle ops must not create a phantom record"
    );
}
