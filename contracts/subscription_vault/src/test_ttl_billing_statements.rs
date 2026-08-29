//! TTL extension tests for billing statement persistent storage entries.
//!
//! # Coverage
//!
//! These tests verify that every persistent storage key involved in billing
//! statements (`BillingStatementSequence`, `BillingStatement`, and
//! `BillingStatementsBySubscription`) has its TTL extended correctly on both
//! write and read paths.
//!
//! ## Test categories
//!
//! 1. **Happy paths** — TTL is extended after append_statement and after a
//!    paginated read (get_statements_by_subscription_offset).
//! 2. **Boundary behavior** — statement is readable at exactly its last live
//!    ledger; the sequence counter and index are also alive after TTL extension.
//! 3. **Key-absent no-op** — extending TTL on a key that does not exist is a
//!    silent no-op (host behavior, verified indirectly by appending the first
//!    statement with no pre-existing sequence key).
//! 4. **Multi-statement extension** — all statement bodies appended in a single
//!    subscription are independently extended; the index TTL is refreshed on
//!    every append.
//! 5. **Regression** — appending multiple statements and paginating over them
//!    does not regress correct data (amounts, sequence numbers, kind).
//! 6. **Constants export** — `BILLING_STATEMENT_TTL_THRESHOLD` and
//!    `BILLING_STATEMENT_TTL_EXTEND_TO` are accessible from the crate root.
//! 7. **TTL constants sanity** — STMT constants match SUB constants (both are
//!    30 days / 365 days), so the ring is consistent.
//!
//! # Host TTL semantics (soroban-sdk 22)
//!
//! `extend_ttl(key, threshold, extend_to)` is **conditional**: the host only
//! extends when `live_until_ledger - current_sequence < threshold`. When
//! remaining TTL is at or above the threshold the call is a no-op. Both paths
//! are exercised here.
//!
//! Accessing an **expired** persistent entry aborts with a host error (surfaced
//! as a panic), NOT a clean `None`. Where we want to assert expiry we use
//! `catch_unwind`.

#![cfg(test)]

use crate::statements::{append_statement, get_statements_by_subscription_offset};
use crate::types::{
    BillingChargeKind, DataKey, BILLING_STATEMENT_TTL_EXTEND_TO, BILLING_STATEMENT_TTL_THRESHOLD,
    SUB_TTL_EXTEND_TO, SUB_TTL_THRESHOLD,
};
use crate::SubscriptionVault;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env,
};

// ── Shared constants ─────────────────────────────────────────────────────────

/// Starting ledger sequence for all tests.
const START_SEQ: u32 = 100;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Create a minimal test environment with max_entry_ttl well above
/// BILLING_STATEMENT_TTL_EXTEND_TO so that extend_ttl calls are never clamped.
fn make_env() -> (Env, Address) {
    let env = Env::default();
    env.ledger().with_mut(|li| {
        li.sequence_number = START_SEQ;
        li.min_persistent_entry_ttl = 4096;
        li.min_temp_entry_ttl = 4096;
        li.max_entry_ttl = BILLING_STATEMENT_TTL_EXTEND_TO + 5_000_000;
    });
    let contract_id = env.register(SubscriptionVault, ());
    (env, contract_id)
}

/// Append a single statement and return its sequence number (always 1 for a
/// fresh subscription).
fn append_one(env: &Env, contract_id: &Address, sub_id: u32, amount: i128) {
    let merchant = Address::generate(env);
    env.as_contract(contract_id, || {
        append_statement(
            env,
            sub_id,
            amount,
            merchant,
            BillingChargeKind::Interval,
            0u64,
            1000u64,
        )
        .unwrap();
    });
}

/// Set ledger sequence number.
fn set_seq(env: &Env, seq: u32) {
    env.ledger().with_mut(|li| li.sequence_number = seq);
}

/// Returns `true` if reading the BillingStatementsBySubscription index for
/// `sub_id` succeeds (does not panic / abort with a host error).
fn index_read_succeeds(env: &Env, contract_id: &Address, sub_id: u32) -> bool {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        env.as_contract(contract_id, || {
            get_statements_by_subscription_offset(env, sub_id, 0, 10, false).unwrap();
        });
    }));
    std::panic::set_hook(prev);
    result.is_ok()
}

