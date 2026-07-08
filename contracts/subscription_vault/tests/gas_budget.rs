//! Gas and storage budget regression tests for mutating entrypoints.
//!
//! Exercises `create_subscription`, `deposit_funds`, `charge_subscription`, and
//! `withdraw_merchant_funds` once each (and under high-id/dense-earnings scenarios),
//! captures `env.cost_estimate().budget()` metrics after each call, and asserts
//! against budgets sourced from `docs/query_performance.md`.
//!
//! Run with:
//!   cargo test -p subscription_vault --test gas_budget -- --nocapture
//!
//! CI prints `[Budget]` lines so the performance-budgets job can graph deltas.
//!
//! Security notes
//! --------------
//! - `charge_subscription` is O(1) per call (no global scan); tested at high IDs.
//! - `deposit_funds` acquires a reentrancy guard; budget accommodates the guard overhead.
//! - `withdraw_merchant_funds` is O(1) over merchant-balance key regardless of earnings depth.
//! - All budgets are 2× conservative baselines. Tighten only with benchmark evidence
//!   and a PR comment citing the measurement (see docs/query_performance.md §Re-benchmarking).

#![cfg(test)]

extern crate alloc;

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient as TokenAdminClient},
    Address, Env,
};
use subscription_vault::{SubscriptionVault, SubscriptionVaultClient};

// ── Budget constants (sourced from docs/query_performance.md) ────────────────
// Mutating entrypoints: conservative 2× headroom over measured baselines.
// To tighten: run the test with --nocapture, record [Budget] cpu/reads/writes,
// set the new constant to measured_max × 2, open a PR with the evidence.

/// `create_subscription`: counter read/write + subscription write + index updates.
const BUDGET_CREATE_CPU: u64 = 500_000;
const BUDGET_CREATE_READS: u64 = 20;
const BUDGET_CREATE_WRITES: u64 = 20;

/// `deposit_funds`: subscription read/write + token transfer + reentrancy guard.
const BUDGET_DEPOSIT_CPU: u64 = 500_000;
const BUDGET_DEPOSIT_READS: u64 = 20;
const BUDGET_DEPOSIT_WRITES: u64 = 20;

/// `charge_subscription`: subscription read/write + merchant balance update.
const BUDGET_CHARGE_CPU: u64 = 1_000_000;
const BUDGET_CHARGE_READS: u64 = 30;
const BUDGET_CHARGE_WRITES: u64 = 30;

/// `withdraw_merchant_funds`: merchant balance read/write + token transfer.
const BUDGET_WITHDRAW_CPU: u64 = 500_000;
const BUDGET_WITHDRAW_READS: u64 = 20;
const BUDGET_WITHDRAW_WRITES: u64 = 20;

/// Soft-warning threshold: print a `[Warn]` line when consumption exceeds this
/// fraction of the hard limit. Does not fail the test on its own.
const WARN_THRESHOLD: f64 = 0.80;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_env() -> (
    Env,
    SubscriptionVaultClient<'static>,
    TokenClient<'static>,
    TokenAdminClient<'static>,
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

    // Reset budget before init so it does not pollute per-call measurements.
    env.cost_estimate().budget().reset_unlimited();
    vault.init(&token.address, &7u32, &admin, &100i128, &(3 * 86_400u64));

    (env, vault, token, token_admin_client, admin)
}

/// Print a `[Budget]` line, emit `[Warn]` if above threshold, and assert hard limits.
fn assert_budget(
    label: &str,
    cpu: u64,
    reads: u64,
    writes: u64,
    cpu_limit: u64,
    read_limit: u64,
    write_limit: u64,
) {
    println!(
        "[Budget] {label}: cpu={cpu} reads={reads} writes={writes} \
         (limits cpu≤{cpu_limit} reads≤{read_limit} writes≤{write_limit})"
    );
    if cpu as f64 / cpu_limit as f64 > WARN_THRESHOLD {
        println!("[Warn] {label} CPU at {:.1}% of budget", cpu as f64 / cpu_limit as f64 * 100.0);
    }
    if reads as f64 / read_limit as f64 > WARN_THRESHOLD {
        println!("[Warn] {label} reads at {:.1}% of budget", reads as f64 / read_limit as f64 * 100.0);
    }
    assert!(cpu <= cpu_limit,   "[Budget] FAIL {label}: cpu={cpu} > limit={cpu_limit}");
    assert!(reads <= read_limit, "[Budget] FAIL {label}: reads={reads} > limit={read_limit}");
    assert!(writes <= write_limit, "[Budget] FAIL {label}: writes={writes} > limit={write_limit}");
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// `create_subscription` stays within gas budget on first call (ID 0).
#[test]
fn budget_create_subscription() {
    let (env, vault, _token, token_admin, _admin) = make_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &10_000_000i128);

    env.cost_estimate().budget().reset_unlimited();
    vault.create_subscription(&subscriber, &merchant, &1_000i128, &(30 * 86_400u64), &false, &None, &None);

    let cpu = env.cost_estimate().budget().cpu_instruction_count();
    let reads = env.cost_estimate().budget().ledger_read_count();
    let writes = env.cost_estimate().budget().ledger_write_count();
    assert_budget("create_subscription", cpu, reads, writes, BUDGET_CREATE_CPU, BUDGET_CREATE_READS, BUDGET_CREATE_WRITES);
}

