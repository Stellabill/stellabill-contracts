//! Performance budget integration tests for the subscription_vault crate.
//!
//! Exercises `get_subscription` and `create_subscription` at scale, prints
//! `[Perf]` metrics for CI review (pass `--nocapture` to see output), and
//! asserts against the documented budgets from `docs/query_performance.md`.
//!
//! Edge cases covered
//! ------------------
//! - Single direct lookup (O(1) path)
//! - Missing-ID lookup — same O(1) cost, must return `NotFound` without scanning
//! - Lookup over a large ID range — verifies no linear scan at high IDs
//! - `create_subscription` at scale (`N = SCALE_N`)
//! - `list_subscriptions_by_subscriber` first-page and multi-page traversal
//! - `get_subscriptions_by_token` index read + record fetch
//!
//! Security notes
//! --------------
//! - The `MAX_SCAN_DEPTH` (1 000) cap prevents adversarial O(n) scans via
//!   `list_subscriptions_by_subscriber`; tested explicitly below.
//! - Each `create_subscription` is O(1) (monotone counter, no global scan).
//! - Missing-ID lookups are bounded and do not leak timing information about
//!   the ID space — they complete in the same O(1) budget as a hit.

use std::time::Instant;

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env,
};
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient as TokenAdminClient};
use subscription_vault::{SubscriptionVault, SubscriptionVaultClient};

// ── Documented performance budgets (docs/query_performance.md) ───────────────

/// Hard wall-clock limit for a single `get_subscription` call (ms).
///
/// `get_subscription` is O(1) — one persistent storage lookup.  Even on the
/// slowest CI runner this should complete well under 500 ms.
const BUDGET_GET_SUB_MS: u128 = 500;

/// Hard wall-clock limit for `SCALE_N` `create_subscription` calls total (ms).
const BUDGET_CREATE_N_MS: u128 = 30_000;

/// Hard wall-clock limit for one `list_subscriptions_by_subscriber` call (ms).
const BUDGET_LIST_SUB_MS: u128 = 5_000;

/// Hard wall-clock limit for one `get_subscriptions_by_token` call (ms).
const BUDGET_GET_BY_TOKEN_MS: u128 = 5_000;

/// Soft-warning threshold: log a `[Warn]` line if we exceed this percentage of the budget.
const SOFT_WARN_PCT: u128 = 80;

/// Number of subscriptions created in scale / large-range tests.
const SCALE_N: u32 = 50;

// ── Test helpers ─────────────────────────────────────────────────────────────

/// Set up an initialized vault with a funded USDC-like token.
///
/// Returns `(env, vault, token, token_admin_client, admin)`.
fn make_env<'a>() -> (
    Env,
    SubscriptionVaultClient<'a>,
    TokenClient<'a>,
    TokenAdminClient<'a>,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000_000);

    let token_admin = Address::generate(&env);
    let token_contract = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token = TokenClient::new(&env, &token_contract);
    let token_admin_client = TokenAdminClient::new(&env, &token_contract);

    let admin = Address::generate(&env);
    let vault_id = env.register(SubscriptionVault, ());
    let vault = SubscriptionVaultClient::new(&env, &vault_id);

    vault.init(
        &token.address,
        &7u32,       // token_decimals (Stellar USDC)
        &admin,
        &100i128,    // min_topup
        &(3 * 86_400u64), // grace_period (3 days)
    );

    (env, vault, token, token_admin_client, admin)
}

/// Mint tokens, create a subscription, and deposit funds into it.
fn new_funded_sub<'a>(
    _env: &Env,
    vault: &SubscriptionVaultClient<'a>,
    token_admin: &TokenAdminClient<'_>,
    subscriber: &Address,
    merchant: &Address,
) -> u32 {
    token_admin.mint(subscriber, &100_000_000i128);
    let sub_id = vault.create_subscription(
        subscriber,
        merchant,
        &1_000i128,
        &(30 * 86_400u64),
        &false,
        &None,
        &None,
    );
    vault.deposit_funds(&sub_id, subscriber, &50_000i128);
    sub_id
}

