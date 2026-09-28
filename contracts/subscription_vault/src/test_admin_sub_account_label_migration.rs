//! Adversarial coverage for
//! `admin::rewrite_subscriptions_for_sub_account_label` (issue #1021).
//!
//! This function is the `v4 -> v5` migration step invoked from `admin::do_migrate`
//! (and therefore from the `migrate` entrypoint). It walks every
//! `DataKey::Sub(id)` record below `DataKey::NextId` and writes it back so the
//! newly added trailing `sub_account_label` field is persisted in the new XDR
//! encoding. It is a pure round-trip, so the properties that matter are:
//!
//! * it returns the number of records it actually rewrote (0 when there are
//!   none, and never more than the number of records that exist);
//! * ids with no record are skipped, not fabricated (sparse / gapped `NextId`);
//! * every field of every rewritten record is preserved bit-for-bit, including
//!   an `sub_account_label` that is already `Some(_)` and one that is `None`;
//! * a missing `NextId` counter is treated as zero (nothing to rewrite);
//! * it is idempotent;
//! * the admin-gated `migrate` entrypoint that reaches it rejects unauthorised
//!   callers without advancing the schema version or touching records.

#![cfg(test)]

use crate::test_utils::setup::TestEnv;
use crate::types::{DataKey, Error};
use soroban_sdk::{testutils::Address as _, Address, Symbol};

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;

/// Create an active subscription with no sub-account label.
fn create_sub(te: &TestEnv) -> u32 {
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    te.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<Symbol>,
    )
}

/// Overwrite a stored subscription's `sub_account_label` directly, simulating a
/// record written by an older binary.
fn set_label(te: &TestEnv, id: u32, label: Option<Symbol>) {
    let mut sub = te.client.get_subscription(&id);
    sub.sub_account_label = label;
    te.env.as_contract(&te.client.address, || {
        te.env.storage().persistent().set(&DataKey::Sub(id), &sub);
    });
}

/// Run the migration step under test in the contract's own context.
fn rewrite(te: &TestEnv) -> u32 {
    te.env.as_contract(&te.client.address, || {
        crate::admin::rewrite_subscriptions_for_sub_account_label(&te.env)
    })
}

/// Force the stored schema version (used to drive `migrate` through `v4 -> v5`).
fn set_schema_version(te: &TestEnv, version: u32) {
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .persistent()
            .set(&DataKey::SchemaVersion, &version);
    });
}

/// Read the stored schema version.
fn schema_version(te: &TestEnv) -> Option<u32> {
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .persistent()
            .get::<_, u32>(&DataKey::SchemaVersion)
    })
}

// ── Happy path ──────────────────────────────────────────────────────────────

#[test]
fn rewrite_rewrites_every_record_and_preserves_all_fields() {
    let te = TestEnv::default();
    let a = create_sub(&te);
    let b = create_sub(&te);
    let c = create_sub(&te);

    set_label(&te, a, None);
    set_label(&te, b, Some(Symbol::new(&te.env, "sub-alpha")));
    set_label(&te, c, Some(Symbol::new(&te.env, "sub-beta")));

    let before_a = te.client.get_subscription(&a);
    let before_b = te.client.get_subscription(&b);
    let before_c = te.client.get_subscription(&c);

    assert_eq!(rewrite(&te), 3);

    assert_eq!(te.client.get_subscription(&a), before_a);
    assert_eq!(te.client.get_subscription(&b), before_b);
    assert_eq!(te.client.get_subscription(&c), before_c);

    // An absent label stays absent; an existing label is not clobbered.
    assert_eq!(te.client.get_subscription(&a).sub_account_label, None);
    assert_eq!(
        te.client.get_subscription(&b).sub_account_label,
        Some(Symbol::new(&te.env, "sub-alpha"))
    );
    assert_eq!(
        te.client.get_subscription(&c).sub_account_label,
        Some(Symbol::new(&te.env, "sub-beta"))
    );
}

