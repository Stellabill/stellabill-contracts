//! Adversarial coverage for governance proposal cancellation.
//!
//! Exercises `governance::do_cancel_proposal` (exposed as the `cancel_proposal`
//! contract entrypoint) beyond its happy path:
//!
//! - valid cancellations (admin authorization, reason propagation to the event);
//! - invalid or boundary `proposal_id` values (unknown ids, `u64::MAX`);
//! - unauthorized callers (missing admin signature, non-admin signer, stale
//!   admin signature after rotation);
//! - state-transition guards (cancel after execution, re-cancellation,
//!   cancellation blocking later execution of a quorum-met proposal);
//! - state invariance: rejected cancellations must leave the stored proposal
//!   byte-for-byte unchanged.
//!
//! The existing `src/test_governance.rs` suite covers the happy path of
//! `cancel_proposal`; this file focuses on the failure and boundary paths.

use soroban_sdk::testutils::{Address as _, Ledger as _, MockAuth, MockAuthInvoke};
use soroban_sdk::{Address, Env, IntoVal, String, Symbol, Vec};
use subscription_vault::{
    Error, ProposalCancelledEvent, ProposalKind, SubscriptionVault, SubscriptionVaultClient,
};

/// Minimum accepted event schema version for the cancelled event.
const EVENT_SCHEMA_VERSION: u32 = 2;

/// Initialize the vault contract with a mocked admin and return the client.
fn setup() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    client.init(
        &token_address,
        &6u32, // decimals
        &admin,
        &10_000_000i128, // min_topup
        &86_400u64,      // grace period
    );

    (env, client, admin)
}

/// Submit a RotateAdmin proposal one hour in the future and return its id.
fn submit_rotate_proposal(env: &Env, client: &SubscriptionVaultClient, new_admin: &Address) -> u64 {
    let eta = env.ledger().timestamp() + 3600;
    client.submit_proposal(
        &ProposalKind::RotateAdmin,
        new_admin,
        &None,
        &0u32,
        &5000u32, // 50% quorum
        &eta,
    )
}

fn cancel_proposal_args(env: &Env, proposal_id: u64, reason: &String) -> Vec<soroban_sdk::Val> {
    let mut args = Vec::new(env);
    args.push_back(proposal_id.into_val(env));
    args.push_back(reason.clone().into_val(env));
    args
}

/// Collect every `proposal_cancelled` event decoded into its typed payload.
fn cancelled_events(env: &Env) -> Vec<ProposalCancelledEvent> {
    let mut found: Vec<ProposalCancelledEvent> = Vec::new(env);
    for event in env.events().all().iter() {
        if event.1.len() == 0 {
            continue;
        }
        let topic: Symbol = Symbol::try_from_val(env, &event.1.get(0).unwrap()).unwrap();
        if topic == Symbol::new(env, "proposal_cancelled") {
            let payload = ProposalCancelledEvent::try_from_val(env, &event.2).unwrap();
            found.push_back(payload);
        }
    }
    found
}

// ── Valid cancellations ─────────────────────────────────────────────────────

#[test]
fn admin_can_cancel_active_proposal() {
    let (env, client, _admin) = setup();
    let new_admin = Address::generate(&env);
    let proposal_id = submit_rotate_proposal(&env, &client, &new_admin);

    let reason = String::from_str(&env, "Superseded by newer proposal");
    client.cancel_proposal(&proposal_id, &reason);

    let proposal = client
        .get_proposal(&proposal_id)
        .expect("cancelled proposal must still be readable");
    assert_eq!(proposal.id, proposal_id);
    assert_eq!(proposal.executed, true);
}

#[test]
fn cancel_event_carries_reason_timestamp_and_schema_version() {
    let (env, client, _admin) = setup();
    let new_admin = Address::generate(&env);
    let proposal_id = submit_rotate_proposal(&env, &client, &new_admin);

    let before = env.ledger().timestamp();
    let reason = String::from_str(&env, "duplicate submission");
    client.cancel_proposal(&proposal_id, &reason);
    let after = env.ledger().timestamp();

    let events = cancelled_events(&env);
    assert_eq!(events.len(), 1, "exactly one cancellation event expected");
    let event = events.get(0).unwrap();
    assert_eq!(event.proposal_id, proposal_id);
    assert_eq!(event.reason, reason);
    assert!(
        event.timestamp >= before && event.timestamp <= after,
        "event timestamp must fall within the invocation window"
    );
    assert_eq!(event.schema_version, EVENT_SCHEMA_VERSION);
}

#[test]
fn empty_reason_string_is_accepted_and_emitted() {
    let (env, client, _admin) = setup();
    let new_admin = Address::generate(&env);
    let proposal_id = submit_rotate_proposal(&env, &client, &new_admin);

    let reason = String::from_str(&env, "");
    client.cancel_proposal(&proposal_id, &reason);

    let events = cancelled_events(&env);
    assert_eq!(events.len(), 1);
    assert_eq!(events.get(0).unwrap().reason, reason);
}

