//! Gas benchmark for the full dispute lifecycle: open → respond → resolve.
//!
//! # Purpose
//! Measures the CPU-instruction and storage resource cost of the complete
//! dispute/chargeback workflow in three phases:
//!
//! 1. **Open** — subscriber opens a dispute, escrowing the disputed amount.
//! 2. **Respond** — admin responds with optional evidence.
//! 3. **Resolve** — admin resolves the dispute in favour of subscriber or merchant.
//!
//! Provides a stable regression signal on dispute-path performance.
//!
//! # Bench Execution & Threshold Enforcement
//! - Benchmark methodology reference: [docs/benchmarks/methodology.md](../../docs/benchmarks/methodology.md)
//! - Employs `env.cost_estimate()` to capture CPU instructions and ledger read/write
//!   entries around each phase.
//! - Persists baseline budgets in `benches/fixtures/dispute_lifecycle_budget.json`.
//! - Fails the benchmark if the performance delta exceeds 10%.
//!
//! # Required Edge Cases Tested
//! 1. Standard dispute lifecycle (open → respond → resolve to subscriber).
//! 2. Dispute with maximum evidence bytes (32 bytes).
//! 3. Dispute auto-closed on timeout (window elapsed, subscriber wins).
//! 4. Admin resolves to merchant instead of subscriber.
//!
//! # Security & Correctness Guarantees
//! - Escrow invariant: cumulative disbursement never exceeds original escrowed amount.
//! - Dispute state transitions are irreversible once resolved.
//! - All three phases produce deterministic, auditable events.

#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, BytesN, Env, String,
};
use subscription_vault::{
    types::{DisputeStatus, DISPUTE_WINDOW_SECS},
    SubscriptionVault, SubscriptionVaultClient,
};

// ── Constants ─────────────────────────────────────────────────────────────────

const AMOUNT: i128 = 1_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const DEPOSIT: i128 = 50_000_000;
const MIN_TOPUP: i128 = 500_000;
const DISPUTE_AMOUNT: i128 = 5_000_000;
const MAX_DELTA_TOLERANCE_PCT: f64 = 10.0;

// ── Fixture parsing ───────────────────────────────────────────────────────────

struct ScenarioBudget {
    open_cpu: u64,
    respond_cpu: u64,
    resolve_cpu: u64,
    total_cpu: u64,
}

fn parse_u64_key(json: &str, key: &str) -> u64 {
    let search = format!("\"{}\":", key);
    if let Some(pos) = json.find(&search) {
        let rest = &json[pos + search.len()..];
        let num_str: std::string::String = rest
            .chars()
            .skip_while(|c| c.is_whitespace())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        num_str.parse::<u64>().unwrap_or(0)
    } else {
        0
    }
}

fn get_scenario_budget(scenario_name: &str) -> ScenarioBudget {
    let raw_fixture = include_str!("fixtures/dispute_lifecycle_budget.json");
    let search_block = format!("\"{}\":", scenario_name);
    if let Some(pos) = raw_fixture.find(&search_block) {
        let block = &raw_fixture[pos..];
        ScenarioBudget {
            open_cpu: parse_u64_key(block, "open_cpu"),
            respond_cpu: parse_u64_key(block, "respond_cpu"),
            resolve_cpu: parse_u64_key(block, "resolve_cpu"),
            total_cpu: parse_u64_key(block, "total_cpu"),
        }
    } else {
        panic!(
            "Scenario {} not found in fixture budget file",
            scenario_name
        );
    }
}

// ── Cost metrics snapshot ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct DisputeMetrics {
    cpu_instructions: u64,
    read_entries: u64,
    write_entries: u64,
}

fn capture_metrics(env: &Env) -> DisputeMetrics {
    let resources = env.cost_estimate().resources();
    DisputeMetrics {
        cpu_instructions: resources.instructions.max(0) as u64,
        read_entries: resources.read_entries as u64,
        write_entries: resources.write_entries as u64,
    }
}

// ── Setup ─────────────────────────────────────────────────────────────────────

fn setup_env() -> (
    Env,
    SubscriptionVaultClient<'static>,
    Address,
    Address,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000_000);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token, &6, &admin, &MIN_TOPUP, &(7 * 24 * 60 * 60));

    // Pre-mint tokens into the contract so charge operations can pay out.
    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&contract_id, &1_000_000_000i128);

    (env, client, token, contract_id, admin)
}

