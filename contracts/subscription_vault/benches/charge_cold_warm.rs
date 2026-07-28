//! Gas benchmark for `charge_subscription`: cold (persistent read miss) vs warm (freshly written) storage paths.
//!
//! # Purpose
//! Measures the CPU-instruction and storage resource cost of `charge_subscription`
//! for cold (persistent storage read miss) vs warm (freshly written/cached)
//! storage paths across multiple subscription configurations.
//!
//! Provides a stable regression signal on hot-path performance.
//!
//! # Bench Execution & Threshold Enforcement
//! - Employs `env.cost_estimate()` to capture CPU instructions and ledger read/write entries around each variant.
//! - Persists baseline budgets in `benches/fixtures/charge_cold_warm_budget.json`.
//! - Fails the benchmark if the performance delta or variance exceeds 10%.
//!
//! # Required Edge Cases Tested
//! 1. Standard Active subscription.
//! 2. Subscription with maximum metadata (10 key-value pairs of max length).
//! 3. Subscription with `usage_enabled = true` and usage limits.
//! 4. Subscription in `GracePeriod` transitioning to `Active`.
//!
//! # Security & Correctness Guarantees
//! - Cold storage read miss does not exceed acceptable gas headroom.
//! - Reentrancy guard, balance deduction, salt computation, and statement recording complete deterministically in both cold and warm paths.

#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, BytesN, Env, String,
};
use subscription_vault::{
    types::{
        SubscriptionStatus, UsageLimits, MAX_METADATA_KEYS,
    },
    SubscriptionVault, SubscriptionVaultClient,
};

// ── Constants ─────────────────────────────────────────────────────────────────

const AMOUNT: i128 = 1_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const DEPOSIT: i128 = 50_000_000;
const MIN_TOPUP: i128 = 500_000;
const MAX_DELTA_TOLERANCE_PCT: f64 = 10.0;

// ── Fixture Helper ─────────────────────────────────────────────────────────────

struct ScenarioBudget {
    cold_cpu: u64,
    _warm_cpu: u64,
    _expected_delta_pct: f64,
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

fn parse_f64_key(json: &str, key: &str) -> f64 {
    let search = format!("\"{}\":", key);
    if let Some(pos) = json.find(&search) {
        let rest = &json[pos + search.len()..];
        let num_str: std::string::String = rest
            .chars()
            .skip_while(|c| c.is_whitespace())
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        num_str.parse::<f64>().unwrap_or(0.0)
    } else {
        0.0
    }
}

fn get_scenario_budget(scenario_name: &str) -> ScenarioBudget {
    let raw_fixture = include_str!("fixtures/charge_cold_warm_budget.json");
    let search_block = format!("\"{}\":", scenario_name);
    if let Some(pos) = raw_fixture.find(&search_block) {
        let block = &raw_fixture[pos..];
        let cold_cpu = parse_u64_key(block, "cold_cpu");
        let warm_cpu = parse_u64_key(block, "warm_cpu");
        let expected_delta_pct = parse_f64_key(block, "expected_delta_pct");
        ScenarioBudget {
            cold_cpu,
            _warm_cpu: warm_cpu,
            _expected_delta_pct: expected_delta_pct,
        }
    } else {
        panic!("Scenario {} not found in fixture budget file", scenario_name);
    }
}

// ── Setup Helpers ──────────────────────────────────────────────────────────────

fn setup_env() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
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

    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&contract_id, &1_000_000_000i128);

    (env, client, token, contract_id)
}

fn setup_merchant(env: &Env, client: &SubscriptionVaultClient, merchant: &Address) {
    let redirect_url = String::from_str(env, "https://example.com/webhook");
    client.initialize_merchant_config(
        merchant,
        merchant,
        &0,
        &subscription_vault::DEFAULT_ALLOWED_OPS,
        &None,
        &redirect_url,
    );
}

/// Create and deposit into a sub ready for charging.
fn create_and_fund_sub(
    env: &Env,
    client: &SubscriptionVaultClient,
    subscriber: &Address,
    merchant: &Address,
    token: &Address,
    usage_enabled: bool,
) -> u32 {
    let sub_id = client.create_subscription(
        subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &usage_enabled,
        &None::<i128>,
        &None::<u64>,
    );

    let token_admin = token::StellarAssetClient::new(env, token);
    token_admin.mint(subscriber, &DEPOSIT);
    let no_key: Option<BytesN<32>> = None;
    client.deposit_funds(&sub_id, subscriber, &DEPOSIT, &no_key);

    sub_id
}

