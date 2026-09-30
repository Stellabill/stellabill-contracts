#![cfg(test)]

//! Adversarial coverage for `SubscriptionVault::cancel_admin_proposal`.
//!
//! The entry point lives in `contracts/subscription_vault/src/lib.rs` and
//! delegates to `admin::do_cancel_admin_proposal`. The happy path (propose ->
//! cancel -> gone) is already thin; what this suite pins down is everything a
//! misbehaving or hostile caller can try *around* it:
//!
//! * a cancellation attempt that must not mutate anything: wrong caller, no
//!   active proposal, already-claimed proposal, or a contract that was never
//!   initialised;
//! * the identity rules — the *stored* admin is the only address allowed to
//!   cancel, so the proposed successor and a configured operator are both
//!   rejected, and neither gains any power from a cancelled proposal;
//! * the interaction with the 7-day claim window: `cancel` deliberately does not
//!   consult `expires_at`, so an expired-but-unclaimed proposal is still
//!   cancellable, and cancelling is not a rotation;
//! * the observable side effects of a successful cancel: exactly one
//!   `admin_proposal_cancelled` event, with the caller and the ledger timestamp,
//!   and no change to unrelated admin configuration.

use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    token::StellarAssetClient as TokenAdminClient,
    Address, Env, Symbol, TryFromVal, Val, Vec,
};
use subscription_vault::{
    AdminProposal, AdminProposalCancelledEvent, Error, SubscriptionVault, SubscriptionVaultClient,
};

/// Admin proposal window mirrored from `admin::PROPOSAL_WINDOW_SECS`.
const PROPOSAL_WINDOW_SECS: u64 = 7 * 24 * 60 * 60;

/// Cooldown between two mutations of the same admin config key.
const CONFIG_COOLDOWN_SECS: u64 = 6 * 60 * 60;

const T0: u64 = 1_000_000;

struct Fixture {
    env: Env,
    client: SubscriptionVaultClient<'static>,
    admin: Address,
    operator: Address,
    proposed: Address,
    stranger: Address,
}

/// Initialise a vault with a known admin, a configured operator and a funded
/// subscriber. `env.mock_all_auths()` is on, so `require_auth()` always passes:
/// every rejection asserted below is therefore a *contract* decision, not a host
/// auth failure, which is exactly the branch being covered.
fn setup() -> Fixture {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_admin_client = TokenAdminClient::new(&env, &token_address);

    let admin = Address::generate(&env);
    let operator = Address::generate(&env);
    let proposed = Address::generate(&env);
    let stranger = Address::generate(&env);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    client.init(&token_address, &7u32, &admin, &1_000_000i128, &(3 * 24 * 60 * 60));
    client.set_operator(&admin, &operator);

    token_admin_client.mint(&admin, &100_000_000);

    Fixture { env, client, admin, operator, proposed, stranger }
}

fn advance_seconds(env: &Env, seconds: u64) {
    let now = env.ledger().timestamp();
    env.ledger().set_timestamp(now + seconds);
}

/// Collect the data payload of every event published under `topic`.
fn event_data(env: &Env, topic: &str) -> Vec<Val> {
    let want = Symbol::new(env, topic);
    let all = env.events().all();
    let mut out = Vec::new(env);
    for i in 0..all.len() {
        let (_, topics, data): (Address, Vec<Val>, Val) = all.get(i).unwrap();
        if let Some(t) = topics.get(0) {
            if let Ok(published) = Symbol::try_from_val(env, &t) {
                if published == want {
                    out.push_back(data);
                }
            }
        }
    }
    out
}

fn count_events(env: &Env, topic: &str) -> u32 {
    event_data(env, topic).len()
}

/// Contract never initialised: `require_admin` must fail before anything is
/// compared, so even a caller that passes host auth is rejected.
fn setup_uninitialised() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    (env, client, admin)
}

// ── Rejections that must leave all state untouched ───────────────────────────

#[test]
fn cancel_without_an_active_proposal_is_rejected_and_leaves_no_trace() {
    let f = setup();

    let result = f.client.try_cancel_admin_proposal(&f.admin);

    assert_eq!(result, Err(Ok(Error::NoActiveProposal)));
    assert!(f.client.get_admin_proposal().is_none());
    assert_eq!(f.client.get_admin(), f.admin);
    assert_eq!(count_events(&f.env, "admin_proposal_cancelled"), 0);
}

