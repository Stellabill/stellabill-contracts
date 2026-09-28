//! Adversarial coverage for the *scope* of `admin::require_admin_or_operator_auth`
//! (issue #982).
//!
//! `test_admin_privileged_auth` already pins the guard's own decision table
//! (admin, operator, stranger, rotation, missing operator). What no test covered
//! is the property the function's documentation actually claims:
//!
//! > This deliberately does **not** widen any other surface: the operator still
//! > has no access to fund withdrawal, admin rotation, or governance.
//!
//! That claim is what makes the operator role safe to hand out, and it lived
//! only in a doc comment. These tests hold the guard and the surfaces it must not
//! widen to the same standard: for every admin-only entrypoint, the configured
//! operator **passes** `require_admin_or_operator_auth` and is still refused by
//! the entrypoint itself, with the surrounding state left intact. The admin is
//! run against the same entrypoints as a control, so a row cannot pass merely
//! because the entrypoint is inert.

#![cfg(test)]

use crate::test_utils::setup::TestEnv;
use crate::types::{Error, TreasurySplitEntry};
use soroban_sdk::{testutils::Address as _, Address, Vec};

const NEW_MIN_TOPUP: i128 = 5_000_000;

/// Setup: initialise the vault and install a distinct operator.
fn setup_with_operator() -> (TestEnv, Address) {
    let te = TestEnv::default();
    let operator = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);
    assert_eq!(
        te.client.get_operator(),
        Some(operator.clone()),
        "fixture must actually install the operator"
    );
    (te, operator)
}

/// The guard under test, invoked inside the contract's own context.
fn guard(te: &TestEnv, caller: &Address) -> Result<(), Error> {
    te.env
        .as_contract(&te.client.address, || {
            crate::admin::require_admin_or_operator_auth(&te.env, caller)
        })
}

/// A valid single-beneficiary split that satisfies the 10_000 bps sum rule.
fn valid_split(te: &TestEnv) -> Vec<TreasurySplitEntry> {
    let mut entries = Vec::new(&te.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: Address::generate(&te.env),
        bps: 10_000,
    });
    entries
}

// ════════════════════════════════════════════════════════════════════
//  The positive control the whole file rests on
// ════════════════════════════════════════════════════════════════════

#[test]
fn the_operator_passes_the_guard_itself() {
    let (te, operator) = setup_with_operator();

    // Without this row, every rejection below could be explained by the operator
    // never being privileged in the first place.
    assert_eq!(guard(&te, &operator), Ok(()));
    assert_eq!(guard(&te, &te.admin), Ok(()));
}

// ════════════════════════════════════════════════════════════════════
//  Surfaces the guard must not widen
// ════════════════════════════════════════════════════════════════════

#[test]
fn operator_is_refused_by_set_min_topup() {
    let (te, operator) = setup_with_operator();
    let before = te.client.get_min_topup();

    assert_eq!(
        te.client.try_set_min_topup(&operator, &NEW_MIN_TOPUP),
        Err(Ok(Error::Forbidden))
    );
    assert_eq!(te.client.get_min_topup(), before);
}

#[test]
fn operator_is_refused_by_set_operator() {
    let (te, operator) = setup_with_operator();
    let replacement = Address::generate(&te.env);

    // An operator that could appoint its own successor would be able to entrench
    // itself past the admin's control.
    assert_eq!(
        te.client.try_set_operator(&operator, &replacement),
        Err(Ok(Error::Forbidden))
    );
    assert_eq!(te.client.get_operator(), Some(operator));
}

#[test]
fn operator_is_refused_by_remove_operator() {
    let (te, operator) = setup_with_operator();

    assert_eq!(
        te.client.try_remove_operator(&operator),
        Err(Ok(Error::Forbidden))
    );
    assert_eq!(te.client.get_operator(), Some(operator));
}

