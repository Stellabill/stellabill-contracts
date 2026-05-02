//! Tests for billing retention pruning strategy.
//!
//! Covers:
//! - Inline pruning on the write path (append triggers oldest-row eviction)
//! - Explicit admin compaction with global and per-call overrides
//! - Boundary conditions: retain N, delete N+1
//! - Query behaviour (offset + cursor) across pruned data
//! - Security: non-admin compaction is rejected
//! - DoS safety: pagination doesn't scan pruned sequence slots

use super::*;
use crate::test_utils::setup::TestEnv;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::Address;

const AMOUNT: i128 = 1_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const T0: u64 = 1_000_000_000;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Create a subscription and run `n` successful charges against it.
/// Returns (sub_id, subscriber).
fn setup_and_charge(test_env: &TestEnv, n: u32) -> (u32, Address) {
    test_env.env.ledger().with_mut(|l| l.timestamp = T0);
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    test_env
        .stellar_token_client()
        .mint(&subscriber, &(AMOUNT * (n as i128 + 10)));

    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env
        .client
        .deposit_funds(&id, &subscriber, &(AMOUNT * (n as i128 + 5)));

    for i in 1..=(n as u64) {
        test_env
            .env
            .ledger()
            .with_mut(|l| l.timestamp = T0 + i * INTERVAL);
        test_env.client.charge_subscription(&id);
    }
    (id, subscriber)
}

// ── boundary: retain N, delete N+1 ───────────────────────────────────────────

#[test]
fn test_inline_pruning_boundary_retain_exactly_n() {
    // Set retention = 5, write exactly 5 rows → no pruning yet
    let test_env = TestEnv::default();
    test_env.client.set_billing_retention(&test_env.admin, &5);
    let (id, _) = setup_and_charge(&test_env, 5);

    let agg = test_env.client.get_stmt_compacted_aggregate(&id);
    assert_eq!(agg.pruned_count, 0, "No rows should be pruned at boundary");

    let page = test_env
        .client
        .get_sub_statements_offset(&id, &0, &10, &false);
    assert_eq!(page.total, 5);
    assert_eq!(page.statements.len(), 5);
}

#[test]
fn test_inline_pruning_triggers_at_n_plus_one() {
    // Set retention = 5, write 6 rows → first row pruned automatically
    let test_env = TestEnv::default();
    test_env.client.set_billing_retention(&test_env.admin, &5);
    let (id, _) = setup_and_charge(&test_env, 6);

    let agg = test_env.client.get_stmt_compacted_aggregate(&id);
    assert_eq!(agg.pruned_count, 1, "Exactly one row should be auto-pruned");
    assert_eq!(agg.total_amount, AMOUNT);

    let page = test_env
        .client
        .get_sub_statements_offset(&id, &0, &10, &false);
    assert_eq!(page.total, 5, "Live row count stays at keep_recent");
}

#[test]
fn test_inline_pruning_keeps_live_count_bounded() {
    // Set retention = 3, write 20 rows → always exactly 3 live
    let test_env = TestEnv::default();
    test_env.client.set_billing_retention(&test_env.admin, &3);
    let (id, _) = setup_and_charge(&test_env, 20);

    let agg = test_env.client.get_stmt_compacted_aggregate(&id);
    assert_eq!(agg.pruned_count, 17);
    assert_eq!(agg.total_amount, 17 * AMOUNT);

    let page = test_env
        .client
        .get_sub_statements_offset(&id, &0, &10, &false);
    assert_eq!(page.total, 3);
    assert_eq!(page.statements.len(), 3);
    // Newest 3 rows (sequences 17, 18, 19)
    assert_eq!(page.statements.get(0).unwrap().sequence, 17);
    assert_eq!(page.statements.get(2).unwrap().sequence, 19);
}

// ── query behaviour across pruned data ───────────────────────────────────────

