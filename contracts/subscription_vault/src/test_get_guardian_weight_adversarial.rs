//! Adversarial coverage for `governance::get_guardian_weight` (issue #1052).
//!
//! `get_guardian_weight` is the read side of the guardian-weight map that
//! gates proposals. The broader governance suite only observes it indirectly;
//! this module pins its own contract:
//!
//! * an unknown address reads `0` and does not materialise `DataKey::Guardians`;
//! * weights round-trip exactly, the latest write wins, removal deletes the key;
//! * `0` is rejected as a write and a non-admin cannot mutate the map;
//! * reads are pure (the stored map is byte-identical afterwards);
//! * the store is exactly what quorum and the vote gate consume, including the
//!   `u32::MAX` saturation of `calculate_total_weight`.

use crate::test_utils::setup::TestEnv;
use crate::types::{DataKey, Error, ProposalExecutedEvent, ProposalKind};
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
use soroban_sdk::{Address, Env, FromVal, IntoVal, Map, Symbol};

/// Read `DataKey::Guardians` straight out of the vault's persistent storage.
fn stored_guardians(env: &Env, contract: &Address) -> Map<Address, u32> {
    env.as_contract(contract, || {
        env.storage()
            .persistent()
            .get::<_, Map<Address, u32>>(&DataKey::Guardians)
            .unwrap_or_else(|| Map::new(env))
    })
}

fn guardians_entry_exists(env: &Env, contract: &Address) -> bool {
    env.as_contract(contract, || {
        env.storage().persistent().has(&DataKey::Guardians)
    })
}

/// Decode the payload of the `proposal_executed` event emitted at execution.
///
/// `Events::all` yields `(contract, topics, data)`; the event symbol is the
/// first topic and the payload is the data `Val`.
fn executed_event(env: &Env) -> ProposalExecutedEvent {
    let wanted = Symbol::new(env, "proposal_executed");
    for event in env.events().all().iter() {
        let topics = event.1.clone();
        if topics.len() >= 1 {
            let topic = Symbol::from_val(env, &topics.get(0).unwrap());
            if topic == wanted {
                return event.2.into_val(env);
            }
        }
    }
    panic!("proposal_executed event must be emitted");
}

// ── Default / unknown address ────────────────────────────────────────────────

#[test]
fn unknown_guardian_reads_zero_without_creating_storage_entry() {
    let te = TestEnv::default();
    let unknown = Address::generate(&te.env);

    assert_eq!(te.client.get_guardian_weight(&unknown), 0);
    assert!(
        !guardians_entry_exists(&te.env, &te.client.address),
        "a pure read must not materialise DataKey::Guardians"
    );
    assert!(stored_guardians(&te.env, &te.client.address)
        .get(unknown)
        .is_none());
}

// ── Happy path: exact storage ────────────────────────────────────────────────

#[test]
fn add_guardian_stores_exact_weight_in_the_guardians_map() {
    let te = TestEnv::default();
    let g = Address::generate(&te.env);

    te.client.add_guardian(&te.admin, &g, &7);

    assert_eq!(te.client.get_guardian_weight(&g), 7);
    let raw = stored_guardians(&te.env, &te.client.address);
    assert_eq!(raw.len(), 1);
    assert_eq!(raw.get(g).unwrap(), 7);
}

// ── Overwrite semantics: latest write wins, never a sum ──────────────────────

#[test]
fn re_adding_guardian_overwrites_and_never_sums() {
    let te = TestEnv::default();
    let g = Address::generate(&te.env);

    te.client.add_guardian(&te.admin, &g, &7);
    te.client.add_guardian(&te.admin, &g, &42);

    assert_eq!(te.client.get_guardian_weight(&g), 42);
    let raw = stored_guardians(&te.env, &te.client.address);
    assert_eq!(raw.len(), 1, "overwrite must not insert a second entry");
    assert_eq!(raw.get(g).unwrap(), 42);
}

// ── Zero-weight rejection ────────────────────────────────────────────────────

#[test]
fn zero_weight_add_is_rejected_and_prior_weight_is_unchanged() {
    let te = TestEnv::default();
    let g = Address::generate(&te.env);
    let fresh = Address::generate(&te.env);
    te.client.add_guardian(&te.admin, &g, &7);

    assert_eq!(
        te.client.try_add_guardian(&te.admin, &g, &0),
        Err(Ok(Error::InvalidInput))
    );
    assert_eq!(te.client.get_guardian_weight(&g), 7);

    // A zero write for a brand-new address must not create an entry either.
    assert_eq!(
        te.client.try_add_guardian(&te.admin, &fresh, &0),
        Err(Ok(Error::InvalidInput))
    );
    assert_eq!(te.client.get_guardian_weight(&fresh), 0);
    assert!(stored_guardians(&te.env, &te.client.address)
        .get(fresh)
        .is_none());
}

