//! Adversarial coverage for `governance::remove_guardian` (issue #1051).
//!
//! `remove_guardian` is an admin-only mutation of the guardian voting set. The
//! properties that matter operationally and for security are:
//!
//! - removing a guardian clears their weight and shrinks the enumeration set,
//! - removing an unknown / never-registered address is a safe no-op,
//! - removal is idempotent,
//! - other guardians (and unrelated storage) are untouched by a removal,
//! - a removed guardian's *prior* votes stop counting toward quorum.

use crate::types::{Error, ProposalKind};
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env,
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

fn listed_weight(client: &SubscriptionVaultClient, guardian: &Address) -> Option<u32> {
    for (addr, weight) in client.list_guardians().iter() {
        if addr == *guardian {
            return Some(weight);
        }
    }
    None
}

// ── Removal clears weight and shrinks the enumeration set ──────────────────

#[test]
fn remove_guardian_clears_weight_and_shrinks_list() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let guardian = Address::generate(&env);
    let client = init_vault(&env, &admin);

    client.try_add_guardian(&admin, &guardian, &100).unwrap();
    assert_eq!(client.get_guardian_weight(&guardian), 100);
    assert_eq!(listed_weight(&client, &guardian), Some(100));
    assert_eq!(client.list_guardians().len(), 1);

    client.try_remove_guardian(&admin, &guardian).unwrap();

    assert_eq!(client.get_guardian_weight(&guardian), 0);
    assert_eq!(listed_weight(&client, &guardian), None);
    assert_eq!(client.list_guardians().len(), 0);
}

#[test]
fn remove_guardian_leaves_other_guardians_and_weights_intact() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    let g3 = Address::generate(&env);
    let client = init_vault(&env, &admin);

    client.try_add_guardian(&admin, &g1, &10).unwrap();
    client.try_add_guardian(&admin, &g2, &20).unwrap();
    client.try_add_guardian(&admin, &g3, &30).unwrap();

    client.try_remove_guardian(&admin, &g2).unwrap();

    assert_eq!(client.get_guardian_weight(&g1), 10);
    assert_eq!(client.get_guardian_weight(&g2), 0);
    assert_eq!(client.get_guardian_weight(&g3), 30);
    assert_eq!(client.list_guardians().len(), 2);
    assert_eq!(listed_weight(&client, &g1), Some(10));
    assert_eq!(listed_weight(&client, &g3), Some(30));
    assert_eq!(listed_weight(&client, &g2), None);
}

// ── No-op / boundary removals ──────────────────────────────────────────────

#[test]
fn remove_unknown_guardian_is_safe_noop() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let real = Address::generate(&env);
    let stranger = Address::generate(&env);
    let client = init_vault(&env, &admin);

    client.try_add_guardian(&admin, &real, &50).unwrap();
    let before = client.list_guardians();

    // Never registered — removal must succeed without disturbing the set.
    client.try_remove_guardian(&admin, &stranger).unwrap();

    assert_eq!(client.get_guardian_weight(&stranger), 0);
    assert_eq!(client.list_guardians().len(), before.len());
    assert_eq!(listed_weight(&client, &real), Some(50));
}

#[test]
fn remove_guardian_on_empty_set_is_noop() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let client = init_vault(&env, &admin);
    assert_eq!(client.list_guardians().len(), 0);

    let result = client.try_remove_guardian(&admin, &Address::generate(&env));

    assert!(result.is_ok(), "removing from an empty set must not error");
    assert_eq!(client.list_guardians().len(), 0);
}

#[test]
fn remove_guardian_is_idempotent() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let guardian = Address::generate(&env);
    let client = init_vault(&env, &admin);

    client.try_add_guardian(&admin, &guardian, &75).unwrap();

    client.try_remove_guardian(&admin, &guardian).unwrap();
    client.try_remove_guardian(&admin, &guardian).unwrap();

    assert_eq!(client.get_guardian_weight(&guardian), 0);
    assert_eq!(client.list_guardians().len(), 0);
}

#[test]
fn removed_guardian_weight_can_be_restored_by_readding() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let guardian = Address::generate(&env);
    let client = init_vault(&env, &admin);

    client.try_add_guardian(&admin, &guardian, &40).unwrap();
    client.try_remove_guardian(&admin, &guardian).unwrap();
    assert_eq!(client.get_guardian_weight(&guardian), 0);

    client.try_add_guardian(&admin, &guardian, &90).unwrap();

    assert_eq!(client.get_guardian_weight(&guardian), 90);
    assert_eq!(client.list_guardians().len(), 1);
}

// ── Input validation on the paired writer ──────────────────────────────────

#[test]
fn add_guardian_rejects_zero_weight_and_leaves_set_unchanged() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let guardian = Address::generate(&env);
    let client = init_vault(&env, &admin);

    let result = client.try_add_guardian(&admin, &guardian, &0);

    assert_eq!(result, Err(Ok(Error::InvalidInput)));
    assert_eq!(client.get_guardian_weight(&guardian), 0);
    assert_eq!(client.list_guardians().len(), 0);
}

// ── Authorization ──────────────────────────────────────────────────────────

#[test]
fn non_admin_remove_is_rejected_and_state_unchanged() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let guardian = Address::generate(&env);
    let client = init_vault(&env, &admin);
    client.try_add_guardian(&admin, &guardian, &60).unwrap();

    let stranger = Address::generate(&env);
    let result = client.try_remove_guardian(&stranger, &guardian);

    assert_eq!(result, Err(Ok(Error::Forbidden)));
    assert_eq!(
        client.get_guardian_weight(&guardian),
        60,
        "rejected removal must not clear the weight",
    );
    assert_eq!(listed_weight(&client, &guardian), Some(60));
}

#[test]
fn unauthenticated_remove_is_rejected() {
    // Deliberately no `mock_all_auths`: the host must demand a signature.
    let env = Env::default();

    let admin = Address::generate(&env);
    let guardian = Address::generate(&env);
    let client = init_vault(&env, &admin);

    let result = client.try_remove_guardian(&admin, &guardian);
    assert!(result.is_err(), "missing auth must be rejected");
}

// ── Removal invalidates prior votes during quorum evaluation ───────────────

#[test]
fn removed_guardian_prior_vote_is_excluded_from_quorum() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let admin = Address::generate(&env);
    let other = Address::generate(&env);
    let client = init_vault(&env, &admin);

    // The stored admin is the only address that can cast votes, so it must be a
    // guardian; `other` keeps the total weight non-zero after the admin leaves.
    client.try_add_guardian(&admin, &admin, &100).unwrap();
    client.try_add_guardian(&admin, &other, &100).unwrap();

    let target = Address::generate(&env);
    let eta = env.ledger().timestamp() + 3_600;
    let proposal_id = client
        .submit_proposal(&ProposalKind::RotateAdmin, &target, &None, &0, &5_000, &eta)
        ;

    // Admin (weight 100) votes yes.
    client.try_vote_proposal(&proposal_id, &true).unwrap();

    // Admin is removed after voting — the recorded vote must stop counting.
    client.try_remove_guardian(&admin, &admin).unwrap();
    assert_eq!(client.get_guardian_weight(&admin), 0);

    env.ledger().set_timestamp(eta + 100);
    let result = client.try_execute_proposal(&proposal_id);

    assert_eq!(
        result,
        Err(Ok(Error::InvalidInput)),
        "a removed guardian's prior vote must not satisfy quorum",
    );
    assert_eq!(client.get_admin(), admin);
    assert!(!client.get_proposal(&proposal_id).unwrap().executed);
}