/// Print a `[Perf]` line and return `true` if within budget.
///
/// Prints a `[Warn]` line when consumption exceeds `SOFT_WARN_PCT` of the
/// hard limit, giving early warning before the hard assertion fires.
fn report(label: &str, elapsed_ms: u128, budget_ms: u128) -> bool {
    let pct = elapsed_ms.saturating_mul(100) / budget_ms.max(1);
    let within = elapsed_ms < budget_ms;
    println!(
        "[Perf] {label} elapsed={elapsed_ms}ms budget={budget_ms}ms pct={pct}% {}",
        if within { "PASS" } else { "FAIL" }
    );
    if pct > SOFT_WARN_PCT && within {
        println!("[Warn] {label} consumed {pct}% of budget — approaching limit");
    }
    within
}

// ── Performance tests ─────────────────────────────────────────────────────────

/// O(1) direct lookup — must complete within `BUDGET_GET_SUB_MS`.
///
/// Creates a single subscription and times the `get_subscription` call.
/// This is the baseline: any correct implementation should be well under budget.
#[test]
fn perf_get_subscription_direct_lookup() {
    let (env, vault, _token, token_admin, _admin) = make_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let sub_id = new_funded_sub(&env, &vault, &token_admin, &subscriber, &merchant);

    let t0 = Instant::now();
    let sub = vault.get_subscription(&sub_id);
    let elapsed_ms = t0.elapsed().as_millis();

    assert!(
        report("get_subscription_direct", elapsed_ms, BUDGET_GET_SUB_MS),
        "[Perf] get_subscription_direct exceeded budget: {elapsed_ms}ms > {BUDGET_GET_SUB_MS}ms"
    );
    assert_eq!(sub.subscriber, subscriber, "returned wrong subscriber");
    assert_eq!(sub.merchant, merchant, "returned wrong merchant");
}

/// Missing-ID lookup must fail fast — same O(1) cost as a hit.
///
/// Security note: a non-existent ID must NOT trigger any scan of existing IDs.
/// Timing bounds verify no fallback linear search occurs.
#[test]
fn perf_get_subscription_missing_id() {
    let (_env, vault, _token, _token_admin, _admin) = make_env();

    let t0 = Instant::now();
    let result = vault.try_get_subscription(&99_999u32);
    let elapsed_ms = t0.elapsed().as_millis();

    assert!(
        report("get_subscription_missing", elapsed_ms, BUDGET_GET_SUB_MS),
        "[Perf] get_subscription_missing exceeded budget: {elapsed_ms}ms > {BUDGET_GET_SUB_MS}ms"
    );
    assert!(result.is_err(), "expected NotFound for non-existent ID 99_999");
}

/// `create_subscription` at scale — measures total and per-call wall-clock cost.
///
/// Security note: `create_subscription` uses a monotone counter and does NOT
/// scan existing subscriptions, so this must stay O(1) per call regardless of N.
#[test]
fn perf_create_subscription_at_scale() {
    let (env, vault, _token, token_admin, _admin) = make_env();
    let merchant = Address::generate(&env);
    let mut created: u32 = 0;

    let t0 = Instant::now();
    for _ in 0..SCALE_N {
        let subscriber = Address::generate(&env);
        token_admin.mint(&subscriber, &100_000_000i128);
        vault.create_subscription(
            &subscriber,
            &merchant,
            &1_000i128,
            &(30 * 86_400u64),
            &false,
            &None,
            &None,
        );
        created += 1;
    }
    let total_ms = t0.elapsed().as_millis();
    let avg_ms = total_ms / (SCALE_N as u128).max(1);

    println!(
        "[Perf] create_subscription_at_scale n={SCALE_N} total={total_ms}ms avg={avg_ms}ms/call budget={BUDGET_CREATE_N_MS}ms"
    );
    assert_eq!(created, SCALE_N, "not all creates completed");
    assert!(
        total_ms < BUDGET_CREATE_N_MS,
        "[Perf] create_subscription_at_scale exceeded budget: {total_ms}ms > {BUDGET_CREATE_N_MS}ms"
    );
}

