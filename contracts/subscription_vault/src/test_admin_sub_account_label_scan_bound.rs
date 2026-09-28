//! Adversarial coverage for the *scan bound* of
//! `admin::rewrite_subscriptions_for_sub_account_label` (issue #1021).
//!
//! `test_admin_sub_account_label_migration` pins the field-level behaviour: the
//! rewrite preserves every field, keeps an existing `Some(label)`, is idempotent,
//! skips gaps *below* the counter, and is admin-gated through `migrate`.
//!
//! What none of those tests pin is which records the rewrite can even see. The
//! loop is
//!
//! ```text
//! let next_id: u32 = read_config(env, &DataKey::NextId).unwrap_or(0);
//! for id in 0..next_id { ... }
//! ```
//!
//! so the scan is bounded by the `NextId` **counter**, not by the records that
//! exist. A record stored at an id at or above `NextId` is invisible to the
//! migration and is left in the old XDR encoding — silently, because the function
//! still returns success and a plausible count. Every existing test has
//! `NextId > max(id)` (the counter is only ever read as "how many were created"),
//! so the bound was never exercised in the direction where it hides work.
//!
//! These tests fix the boundary from both sides: below the counter a record is
//! rewritten and counted, at or above it the record is left alone.

#![cfg(test)]

use crate::test_utils::fixtures::seed_counter;
use crate::test_utils::setup::TestEnv;
use crate::types::{DataKey, Subscription};
use soroban_sdk::{testutils::Address as _, Address, Symbol};

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;

/// Create an active subscription and return its id.
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

/// Store a subscription record directly at `id`, bypassing the counter.
///
/// This is how a record ends up at or above `NextId`: a restored backup, a
/// counter reset, or a record written by an older binary.
fn write_record_at(te: &TestEnv, id: u32, template: u32, label: Option<Symbol>) {
    // `Subscription` carries no id of its own — the id lives in `DataKey::Sub`.
    let mut sub: Subscription = te.client.get_subscription(&template);
    sub.sub_account_label = label;
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .persistent()
            .set(&DataKey::Sub(id), &sub);
    });
}

/// Run the migration step under test in the contract's own context.
fn rewrite(te: &TestEnv) -> u32 {
    te.env.as_contract(&te.client.address, || {
        crate::admin::rewrite_subscriptions_for_sub_account_label(&te.env)
    })
}

/// Read a record straight from storage, bypassing the client's bounds checks.
fn stored(te: &TestEnv, id: u32) -> Option<Subscription> {
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .persistent()
            .get::<_, Subscription>(&DataKey::Sub(id))
    })
}

/// Force the stored `NextId` counter.
fn set_counter(te: &TestEnv, value: u32) {
    seed_counter(&te.env, &te.client.address, value);
}

// ════════════════════════════════════════════════════════════════════
//  Records at or above the counter are not scanned
// ════════════════════════════════════════════════════════════════════

#[test]
fn a_record_at_exactly_next_id_is_not_rewritten_or_counted() {
    let te = TestEnv::default();
    let a = create_sub(&te);
    let b = create_sub(&te);
    assert_eq!(a, 0);
    assert_eq!(b, 1);

    // `NextId` is now 2, so a record stored at id 2 sits outside `0..2`.
    write_record_at(&te, 2, a, Some(Symbol::new(&te.env, "orphan")));
    let orphan_before = stored(&te, 2).expect("orphan record must exist");

    // Only the two in-range records are rewritten; the orphan is invisible.
    assert_eq!(rewrite(&te), 2);

    let orphan_after = stored(&te, 2).expect("the rewrite must not delete the orphan");
    assert_eq!(orphan_after, orphan_before);
    assert_eq!(
        orphan_after.sub_account_label,
        Some(Symbol::new(&te.env, "orphan"))
    );
}

#[test]
fn a_record_above_next_id_is_not_rewritten_or_counted() {
    let te = TestEnv::default();
    let a = create_sub(&te);

    // Counter deliberately below the record's id.
    write_record_at(&te, 7, a, None);
    let before = stored(&te, 7).expect("record must exist");

    assert_eq!(rewrite(&te), 1, "only id 0 is inside 0..1");

    assert_eq!(stored(&te, 7).expect("record must survive"), before);
}

#[test]
fn raising_the_counter_brings_the_orphan_into_the_scan() {
    let te = TestEnv::default();
    let a = create_sub(&te);
    write_record_at(&te, 2, a, None);

    // Out of range: not counted.
    assert_eq!(rewrite(&te), 1);

    // Bring it in range and it is now rewritten like any other record.
    set_counter(&te, 3);
    assert_eq!(
        rewrite(&te),
        2,
        "ids 0 and 2 are both inside 0..3 once the counter is raised"
    );
    assert!(stored(&te, 2).is_some());
}

#[test]
fn lowering_the_counter_hides_an_existing_record_from_the_rewrite() {
    let te = TestEnv::default();
    create_sub(&te);
    let second = create_sub(&te);
    let third = create_sub(&te);
    assert_eq!((second, third), (1, 2));

    // All three exist, but the counter claims only two were ever created.
    set_counter(&te, 2);
    let hidden_before = stored(&te, 2).expect("record 2 must exist");

    assert_eq!(rewrite(&te), 2, "id 2 is outside 0..2");

    assert_eq!(
        stored(&te, 2).expect("the rewrite must not delete it"),
        hidden_before
    );
}

#[test]
fn a_record_at_id_zero_is_skipped_when_the_counter_is_zero() {
    let te = TestEnv::default();
    let a = create_sub(&te);
    assert_eq!(a, 0);

    // An explicitly-zero counter makes `0..0` empty, even though a record exists.
    set_counter(&te, 0);
    let before = stored(&te, 0).expect("record 0 must exist");

    assert_eq!(rewrite(&te), 0);
    assert_eq!(stored(&te, 0).expect("record must survive"), before);
}

// ════════════════════════════════════════════════════════════════════
//  The admin-gated entrypoint inherits the same bound
// ════════════════════════════════════════════════════════════════════

#[test]
fn migrate_inherits_the_scan_bound_and_preserves_out_of_range_records() {
    let te = TestEnv::default();
    let a = create_sub(&te);
    let b = create_sub(&te);
    write_record_at(&te, 5, a, Some(Symbol::new(&te.env, "orphan")));

    let in_range_before = te.client.get_subscription(&b);
    let orphan_before = stored(&te, 5).expect("orphan must exist");

    // Drive `migrate` through `v4 -> v5`, the step that runs the rewrite.
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .persistent()
            .set(&DataKey::SchemaVersion, &4u32);
    });
    te.client.migrate(&te.admin);

    assert_eq!(te.client.get_subscription(&b), in_range_before);
    assert_eq!(
        stored(&te, 5).expect("migrate must not delete the orphan"),
        orphan_before,
        "a record above NextId is outside the v4->v5 rewrite"
    );
    assert_eq!(
        stored(&te, 5).unwrap().sub_account_label,
        Some(Symbol::new(&te.env, "orphan"))
    );
}