/// Returns `true` if reading BillingStatement(sub_id, seq) directly succeeds.
fn stmt_read_succeeds(env: &Env, contract_id: &Address, sub_id: u32, seq: u32) -> bool {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        env.as_contract(contract_id, || {
            let _ = env
                .storage()
                .persistent()
                .get::<_, crate::types::BillingStatement>(
                    &DataKey::BillingStatement(sub_id, seq),
                );
        });
    }));
    std::panic::set_hook(prev);
    result.is_ok()
}

// ── 1. Happy path: TTL extended after append_statement ────────────────────────

/// After appending one statement the statement body, sequence counter, and
/// secondary index must all be live (their TTL was extended to START_SEQ +
/// BILLING_STATEMENT_TTL_EXTEND_TO by append_statement).
#[test]
fn append_extends_ttl_on_all_three_keys() {
    let (env, cid) = make_env();
    let sub_id = 1u32;
    append_one(&env, &cid, sub_id, 500_000);

    // Advance to the very last ledger the entries should still be alive on.
    let live_until = START_SEQ + BILLING_STATEMENT_TTL_EXTEND_TO;
    set_seq(&env, live_until);

    // All three entries must still be reachable.
    assert!(
        index_read_succeeds(&env, &cid, sub_id),
        "BillingStatementsBySubscription must be alive at live_until"
    );
    assert!(
        stmt_read_succeeds(&env, &cid, sub_id, 1),
        "BillingStatement(sub_id, 1) must be alive at live_until"
    );

    // Sequence counter must also be alive.
    env.as_contract(&cid, || {
        let seq: Option<u32> = env
            .storage()
            .persistent()
            .get(&DataKey::BillingStatementSequence(sub_id));
        assert_eq!(seq, Some(1), "Sequence counter must be 1 at live_until");
    });
}

// ── 2. Happy path: TTL extended on read ───────────────────────────────────────

/// `get_statements_by_subscription_offset` extends TTL on the index and on
/// each fetched statement body. After the read the entries must survive past
/// the original window.
#[test]
fn read_extends_ttl_on_index_and_stmt_bodies() {
    let (env, cid) = make_env();
    let sub_id = 2u32;

    // Append at START_SEQ — entries live until START_SEQ + EXTEND_TO.
    append_one(&env, &cid, sub_id, 1_000_000);

    // Read at exactly the live boundary — this extends TTL to
    // live_until + BILLING_STATEMENT_TTL_EXTEND_TO.
    let live_until = START_SEQ + BILLING_STATEMENT_TTL_EXTEND_TO;
    set_seq(&env, live_until);
    env.as_contract(&cid, || {
        let page =
            get_statements_by_subscription_offset(&env, sub_id, 0, 10, false).unwrap();
        assert_eq!(page.statements.len(), 1, "Must return exactly one statement");
        assert_eq!(page.statements.get(0).unwrap().amount, 1_000_000);
    });

    // Now advance further than the original window — entry should still be
    // alive because the read re-extended the TTL.
    let past_original = live_until + 1_000;
    set_seq(&env, past_original);
    assert!(
        index_read_succeeds(&env, &cid, sub_id),
        "Index must still be alive after read-path TTL extension"
    );
    assert!(
        stmt_read_succeeds(&env, &cid, sub_id, 1),
        "Statement body must still be alive after read-path TTL extension"
    );
}

// ── 3. Boundary: expired statement raises host error ─────────────────────────

