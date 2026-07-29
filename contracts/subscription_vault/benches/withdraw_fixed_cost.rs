//! Fixed-cost budget benchmark for `withdraw_merchant_funds`.
//!
//! Pins the CPU-instruction cost of a merchant withdrawal to a baseline stored
//! in `fixtures/withdraw_fixed_cost_budget.json`. The test fails if any future
//! change causes cost to exceed the baseline by more than `tolerance_pct`,
//! acting as a canary for accidental storage churn in the withdrawal path.
//!
//! # Scenarios
//! 1. **standard** — single-token withdraw of a full balance.
//! 2. **multi_token** — merchant has earnings in two tokens; withdraw each.
//! 3. **cap_boundary** — withdraw exactly `amount == balance` (edge case: leaves balance at 0).
//!
//! # Security notes
//! - All scenarios follow the CEI pattern in `merchant.rs` — effects (balance
//!   zeroed, earnings updated) are written before the token transfer.
//! - The benchmark does not test dispute-blocked withdrawals because that guard
//!   lives in the entrypoint layer (`lib.rs`), not the cost-sensitive inner path.

#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env, String,
};
use subscription_vault::{SubscriptionVault, SubscriptionVaultClient};

// ── constants ─────────────────────────────────────────────────────────────────

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const DEPOSIT: i128 = 50_000_000;
const MIN_TOPUP: i128 = 1_000_000;

// ── baseline helpers ──────────────────────────────────────────────────────────

fn baseline_cpu(scenario: &str) -> u64 {
    let json = include_str!("fixtures/withdraw_fixed_cost_budget.json");
    // find  "scenario": { "cpu": NNN }
    let marker = format!("\"{}\"", scenario);
    let pos = json.find(&marker).expect("scenario not found in baseline");
    let block = &json[pos..];
    let cpu_key = "\"cpu\":";
    let kpos = block.find(cpu_key).expect("cpu key not found in scenario block");
    let rest = &block[kpos + cpu_key.len()..];
    rest.chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect::<std::string::String>()
        .parse::<u64>()
        .expect("cpu value not a number")
}

fn tolerance_pct() -> f64 {
    let json = include_str!("fixtures/withdraw_fixed_cost_budget.json");
    let key = "\"tolerance_pct\":";
    let pos = json.find(key).expect("tolerance_pct not found");
    let rest = &json[pos + key.len()..];
    rest.chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect::<std::string::String>()
        .parse::<f64>()
        .unwrap_or(10.0)
}

fn assert_within_budget(scenario: &str, measured: u64) {
    let baseline = baseline_cpu(scenario);
    let tol = tolerance_pct();
    std::println!(
        "[withdraw_fixed_cost] {}: measured={} baseline={} tolerance={:.0}%",
        scenario, measured, baseline, tol
    );
    if baseline > 0 {
        let over = measured.saturating_sub(baseline) as f64 / baseline as f64 * 100.0;
        assert!(
            over <= tol,
            "[{}] CPU cost {} exceeds baseline {} by {:.1}% (limit {:.0}%). \
             Update the baseline in fixtures/withdraw_fixed_cost_budget.json \
             with a documented rationale if the increase is intentional.",
            scenario, measured, baseline, over, tol
        );
    }
    assert!(measured > 0, "CPU cost must be non-zero");
}

// ── setup ──────────────────────────────────────────────────────────────────────

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
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
    // Seed the vault so withdrawals can succeed
    token::StellarAssetClient::new(&env, &token).mint(&contract_id, &1_000_000_000i128);

    (env, client, token, admin)
}

/// Register a merchant config and return the merchant address.
fn setup_merchant(env: &Env, client: &SubscriptionVaultClient) -> Address {
    let merchant = Address::generate(env);
    let url = String::from_str(env, "https://example.com");
    client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0,
        &subscription_vault::DEFAULT_ALLOWED_OPS,
        &None,
        &url,
    );
    merchant
}