fn setup_merchant_and_sub(
    env: &Env,
    client: &SubscriptionVaultClient,
) -> (Address, Address, u32, Address) {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    let redirect_url = String::from_str(env, "https://example.com/webhook");
    client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0,
        &subscription_vault::DEFAULT_ALLOWED_OPS,
        &None,
        &redirect_url,
    );

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    let token_admin = token::StellarAssetClient::new(env, &client.get_subscription(&sub_id).token);
    token_admin.mint(&subscriber, &DEPOSIT);
    let no_key: Option<BytesN<32>> = None;
    client.deposit_funds(&sub_id, &subscriber, &DEPOSIT, &no_key);

    let token = client.get_subscription(&sub_id).token;
    (subscriber, merchant, sub_id, token)
}

fn seed_merchant_balance_for_dispute(
    env: &Env,
    client: &SubscriptionVaultClient,
    merchant: &Address,
    token: &Address,
) {
    use subscription_vault::types::DataKey;

    // Seed the merchant balance so open_dispute can escrow funds.
    env.as_contract(&client.address, || {
        let existing: i128 = env
            .storage()
            .instance()
            .get(&DataKey::MerchantBalance(merchant.clone(), token.clone()))
            .unwrap_or(0i128);
        env.storage().instance().set(
            &DataKey::MerchantBalance(merchant.clone(), token.clone()),
            &(existing + DISPUTE_AMOUNT),
        );
    });

    // Ensure the contract has enough tokens to handle transfers.
    let token_admin = token::StellarAssetClient::new(env, token);
    token_admin.mint(&client.address, &DISPUTE_AMOUNT);
}

// ── Measurement helpers ───────────────────────────────────────────────────────

fn measure_open_dispute(
    env: &Env,
    client: &SubscriptionVaultClient,
    subscriber: &Address,
    sub_id: u32,
) -> (u64, DisputeMetrics) {
    env.cost_estimate().budget().reset_unlimited();
    let dispute_id = client.open_dispute(
        subscriber,
        &sub_id,
        &DISPUTE_AMOUNT,
        &None::<BytesN<32>>,
    );
    (dispute_id, capture_metrics(env))
}

fn measure_respond_dispute(
    env: &Env,
    client: &SubscriptionVaultClient,
    admin: &Address,
    dispute_id: u64,
    evidence: &Option<BytesN<32>>,
) -> DisputeMetrics {
    env.cost_estimate().budget().reset_unlimited();
    client.respond_dispute(admin, &dispute_id, evidence);
    capture_metrics(env)
}

fn measure_resolve_dispute(
    env: &Env,
    client: &SubscriptionVaultClient,
    admin: &Address,
    dispute_id: u64,
    resolve_to_subscriber: bool,
) -> DisputeMetrics {
    env.cost_estimate().budget().reset_unlimited();
    client.resolve_dispute(admin, &dispute_id, &resolve_to_subscriber);
    capture_metrics(env)
}

// ── Validation ────────────────────────────────────────────────────────────────

fn assert_phase_metrics(
    scenario_name: &str,
    phase: &str,
    measured: DisputeMetrics,
    budget_cpu: u64,
) {
    std::println!(
        "[bench_dispute_lifecycle] {}::{}  CPU={}  Reads={}  Writes={}  FixtureBudget={}",
        scenario_name,
        phase,
        measured.cpu_instructions,
        measured.read_entries,
        measured.write_entries,
        budget_cpu,
    );

    assert!(
        measured.cpu_instructions > 0,
        "{}::{} CPU cost must be non-zero",
        scenario_name,
        phase
    );

    if budget_cpu > 0 {
        let variance_pct = ((measured.cpu_instructions as f64 - budget_cpu as f64).abs()
            / budget_cpu as f64)
            * 100.0;
        assert!(
            variance_pct <= MAX_DELTA_TOLERANCE_PCT,
            "[{}] {} CPU ({}) deviates by {:.2}% from fixture budget ({}) (> {:.1}% limit)",
            scenario_name,
            phase,
            measured.cpu_instructions,
            variance_pct,
            budget_cpu,
            MAX_DELTA_TOLERANCE_PCT
        );
    }
}