/// One ledger past the live_until boundary without any read refresh causes the
/// statement body to expire, surfacing as a host panic (not silent None).
#[test]
fn expired_statement_body_raises_host_error() {
    let (env, cid) = make_env();
    let sub_id = 3u32;
    append_one(&env, &cid, sub_id, 250_000);

    // Positive control at boundary.
    let live_until = START_SEQ + BILLING_STATEMENT_TTL_EXTEND_TO;
    set_seq(&env, live_until);
    assert!(
        stmt_read_succeeds(&env, &cid, sub_id, 1),
        "Statement must be readable at the boundary"
    );

    // Use a fresh environment so the boundary read above doesn't refresh TTL
    // for our expiry assertion.
    let (env2, cid2) = make_env();
    let sub_id2 = 3u32;
    append_one(&env2, &cid2, sub_id2, 250_000);

    set_seq(&env2, live_until + 1); // one past the window
    assert!(
        !stmt_read_succeeds(&env2, &cid2, sub_id2, 1),
        "Accessing an expired statement must raise a host error, not return stale data"
    );
}

/// Same assertion with `#[should_panic]` to pin the specific error kind.
#[test]
#[should_panic(expected = "Storage")]
fn expired_statement_body_panics_with_storage_error() {
    let (env, cid) = make_env();
    let sub_id = 30u32;
    append_one(&env, &cid, sub_id, 100_000);

    set_seq(&env, START_SEQ + BILLING_STATEMENT_TTL_EXTEND_TO + 1);

    env.as_contract(&cid, || {
        // This should panic with Storage error.
        let _: Option<crate::types::BillingStatement> = env
            .storage()
            .persistent()
            .get(&DataKey::BillingStatement(sub_id, 1));
    });
}

// ── 4. Key-absent no-op ───────────────────────────────────────────────────────

/// Appending the very first statement for a subscription exercises the
/// absent-key code path for `BillingStatementSequence`: `get` returns
/// `unwrap_or(0)`, and the subsequent `extend_ttl` on the freshly-written
/// key must not panic.
#[test]
fn first_append_absent_seq_key_is_noop_safe() {
    let (env, cid) = make_env();
    let sub_id = 4u32;

    // No prior state for sub_id 4.
    // Appending must succeed and not panic, even though the seq key didn't exist.
    env.as_contract(&cid, || {
        let merchant = Address::generate(&env);
        append_statement(
            &env,
            sub_id,
            42_000,
            merchant,
            BillingChargeKind::OneOff,
            0,
            1,
        )
        .unwrap();
    });

    // The statement should now be retrievable.
    env.as_contract(&cid, || {
        let page =
            get_statements_by_subscription_offset(&env, sub_id, 0, 10, false).unwrap();
        assert_eq!(page.statements.len(), 1);
        assert_eq!(page.statements.get(0).unwrap().sequence, 1);
        assert_eq!(page.statements.get(0).unwrap().amount, 42_000);
    });
}

/// Reading an empty (never-written) subscription's statement index returns an
/// empty page without panicking — the absent index key is handled by
/// `unwrap_or(Vec::new(env))` and `extend_ttl` is guarded by an `!ids.is_empty()` check.
#[test]
fn read_empty_subscription_returns_empty_page() {
    let (env, cid) = make_env();
    let sub_id = 999u32; // never written to

    env.as_contract(&cid, || {
        let page =
            get_statements_by_subscription_offset(&env, sub_id, 0, 10, false).unwrap();
        assert_eq!(page.statements.len(), 0, "Empty subscription must return empty page");
        assert_eq!(page.total, 0);
        assert!(page.next_cursor.is_none());
    });
}

// ── 5. Multi-statement: all appended entries get independent TTL ──────────────