/// Create a subscription, deposit funds, advance time, and charge once so the
/// merchant has a non-zero balance to withdraw.
fn fund_merchant_balance(
    env: &Env,
    client: &SubscriptionVaultClient,
    token: &Address,
    merchant: &Address,
) {
    let subscriber = Address::generate(env);
    let sub_id = client.create_subscription(
        &subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    token::StellarAssetClient::new(env, token).mint(&subscriber, &DEPOSIT);
    client.deposit_funds(&sub_id, &subscriber, &DEPOSIT, &None);
    env.ledger().set_timestamp(env.ledger().timestamp() + INTERVAL + 1);
    client.charge_subscription(&sub_id, &None);
}

// ── bench tests ───────────────────────────────────────────────────────────────

/// Standard single-token withdraw: merchant has a balance in the primary token
/// and withdraws the full amount.
#[test]
fn bench_withdraw_standard() {
    let (env, client, token, _admin) = setup();
    let merchant = setup_merchant(&env, &client);
    fund_merchant_balance(&env, &client, &token, &merchant);

    let balance = client.get_merchant_balance_by_token(&merchant, &token);
    assert!(balance > 0, "merchant must have a balance before bench");

    env.cost_estimate().budget().reset_unlimited();
    client.withdraw_merchant_token_funds(&merchant, &token, &balance);
    let cpu = env.cost_estimate().resources().instructions.max(0) as u64;

    assert_within_budget("standard", cpu);
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        0,
        "balance must be zero after full withdrawal"
    );
}

/// Multi-token withdraw: merchant has earnings in two different tokens.
/// Measures each withdrawal independently and checks both against the budget.
#[test]
fn bench_withdraw_multi_token() {
    let (env, client, token1, admin) = setup();
    let contract_id = client.address.clone();

    // Register a second token
    let token2 = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    client.add_accepted_token(&admin, &token2, &6u32);
    token::StellarAssetClient::new(&env, &token2).mint(&contract_id, &1_000_000_000i128);

    let merchant = setup_merchant(&env, &client);

    // Build balance in token1
    fund_merchant_balance(&env, &client, &token1, &merchant);

    // Build balance in token2 via a second subscription
    let subscriber2 = Address::generate(&env);
    let sub2 = client.create_subscription(
        &subscriber2,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    token::StellarAssetClient::new(&env, &token2).mint(&subscriber2, &DEPOSIT);
    client.deposit_funds(&sub2, &subscriber2, &DEPOSIT, &None);
    env.ledger().set_timestamp(env.ledger().timestamp() + INTERVAL + 1);
    client.charge_subscription(&sub2, &None);

    let bal1 = client.get_merchant_balance_by_token(&merchant, &token1);
    let bal2 = client.get_merchant_balance_by_token(&merchant, &token2);
    assert!(bal1 > 0 && bal2 > 0);

    // Measure token1 withdraw
    env.cost_estimate().budget().reset_unlimited();
    client.withdraw_merchant_token_funds(&merchant, &token1, &bal1);
    let cpu1 = env.cost_estimate().resources().instructions.max(0) as u64;

    // Measure token2 withdraw
    env.cost_estimate().budget().reset_unlimited();
    client.withdraw_merchant_token_funds(&merchant, &token2, &bal2);
    let cpu2 = env.cost_estimate().resources().instructions.max(0) as u64;

    // Report the higher of the two (worst case for multi-token)
    assert_within_budget("multi_token", cpu1.max(cpu2));
}

/// Cap boundary: withdraw exactly `balance` (the maximum permissible amount).
/// Verifies cost is stable at the edge and balance lands at exactly 0.
#[test]
fn bench_withdraw_cap_boundary() {
    let (env, client, token, _admin) = setup();
    let merchant = setup_merchant(&env, &client);
    fund_merchant_balance(&env, &client, &token, &merchant);

    let balance = client.get_merchant_balance_by_token(&merchant, &token);
    assert!(balance > 0);

    // Withdraw exactly the full balance
    env.cost_estimate().budget().reset_unlimited();
    client.withdraw_merchant_token_funds(&merchant, &token, &balance);
    let cpu = env.cost_estimate().resources().instructions.max(0) as u64;

    assert_within_budget("cap_boundary", cpu);
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        0,
        "balance must be 0 after cap-boundary withdrawal"
    );
}