#[test]
fn operator_is_refused_by_propose_admin() {
    let (te, operator) = setup_with_operator();
    let replacement = Address::generate(&te.env);
    let admin_before = te.client.get_admin();

    // `do_propose_admin` reports a non-admin proposer as `Unauthorized` (it
    // compares against the stored admin directly rather than going through
    // `require_admin_auth`).
    assert_eq!(
        te.client.try_propose_admin(&operator, &replacement),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(te.client.get_admin(), admin_before);
}

#[test]
fn operator_is_refused_by_set_treasury_split() {
    let (te, operator) = setup_with_operator();
    let before = te.client.get_treasury_split();

    assert_eq!(
        te.client.try_set_treasury_split(&operator, &valid_split(&te)),
        Err(Ok(Error::Forbidden))
    );
    assert_eq!(te.client.get_treasury_split(), before);
}

#[test]
fn operator_is_refused_by_clear_treasury_split() {
    let (te, operator) = setup_with_operator();
    te.client.set_treasury_split(&te.admin, &valid_split(&te));
    let before = te.client.get_treasury_split();

    assert_eq!(
        te.client.try_clear_treasury_split(&operator),
        Err(Ok(Error::Forbidden))
    );
    assert_eq!(te.client.get_treasury_split(), before);
}

#[test]
fn operator_is_refused_by_migrate() {
    let (te, operator) = setup_with_operator();

    assert_eq!(
        te.client.try_migrate(&operator),
        Err(Ok(Error::Forbidden))
    );
}

#[test]
fn operator_is_refused_by_migrate_config_to_persistent() {
    let (te, operator) = setup_with_operator();

    assert_eq!(
        te.client.try_migrate_config_to_persistent(&operator),
        Err(Ok(Error::Forbidden))
    );
}

// ════════════════════════════════════════════════════════════════════
//  Control: the admin is accepted by the same surfaces
// ════════════════════════════════════════════════════════════════════

#[test]
fn admin_is_accepted_by_every_surface_the_operator_is_denied() {
    let te = TestEnv::default();

    // Governance / admin rotation.
    let replacement = Address::generate(&te.env);
    assert_eq!(te.client.try_propose_admin(&te.admin, &replacement), Ok(()));

    // Operator management.
    let operator = Address::generate(&te.env);
    assert_eq!(te.client.try_set_operator(&te.admin, &operator), Ok(()));
    assert_eq!(te.client.get_operator(), Some(operator.clone()));

    // Config.
    assert_eq!(te.client.try_set_min_topup(&te.admin, &NEW_MIN_TOPUP), Ok(()));
    assert_eq!(te.client.get_min_topup(), Ok(NEW_MIN_TOPUP));

    // Treasury split, then the migration entrypoints.
    assert_eq!(
        te.client.try_set_treasury_split(&te.admin, &valid_split(&te)),
        Ok(())
    );
    assert!(te.client.get_treasury_split().is_some());

    assert_eq!(te.client.try_migrate(&te.admin), Ok(()));
    assert_eq!(
        te.client.try_migrate_config_to_persistent(&te.admin),
        Ok(())
    );
}

// ════════════════════════════════════════════════════════════════════
//  Removing the operator restores the admin-only decision for the guard
// ════════════════════════════════════════════════════════════════════

#[test]
fn the_guard_stops_authorizing_once_the_operator_is_removed() {
    let (te, operator) = setup_with_operator();
    assert_eq!(guard(&te, &operator), Ok(()));

    // `remove_operator` is itself admin-only and cooldown-gated.
    te.jump(crate::admin::CONFIG_COOLDOWN_SECS);
    assert_eq!(te.client.try_remove_operator(&te.admin), Ok(()));
    assert_eq!(te.client.get_operator(), None);

    assert_eq!(guard(&te, &operator), Err(Error::Unauthorized));
    // The admin keeps the surface, and the operator can no longer reach it.
    assert_eq!(guard(&te, &te.admin), Ok(()));
    assert_eq!(
        te.client.try_set_min_topup(&operator, &NEW_MIN_TOPUP),
        Err(Ok(Error::Forbidden))
    );
}
