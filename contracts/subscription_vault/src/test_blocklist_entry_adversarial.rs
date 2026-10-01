#![cfg(test)]

//! # Adversarial coverage: `blocklist::get_blocklist_entry`
//!
//! `get_blocklist_entry` is the audit-facing read of the blocklist table: it is
//! what maintainers, indexers and dispute reviewers use to explain *why* an
//! address is frozen. The existing `test.rs` blocklist suite covers the
//! enforcement edges; this suite attacks the read/record contract itself:
//!
//! * a never-blocked address returns `NotFound` (not an empty entry),
//! * a removed address is truly deleted — the read reports `NotFound` again,
//! * every field round-trips exactly, including `None` vs `Some("")` reasons,
//! * rejected mutations (duplicate add, non-admin add/remove) leave the stored
//!   record byte-identical, so the audit trail cannot be silently rewritten,
//! * the table is keyed per address and the read is side-effect free,
//! * re-adding after a removal issues a fresh record with the later timestamp,
//! * the record survives the persistent-storage TTL window, so a long-blocked
//!   address cannot be silently unfrozen (or wedge the read path) by archival.

extern crate std;

use crate::test_utils::setup::TestEnv;
use crate::{BlocklistEntry, Error};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, String,
};

const DAY: u64 = 24 * 60 * 60;
/// Mirrors `types::SUB_TTL_EXTEND_TO` (365 days) — the window a persistent
/// record must stay live for.
const SUB_TTL_EXTEND_TO: u32 = 365 * 24 * 60 * 60;

fn reason(te: &TestEnv, s: &str) -> String {
    String::from_str(&te.env, s)
}

// ────────────────────────────────────────────────────────────────────
//  Absent / deleted records
// ────────────────────────────────────────────────────────────────────

#[test]
fn never_blocked_address_reads_not_found() {
    let te = TestEnv::default();
    let stranger = Address::generate(&te.env);

    let result = te.client.try_get_blocklist_entry(&stranger);

    assert!(matches!(result, Err(Ok(Error::NotFound))));
    assert!(!te.client.is_blocklisted(&stranger));
}

#[test]
fn removed_address_is_deleted_not_flagged() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);

    te.client
        .add_to_blocklist(&te.admin, &subscriber, &None::<String>);
    assert!(te.client.is_blocklisted(&subscriber));

    te.client.remove_from_blocklist(&te.admin, &subscriber);

    assert!(matches!(
        te.client.try_get_blocklist_entry(&subscriber),
        Err(Ok(Error::NotFound))
    ));
    assert!(!te.client.is_blocklisted(&subscriber));
}

#[test]
fn removing_a_non_blocked_address_leaves_other_records_intact() {
    let te = TestEnv::default();
    let blocked = Address::generate(&te.env);
    let untouched = Address::generate(&te.env);

    te.client
        .add_to_blocklist(&te.admin, &blocked, &Some(reason(&te, "fraud")));
    let before = te.client.get_blocklist_entry(&blocked);

    let result = te.client.try_remove_from_blocklist(&te.admin, &untouched);
    assert!(matches!(result, Err(Ok(Error::NotFound))));

    let after = te.client.get_blocklist_entry(&blocked);
    assert_eq!(after.subscriber, before.subscriber);
    assert_eq!(after.added_by, before.added_by);
    assert_eq!(after.added_at, before.added_at);
    assert_eq!(after.reason, before.reason);
}

// ────────────────────────────────────────────────────────────────────
//  Exact record round-trip
// ────────────────────────────────────────────────────────────────────

#[test]
fn entry_round_trips_every_field_at_the_exact_ledger_time() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let text = "chargeback fraud 2026-09-28";

    te.env.ledger().with_mut(|li| li.timestamp = 1_234_567);
    te.client
        .add_to_blocklist(&te.admin, &subscriber, &Some(reason(&te, text)));

    let entry: BlocklistEntry = te.client.get_blocklist_entry(&subscriber);
    assert_eq!(entry.subscriber, subscriber);
    assert_eq!(entry.added_by, te.admin);
    assert_eq!(entry.added_at, 1_234_567);
    assert_eq!(entry.reason, Some(reason(&te, text)));
}

