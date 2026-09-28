//! Adversarial coverage for `admin::require_admin_or_operator_auth` (issue #982).
//!
//! `require_admin_or_operator_auth` is the single privileged-caller guard shared
//! by the bulk pause / cancel / deposit entrypoints (via
//! `subscription::bulk_precheck`), and it is the only place where the operator
//! role is widened beyond operator batch charging. That makes its negative paths
//! security-relevant:
//!
//! 1. `caller.require_auth()` runs first, so an unauthenticated caller panics
//!    with `Error(Auth, InvalidAction)` before any identity comparison;
//! 2. the stored admin is always accepted;
//! 3. the stored operator is accepted only while it is still configured;
//! 4. every other address is rejected with `Error::Unauthorized` (1001);
//! 5. a rejected call mutates nothing and emits no events;
//! 6. with no stored admin the guard reports `Error::NotInitialized` (2002).

#![cfg(test)]

use crate::test_utils::setup::TestEnv;
use crate::types::{DataKey, Error};
use soroban_sdk::{
    testutils::{Address as _, Events as _},
    Address, Env,
};

/// Read `DataKey::Admin` straight from contract storage (either tier).
fn stored_admin(env: &Env, contract: &Address) -> Option<Address> {
    env.as_contract(contract, || {
        crate::admin::read_config::<Address>(env, &DataKey::Admin)
    })
}

/// Read `DataKey::Operator` straight from contract storage (either tier).
fn stored_operator(env: &Env, contract: &Address) -> Option<Address> {
    env.as_contract(contract, || {
        crate::admin::read_config::<Address>(env, &DataKey::Operator)
    })
}

/// Invoke the guard exactly the way `subscription::bulk_precheck` does.
fn check(env: &Env, contract: &Address, caller: &Address) -> Result<(), Error> {
    env.as_contract(contract, || {
        crate::admin::require_admin_or_operator_auth(env, caller)
    })
}

// ── Happy paths ─────────────────────────────────────────────────────────────

#[test]
fn stored_admin_is_authorized() {
    let te = TestEnv::default();
    let caller = te.admin.clone();
    assert_eq!(check(&te.env, &te.client.address, &caller), Ok(()));
}

#[test]
fn stored_admin_is_authorized_when_no_operator_is_configured() {
    let te = TestEnv::default();
    assert_eq!(te.client.get_operator(), None);
    assert_eq!(check(&te.env, &te.client.address, &te.admin.clone()), Ok(()));
}

#[test]
fn stored_operator_is_authorized() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);

    assert_eq!(check(&te.env, &te.client.address, &operator), Ok(()));
}

#[test]
fn admin_also_configured_as_operator_is_authorized() {
    let te = TestEnv::default();
    te.client.set_operator(&te.admin, &te.admin.clone());

    assert_eq!(check(&te.env, &te.client.address, &te.admin.clone()), Ok(()));
}

// ── Rejection paths ─────────────────────────────────────────────────────────

#[test]
fn stranger_is_rejected_and_state_is_untouched() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);

    let stranger = Address::generate(&te.env);
    let admin_before = stored_admin(&te.env, &te.client.address);
    let operator_before = stored_operator(&te.env, &te.client.address);
    let events_before = te.env.events().all().len();

    assert_eq!(
        check(&te.env, &te.client.address, &stranger),
        Err(Error::Unauthorized)
    );

    // The guard is read-only: it must not touch the admin/operator slots and
    // must not emit an event for a rejected caller.
    assert_eq!(stored_admin(&te.env, &te.client.address), admin_before);
    assert_eq!(stored_operator(&te.env, &te.client.address), operator_before);
    assert_eq!(te.client.get_operator(), Some(operator));
    assert_eq!(te.env.events().all().len(), events_before);
}

#[test]
fn stranger_is_rejected_when_no_operator_is_configured() {
    let te = TestEnv::default();
    let stranger = Address::generate(&te.env);
    assert_eq!(
        check(&te.env, &te.client.address, &stranger),
        Err(Error::Unauthorized)
    );
}

#[test]
fn contract_address_is_not_privileged() {
    let te = TestEnv::default();
    let caller = te.client.address.clone();
    assert_eq!(
        check(&te.env, &te.client.address, &caller),
        Err(Error::Unauthorized)
    );
}

#[test]
fn removed_operator_loses_access_while_admin_keeps_it() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);
    assert_eq!(check(&te.env, &te.client.address, &operator), Ok(()));

    te.client.remove_operator(&te.admin);
    assert_eq!(te.client.get_operator(), None);

    // The role is revoked by storage state, not cached: the very same address
    // is now a stranger.
    assert_eq!(
        check(&te.env, &te.client.address, &operator),
        Err(Error::Unauthorized)
    );
    assert_eq!(check(&te.env, &te.client.address, &te.admin.clone()), Ok(()));
}

#[test]
fn old_admin_is_rejected_after_rotation_and_operator_is_unaffected() {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);

    let new_admin = Address::generate(&te.env);
    te.client.rotate_admin(&te.admin, &new_admin, &0u64);

    assert_eq!(
        check(&te.env, &te.client.address, &te.admin.clone()),
        Err(Error::Unauthorized)
    );
    assert_eq!(check(&te.env, &te.client.address, &new_admin), Ok(()));
    assert_eq!(check(&te.env, &te.client.address, &operator), Ok(()));
}

#[test]
fn not_initialized_reports_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let caller = Address::generate(&env);

    let res = env.as_contract(&contract_id, || {
        crate::admin::require_admin_or_operator_auth(&env, &caller)
    });
    assert_eq!(res, Err(Error::NotInitialized));
}

// ── Auth ordering ───────────────────────────────────────────────────────────

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn unauthenticated_stored_operator_panics_before_identity_check() {
    // No `mock_all_auths`: `caller.require_auth()` is the guard's first
    // statement, so it must fire even though `caller` *is* the stored operator
    // and would otherwise be accepted.
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let client = crate::SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let _ = client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    // Seed the operator directly; `set_operator` itself requires auth.
    let operator = Address::generate(&env);
    env.as_contract(&contract_id, || {
        crate::admin::write_config(&env, &DataKey::Operator, &operator);
    });

    let _ = env.as_contract(&contract_id, || {
        crate::admin::require_admin_or_operator_auth(&env, &operator)
    });
}