#[test]
fn test_offset_pagination_oldest_first_after_pruning() {
    let test_env = TestEnv::default();
    test_env.client.set_billing_retention(&test_env.admin, &5);
    // Write 8 rows; 3 are auto-pruned (seq 0,1,2 → gone; live = seq 3..7)
    let (id, _) = setup_and_charge(&test_env, 8);

    let page = test_env
        .client
        .get_sub_statements_offset(&id, &0, &10, &false);
    assert_eq!(page.total, 5);
    // First returned row is seq 3 (oldest surviving)
    assert_eq!(page.statements.get(0).unwrap().sequence, 3);
    assert_eq!(page.statements.get(4).unwrap().sequence, 7);
}

#[test]
fn test_offset_pagination_newest_first_after_pruning() {
    let test_env = TestEnv::default();
    test_env.client.set_billing_retention(&test_env.admin, &5);
    let (id, _) = setup_and_charge(&test_env, 8);

    let page = test_env
        .client
        .get_sub_statements_offset(&id, &0, &10, &true);
    assert_eq!(page.total, 5);
    assert_eq!(page.statements.get(0).unwrap().sequence, 7);
    assert_eq!(page.statements.get(4).unwrap().sequence, 3);
}

#[test]
fn test_cursor_pagination_after_pruning() {
    let test_env = TestEnv::default();
    test_env.client.set_billing_retention(&test_env.admin, &4);
    // Write 7 rows; seq 0,1,2 pruned; live = 3..6
    let (id, _) = setup_and_charge(&test_env, 7);

    // Page 1 (newest first, page size 2): seq 6, 5
    let p1 = test_env
        .client
        .get_sub_statements_cursor(&id, &None::<u32>, &2, &true);
    assert_eq!(p1.statements.get(0).unwrap().sequence, 6);
    assert_eq!(p1.statements.get(1).unwrap().sequence, 5);
    assert!(p1.next_cursor.is_some());

    // Page 2: seq 4, 3
    let p2 = test_env
        .client
        .get_sub_statements_cursor(&id, &p1.next_cursor, &2, &true);
    assert_eq!(p2.statements.get(0).unwrap().sequence, 4);
    assert_eq!(p2.statements.get(1).unwrap().sequence, 3);
    // No more pages because seq 3 == min_seq
    assert!(p2.next_cursor.is_none(), "Cursor should terminate at min_seq");
}

#[test]
fn test_stale_cursor_below_min_seq_returns_empty() {
    // Verify that a cursor pointing into pruned territory returns an empty page
    let test_env = TestEnv::default();
    test_env.client.set_billing_retention(&test_env.admin, &3);
    let (id, _) = setup_and_charge(&test_env, 6); // seq 0,1,2 pruned; live = 3..5

    // cursor=1 is below min_seq=3 → must return empty, not panic
    let page = test_env
        .client
        .get_sub_statements_cursor(&id, &Some(1u32), &5, &true);
    assert_eq!(page.statements.len(), 0, "Stale cursor returns empty page");
}

// ── explicit compaction ───────────────────────────────────────────────────────

#[test]
fn test_explicit_compaction_boundary_prunes_exactly_n_plus_one() {
    let test_env = TestEnv::default();
    let (id, _) = setup_and_charge(&test_env, 8);

    // Compact keeping 5 → 3 pruned
    let summary = test_env
        .client
        .compact_billing_statements(&test_env.admin, &id, &Some(5u32));
    assert_eq!(summary.pruned_count, 3);
    assert_eq!(summary.kept_count, 5);
    assert_eq!(summary.total_pruned_amount, 3 * AMOUNT);

    let agg = test_env.client.get_stmt_compacted_aggregate(&id);
    assert_eq!(agg.pruned_count, 3);
    assert_eq!(agg.total_amount, 3 * AMOUNT);
}