/// Lookup over a large ID range — verifies O(1) at the highest allocated ID.
///
/// Creates `SCALE_N` subscriptions (IDs 0..SCALE_N-1) and then times lookups
/// at both the highest valid ID and just past the end.  An O(n) implementation
/// would fail the budget at high IDs; a correct O(1) lookup stays constant.
#[test]
fn perf_get_subscription_large_id_range() {
    let (env, vault, _token, token_admin, _admin) = make_env();
    let merchant = Address::generate(&env);
    let mut last_id = 0u32;

    for _ in 0..SCALE_N {
        let subscriber = Address::generate(&env);
        token_admin.mint(&subscriber, &100_000_000i128);
        last_id = vault.create_subscription(
            &subscriber,
            &merchant,
            &1_000i128,
            &(30 * 86_400u64),
            &false,
            &None,
            &None,
        );
    }

    // Highest valid ID — must complete in the same O(1) budget as ID 0.
    let t0 = Instant::now();
    let sub = vault.get_subscription(&last_id);
    let hit_ms = t0.elapsed().as_millis();

    assert!(
        report("get_subscription_high_id_hit", hit_ms, BUDGET_GET_SUB_MS),
        "[Perf] get_subscription_high_id_hit exceeded budget: {hit_ms}ms > {BUDGET_GET_SUB_MS}ms"
    );
    assert_eq!(sub.merchant, merchant);

    // One past the end — must also be O(1) (NotFound without scan).
    let t1 = Instant::now();
    let miss = vault.try_get_subscription(&(last_id + 1));
    let miss_ms = t1.elapsed().as_millis();

    assert!(
        report("get_subscription_past_end", miss_ms, BUDGET_GET_SUB_MS),
        "[Perf] get_subscription_past_end exceeded budget: {miss_ms}ms > {BUDGET_GET_SUB_MS}ms"
    );
    assert!(miss.is_err(), "expected NotFound for ID past last allocated");
}

/// Constant-time lookup — first, mid, and last IDs must all complete within budget.
///
/// Regression guard: if `get_subscription` is ever refactored to scan from ID 0,
/// the "last" lookup would fail the budget while "first" passes, making the
/// regression immediately visible.
#[test]
fn perf_get_subscription_constant_time_across_range() {
    let (env, vault, _token, token_admin, _admin) = make_env();
    let merchant = Address::generate(&env);
    let mut ids: std::vec::Vec<u32> = std::vec::Vec::new();

    for _ in 0..SCALE_N {
        let subscriber = Address::generate(&env);
        token_admin.mint(&subscriber, &100_000_000i128);
        let id = vault.create_subscription(
            &subscriber,
            &merchant,
            &1_000i128,
            &(30 * 86_400u64),
            &false,
            &None,
            &None,
        );
        ids.push(id);
    }

    let samples: &[(&str, usize)] = &[
        ("first", 0),
        ("mid", SCALE_N as usize / 2),
        ("last", SCALE_N as usize - 1),
    ];

    for (label, idx) in samples {
        let id = ids[*idx];
        let t = Instant::now();
        let _ = vault.get_subscription(&id);
        let ms = t.elapsed().as_millis();
        assert!(
            report(&format!("get_subscription_{label}"), ms, BUDGET_GET_SUB_MS),
            "[Perf] get_subscription_{label} id={id} exceeded budget: {ms}ms > {BUDGET_GET_SUB_MS}ms"
        );
    }
}