#[test]
#[should_panic(expected = "Error(Contract, #3002)")]
fn zero_weight_add_panics_with_invalid_input_contract_code() {
    let te = TestEnv::default();
    let g = Address::generate(&te.env);

    // The panicking client must surface the InvalidInput discriminant.
    te.client.add_guardian(&te.admin, &g, &0);
}

// ── Removal deletes the key and resets the getter to zero ────────────────────

#[test]
fn remove_guardian_deletes_entry_and_resets_weight_to_zero() {
    let te = TestEnv::default();
    let g = Address::generate(&te.env);
    te.client.add_guardian(&te.admin, &g, &9);

    te.client.remove_guardian(&te.admin, &g);

    // `remove_guardian` deletes the map key (it does not zero it in place).
    assert_eq!(te.client.get_guardian_weight(&g), 0);
    let raw = stored_guardians(&te.env, &te.client.address);
    assert_eq!(raw.len(), 0);
    assert!(raw.get(g).is_none());
}

// ── Independence across guardians ────────────────────────────────────────────

#[test]
fn guardians_are_independent_and_removal_does_not_affect_others() {
    let te = TestEnv::default();
    let a = Address::generate(&te.env);
    let b = Address::generate(&te.env);
    let c = Address::generate(&te.env);
    te.client.add_guardian(&te.admin, &a, &3);
    te.client.add_guardian(&te.admin, &b, &5);
    te.client.add_guardian(&te.admin, &c, &8);

    assert_eq!(te.client.get_guardian_weight(&a), 3);
    assert_eq!(te.client.get_guardian_weight(&b), 5);
    assert_eq!(te.client.get_guardian_weight(&c), 8);

    te.client.remove_guardian(&te.admin, &b);

    assert_eq!(te.client.get_guardian_weight(&a), 3);
    assert_eq!(te.client.get_guardian_weight(&b), 0);
    assert_eq!(te.client.get_guardian_weight(&c), 8);
}

// ── Boundary: u32::MAX round-trips without truncation ────────────────────────

#[test]
fn max_u32_weight_is_accepted_and_read_back_exactly() {
    let te = TestEnv::default();
    let g = Address::generate(&te.env);

    te.client.add_guardian(&te.admin, &g, &u32::MAX);

    assert_eq!(te.client.get_guardian_weight(&g), u32::MAX);
    assert_eq!(
        stored_guardians(&te.env, &te.client.address)
            .get(g)
            .unwrap(),
        u32::MAX
    );
}

// ── Consumption site: calculate_total_weight ─────────────────────────────────

#[test]
fn total_weight_equals_sum_of_get_guardian_weight() {
    let te = TestEnv::default();
    let g2 = Address::generate(&te.env);
    // `do_vote_proposal` authenticates the stored admin, so admin must be a
    // guardian for the execution path below to be reachable.
    te.client.add_guardian(&te.admin, &te.admin, &3);
    te.client.add_guardian(&te.admin, &g2, &5);

    let sum = te.client.get_guardian_weight(&te.admin) + te.client.get_guardian_weight(&g2);
    assert_eq!(sum, 8);

    let eta = te.env.ledger().timestamp() + 3600;
    let target = Address::generate(&te.env);
    let id = te
        .client
        .submit_proposal(&ProposalKind::RotateAdmin, &target, &None, &0, &3000, &eta);
    te.client.vote_proposal(&id, &true);
    te.env.ledger().set_timestamp(eta + 1);
    te.client.execute_proposal(&id);

    let event = executed_event(&te.env);
    assert_eq!(
        event.total_weight, sum,
        "calculate_total_weight must equal the sum of get_guardian_weight"
    );
    assert_eq!(event.votes_for, 3);
}