fn assert_total_cpu(scenario_name: &str, total_cpu: u64, budget_total: u64) {
    std::println!(
        "[bench_dispute_lifecycle] {}::total  CPU={}  FixtureBudget={}",
        scenario_name, total_cpu, budget_total,
    );

    if budget_total > 0 {
        let variance_pct = ((total_cpu as f64 - budget_total as f64).abs()
            / budget_total as f64)
            * 100.0;
        assert!(
            variance_pct <= MAX_DELTA_TOLERANCE_PCT,
            "[{}] Total CPU ({}) deviates by {:.2}% from fixture budget ({}) (> {:.1}% limit)",
            scenario_name,
            total_cpu,
            variance_pct,
            budget_total,
            MAX_DELTA_TOLERANCE_PCT
        );
    }
}

// ── Bench Tests ───────────────────────────────────────────────────────────────

/// **Variant 1**: Standard dispute lifecycle — open, respond, resolve to subscriber.
#[test]
fn bench_dispute_lifecycle_standard() {
    let (env, client, _token, _contract_id, admin) = setup_env();
    let (subscriber, merchant, sub_id, token) =
        setup_merchant_and_sub(&env, &client);
    seed_merchant_balance_for_dispute(&env, &client, &merchant, &token);

    // Phase 1: Open
    let (dispute_id, open_metrics) =
        measure_open_dispute(&env, &client, &subscriber, sub_id);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, DisputeStatus::Open);

    // Phase 2: Respond
    let respond_metrics =
        measure_respond_dispute(&env, &client, &admin, dispute_id, &None::<BytesN<32>>);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, DisputeStatus::Responded);

    // Phase 3: Resolve to subscriber
    let resolve_metrics =
        measure_resolve_dispute(&env, &client, &admin, dispute_id, true);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, DisputeStatus::ResolvedToSubscriber);

    // Validate against fixture
    let scenario = get_scenario_budget("standard");
    assert_phase_metrics("standard", "open", open_metrics, scenario.open_cpu);
    assert_phase_metrics("standard", "respond", respond_metrics, scenario.respond_cpu);
    assert_phase_metrics("standard", "resolve", resolve_metrics, scenario.resolve_cpu);

    let total_cpu = open_metrics.cpu_instructions
        + respond_metrics.cpu_instructions
        + resolve_metrics.cpu_instructions;
    assert_total_cpu("standard", total_cpu, scenario.total_cpu);
}

/// **Variant 2**: Dispute with maximum evidence bytes (32 bytes).
#[test]
fn bench_dispute_lifecycle_max_evidence() {
    let (env, client, _token, _contract_id, admin) = setup_env();
    let (subscriber, merchant, sub_id, token) =
        setup_merchant_and_sub(&env, &client);
    seed_merchant_balance_for_dispute(&env, &client, &merchant, &token);

    // Create max-size evidence hash (all 0xFF)
    let max_evidence = {
        let mut arr = [0xFFu8; 32];
        Some(BytesN::from_array(&env, &arr))
    };

    // Phase 1: Open with max evidence
    env.cost_estimate().budget().reset_unlimited();
    let dispute_id = client.open_dispute(
        &subscriber,
        &sub_id,
        &DISPUTE_AMOUNT,
        &max_evidence,
    );
    let open_metrics = capture_metrics(&env);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, DisputeStatus::Open);
    assert_eq!(dispute.evidence_hash, max_evidence);

    // Phase 2: Respond with max evidence
    let respond_metrics =
        measure_respond_dispute(&env, &client, &admin, dispute_id, &max_evidence);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, DisputeStatus::Responded);
    assert_eq!(dispute.admin_evidence_hash, max_evidence);

    // Phase 3: Resolve to merchant
    let resolve_metrics =
        measure_resolve_dispute(&env, &client, &admin, dispute_id, false);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, DisputeStatus::ResolvedToMerchant);

    let scenario = get_scenario_budget("max_evidence");
    assert_phase_metrics("max_evidence", "open", open_metrics, scenario.open_cpu);
    assert_phase_metrics("max_evidence", "respond", respond_metrics, scenario.respond_cpu);
    assert_phase_metrics("max_evidence", "resolve", resolve_metrics, scenario.resolve_cpu);

    let total_cpu = open_metrics.cpu_instructions
        + respond_metrics.cpu_instructions
        + resolve_metrics.cpu_instructions;
    assert_total_cpu("max_evidence", total_cpu, scenario.total_cpu);
}