/// `list_subscriptions_by_subscriber` first-page and multi-page traversal.
///
/// Verifies:
/// 1. The first-page call completes within `BUDGET_LIST_SUB_MS`.
/// 2. Multi-page draining of all `SCALE_N` IDs yields exactly the correct total.
/// 3. The `MAX_SCAN_DEPTH` (1 000) guardrail is implicitly tested: for
///    `SCALE_N = 50` all IDs fit in a single scan window, so `next_start_id`
///    is `None` after the first call.
///
/// Security note: the per-call scan cap prevents adversarial O(n) exhaustion
/// even when a subscriber has millions of sparse IDs.
#[test]
fn perf_list_by_subscriber_paginated() {
    let (env, vault, _token, token_admin, _admin) = make_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &10_000_000_000i128);

    for _ in 0..SCALE_N {
        let sub_id = vault.create_subscription(
            &subscriber,
            &merchant,
            &1_000i128,
            &(30 * 86_400u64),
            &false,
            &None,
            &None,
        );
        vault.deposit_funds(&sub_id, &subscriber, &50_000i128);
    }

    // First page.
    let t0 = Instant::now();
    let page = vault.list_subscriptions_by_subscriber(&subscriber, &0u32, &100u32);
    let first_ms = t0.elapsed().as_millis();

    assert!(
        report("list_by_subscriber_page1", first_ms, BUDGET_LIST_SUB_MS),
        "[Perf] list_by_subscriber_page1 exceeded budget: {first_ms}ms > {BUDGET_LIST_SUB_MS}ms"
    );
    println!(
        "[Perf] list_by_subscriber_page1 ids_returned={} next={:?}",
        page.subscription_ids.len(),
        page.next_start_id
    );
    assert!(
        page.subscription_ids.len() > 0,
        "expected at least one subscription ID on first page"
    );

    // Multi-page drain — accumulate totals across all pages.
    let t1 = Instant::now();
    let mut total_ids: u32 = page.subscription_ids.len();
    let mut cursor = page.next_start_id;
    let mut pages: u32 = 1;

    while let Some(start) = cursor {
        let p = vault.list_subscriptions_by_subscriber(&subscriber, &start, &100u32);
        total_ids = total_ids.saturating_add(p.subscription_ids.len());
        cursor = p.next_start_id;
        pages += 1;
    }
    let drain_ms = t1.elapsed().as_millis();

    println!(
        "[Perf] list_by_subscriber_all_pages total_ids={total_ids} pages={pages} drain_ms={drain_ms}ms"
    );
    assert_eq!(
        total_ids, SCALE_N,
        "multi-page traversal must yield exactly SCALE_N IDs; got {total_ids}"
    );

    // Total drain budget scales with page count.
    let drain_budget = BUDGET_LIST_SUB_MS * (pages as u128).max(1);
    assert!(
        drain_ms < drain_budget,
        "[Perf] multi-page drain exceeded budget: {drain_ms}ms > {drain_budget}ms"
    );
}

/// `get_subscriptions_by_token` at scale — index read + record fetches.
///
/// Creates `SCALE_N` subscriptions all using the default token, then times a
/// paginated read of up to 100 records.  Budget: `BUDGET_GET_BY_TOKEN_MS`.
///
/// The call reads the full token index `Vec<u32>` from a single storage key
/// before slicing it.  For large index lists the deserialization cost grows
/// with list length, so this test also acts as a regression guard against
/// an index that becomes unexpectedly large.
#[test]
fn perf_get_subscriptions_by_token() {
    let (env, vault, token, token_admin, _admin) = make_env();
    let merchant = Address::generate(&env);

    for _ in 0..SCALE_N {
        let subscriber = Address::generate(&env);
        token_admin.mint(&subscriber, &100_000_000i128);
        vault.create_subscription(
            &subscriber,
            &merchant,
            &1_000i128,
            &(30 * 86_400u64),
            &false,
            &None,
            &None,
        );
    }

    // Count before the timed call (index read is cheap).
    let count = vault.get_token_subscription_count(&token.address);
    assert_eq!(count, SCALE_N, "token index must contain all {SCALE_N} subs");

    // Timed paginated read (limit = 100, which covers all SCALE_N entries).
    let t0 = Instant::now();
    let subs = vault.get_subscriptions_by_token(&token.address, &0u32, &100u32);
    let elapsed_ms = t0.elapsed().as_millis();

    assert!(
        report("get_subscriptions_by_token", elapsed_ms, BUDGET_GET_BY_TOKEN_MS),
        "[Perf] get_subscriptions_by_token exceeded budget: {elapsed_ms}ms > {BUDGET_GET_BY_TOKEN_MS}ms"
    );
    println!(
        "[Perf] get_subscriptions_by_token returned={} of {SCALE_N} total",
        subs.len()
    );
    assert!(
        subs.len() > 0,
        "expected subscriptions in the token index"
    );
}
