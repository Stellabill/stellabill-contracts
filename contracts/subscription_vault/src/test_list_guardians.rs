#![cfg(test)]
//! # Adversarial coverage for `governance::list_guardians`
//!
//! `list_guardians` is the read surface for governance membership: it is used by
//! off-chain dashboards and by quorum accounting. The existing happy-path test
//! only adds two guardians and checks the length, so these tests pin the
//! behaviour that matters for correctness:
//!
//!   * the empty-result path (no guardians configured),
//!   * update-in-place semantics (re-adding must replace, not duplicate),
//!   * removal and idempotent removal,
//!   * rejected mutations (`weight == 0`, non-admin caller) leaving the listing
//!     and existing weights untouched,
//!   * the `u32` weight boundary,
//!   * that listing is a pure read.

use crate::types::Error;
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, Vec};

fn init_vault<'a>(env: &'a Env, admin: &Address) -> (Address, SubscriptionVaultClient<'a>) {
    let token_admin = Address::generate(env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);

    client.init(
        &token_address,
        &6, // decimals
        admin,
        &10_000_000, // min_topup
        &86400,      // grace period
    );

    (token_address, client)
}

/// Order-independent lookup: Soroban `Map` iteration order is not part of the
/// public contract, so assertions must never depend on it.
fn listed_weight(guardians: &Vec<(Address, u32)>, guardian: &Address) -> Option<u32> {
    for i in 0..guardians.len() {
        let (address, weight) = guardians.get(i).unwrap();
        if &address == guardian {
            return Some(weight);
        }
    }
    None
}

// ── Empty-result path ──────────────────────────────────────────────────────

#[test]
fn list_guardians_is_empty_before_any_guardian_is_configured() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    let guardians = client.list_guardians();
    assert_eq!(guardians.len(), 0);
}

#[test]
fn list_guardians_returns_to_empty_after_every_guardian_is_removed() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let guardian = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    client.add_guardian(&admin, &guardian, &10);
    assert_eq!(client.list_guardians().len(), 1);

    client.remove_guardian(&admin, &guardian);
    assert_eq!(client.list_guardians().len(), 0);
    assert_eq!(client.get_guardian_weight(&guardian), 0);
}

// ── Happy path, harder inputs ──────────────────────────────────────────────

#[test]
fn list_guardians_reports_every_guardian_with_its_weight() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    let g3 = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    client.add_guardian(&admin, &g1, &100);
    client.add_guardian(&admin, &g2, &50);
    client.add_guardian(&admin, &g3, &7);

    let guardians = client.list_guardians();
    assert_eq!(guardians.len(), 3);
    assert_eq!(listed_weight(&guardians, &g1), Some(100));
    assert_eq!(listed_weight(&guardians, &g2), Some(50));
    assert_eq!(listed_weight(&guardians, &g3), Some(7));
}

#[test]
fn re_adding_a_guardian_replaces_the_weight_without_duplicating() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let guardian = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    client.add_guardian(&admin, &guardian, &10);
    client.add_guardian(&admin, &guardian, &25);

    let guardians = client.list_guardians();
    assert_eq!(guardians.len(), 1, "re-adding must not duplicate the entry");
    assert_eq!(listed_weight(&guardians, &guardian), Some(25));
}

#[test]
fn updating_one_guardian_weight_leaves_other_guardians_untouched() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    client.add_guardian(&admin, &g1, &1);
    client.add_guardian(&admin, &g2, &2);
    client.add_guardian(&admin, &g1, &5);

    let guardians = client.list_guardians();
    assert_eq!(guardians.len(), 2);
    assert_eq!(listed_weight(&guardians, &g1), Some(5));
    assert_eq!(listed_weight(&guardians, &g2), Some(2));
}

#[test]
fn max_u32_weight_is_listed_verbatim() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let guardian = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    client.add_guardian(&admin, &guardian, &u32::MAX);

    let guardians = client.list_guardians();
    assert_eq!(guardians.len(), 1);
    assert_eq!(listed_weight(&guardians, &guardian), Some(u32::MAX));
}

// ── Removal ────────────────────────────────────────────────────────────────

#[test]
fn removing_a_guardian_drops_only_that_guardian() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let removed = Address::generate(&env);
    let kept = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    client.add_guardian(&admin, &removed, &3);
    client.add_guardian(&admin, &kept, &9);
    client.remove_guardian(&admin, &removed);

    let guardians = client.list_guardians();
    assert_eq!(guardians.len(), 1);
    assert_eq!(listed_weight(&guardians, &removed), None);
    assert_eq!(listed_weight(&guardians, &kept), Some(9));
}

#[test]
fn removing_an_unknown_guardian_is_a_noop() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let guardian = Address::generate(&env);
    let stranger = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    client.add_guardian(&admin, &guardian, &12);
    client.remove_guardian(&admin, &stranger);

    let guardians = client.list_guardians();
    assert_eq!(guardians.len(), 1);
    assert_eq!(listed_weight(&guardians, &guardian), Some(12));
}

// ── Rejected mutations leave the listing unchanged ─────────────────────────

#[test]
fn zero_weight_is_rejected_and_adds_nothing_to_the_listing() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let guardian = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    let result = client.try_add_guardian(&admin, &guardian, &0);
    assert_eq!(result, Err(Ok(Error::InvalidInput)));

    assert_eq!(client.list_guardians().len(), 0);
    assert_eq!(client.get_guardian_weight(&guardian), 0);
}

#[test]
fn rejected_zero_weight_update_keeps_the_previous_weight() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let guardian = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    client.add_guardian(&admin, &guardian, &42);
    let result = client.try_add_guardian(&admin, &guardian, &0);
    assert_eq!(result, Err(Ok(Error::InvalidInput)));

    let guardians = client.list_guardians();
    assert_eq!(guardians.len(), 1);
    assert_eq!(listed_weight(&guardians, &guardian), Some(42));
}

#[test]
fn non_admin_cannot_mutate_guardians_and_the_listing_is_unchanged() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let guardian = Address::generate(&env);
    let stranger = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    client.add_guardian(&admin, &guardian, &8);

    // `require_admin_auth` rejects any caller that is not the stored admin.
    assert_eq!(
        client.try_add_guardian(&stranger, &stranger, &99),
        Err(Ok(Error::Forbidden))
    );
    assert_eq!(
        client.try_remove_guardian(&stranger, &guardian),
        Err(Ok(Error::Forbidden))
    );

    let guardians = client.list_guardians();
    assert_eq!(guardians.len(), 1);
    assert_eq!(listed_weight(&guardians, &guardian), Some(8));
    assert_eq!(listed_weight(&guardians, &stranger), None);
}

// ── Purity ─────────────────────────────────────────────────────────────────

#[test]
fn list_guardians_is_a_pure_read() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let guardian = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    client.add_guardian(&admin, &guardian, &4);

    let first = client.list_guardians();
    let second = client.list_guardians();

    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(listed_weight(&second, &guardian), Some(4));
    // Repeated reads never accumulate entries.
    assert_eq!(client.get_guardian_weight(&guardian), 4);
}