/// `deposit_funds` stays within gas budget.
#[test]
fn budget_deposit_funds() {
    let (env, vault, _token, token_admin, _admin) = make_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &10_000_000i128);

    let sub_id = vault.create_subscription(
        &subscriber, &merchant, &1_000i128, &(30 * 86_400u64), &false, &None, &None,
    );

    env.cost_estimate().budget().reset_unlimited();
    vault.deposit_funds(&sub_id, &subscriber, &50_000i128);

    let cpu = env.cost_estimate().budget().cpu_instruction_count();
    let reads = env.cost_estimate().budget().ledger_read_count();
    let writes = env.cost_estimate().budget().ledger_write_count();
    assert_budget("deposit_funds", cpu, reads, writes, BUDGET_DEPOSIT_CPU, BUDGET_DEPOSIT_READS, BUDGET_DEPOSIT_WRITES);
}

/// `charge_subscription` stays within gas budget.
///
/// Security note: `charge_core` does not perform external token transfers;
/// merchant crediting is a pure storage update. The reentrancy guard adds a
/// constant overhead that is already factored into BUDGET_CHARGE_*.
#[test]
fn budget_charge_subscription() {
    let (env, vault, _token, token_admin, _admin) = make_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &10_000_000i128);

    let sub_id = vault.create_subscription(
        &subscriber, &merchant, &1_000i128, &(30 * 86_400u64), &false, &None, &None,
    );
    vault.deposit_funds(&sub_id, &subscriber, &50_000i128);
    env.ledger().set_timestamp(1_000_000 + 30 * 86_400 + 1);

    env.cost_estimate().budget().reset_unlimited();
    vault.charge_subscription(&sub_id);

    let cpu = env.cost_estimate().budget().cpu_instruction_count();
    let reads = env.cost_estimate().budget().ledger_read_count();
    let writes = env.cost_estimate().budget().ledger_write_count();
    assert_budget("charge_subscription", cpu, reads, writes, BUDGET_CHARGE_CPU, BUDGET_CHARGE_READS, BUDGET_CHARGE_WRITES);
}

/// `withdraw_merchant_funds` stays within gas budget.
#[test]
fn budget_withdraw_merchant_funds() {
    let (env, vault, _token, token_admin, _admin) = make_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &10_000_000i128);

    let sub_id = vault.create_subscription(
        &subscriber, &merchant, &1_000i128, &(30 * 86_400u64), &false, &None, &None,
    );
    vault.deposit_funds(&sub_id, &subscriber, &50_000i128);
    env.ledger().set_timestamp(1_000_000 + 30 * 86_400 + 1);
    vault.charge_subscription(&sub_id);

    env.cost_estimate().budget().reset_unlimited();
    vault.withdraw_merchant_funds(&merchant, &1_000i128);

    let cpu = env.cost_estimate().budget().cpu_instruction_count();
    let reads = env.cost_estimate().budget().ledger_read_count();
    let writes = env.cost_estimate().budget().ledger_write_count();
    assert_budget("withdraw_merchant_funds", cpu, reads, writes, BUDGET_WITHDRAW_CPU, BUDGET_WITHDRAW_READS, BUDGET_WITHDRAW_WRITES);
}

/// `charge_subscription` at a high subscription ID stays O(1).
///
/// Creates 50 subscriptions to push the ID counter high, then measures charge
/// on the last one. Cost must not grow with ID magnitude — a regression to a
/// scan-based implementation would fail here but pass budget_charge_subscription.
#[test]
fn budget_charge_subscription_high_id() {
    let (env, vault, _token, token_admin, _admin) = make_env();
    let merchant = Address::generate(&env);
    let mut last_id = 0u32;

    for _ in 0..50u32 {
        let subscriber = Address::generate(&env);
        token_admin.mint(&subscriber, &10_000_000i128);
        last_id = vault.create_subscription(
            &subscriber, &merchant, &1_000i128, &(30 * 86_400u64), &false, &None, &None,
        );
        vault.deposit_funds(&last_id, &subscriber, &50_000i128);
    }

    env.ledger().set_timestamp(1_000_000 + 30 * 86_400 + 1);

    env.cost_estimate().budget().reset_unlimited();
    vault.charge_subscription(&last_id);

    let cpu = env.cost_estimate().budget().cpu_instruction_count();
    let reads = env.cost_estimate().budget().ledger_read_count();
    let writes = env.cost_estimate().budget().ledger_write_count();
    assert_budget("charge_subscription_high_id", cpu, reads, writes, BUDGET_CHARGE_CPU, BUDGET_CHARGE_READS, BUDGET_CHARGE_WRITES);
}

/// `withdraw_merchant_funds` with a dense merchant earnings map stays O(1).
///
/// Charges the same merchant across 20 subscriptions to grow the earnings
/// balance, then withdraws. The merchant-balance key is a single storage
/// entry regardless of contribution count, so cost must remain constant.
#[test]
fn budget_withdraw_dense_merchant_earnings() {
    let (env, vault, _token, token_admin, _admin) = make_env();
    let merchant = Address::generate(&env);

    for _ in 0..20u32 {
        let subscriber = Address::generate(&env);
        token_admin.mint(&subscriber, &10_000_000i128);
        let sub_id = vault.create_subscription(
            &subscriber, &merchant, &1_000i128, &(30 * 86_400u64), &false, &None, &None,
        );
        vault.deposit_funds(&sub_id, &subscriber, &50_000i128);
    }

    env.ledger().set_timestamp(1_000_000 + 30 * 86_400 + 1);
    for id in 0..20u32 {
        vault.charge_subscription(&id);
    }

    env.cost_estimate().budget().reset_unlimited();
    vault.withdraw_merchant_funds(&merchant, &5_000i128);

    let cpu = env.cost_estimate().budget().cpu_instruction_count();
    let reads = env.cost_estimate().budget().ledger_read_count();
    let writes = env.cost_estimate().budget().ledger_write_count();
    assert_budget("withdraw_dense_merchant_earnings", cpu, reads, writes, BUDGET_WITHDRAW_CPU, BUDGET_WITHDRAW_READS, BUDGET_WITHDRAW_WRITES);
}