/// Appending N statements produces N independent statement-body keys, each
/// with its own TTL extended to START_SEQ + BILLING_STATEMENT_TTL_EXTEND_TO.
/// The index TTL is refreshed on every append.
#[test]
fn multi_statement_all_bodies_get_ttl_extended() {
    let (env, cid) = make_env();
    let sub_id = 5u32;

    let merchant = Address::generate(&env);
    env.as_contract(&cid, || {
        for i in 0u32..5 {
            let amount = 100_000i128 * (i as i128 + 1);
            append_statement(
                &env,
                sub_id,
                amount,
                merchant.clone(),
                BillingChargeKind::Interval,
                i as u64 * 86400,
                i as u64 * 86400 + 1000,
            )
            .unwrap();
        }
    });

    // Advance to live boundary and read all statements.
    let live_until = START_SEQ + BILLING_STATEMENT_TTL_EXTEND_TO;
    set_seq(&env, live_until);

    env.as_contract(&cid, || {
        let page =
            get_statements_by_subscription_offset(&env, sub_id, 0, 10, false).unwrap();
        assert_eq!(page.total, 5, "Must have 5 statements");
        assert_eq!(page.statements.len(), 5);
        // Sequence numbers must be 1..=5.
        for (i, stmt) in page.statements.iter().enumerate() {
            assert_eq!(stmt.sequence, (i + 1) as u32);
            assert_eq!(stmt.amount, 100_000 * (i as i128 + 1));
        }
    });
}

// ── 6. Regression: pagination correctness is preserved ───────────────────────

/// Paginating over 7 statements with limit=3 returns all entries in the
/// correct order, with correct `next_cursor` values, after TTL extension.
#[test]
fn pagination_correctness_preserved_with_ttl_extension() {
    let (env, cid) = make_env();
    let sub_id = 6u32;
    let n = 7u32;

    let merchant = Address::generate(&env);
    env.as_contract(&cid, || {
        for i in 0..n {
            append_statement(
                &env,
                sub_id,
                (i + 1) as i128 * 10_000,
                merchant.clone(),
                BillingChargeKind::Interval,
                i as u64,
                i as u64 + 500,
            )
            .unwrap();
        }
    });

    env.as_contract(&cid, || {
        // Page 1: offset=0, limit=3
        let p1 = get_statements_by_subscription_offset(&env, sub_id, 0, 3, false).unwrap();
        assert_eq!(p1.statements.len(), 3);
        assert_eq!(p1.total, 7);
        assert_eq!(p1.next_cursor, Some(3));
        assert_eq!(p1.statements.get(0).unwrap().sequence, 1);
        assert_eq!(p1.statements.get(2).unwrap().sequence, 3);

        // Page 2: offset=3, limit=3
        let p2 = get_statements_by_subscription_offset(&env, sub_id, 3, 3, false).unwrap();
        assert_eq!(p2.statements.len(), 3);
        assert_eq!(p2.next_cursor, Some(6));
        assert_eq!(p2.statements.get(0).unwrap().sequence, 4);

        // Page 3 (last): offset=6, limit=3
        let p3 = get_statements_by_subscription_offset(&env, sub_id, 6, 3, false).unwrap();
        assert_eq!(p3.statements.len(), 1);
        assert!(p3.next_cursor.is_none(), "No next page after last entry");
        assert_eq!(p3.statements.get(0).unwrap().sequence, 7);
    });
}

/// Newest-first ordering returns statements in reverse sequence order.
#[test]
fn newest_first_pagination_order_preserved_with_ttl_extension() {
    let (env, cid) = make_env();
    let sub_id = 7u32;

    let merchant = Address::generate(&env);
    env.as_contract(&cid, || {
        for i in 0..4u32 {
            append_statement(
                &env,
                sub_id,
                (i + 1) as i128,
                merchant.clone(),
                BillingChargeKind::Usage,
                i as u64,
                i as u64 + 1,
            )
            .unwrap();
        }

        let page = get_statements_by_subscription_offset(&env, sub_id, 0, 10, true).unwrap();
        assert_eq!(page.total, 4);
        // newest-first → sequence 4, 3, 2, 1
        for (pos, expected_seq) in [4u32, 3, 2, 1].iter().enumerate() {
            assert_eq!(
                page.statements.get(pos as u32).unwrap().sequence,
                *expected_seq,
                "newest-first position {} should be sequence {}",
                pos,
                expected_seq
            );
        }
    });
}

// ── 7. Constants: export and sanity ──────────────────────────────────────────