#[test]
fn none_and_empty_reason_are_distinguishable() {
    let te = TestEnv::default();
    let no_reason = Address::generate(&te.env);
    let empty_reason = Address::generate(&te.env);

    te.client
        .add_to_blocklist(&te.admin, &no_reason, &None::<String>);
    te.client
        .add_to_blocklist(&te.admin, &empty_reason, &Some(reason(&te, "")));

    let a = te.client.get_blocklist_entry(&no_reason);
    let b = te.client.get_blocklist_entry(&empty_reason);

    assert_eq!(a.reason, None, "an omitted reason must stay absent");
    assert_eq!(
        b.reason,
        Some(reason(&te, "")),
        "an explicit empty reason must not be normalised away"
    );
    assert!(a.reason != b.reason);
}

#[test]
fn long_reason_is_stored_verbatim() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let long: std::string::String = "x".repeat(512);

    te.client
        .add_to_blocklist(&te.admin, &subscriber, &Some(reason(&te, &long)));

    let entry = te.client.get_blocklist_entry(&subscriber);
    assert_eq!(entry.reason, Some(reason(&te, &long)));
    assert_eq!(entry.reason.unwrap().len(), 512);
}

// ────────────────────────────────────────────────────────────────────
//  Rejected mutations must not rewrite the record
// ────────────────────────────────────────────────────────────────────

#[test]
fn duplicate_add_is_rejected_and_cannot_overwrite_the_audit_trail() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);

    te.env.ledger().with_mut(|li| li.timestamp = 1_000);
    te.client.add_to_blocklist(
        &te.admin,
        &subscriber,
        &Some(reason(&te, "original reason")),
    );
    let before = te.client.get_blocklist_entry(&subscriber);

    // Much later, a second attempt with a different reason.
    te.env.ledger().with_mut(|li| li.timestamp = 9_999_999);
    let result = te.client.try_add_to_blocklist(
        &te.admin,
        &subscriber,
        &Some(reason(&te, "rewritten reason")),
    );
    assert!(matches!(result, Err(Ok(Error::InvalidInput))));

    let after = te.client.get_blocklist_entry(&subscriber);
    assert_eq!(after.added_at, before.added_at, "added_at must not move");
    assert_eq!(after.added_by, before.added_by);
    assert_eq!(after.reason, before.reason, "the reason must not be rewritten");
}

#[test]
fn non_admin_add_is_forbidden_and_writes_nothing() {
    let te = TestEnv::default();
    let stranger = Address::generate(&te.env);
    let target = Address::generate(&te.env);

    let result = te.client.try_add_to_blocklist(
        &stranger,
        &target,
        &Some(reason(&te, "unauthorised")),
    );
    assert!(matches!(result, Err(Ok(Error::Forbidden))));

    assert!(matches!(
        te.client.try_get_blocklist_entry(&target),
        Err(Ok(Error::NotFound))
    ));
    assert!(!te.client.is_blocklisted(&target));
}

#[test]
fn non_admin_remove_is_forbidden_and_keeps_the_record() {
    let te = TestEnv::default();
    let stranger = Address::generate(&te.env);
    let subscriber = Address::generate(&te.env);

    te.client.add_to_blocklist(
        &te.admin,
        &subscriber,
        &Some(reason(&te, "keep me")),
    );
    let before = te.client.get_blocklist_entry(&subscriber);

    let result = te.client.try_remove_from_blocklist(&stranger, &subscriber);
    assert!(matches!(result, Err(Ok(Error::Forbidden))));

    let after = te.client.get_blocklist_entry(&subscriber);
    assert_eq!(after.added_by, before.added_by);
    assert_eq!(after.added_at, before.added_at);
    assert_eq!(after.reason, before.reason);
    assert!(te.client.is_blocklisted(&subscriber));
}

// ────────────────────────────────────────────────────────────────────
//  Read purity, per-address keying, and re-add
// ────────────────────────────────────────────────────────────────────