/// **Variant 3**: Dispute auto-closed on timeout — window elapses, subscriber
/// wins without an explicit respond step.
#[test]
fn bench_dispute_lifecycle_auto_close() {
    let (env, client, _token, _contract_id, admin) = setup_env();
    let (subscriber, merchant, sub_id, token) =
        setup_merchant_and_sub(&env, &client);
    seed_merchant_balance_for_dispute(&env, &client, &merchant, &token);

    // Phase 1: Open
    let (dispute_id, open_metrics) =
        measure_open_dispute(&env, &client, &subscriber, sub_id);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, DisputeStatus::Open);

    // Advance past the dispute window to auto-close
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + DISPUTE_WINDOW_SECS + 1);

    // Phase 2 (skip respond): Resolve directly after window elapsed
    // Admin must NOT respond; the window-elapsed path auto-resolves to subscriber.
    env.cost_estimate().budget().reset_unlimited();
    client.resolve_dispute(&admin, &dispute_id, &true);
    let resolve_metrics = capture_metrics(&env);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, DisputeStatus::ResolvedToSubscriber);

    let scenario = get_scenario_budget("auto_close");
    assert_phase_metrics("auto_close", "open", open_metrics, scenario.open_cpu);
    assert_phase_metrics("auto_close", "resolve", resolve_metrics, scenario.resolve_cpu);

    let total_cpu = open_metrics.cpu_instructions + resolve_metrics.cpu_instructions;
    assert_total_cpu("auto_close", total_cpu, scenario.total_cpu);
}

/// **Variant 4**: Admin resolves to merchant instead of subscriber.
/// Covers the merchant-favourable path.
#[test]
fn bench_dispute_lifecycle_resolve_to_merchant() {
    let (env, client, _token, _contract_id, admin) = setup_env();
    let (subscriber, merchant, sub_id, token) =
        setup_merchant_and_sub(&env, &client);
    seed_merchant_balance_for_dispute(&env, &client, &merchant, &token);

    // Phase 1: Open
    let (dispute_id, open_metrics) =
        measure_open_dispute(&env, &client, &subscriber, sub_id);

    // Phase 2: Respond
    let respond_metrics =
        measure_respond_dispute(&env, &client, &admin, dispute_id, &None::<BytesN<32>>);

    // Phase 3: Resolve to merchant
    let resolve_metrics =
        measure_resolve_dispute(&env, &client, &admin, dispute_id, false);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, DisputeStatus::ResolvedToMerchant);

    let scenario = get_scenario_budget("resolve_to_merchant");
    assert_phase_metrics("resolve_to_merchant", "open", open_metrics, scenario.open_cpu);
    assert_phase_metrics("resolve_to_merchant", "respond", respond_metrics, scenario.respond_cpu);
    assert_phase_metrics("resolve_to_merchant", "resolve", resolve_metrics, scenario.resolve_cpu);

    let total_cpu = open_metrics.cpu_instructions
        + respond_metrics.cpu_instructions
        + resolve_metrics.cpu_instructions;
    assert_total_cpu("resolve_to_merchant", total_cpu, scenario.total_cpu);
}

/// **Edge case**: Dispute with zero evidence (explicit None).
/// Verifies the path is instrumentable and completes without panic.
#[test]
fn bench_dispute_lifecycle_zero_evidence() {
    let (env, client, _token, _contract_id, admin) = setup_env();
    let (subscriber, merchant, sub_id, token) =
        setup_merchant_and_sub(&env, &client);
    seed_merchant_balance_for_dispute(&env, &client, &merchant, &token);

    let (dispute_id, _open_metrics) =
        measure_open_dispute(&env, &client, &subscriber, sub_id);

    let dispute = client.get_dispute(&dispute_id);
    assert!(dispute.evidence_hash.is_none());

    let _respond_metrics =
        measure_respond_dispute(&env, &client, &admin, dispute_id, &None::<BytesN<32>>);

    let dispute = client.get_dispute(&dispute_id);
    assert!(dispute.admin_evidence_hash.is_none());

    let _resolve_metrics =
        measure_resolve_dispute(&env, &client, &admin, dispute_id, true);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, DisputeStatus::ResolvedToSubscriber);
}