#[test]
fn cancel_is_rejected_on_a_contract_that_was_never_initialised() {
    let (env, client, admin) = setup_uninitialised();

    let result = client.try_cancel_admin_proposal(&admin);

    assert_eq!(result, Err(Ok(Error::NotInitialized)));
    assert_eq!(count_events(&env, "admin_proposal_cancelled"), 0);
}

#[test]
fn cancel_by_a_stranger_does_not_clear_the_pending_proposal() {
    let f = setup();
    f.client.propose_admin(&f.admin, &f.proposed);
    let before: AdminProposal = f.client.get_admin_proposal().unwrap();

    let result = f.client.try_cancel_admin_proposal(&f.stranger);

    assert_eq!(result, Err(Ok(Error::Unauthorized)));
    let after: AdminProposal = f.client.get_admin_proposal().unwrap();
    assert_eq!(after.new_admin, before.new_admin);
    assert_eq!(after.proposed_at, before.proposed_at);
    assert_eq!(after.expires_at, before.expires_at);
    assert_eq!(count_events(&f.env, "admin_proposal_cancelled"), 0);
}

#[test]
fn the_proposed_successor_cannot_cancel_its_own_pending_proposal() {
    let f = setup();
    f.client.propose_admin(&f.admin, &f.proposed);

    // `proposed` is the claimed successor-designate; it is still not the stored
    // admin, so it has no cancellation authority.
    let result = f.client.try_cancel_admin_proposal(&f.proposed);

    assert_eq!(result, Err(Ok(Error::Unauthorized)));
    assert_eq!(f.client.get_admin_proposal().unwrap().new_admin, f.proposed);
    assert_eq!(count_events(&f.env, "admin_proposal_cancelled"), 0);
}

#[test]
fn a_configured_operator_cannot_cancel_an_admin_proposal() {
    let f = setup();
    f.client.propose_admin(&f.admin, &f.proposed);
    assert_eq!(f.client.get_operator().unwrap(), f.operator);

    // The operator has privileged bulk-charge powers but must not reach the
    // admin-rotation surface.
    let result = f.client.try_cancel_admin_proposal(&f.operator);

    assert_eq!(result, Err(Ok(Error::Unauthorized)));
    assert!(f.client.get_admin_proposal().is_some());
    assert_eq!(count_events(&f.env, "admin_proposal_cancelled"), 0);
}

#[test]
fn cancelling_a_second_time_is_rejected() {
    let f = setup();
    f.client.propose_admin(&f.admin, &f.proposed);

    f.client.cancel_admin_proposal(&f.admin);
    // `Env::events().all()` reflects the most recent invocation, so the event
    // assertion has to be taken while the successful cancel is still the last
    // call made.
    assert_eq!(count_events(&f.env, "admin_proposal_cancelled"), 1);

    let second = f.client.try_cancel_admin_proposal(&f.admin);
    assert_eq!(second, Err(Ok(Error::NoActiveProposal)));
    assert!(f.client.get_admin_proposal().is_none());
    assert_eq!(count_events(&f.env, "admin_proposal_cancelled"), 0);
}

// ── Expiry window interaction ───────────────────────────────────────────────

#[test]
fn an_expired_but_unclaimed_proposal_is_still_cancellable() {
    let f = setup();
    f.client.propose_admin(&f.admin, &f.proposed);

    // Past the 7-day claim window: the proposal is stale but still stored.
    advance_seconds(&f.env, PROPOSAL_WINDOW_SECS + 1);
    assert!(f.client.get_admin_proposal().is_some());

    f.client.cancel_admin_proposal(&f.admin);

    // Cancelling an expired proposal must clear it, not leave a tombstone that a
    // later `propose_admin` would trip over with `ProposalAlreadyExists`.
    assert!(f.client.get_admin_proposal().is_none());
    f.client.propose_admin(&f.admin, &f.stranger);
    assert_eq!(f.client.get_admin_proposal().unwrap().new_admin, f.stranger);
}

#[test]
fn cancelling_is_not_a_rotation_so_the_successor_never_becomes_admin() {
    let f = setup();
    f.client.propose_admin(&f.admin, &f.proposed);
    f.client.cancel_admin_proposal(&f.admin);

    // The proposed address is not the admin and holds no admin privilege.
    assert_eq!(f.client.get_admin(), f.admin);
    let as_successor = f.client.try_set_min_topup(&f.proposed, &2_000_000i128);
    assert_eq!(as_successor, Err(Ok(Error::Forbidden)));
    let successor_cancel = f.client.try_cancel_admin_proposal(&f.proposed);
    assert_eq!(successor_cancel, Err(Ok(Error::Unauthorized)));

    // The real admin is still fully in charge.
    f.client.set_min_topup(&f.admin, &2_000_000i128);
    assert_eq!(f.client.get_min_topup(), 2_000_000);
}