#[test]
fn repeated_reads_are_stable_and_side_effect_free() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);

    te.env.ledger().with_mut(|li| li.timestamp = 777);
    te.client
        .add_to_blocklist(&te.admin, &subscriber, &Some(reason(&te, "stable")));

    let first = te.client.get_blocklist_entry(&subscriber);
    for step in 0..5u64 {
        te.env.ledger().with_mut(|li| li.timestamp = 777 + step * DAY);
        let again = te.client.get_blocklist_entry(&subscriber);
        assert_eq!(again.added_at, first.added_at, "reads must not restamp the entry");
        assert_eq!(again.reason, first.reason);
        assert_eq!(again.added_by, first.added_by);
    }
}

#[test]
fn the_table_is_keyed_per_address() {
    let te = TestEnv::default();
    let a = Address::generate(&te.env);
    let b = Address::generate(&te.env);

    te.client.add_to_blocklist(&te.admin, &a, &Some(reason(&te, "a")));

    assert_eq!(te.client.get_blocklist_entry(&a).subscriber, a);
    assert!(matches!(
        te.client.try_get_blocklist_entry(&b),
        Err(Ok(Error::NotFound))
    ));
    assert!(te.client.is_blocklisted(&a));
    assert!(!te.client.is_blocklisted(&b));
}

#[test]
fn re_add_after_removal_issues_a_fresh_record() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);

    te.env.ledger().with_mut(|li| li.timestamp = 100);
    te.client
        .add_to_blocklist(&te.admin, &subscriber, &Some(reason(&te, "first")));
    te.client.remove_from_blocklist(&te.admin, &subscriber);
    assert!(matches!(
        te.client.try_get_blocklist_entry(&subscriber),
        Err(Ok(Error::NotFound))
    ));

    te.env.ledger().with_mut(|li| li.timestamp = 500);
    te.client
        .add_to_blocklist(&te.admin, &subscriber, &Some(reason(&te, "second")));

    let entry = te.client.get_blocklist_entry(&subscriber);
    assert_eq!(entry.added_at, 500, "the new record must carry the new timestamp");
    assert_eq!(entry.reason, Some(reason(&te, "second")));
    assert!(te.client.is_blocklisted(&subscriber));
}

// ────────────────────────────────────────────────────────────────────
//  Persistent TTL: the record must outlive the block window
// ────────────────────────────────────────────────────────────────────

/// A blocklist entry is written to persistent storage. Without an explicit TTL
/// extension it only lives for the host default, after which the read path
/// traps on an archived entry — silently lifting the block (or wedging every
/// operation that consults it). The record must still be live one ledger
/// inside the documented subscription TTL window.
#[test]
fn entry_outlives_the_default_persistent_ttl_window() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);

    te.client.add_to_blocklist(
        &te.admin,
        &subscriber,
        &Some(reason(&te, "long-term fraud block")),
    );

    // Advance to one ledger inside the extended window.
    te.env
        .ledger()
        .with_mut(|li| li.sequence_number = SUB_TTL_EXTEND_TO - 1);

    assert!(
        te.client.is_blocklisted(&subscriber),
        "the block must still be in force inside the TTL window"
    );
    let entry = te.client.get_blocklist_entry(&subscriber);
    assert_eq!(entry.reason, Some(reason(&te, "long-term fraud block")));
}

/// The write path must refresh the TTL of an existing record too, so a
/// re-block after a long quiet period cannot inherit a nearly-expired entry.
#[test]
fn re_add_refreshes_the_record_live_window() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);

    te.client.add_to_blocklist(&te.admin, &subscriber, &None::<String>);
    te.client.remove_from_blocklist(&te.admin, &subscriber);
    te.client.add_to_blocklist(&te.admin, &subscriber, &None::<String>);

    te.env
        .ledger()
        .with_mut(|li| li.sequence_number = SUB_TTL_EXTEND_TO - 1);

    assert!(te.client.is_blocklisted(&subscriber));
    assert_eq!(
        te.client.get_blocklist_entry(&subscriber).subscriber,
        subscriber
    );
}