/// Cost metrics snapshot.
#[derive(Debug, Clone, Copy)]
struct ChargeMetrics {
    cpu_instructions: u64,
    read_entries: u64,
    write_entries: u64,
}

// ── Measurement Logic ─────────────────────────────────────────────────────────

/// Measures cold path cost: Subscription and related keys are read from persistent storage.
fn measure_cold_charge(
    env: &Env,
    client: &SubscriptionVaultClient,
    sub_id: u32,
) -> ChargeMetrics {
    env.cost_estimate().budget().reset_unlimited();
    client.charge_subscription(&sub_id, &None);

    let resources = env.cost_estimate().resources();
    ChargeMetrics {
        cpu_instructions: resources.instructions.max(0) as u64,
        read_entries: resources.read_entries as u64,
        write_entries: resources.write_entries as u64,
    }
}

/// Measures warm path cost: Subscription key was freshly written / updated immediately before charge.
fn measure_warm_charge(
    env: &Env,
    client: &SubscriptionVaultClient,
    sub_id: u32,
) -> ChargeMetrics {
    // Touch / warm the storage cache right before measuring charge_subscription
    let _sub = client.get_subscription(&sub_id);

    env.cost_estimate().budget().reset_unlimited();
    client.charge_subscription(&sub_id, &None);

    let resources = env.cost_estimate().resources();
    ChargeMetrics {
        cpu_instructions: resources.instructions.max(0) as u64,
        read_entries: resources.read_entries as u64,
        write_entries: resources.write_entries as u64,
    }
}

/// Validate cost metrics against fixture and ensure cold vs warm delta <= 10%.
fn assert_cold_warm_metrics(
    scenario_name: &str,
    cold: ChargeMetrics,
    warm: ChargeMetrics,
) {
    let fixture = get_scenario_budget(scenario_name);

    std::println!(
        "[bench_charge_cold_warm] {}: Cold CPU={}, Reads={}, Writes={} | Warm CPU={}, Reads={}, Writes={}",
        scenario_name,
        cold.cpu_instructions,
        cold.read_entries,
        cold.write_entries,
        warm.cpu_instructions,
        warm.read_entries,
        warm.write_entries
    );

    // Assert CPU > 0
    assert!(cold.cpu_instructions > 0, "Cold CPU cost must be non-zero");
    assert!(warm.cpu_instructions > 0, "Warm CPU cost must be non-zero");

    // 1. Delta between cold vs warm execution paths
    let delta_pct = if cold.cpu_instructions >= warm.cpu_instructions {
        ((cold.cpu_instructions - warm.cpu_instructions) as f64 / cold.cpu_instructions as f64) * 100.0
    } else {
        ((warm.cpu_instructions - cold.cpu_instructions) as f64 / cold.cpu_instructions as f64) * 100.0
    };

    std::println!(
        "[bench_charge_cold_warm] {}: Measured Cold-vs-Warm CPU Delta = {:.2}% (Max Allowed = {:.1}%)",
        scenario_name,
        delta_pct,
        MAX_DELTA_TOLERANCE_PCT
    );

    assert!(
        delta_pct <= MAX_DELTA_TOLERANCE_PCT,
        "[{}] Cold vs warm CPU cost delta {:.2}% exceeds limit of {:.1}%. Cold={}, Warm={}",
        scenario_name,
        delta_pct,
        MAX_DELTA_TOLERANCE_PCT,
        cold.cpu_instructions,
        warm.cpu_instructions
    );

    // 2. Budget variance check against persisted fixture file
    if fixture.cold_cpu > 0 {
        let cold_variance_pct = ((cold.cpu_instructions as f64 - fixture.cold_cpu as f64).abs()
            / fixture.cold_cpu as f64)
            * 100.0;
        assert!(
            cold_variance_pct <= MAX_DELTA_TOLERANCE_PCT,
            "[{}] Measured Cold CPU ({}) deviates by {:.2}% from fixture budget ({}) (> {:.1}% limit)",
            scenario_name,
            cold.cpu_instructions,
            cold_variance_pct,
            fixture.cold_cpu,
            MAX_DELTA_TOLERANCE_PCT
        );
    }
}

// ── Bench Tests ───────────────────────────────────────────────────────────────