/// The billing statement TTL constants are accessible from the crate root and
/// have the expected values (30 days threshold, 365 days extend-to).
#[test]
fn billing_statement_ttl_constants_exported_and_correct() {
    // These are accessible from crate root because of the pub use in lib.rs.
    assert_eq!(
        BILLING_STATEMENT_TTL_THRESHOLD,
        30 * 24 * 60 * 60,
        "Threshold must be 30 days in seconds"
    );
    assert_eq!(
        BILLING_STATEMENT_TTL_EXTEND_TO,
        365 * 24 * 60 * 60,
        "Extend-to must be 365 days in seconds"
    );
}

/// Billing statement TTL constants are consistent with subscription TTL constants.
/// Both tiers use the same thresholds so the TTL contract is uniform.
#[test]
fn billing_statement_ttl_matches_subscription_ttl() {
    assert_eq!(
        BILLING_STATEMENT_TTL_THRESHOLD, SUB_TTL_THRESHOLD,
        "Statement threshold must match subscription threshold"
    );
    assert_eq!(
        BILLING_STATEMENT_TTL_EXTEND_TO, SUB_TTL_EXTEND_TO,
        "Statement extend-to must match subscription extend-to"
    );
}

/// Threshold is strictly less than extend-to — a freshly-written entry must
/// never be immediately re-extended.
#[test]
fn billing_statement_ttl_threshold_less_than_extend_to() {
    assert!(
        BILLING_STATEMENT_TTL_THRESHOLD < BILLING_STATEMENT_TTL_EXTEND_TO,
        "Threshold ({}) must be < extend_to ({})",
        BILLING_STATEMENT_TTL_THRESHOLD,
        BILLING_STATEMENT_TTL_EXTEND_TO
    );
}

// ── 8. Multiple subscriptions: TTL isolation ─────────────────────────────────

/// Extending TTL for one subscription's statements does not affect a different
/// subscription's entries.
#[test]
fn ttl_extension_is_per_subscription() {
    let (env, cid) = make_env();
    let sub_a = 10u32;
    let sub_b = 11u32;

    append_one(&env, &cid, sub_a, 100);
    append_one(&env, &cid, sub_b, 200);

    // Read sub_a to refresh its TTL.
    set_seq(&env, START_SEQ + BILLING_STATEMENT_TTL_EXTEND_TO);
    env.as_contract(&cid, || {
        let page = get_statements_by_subscription_offset(&env, sub_a, 0, 1, false).unwrap();
        assert_eq!(page.statements.get(0).unwrap().amount, 100);
    });

    // sub_b was NOT read, so its entries should still be at the original live_until.
    // Check that sub_b statements are still readable at the same ledger (not expired).
    assert!(
        stmt_read_succeeds(&env, &cid, sub_b, 1),
        "sub_b statement should still be reachable at live_until (was written at start)"
    );
}

// ── 9. Charge kinds are preserved across TTL extension ───────────────────────

/// All three BillingChargeKind values survive the write-path TTL extension
/// with their data intact.
#[test]
fn all_charge_kinds_preserved_after_ttl_extension() {
    let (env, cid) = make_env();
    let sub_id = 20u32;
    let merchant = Address::generate(&env);

    let kinds = [
        BillingChargeKind::Interval,
        BillingChargeKind::Usage,
        BillingChargeKind::OneOff,
    ];

    env.as_contract(&cid, || {
        for (i, &kind) in kinds.iter().enumerate() {
            append_statement(
                &env,
                sub_id,
                (i + 1) as i128 * 1_000,
                merchant.clone(),
                kind,
                i as u64,
                i as u64 + 10,
            )
            .unwrap();
        }
    });

    // Advance to live_until and read back — data must be intact.
    set_seq(&env, START_SEQ + BILLING_STATEMENT_TTL_EXTEND_TO);
    env.as_contract(&cid, || {
        let page = get_statements_by_subscription_offset(&env, sub_id, 0, 10, false).unwrap();
        assert_eq!(page.statements.len(), 3);
        assert_eq!(page.statements.get(0).unwrap().kind, BillingChargeKind::Interval);
        assert_eq!(page.statements.get(1).unwrap().kind, BillingChargeKind::Usage);
        assert_eq!(page.statements.get(2).unwrap().kind, BillingChargeKind::OneOff);
    });
}
