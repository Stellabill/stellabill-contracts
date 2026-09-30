#![cfg(test)]

//! Adversarial coverage for `require_admin_auth`
//! (`contracts/subscription_vault/src/admin.rs`).
//!
//! `require_admin_auth` is the single choke point that every privileged admin
//! entrypoint funnels through, so its failure modes are load-bearing. These
//! tests pin:
//!
//! * the stored admin is accepted,
//! * any other authenticated caller is `Forbidden` (1002) — deliberately
//!   *not* `Unauthorized` (1001), so callers can distinguish "not the admin"
//!   from "no credentials",
//! * the check reads the stored admin rather than trusting the supplied address,
//! * an uninitialised contract reports `NotInitialized` instead of silently
//!   accepting the caller,
//! * rotation revokes the previous admin's authority and grants it to the new
//!   admin,
//! * a caller without a valid signature is rejected by the host before the
//!   stored admin is ever compared.

extern crate std;

use crate::admin::require_admin_auth;
use crate::test_utils::{create_test_client, setup_env};
use crate::types::Error;
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{testutils::Address as _, Address, Env};

// ── Accepting the stored admin ────────────────────────────────────────────────

#[test]
fn require_admin_auth_accepts_the_stored_admin() {
    let env = setup_env();
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let client = create_test_client(&env, &admin, &token);

    let res = env.as_contract(&client.address, || require_admin_auth(&env, &admin));
    assert!(res.is_ok());
}

// ── Rejecting everyone else ───────────────────────────────────────────────────

#[test]
fn require_admin_auth_rejects_a_non_admin_with_forbidden() {
    let env = setup_env();
    let admin = Address::generate(&env);
    let stranger = Address::generate(&env);
    let token = Address::generate(&env);
    let client = create_test_client(&env, &admin, &token);

    let res = env.as_contract(&client.address, || require_admin_auth(&env, &stranger));
    assert_eq!(res.err().unwrap().to_code(), Error::Forbidden.to_code());
}

#[test]
fn forbidden_is_not_unauthorized() {
    // The distinction matters: `Unauthorized` means "no/ bad credentials",
    // `Forbidden` means "authenticated but not the admin". Collapsing them
    // would make the two indistinguishable to callers and indexers.
    assert_ne!(Error::Forbidden.to_code(), Error::Unauthorized.to_code());
    assert_eq!(Error::Forbidden.to_code(), 1002);
    assert_eq!(Error::Unauthorized.to_code(), 1001);
}

#[test]
fn require_admin_auth_compares_against_storage_not_the_argument() {
    let env = setup_env();
    let admin = Address::generate(&env);
    let stranger = Address::generate(&env);
    let token = Address::generate(&env);
    let client = create_test_client(&env, &admin, &token);

    // Same contract, same mocked auth: only the supplied address differs.
    assert!(env
        .as_contract(&client.address, || require_admin_auth(&env, &stranger))
        .is_err());
    assert!(env
        .as_contract(&client.address, || require_admin_auth(&env, &admin))
        .is_ok());
}

#[test]
fn require_admin_auth_on_uninitialized_contract_is_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let caller = Address::generate(&env);

    let res = env.as_contract(&contract_id, || require_admin_auth(&env, &caller));
    assert_eq!(res.err().unwrap().to_code(), Error::NotInitialized.to_code());
}

// ── Rotation ──────────────────────────────────────────────────────────────────

#[test]
fn rotation_revokes_the_previous_admin_and_accepts_the_new_one() {
    let env = setup_env();
    let old_admin = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let token = Address::generate(&env);
    let client = create_test_client(&env, &old_admin, &token);

    // Nonce for the admin-rotation domain (fresh env ⇒ 0).
    let nonce = client.get_admin_nonce(&old_admin, &crate::nonce::DOMAIN_ADMIN_ROTATION);
    client
        .mock_all_auths()
        .rotate_admin(&old_admin, &new_admin, &nonce);

    let old_res = env.as_contract(&client.address, || require_admin_auth(&env, &old_admin));
    assert_eq!(
        old_res.err().unwrap().to_code(),
        Error::Forbidden.to_code(),
        "the rotated-out admin must lose authority immediately"
    );

    let new_res = env.as_contract(&client.address, || require_admin_auth(&env, &new_admin));
    assert!(
        new_res.is_ok(),
        "the rotated-in admin must be authorised immediately"
    );
}

// ── Missing signature ─────────────────────────────────────────────────────────

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn require_admin_auth_without_a_signature_is_rejected_by_the_host() {
    // No `mock_all_auths`: `init` does not require auth, but
    // `require_admin_auth(&stored_admin)` must.
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    env.as_contract(&contract_id, || {
        let _ = require_admin_auth(&env, &admin);
    });
}