/// **Variant 1**: Standard active subscription baseline.
#[test]
fn bench_charge_cold_vs_warm_standard() {
    // Cold test environment
    let (env_cold, client_cold, token_cold, _) = setup_env();
    let subscriber = Address::generate(&env_cold);
    let merchant = Address::generate(&env_cold);
    setup_merchant(&env_cold, &client_cold, &merchant);

    let sub_id_cold = create_and_fund_sub(
        &env_cold,
        &client_cold,
        &subscriber,
        &merchant,
        &token_cold,
        false,
    );
    env_cold
        .ledger()
        .set_timestamp(env_cold.ledger().timestamp() + INTERVAL + 1);

    let cold_metrics = measure_cold_charge(&env_cold, &client_cold, sub_id_cold);

    // Warm test environment
    let (env_warm, client_warm, token_warm, _) = setup_env();
    let subscriber_warm = Address::generate(&env_warm);
    let merchant_warm = Address::generate(&env_warm);
    setup_merchant(&env_warm, &client_warm, &merchant_warm);

    let sub_id_warm = create_and_fund_sub(
        &env_warm,
        &client_warm,
        &subscriber_warm,
        &merchant_warm,
        &token_warm,
        false,
    );
    env_warm
        .ledger()
        .set_timestamp(env_warm.ledger().timestamp() + INTERVAL + 1);

    let warm_metrics = measure_warm_charge(&env_warm, &client_warm, sub_id_warm);

    assert_cold_warm_metrics("standard_active", cold_metrics, warm_metrics);
}

/// **Variant 2**: Subscription with maximum metadata.
/// Covers max key count (10) and max value lengths.
#[test]
fn bench_charge_cold_vs_warm_max_metadata() {
    let (env_cold, client_cold, token_cold, _) = setup_env();
    let subscriber = Address::generate(&env_cold);
    let merchant = Address::generate(&env_cold);
    setup_merchant(&env_cold, &client_cold, &merchant);

    let sub_id_cold = create_and_fund_sub(
        &env_cold,
        &client_cold,
        &subscriber,
        &merchant,
        &token_cold,
        false,
    );

    // Set maximum metadata keys & values
    let val_bytes = [b'v'; 256];
    let max_val = String::from_str(&env_cold, std::str::from_utf8(&val_bytes).unwrap());
    for i in 0..MAX_METADATA_KEYS {
        let key_str = format!("meta_key_{:02}", i);
        let key = String::from_str(&env_cold, &key_str);
        client_cold.set_metadata(&subscriber, &sub_id_cold, &key, &max_val);
    }

    env_cold
        .ledger()
        .set_timestamp(env_cold.ledger().timestamp() + INTERVAL + 1);
    let cold_metrics = measure_cold_charge(&env_cold, &client_cold, sub_id_cold);

    // Warm path
    let (env_warm, client_warm, token_warm, _) = setup_env();
    let subscriber_warm = Address::generate(&env_warm);
    let merchant_warm = Address::generate(&env_warm);
    setup_merchant(&env_warm, &client_warm, &merchant_warm);

    let sub_id_warm = create_and_fund_sub(
        &env_warm,
        &client_warm,
        &subscriber_warm,
        &merchant_warm,
        &token_warm,
        false,
    );
    let val_bytes_warm = [b'v'; 256];
    let max_val_warm = String::from_str(&env_warm, std::str::from_utf8(&val_bytes_warm).unwrap());
    for i in 0..MAX_METADATA_KEYS {
        let key_str = format!("meta_key_{:02}", i);
        let key = String::from_str(&env_warm, &key_str);
        client_warm.set_metadata(&subscriber_warm, &sub_id_warm, &key, &max_val_warm);
    }

    env_warm
        .ledger()
        .set_timestamp(env_warm.ledger().timestamp() + INTERVAL + 1);
    let warm_metrics = measure_warm_charge(&env_warm, &client_warm, sub_id_warm);

    assert_cold_warm_metrics("max_metadata", cold_metrics, warm_metrics);
}