#[test]
fn cancelling_after_a_claim_is_rejected_for_both_the_old_and_new_admin() {
    let f = setup();
    f.client.propose_admin(&f.admin, &f.proposed);
    f.client.claim_admin_role(&f.proposed);

    // The proposal is consumed by the claim, so there is nothing left to cancel.
    assert_eq!(
        f.client.try_cancel_admin_proposal(&f.proposed),
        Err(Ok(Error::NoActiveProposal))
    );
    // The old admin is no longer the stored admin: identity is checked before
    // the proposal lookup, so this is `Unauthorized`, not `NoActiveProposal`.
    assert_eq!(
        f.client.try_cancel_admin_proposal(&f.admin),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(f.client.get_admin(), f.proposed);
}

// ── Success path side effects ───────────────────────────────────────────────

#[test]
fn a_successful_cancel_emits_exactly_one_event_carrying_the_caller_and_timestamp() {
    let f = setup();
    f.client.propose_admin(&f.admin, &f.proposed);

    advance_seconds(&f.env, 1_234);
    let expected_ts = f.env.ledger().timestamp();

    f.client.cancel_admin_proposal(&f.admin);

    let payloads = event_data(&f.env, "admin_proposal_cancelled");
    assert_eq!(payloads.len(), 1);
    let parsed = AdminProposalCancelledEvent::try_from_val(&f.env, &payloads.get(0).unwrap()).unwrap();
    assert_eq!(parsed.admin, f.admin);
    assert_eq!(parsed.timestamp, expected_ts);
}

#[test]
fn a_successful_cancel_leaves_unrelated_configuration_untouched() {
    let f = setup();
    let min_topup_before = f.client.get_min_topup();

    f.client.propose_admin(&f.admin, &f.proposed);
    f.client.cancel_admin_proposal(&f.admin);

    assert_eq!(f.client.get_min_topup(), min_topup_before);
    assert_eq!(f.client.get_operator().unwrap(), f.operator);
    assert_eq!(f.client.get_admin(), f.admin);
}

#[test]
fn cancel_does_not_burn_the_admin_config_cooldown() {
    let f = setup();

    f.client.set_min_topup(&f.admin, &2_000_000i128);
    // A cancel in between must not touch any `AdminConfigLastChangedAt` entry.
    f.client.propose_admin(&f.admin, &f.proposed);
    f.client.cancel_admin_proposal(&f.admin);

    advance_seconds(&f.env, CONFIG_COOLDOWN_SECS);
    f.client.set_min_topup(&f.admin, &3_000_000i128);
    assert_eq!(f.client.get_min_topup(), 3_000_000);
}

#[test]
fn the_cancelled_successor_identity_can_be_reused_and_claimed() {
    let f = setup();
    f.client.propose_admin(&f.admin, &f.proposed);
    f.client.cancel_admin_proposal(&f.admin);

    // No zombie state: proposing the same address again succeeds...
    f.client.propose_admin(&f.admin, &f.proposed);
    assert_eq!(f.client.get_admin_proposal().unwrap().new_admin, f.proposed);

    // ...and a claim after that really does rotate the admin.
    f.client.claim_admin_role(&f.proposed);
    assert!(f.client.get_admin_proposal().is_none());
    assert_eq!(f.client.get_admin(), f.proposed);
}

#[test]
fn cancelling_frees_the_slot_for_a_different_successor() {
    let f = setup();
    let first = Address::generate(&f.env);
    let second = Address::generate(&f.env);

    f.client.propose_admin(&f.admin, &first);
    f.client.cancel_admin_proposal(&f.admin);

    f.client.propose_admin(&f.admin, &second);
    let proposal = f.client.get_admin_proposal().unwrap();
    assert_eq!(proposal.new_admin, second);
    assert_eq!(proposal.proposed_at, f.env.ledger().timestamp());
    assert_eq!(
        proposal.expires_at,
        f.env.ledger().timestamp() + PROPOSAL_WINDOW_SECS
    );
}