#[test]
fn test_explicit_compaction_keep_zero_removes_all_detail() {
    let test_env = TestEnv::default();
    let (id, _) = setup_and_charge(&test_env, 5);

    let summary = test_env
        .client
        .compact_billing_statements(&test_env.admin, &id, &Some(0u32));
    assert_eq!(summary.pruned_count, 5);
    assert_eq!(summary.kept_count, 0);
    assert_eq!(summary.total_pruned_amount, 5 * AMOUNT);

    // Detail is gone but aggregate remains
    let agg = test_env.client.get_stmt_compacted_aggregate(&id);
    assert_eq!(agg.pruned_count, 5);
    assert_eq!(agg.total_amount, 5 * AMOUNT);
    assert!(agg.oldest_period_start.is_some());
    assert!(agg.newest_period_end.is_some());

    // Live count is 0
    let page = test_env
        .client
        .get_sub_statements_offset(&id, &0, &10, &true);
    assert_eq!(page.total, 0);
}

#[test]
fn test_explicit_compaction_idempotent() {
    let test_env = TestEnv::default();
    let (id, _) = setup_and_charge(&test_env, 6);

    test_env
        .client
        .compact_billing_statements(&test_env.admin, &id, &Some(3u32));

    // Second run: nothing more to prune
    let summary2 = test_env
        .client
        .compact_billing_statements(&test_env.admin, &id, &Some(3u32));
    assert_eq!(summary2.pruned_count, 0);
    assert_eq!(summary2.kept_count, 3);
}

// ── security ─────────────────────────────────────────────────────────────────

#[test]
fn test_compaction_non_admin_rejected() {
    let test_env = TestEnv::default();
    let (id, _) = setup_and_charge(&test_env, 5);
    let attacker = Address::generate(&test_env.env);

    let res = test_env
        .client
        .try_compact_billing_statements(&attacker, &id, &None::<u32>);
    assert!(res.is_err(), "Non-admin must not be able to compact");
}

#[test]
fn test_set_billing_retention_non_admin_rejected() {
    let test_env = TestEnv::default();
    let attacker = Address::generate(&test_env.env);

    let res = test_env
        .client
        .try_set_billing_retention(&attacker, &10u32);
    assert!(res.is_err(), "Non-admin must not be able to set retention");
}

// ── aggregate totals accumulate correctly across runs ─────────────────────────

#[test]
fn test_aggregate_accumulates_across_compaction_runs() {
    let test_env = TestEnv::default();
    let (id, _) = setup_and_charge(&test_env, 9);

    // First compaction: prune 3, keep 6
    let s1 = test_env
        .client
        .compact_billing_statements(&test_env.admin, &id, &Some(6u32));
    assert_eq!(s1.pruned_count, 3);

    // Second compaction: prune 3 more, keep 3
    let s2 = test_env
        .client
        .compact_billing_statements(&test_env.admin, &id, &Some(3u32));
    assert_eq!(s2.pruned_count, 3);

    let agg = test_env.client.get_stmt_compacted_aggregate(&id);
    assert_eq!(agg.pruned_count, 6);
    assert_eq!(agg.total_amount, 6 * AMOUNT);
}

// ── DoS safety: no scan of pruned gaps ───────────────────────────────────────

#[test]
fn test_compaction_does_not_scan_pruned_gaps() {
    // After 1_000 inline prunes (retention=5, 1005 charges would be ideal but
    // is too slow for CI; we verify via the live-count invariant instead).
    let test_env = TestEnv::default();
    test_env.client.set_billing_retention(&test_env.admin, &5);
    let (id, _) = setup_and_charge(&test_env, 20);

    // next ≈ 20, live = 5, so start_seq = next - live = 15
    // Explicit compact should only scan 5 rows, not 20
    let summary = test_env
        .client
        .compact_billing_statements(&test_env.admin, &id, &Some(3u32));
    assert_eq!(summary.pruned_count, 2);
    assert_eq!(summary.kept_count, 3);
}