#[test]
fn long_reason_string_is_accepted_and_emitted() {
    let (env, client, _admin) = setup();
    let new_admin = Address::generate(&env);
    let proposal_id = submit_rotate_proposal(&env, &client, &new_admin);

    let long_text = "r".repeat(256);
    let reason = String::from_str(&env, &long_text);
    client.cancel_proposal(&proposal_id, &reason);

    let events = cancelled_events(&env);
    assert_eq!(events.len(), 1);
    assert_eq!(events.get(0).unwrap().reason, reason);
}

// ── Invalid and boundary proposal ids ───────────────────────────────────────

#[test]
fn cancel_unknown_proposal_id_fails_with_not_found() {
    let (env, client, _admin) = setup();
    // No proposal has been submitted, so id 0 cannot exist.
    let reason = String::from_str(&env, "invalid id");
    let result = client.try_cancel_proposal(&0u64, &reason);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn cancel_proposal_id_at_u64_boundary_fails_with_not_found() {
    let (env, client, _admin) = setup();
    let new_admin = Address::generate(&env);
    let _real_id = submit_rotate_proposal(&env, &client, &new_admin);

    let reason = String::from_str(&env, "boundary id");
    let result_max = client.try_cancel_proposal(&u64::MAX, &reason);
    assert_eq!(result_max, Err(Ok(Error::NotFound)));

    let result_max_minus_one = client.try_cancel_proposal(&(u64::MAX - 1), &reason);
    assert_eq!(result_max_minus_one, Err(Ok(Error::NotFound)));

    // The real proposal is unaffected by the rejected boundary attempts.
    let proposal = client.get_proposal(&_real_id).unwrap();
    assert_eq!(proposal.executed, false);
}

#[test]
fn cancelling_one_proposal_leaves_sibling_proposals_intact() {
    let (env, client, admin) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian, &100u32);

    let target_a = Address::generate(&env);
    let target_b = Address::generate(&env);
    let id_a = submit_rotate_proposal(&env, &client, &target_a);
    let id_b = submit_rotate_proposal(&env, &client, &target_b);

    let reason = String::from_str(&env, "withdraw proposal a");
    client.cancel_proposal(&id_a, &reason);

    // Sibling proposal must remain active and votable.
    let sibling = client.get_proposal(&id_b).unwrap();
    assert_eq!(sibling.executed, false);
    client.vote_proposal(&id_b, &true);
    let sibling = client.get_proposal(&id_b).unwrap();
    assert_eq!(sibling.votes.len(), 1);
}

// ── Unauthorized callers ────────────────────────────────────────────────────

#[test]
fn cancel_proposal_without_admin_signature_is_rejected() {
    let (env, client, _admin) = setup();
    let new_admin = Address::generate(&env);
    let proposal_id = submit_rotate_proposal(&env, &client, &new_admin);

    // Clear every mocked signature: the stored admin has not signed.
    env.mock_auths(&[]);
    let reason = String::from_str(&env, "unsigned attempt");
    let result = client.try_cancel_proposal(&proposal_id, &reason);
    assert!(
        result.is_err(),
        "cancellation without the admin signature must be rejected"
    );

    // State must be unchanged after the rejected operation.
    let proposal = client.get_proposal(&proposal_id).unwrap();
    assert_eq!(proposal.executed, false);
}

#[test]
fn cancel_proposal_signed_by_non_admin_is_rejected() {
    let (env, client, _admin) = setup();
    let new_admin = Address::generate(&env);
    let stranger = Address::generate(&env);
    let proposal_id = submit_rotate_proposal(&env, &client, &new_admin);

    // A valid signature from a non-admin address must not satisfy the
    // stored-admin authorization requirement.
    let reason = String::from_str(&env, "stranger attempt");
    env.mock_auths(&[MockAuth {
        address: &stranger,
        invoke: &MockAuthInvoke {
            contract: &client.address,
            fn_name: "cancel_proposal",
            args: cancel_proposal_args(&env, proposal_id, &reason),
            sub_invokes: &[],
        },
    }]);
    let result = client.try_cancel_proposal(&proposal_id, &reason);
    assert!(
        result.is_err(),
        "a non-admin signature must not be able to cancel a proposal"
    );

    let proposal = client.get_proposal(&proposal_id).unwrap();
    assert_eq!(proposal.executed, false);
}

#[test]
fn stale_admin_signature_cannot_cancel_after_rotation() {
    let (env, client, old_admin) = setup();
    let new_admin = Address::generate(&env);
    let proposal_id = submit_rotate_proposal(&env, &client, &new_admin);

    // Rotate to a new admin. `do_cancel_proposal` authorizes the *stored*
    // admin, so the previous admin's signature must no longer suffice.
    client.rotate_admin(&old_admin, &new_admin, &0u64);
    assert_eq!(client.get_admin(), new_admin);

    let reason = String::from_str(&env, "stale admin attempt");
    env.mock_auths(&[MockAuth {
        address: &old_admin,
        invoke: &MockAuthInvoke {
            contract: &client.address,
            fn_name: "cancel_proposal",
            args: cancel_proposal_args(&env, proposal_id, &reason),
            sub_invokes: &[],
        },
    }]);
    let result = client.try_cancel_proposal(&proposal_id, &reason);
    assert!(
        result.is_err(),
        "the previous admin's signature must not cancel proposals after rotation"
    );

    // The current admin can still cancel.
    env.mock_all_auths();
    client.cancel_proposal(&proposal_id, &String::from_str(&env, "cancelled by current admin"));
    let proposal = client.get_proposal(&proposal_id).unwrap();
    assert_eq!(proposal.executed, true);
}