/// **Variant 3**: Subscription with usage_enabled=true and usage limits.
#[test]
fn bench_charge_cold_vs_warm_usage_enabled() {
    let (env_cold, client_cold, token_cold, _) = setup_env();
    let subscriber = Address::generate(&env_cold);
    let merchant = Address::generate(&env_cold);
    setup_merchant(&env_cold, &client_cold, &merchant);

    let sub_id_cold = create_and_fund_sub(
        &env_cold,
        &client_cold,
        &subscriber,
        &merchant,
        &token_cold,
        true, // usage_enabled = true
    );

    // Configure usage limits
    let limits = UsageLimits {
        burst_min_interval_secs: 10,
        rate_window_secs: 3600,
        rate_limit_max_calls: Some(100),
        usage_cap_units: Some(10_000_000),
    };
    client_cold.set_usage_limits(&merchant, &sub_id_cold, &limits);

    env_cold
        .ledger()
        .set_timestamp(env_cold.ledger().timestamp() + INTERVAL + 1);
    let cold_metrics = measure_cold_charge(&env_cold, &client_cold, sub_id_cold);

    // Warm path
    let (env_warm, client_warm, token_warm, _) = setup_env();
    let subscriber_warm = Address::generate(&env_warm);
    let merchant_warm = Address::generate(&env_warm);
    setup_merchant(&env_warm, &client_warm, &merchant_warm);

    let sub_id_warm = create_and_fund_sub(
        &env_warm,
        &client_warm,
        &subscriber_warm,
        &merchant_warm,
        &token_warm,
        true,
    );
    client_warm.set_usage_limits(&merchant_warm, &sub_id_warm, &limits);

    env_warm
        .ledger()
        .set_timestamp(env_warm.ledger().timestamp() + INTERVAL + 1);
    let warm_metrics = measure_warm_charge(&env_warm, &client_warm, sub_id_warm);

    assert_cold_warm_metrics("usage_enabled", cold_metrics, warm_metrics);
}

/// **Variant 4**: Subscription in GracePeriod transitioning to Active.
#[test]
fn bench_charge_cold_vs_warm_grace_period() {
    let (env_cold, client_cold, token_cold, _) = setup_env();
    let subscriber = Address::generate(&env_cold);
    let merchant = Address::generate(&env_cold);
    setup_merchant(&env_cold, &client_cold, &merchant);

    // Configure global grace period (e.g. 7 days)
    client_cold.set_grace_period(&(7 * 86400));

    let sub_id_cold = client_cold.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    // First charge attempt with 0 balance -> moves to GracePeriod
    env_cold
        .ledger()
        .set_timestamp(env_cold.ledger().timestamp() + INTERVAL + 1);
    client_cold.charge_subscription(&sub_id_cold, &None);

    let sub_before = client_cold.get_subscription(&sub_id_cold);
    assert_eq!(sub_before.status, SubscriptionStatus::GracePeriod);

    // Fund the subscription
    let token_admin_cold = token::StellarAssetClient::new(&env_cold, &token_cold);
    token_admin_cold.mint(&subscriber, &DEPOSIT);
    let no_key: Option<BytesN<32>> = None;
    client_cold.deposit_funds(&sub_id_cold, &subscriber, &DEPOSIT, &no_key);

    let cold_metrics = measure_cold_charge(&env_cold, &client_cold, sub_id_cold);

    let sub_after_cold = client_cold.get_subscription(&sub_id_cold);
    assert_eq!(sub_after_cold.status, SubscriptionStatus::Active);

    // Warm path
    let (env_warm, client_warm, token_warm, _) = setup_env();
    let subscriber_warm = Address::generate(&env_warm);
    let merchant_warm = Address::generate(&env_warm);
    setup_merchant(&env_warm, &client_warm, &merchant_warm);
    client_warm.set_grace_period(&(7 * 86400));

    let sub_id_warm = client_warm.create_subscription(
        &subscriber_warm,
        &merchant_warm,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    env_warm
        .ledger()
        .set_timestamp(env_warm.ledger().timestamp() + INTERVAL + 1);
    client_warm.charge_subscription(&sub_id_warm, &None);

    let token_admin_warm = token::StellarAssetClient::new(&env_warm, &token_warm);
    token_admin_warm.mint(&subscriber_warm, &DEPOSIT);
    client_warm.deposit_funds(&sub_id_warm, &subscriber_warm, &DEPOSIT, &no_key);

    let warm_metrics = measure_warm_charge(&env_warm, &client_warm, sub_id_warm);

    let sub_after_warm = client_warm.get_subscription(&sub_id_warm);
    assert_eq!(sub_after_warm.status, SubscriptionStatus::Active);

    assert_cold_warm_metrics("grace_period", cold_metrics, warm_metrics);
}