#[test]
fn total_weight_saturates_at_u32_max_instead_of_wrapping() {
    let te = TestEnv::default();
    let g2 = Address::generate(&te.env);
    te.client.add_guardian(&te.admin, &te.admin, &u32::MAX);
    te.client.add_guardian(&te.admin, &g2, &1);

    // A wrapping sum of `u32::MAX + 1` would be 0; `checked_add` clamps instead.
    let eta = te.env.ledger().timestamp() + 3600;
    let target = Address::generate(&te.env);
    let id = te.client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &target,
        &None,
        &0,
        &10_000,
        &eta,
    );
    te.client.vote_proposal(&id, &true);
    te.env.ledger().set_timestamp(eta + 1);
    te.client.execute_proposal(&id);

    let event = executed_event(&te.env);
    assert_eq!(event.total_weight, u32::MAX, "total weight must saturate");
    assert_eq!(event.votes_for, u32::MAX);
}

// ── Consumption site: the vote gate keys off get_guardian_weight ─────────────

#[test]
fn removed_guardian_cannot_vote_and_is_unauthorized() {
    let te = TestEnv::default();
    let g2 = Address::generate(&te.env);
    te.client.add_guardian(&te.admin, &te.admin, &100);
    te.client.add_guardian(&te.admin, &g2, &100);

    let eta = te.env.ledger().timestamp() + 3600;
    let target = Address::generate(&te.env);
    let id = te
        .client
        .submit_proposal(&ProposalKind::RotateAdmin, &target, &None, &0, &5000, &eta);

    te.client.remove_guardian(&te.admin, &te.admin);
    assert_eq!(te.client.get_guardian_weight(&te.admin), 0);
    assert_eq!(
        te.client.try_vote_proposal(&id, &true),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn removal_mid_vote_invalidates_prior_votes_at_execution() {
    let te = TestEnv::default();
    let g2 = Address::generate(&te.env);
    te.client.add_guardian(&te.admin, &te.admin, &100);
    te.client.add_guardian(&te.admin, &g2, &100);

    let eta = te.env.ledger().timestamp() + 3600;
    let target = Address::generate(&te.env);
    let id = te
        .client
        .submit_proposal(&ProposalKind::RotateAdmin, &target, &None, &0, &5000, &eta);
    te.client.vote_proposal(&id, &true);

    // The admin votes, then is removed: the stored vote must be excluded because
    // `calculate_quorum` re-reads the same Guardians map the getter returns.
    te.client.remove_guardian(&te.admin, &te.admin);
    assert_eq!(te.client.get_guardian_weight(&te.admin), 0);

    te.env.ledger().set_timestamp(eta + 1);
    // votes_for = 0, total_weight = 100, required = 50 -> quorum fails.
    assert_eq!(
        te.client.try_execute_proposal(&id),
        Err(Ok(Error::InvalidInput))
    );
}

// ── Reads are pure ───────────────────────────────────────────────────────────

#[test]
fn reads_do_not_mutate_guardian_storage() {
    let te = TestEnv::default();
    let g = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);
    te.client.add_guardian(&te.admin, &g, &11);

    let before = stored_guardians(&te.env, &te.client.address);
    assert_eq!(te.client.get_guardian_weight(&g), 11);
    assert_eq!(te.client.get_guardian_weight(&stranger), 0);
    let after = stored_guardians(&te.env, &te.client.address);

    assert!(
        before == after,
        "reads must leave DataKey::Guardians identical"
    );
    assert_eq!(after.len(), 1);
    assert!(after.get(stranger).is_none());
}

// ── Authorization on the mutating wrappers ───────────────────────────────────

#[test]
fn non_admin_cannot_add_or_remove_and_state_is_unchanged() {
    let te = TestEnv::default();
    let g = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);
    te.client.add_guardian(&te.admin, &g, &5);

    assert_eq!(
        te.client.try_add_guardian(&stranger, &stranger, &99),
        Err(Ok(Error::Forbidden))
    );
    assert_eq!(
        te.client.try_remove_guardian(&stranger, &g),
        Err(Ok(Error::Forbidden))
    );

    assert_eq!(te.client.get_guardian_weight(&g), 5);
    assert_eq!(te.client.get_guardian_weight(&stranger), 0);
}

// ── Invalid input: no admin configured ───────────────────────────────────────

#[test]
fn governance_writes_before_init_return_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let g = Address::generate(&env);

    assert_eq!(
        client.try_add_guardian(&admin, &g, &1),
        Err(Ok(Error::NotInitialized))
    );
    assert_eq!(
        client.try_remove_guardian(&admin, &g),
        Err(Ok(Error::NotInitialized))
    );
    // The read side stays safe and defaults to zero with no stored guardians.
    assert_eq!(client.get_guardian_weight(&g), 0);
}