// ── State-transition guards ─────────────────────────────────────────────────

#[test]
fn cancel_proposal_twice_fails_with_invalid_input() {
    let (env, client, _admin) = setup();
    let new_admin = Address::generate(&env);
    let proposal_id = submit_rotate_proposal(&env, &client, &new_admin);

    let reason = String::from_str(&env, "first cancellation");
    client.cancel_proposal(&proposal_id, &reason);

    let second = client.try_cancel_proposal(&proposal_id, &reason);
    assert_eq!(second, Err(Ok(Error::InvalidInput)));

    // Exactly one cancellation event must have been emitted.
    assert_eq!(cancelled_events(&env).len(), 1);
}

#[test]
fn cancelled_proposal_cannot_be_voted_on() {
    let (env, client, admin) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian, &100u32);

    let new_admin = Address::generate(&env);
    let proposal_id = submit_rotate_proposal(&env, &client, &new_admin);

    let reason = String::from_str(&env, "withdrawn from consideration");
    client.cancel_proposal(&proposal_id, &reason);

    let vote = client.try_vote_proposal(&proposal_id, &true);
    assert_eq!(vote, Err(Ok(Error::InvalidInput)));

    let proposal = client.get_proposal(&proposal_id).unwrap();
    assert_eq!(proposal.votes.len(), 0);
}

#[test]
fn cancelled_proposal_cannot_be_executed_even_with_quorum() {
    let (env, client, admin) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian, &100u32);

    let new_admin = Address::generate(&env);
    let cancelled_id = submit_rotate_proposal(&env, &client, &new_admin);
    client.vote_proposal(&cancelled_id, &true);

    let reason = String::from_str(&env, "quorum met, but withdrawn");
    client.cancel_proposal(&cancelled_id, &reason);

    // Advance well past the proposal's ETA.
    env.ledger().set_timestamp(env.ledger().timestamp() + 7200);

    // Quorum was met before cancellation, so execution is only blocked by the
    // cancellation itself.
    let execute = client.try_execute_proposal(&cancelled_id);
    assert_eq!(execute, Err(Ok(Error::InvalidInput)));
    assert_ne!(client.get_admin(), new_admin);

    // Counterfactual: an identical, non-cancelled proposal executes fine,
    // proving the guard is the cancellation and not the setup.
    let replacement_admin = Address::generate(&env);
    let replacement_id = submit_rotate_proposal(&env, &client, &replacement_admin);
    client.vote_proposal(&replacement_id, &true);
    env.ledger().set_timestamp(env.ledger().timestamp() + 7200);
    client.execute_proposal(&replacement_id);
    assert_eq!(client.get_admin(), replacement_admin);
}

#[test]
fn cannot_cancel_already_executed_proposal() {
    let (env, client, admin) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian, &100u32);

    let new_admin = Address::generate(&env);
    let proposal_id = submit_rotate_proposal(&env, &client, &new_admin);
    client.vote_proposal(&proposal_id, &true);

    env.ledger().set_timestamp(env.ledger().timestamp() + 7200);
    client.execute_proposal(&proposal_id);

    let reason = String::from_str(&env, "too late");
    let result = client.try_cancel_proposal(&proposal_id, &reason);
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

// ── State invariance after rejected cancellations ──────────────────────────

#[test]
fn rejected_cancellations_leave_proposal_state_unchanged() {
    let (env, client, admin) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian, &100u32);

    let new_admin = Address::generate(&env);
    let proposal_id = submit_rotate_proposal(&env, &client, &new_admin);
    client.vote_proposal(&proposal_id, &true);

    let before = client.get_proposal(&proposal_id).unwrap();

    // Missing admin signature.
    env.mock_auths(&[]);
    let _ = client.try_cancel_proposal(&proposal_id, &String::from_str(&env, "unsigned"));

    // Restored signature, but a non-existent id.
    env.mock_all_auths();
    let _ = client.try_cancel_proposal(&u64::MAX, &String::from_str(&env, "missing id"));

    let after = client.get_proposal(&proposal_id).unwrap();

    // Full snapshot comparison: nothing may have moved.
    assert_eq!(after.id, before.id);
    assert_eq!(after.kind, before.kind);
    assert_eq!(after.target, before.target);
    assert_eq!(after.target2, before.target2);
    assert_eq!(after.target3, before.target3);
    assert_eq!(after.quorum_bps, before.quorum_bps);
    assert_eq!(after.eta, before.eta);
    assert_eq!(after.submitted_at, before.submitted_at);
    assert_eq!(after.executed, before.executed);
    assert_eq!(after.votes.len(), before.votes.len());

    // No cancellation event may have been emitted.
    assert_eq!(cancelled_events(&env).len(), 0);
}