#[test]
fn rewrite_is_idempotent() {
    let te = TestEnv::default();
    let a = create_sub(&te);
    set_label(&te, a, Some(Symbol::new(&te.env, "sub-alpha")));
    let before = te.client.get_subscription(&a);

    assert_eq!(rewrite(&te), 1);
    assert_eq!(rewrite(&te), 1);

    assert_eq!(te.client.get_subscription(&a), before);
}

// ── Boundaries ──────────────────────────────────────────────────────────────

#[test]
fn rewrite_with_no_subscriptions_returns_zero() {
    let te = TestEnv::default();
    assert_eq!(rewrite(&te), 0);
    assert_eq!(schema_version(&te), Some(crate::STORAGE_VERSION));
}

#[test]
fn rewrite_skips_gaps_between_next_id_and_existing_records() {
    let te = TestEnv::default();
    let a = create_sub(&te);
    let b = create_sub(&te);

    // NextId 1_000 with only ids 0 and 1 populated: the scan must not
    // fabricate the 998 missing records nor count them as rewritten.
    crate::test_utils::fixtures::seed_counter(&te.env, &te.client.address, 1_000);
    assert_eq!(rewrite(&te), 2);

    te.env.as_contract(&te.client.address, || {
        for id in 0..1_000u32 {
            let present = te.env.storage().persistent().has(&DataKey::Sub(id));
            if id == a || id == b {
                assert!(present, "existing record {id} must survive the rewrite");
            } else {
                assert!(!present, "migration must not create record {id}");
            }
        }
    });
}

#[test]
fn rewrite_treats_a_missing_next_id_counter_as_zero() {
    let te = TestEnv::default();
    let a = create_sub(&te);
    let before = te.client.get_subscription(&a);

    te.env.as_contract(&te.client.address, || {
        crate::admin::remove_config(&te.env, &DataKey::NextId);
    });

    assert_eq!(rewrite(&te), 0);
    // Nothing was rewritten, but the existing record is untouched.
    assert_eq!(te.client.get_subscription(&a), before);
}

// ── Wiring through the admin-gated `migrate` entrypoint ─────────────────────

#[test]
fn migrate_from_v4_runs_the_label_rewrite_and_preserves_records() {
    let te = TestEnv::default();
    let a = create_sub(&te);
    let b = create_sub(&te);
    set_label(&te, b, Some(Symbol::new(&te.env, "sub-alpha")));

    let before_a = te.client.get_subscription(&a);
    let before_b = te.client.get_subscription(&b);

    // Simulate a live contract still on schema v4.
    set_schema_version(&te, 4);

    te.client.migrate(&te.admin);

    assert_eq!(schema_version(&te), Some(crate::STORAGE_VERSION));
    assert_eq!(te.client.get_subscription(&a), before_a);
    assert_eq!(te.client.get_subscription(&b), before_b);
    assert_eq!(
        te.client.get_subscription(&b).sub_account_label,
        Some(Symbol::new(&te.env, "sub-alpha"))
    );
}

#[test]
fn migrate_entrypoint_rejects_a_non_admin_and_preserves_records() {
    let te = TestEnv::default();
    let a = create_sub(&te);
    set_label(&te, a, Some(Symbol::new(&te.env, "sub-alpha")));
    let before = te.client.get_subscription(&a);

    set_schema_version(&te, 4);

    let stranger = Address::generate(&te.env);
    assert_eq!(te.client.try_migrate(&stranger), Err(Ok(Error::Forbidden)));

    // The rejected migration must not advance the schema version or touch a
    // single subscription record.
    assert_eq!(schema_version(&te), Some(4));
    assert_eq!(te.client.get_subscription(&a), before);
    assert_eq!(
        te.client.get_subscription(&a).sub_account_label,
        Some(Symbol::new(&te.env, "sub-alpha"))
    );
}
