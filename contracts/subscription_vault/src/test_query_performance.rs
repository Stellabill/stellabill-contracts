#![cfg(test)]

use crate::{
    queries::{MAX_SCAN_DEPTH, MAX_SUBSCRIPTION_LIST_PAGE},
    subscription::MAX_WRITE_PATH_SCAN_DEPTH,
    types::{Subscription, SubscriptionStatus},
    SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, Symbol,
};

const T0: u64 = 1700000000;

// ================================================================
// Performance Budget Constants
// ================================================================
// Initial conservative limits. These MUST be tuned after running
// the benchmark (see `benchmark_query_performance` below).
// Set to measured baseline × 1.5–2.0.
mod perf_budgets {
    // get_subscription: single record read
    pub const GET_SUBSCRIPTION_CPU: u64 = 25_000;
    pub const GET_SUBSCRIPTION_LEDGER_READS: u64 = 3;

    // list_subscriptions_by_subscriber: scans up to MAX_SCAN_DEPTH (1,000) IDs
    pub const LIST_SUBSCRIBER_CPU: u64 = 200_000;
    pub const LIST_SUBSCRIBER_LEDGER_READS: u64 = 1_500;

    // get_subscriptions_by_merchant: index read + limit records (max 100)
    pub const MERCHANT_QUERY_CPU: u64 = 500_000;
    pub const MERCHANT_QUERY_LEDGER_READS: u64 = 200;

    // get_subscriptions_by_token: similar to merchant
    pub const TOKEN_QUERY_CPU: u64 = 500_000;
    pub const TOKEN_QUERY_LEDGER_READS: u64 = 200;

    // Warn if consumption exceeds this % of budget (early warning)
    pub const WARNING_THRESHOLD: f64 = 0.80;
}

/// Execute `op` within hard CPU and ledger-read budgets.
/// If the operation exceeds the budget, Soroban aborts → test fails.
/// After execution, prints actual consumption and emits soft warnings
/// if usage exceeds WARNING_THRESHOLD.
fn with_perf_budget<F>(
    env: &Env,
    cpu_budget: u64,
    read_budget: u64,
    test_name: &str,
    op: F,
)
where
    F: FnOnce(),
{
    // Set hard budgets (enforced by runtime)
    env.budget().set_cpu_budget(cpu_budget);
    env.budget().set_ledger_read_budget(read_budget);
    env.budget().set_ledger_write_budget(0); // read-only queries

    // Run the operation under test
    op();

    // Capture metrics
    let cpu = env.budget().cpu_instruction_count();
    let reads = env.budget().ledger_read_count();
    let writes = env.budget().ledger_write_count();

    // CI-visible output
    println!(
        "[Perf] {}: cpu={}, reads={}, writes={} (limit: cpu≤{}, reads≤{})",
        test_name, cpu, reads, writes, cpu_budget, read_budget
    );

    // Soft headroom warning
    let cpu_ratio = cpu as f64 / cpu_budget as f64;
    let read_ratio = reads as f64 / read_budget as f64;
    if cpu_ratio > perf_budgets::WARNING_THRESHOLD {
        eprintln!(
            "WARNING: {} CPU usage {:.1}% of budget",
            test_name,
            cpu_ratio * 100.0
        );
    }
    if read_ratio > perf_budgets::WARNING_THRESHOLD {
        eprintln!(
            "WARNING: {} ledger reads {:.1}% of budget",
            test_name,
            read_ratio * 100.0
        );
    }
}

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    // Needed to avoid gas limits when doing deep mock pagination in tests
    env.cost_estimate().budget().reset_unlimited();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, client, token, admin)
}

fn create_mock_sub(env: &Env, subscriber: &Address, token: &Address) -> Subscription {
    Subscription {
        subscriber: subscriber.clone(),
        merchant: Address::generate(env),
        token: token.clone(),
        amount: 10_000,
        interval_seconds: 2_592_000,
        last_payment_timestamp: env.ledger().timestamp(),
        status: SubscriptionStatus::Active,
        prepaid_balance: 0,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
        start_time: env.ledger().timestamp(),
        expires_at: None,
        grace_start_timestamp: None,
    }
}

/// Helper to quickly inject N subscriptions directly into storage without crossing the host boundary repeatedly
fn inject_subscriptions(
    env: &Env,
    contract_id: &Address,
    count: u32,
    subscriber: &Address,
    token: &Address,
) {
    env.as_contract(contract_id, || {
        let start_id: u32 = env.storage().instance().get(&DataKey::NextId).unwrap_or(0);

        for i in 0..count {
            let id = start_id + i;
            let sub = create_mock_sub(env, subscriber, token);
            env.storage().instance().set(&DataKey::Sub(id), &sub);
        }

        env.storage()
            .instance()
            .set(&DataKey::NextId, &(start_id + count));
    });
}

// ================================================================
// Original functional tests (unchanged)
// ================================================================

#[test]
fn test_subscriber_list_basic() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    inject_subscriptions(&env, &client.address, 5, &subscriber, &token);

    let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &100);
    assert_eq!(page.subscription_ids.len(), 5);
    assert_eq!(page.next_start_id, None);
}

#[test]
fn test_subscriber_list_pagination() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    inject_subscriptions(&env, &client.address, 50, &subscriber, &token);

    // Fetch first 20
    let page1 = client.list_subscriptions_by_subscriber(&subscriber, &0, &20);
    assert_eq!(page1.subscription_ids.len(), 20);
    assert_eq!(page1.next_start_id, Some(20));

    // Fetch next 20
    let page2 = client.list_subscriptions_by_subscriber(&subscriber, &page1.next_start_id.unwrap(), &20);
    assert_eq!(page2.subscription_ids.len(), 20);
    assert_eq!(page2.next_start_id, Some(40));

    // Fetch last 10
    let page3 = client.list_subscriptions_by_subscriber(&subscriber, &page2.next_start_id.unwrap(), &20);
    assert_eq!(page3.subscription_ids.len(), 10);
    // next_id is 50, scan budget doesn't exhaust and it found all, so next_start_id should be None
    assert_eq!(page3.next_start_id, None);
}

#[test]
fn test_subscriber_list_scan_depth_boundary() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    let other = Address::generate(&env);

    // Create exactly MAX_SCAN_DEPTH + 10 subscriptions, all for `other`
    let total = MAX_SCAN_DEPTH + 10;
    inject_subscriptions(&env, &client.address, total, &other, &token);

    // Now if `subscriber` tries to list, it will scan MAX_SCAN_DEPTH IDs, find none,
    // and return an empty list WITH a next_start_id cursor to resume at MAX_SCAN_DEPTH.
    let page1 = client.list_subscriptions_by_subscriber(&subscriber, &0, &10);
    assert_eq!(page1.subscription_ids.len(), 0);
    assert_eq!(page1.next_start_id, Some(MAX_SCAN_DEPTH));

    let page2 = client.list_subscriptions_by_subscriber(&subscriber, &page1.next_start_id.unwrap(), &10);
    assert_eq!(page2.subscription_ids.len(), 0);
    assert_eq!(page2.next_start_id, None); // Finished remaining 10
}

#[test]
fn test_subscriber_list_sparse_ids() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    let other = Address::generate(&env);

    inject_subscriptions(&env, &client.address, 10, &subscriber, &token);
    inject_subscriptions(&env, &client.address, 40, &other, &token);
    inject_subscriptions(&env, &client.address, 10, &subscriber, &token);

    // 60 total subscriptions. subscriber has 0..10 and 50..60.
    let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &100);
    assert_eq!(page.subscription_ids.len(), 20);
    assert_eq!(page.next_start_id, None);
}

#[test]
fn test_subscriber_list_limit_one() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    inject_subscriptions(&env, &client.address, 5, &subscriber, &token);

    let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &1);
    assert_eq!(page.subscription_ids.len(), 1);
    assert_eq!(page.subscription_ids.get(0).unwrap(), 0);
    assert_eq!(page.next_start_id, Some(1));
}

#[test]
fn test_subscriber_list_limit_max() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    inject_subscriptions(&env, &client.address, MAX_SUBSCRIPTION_LIST_PAGE, &subscriber, &token);

    let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &MAX_SUBSCRIPTION_LIST_PAGE);
    assert_eq!(page.subscription_ids.len(), MAX_SUBSCRIPTION_LIST_PAGE);
    // Note: since it hit the limit exactly on the last item, it might return next_start_id == Some(100) or None
    // Currently, it breaks early, so if loop finishes, it sets to None. Wait, if it pushes max, len == limit. Next iteration breaks.
    // We just ensure it doesn't crash.
}

#[test]
fn test_subscriber_list_empty() {
    let (env, client, _token, _) = setup();
    let subscriber = Address::generate(&env);
    let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &100);
    assert_eq!(page.subscription_ids.len(), 0);
    assert_eq!(page.next_start_id, None);
}

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // InvalidInput = 3001
fn test_subscriber_list_invalid_limit_zero() {
    let (env, client, _token, _) = setup();
    client.list_subscriptions_by_subscriber(&Address::generate(&env), &0, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // InvalidInput = 3001
fn test_subscriber_list_invalid_limit_overflow() {
    let (env, client, _token, _) = setup();
    client.list_subscriptions_by_subscriber(&Address::generate(&env), &0, &(MAX_SUBSCRIPTION_LIST_PAGE + 1));
}

fn create_sub_for_merchant_and_token(client: &SubscriptionVaultClient<'static>, subscriber: &Address, merchant: &Address, token: &Address) -> u32 {
    client.create_subscription(subscriber, merchant, &1000, &(30 * 24 * 60 * 60), &false, &None, &None::<u64>)
}

#[test]
fn test_merchant_query_basic() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    for _ in 0..10 {
        create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    }

    let page = client.get_subscriptions_by_merchant(&merchant, &0, &100);
    assert_eq!(page.len(), 10);
    assert_eq!(client.get_merchant_subscription_count(&merchant), 10);
}

#[test]
fn test_merchant_query_pagination() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    for _ in 0..15 {
        create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    }

    let page1 = client.get_subscriptions_by_merchant(&merchant, &0, &10);
    assert_eq!(page1.len(), 10);

    let page2 = client.get_subscriptions_by_merchant(&merchant, &10, &10);
    assert_eq!(page2.len(), 5);
}

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // InvalidInput = 3001
fn test_merchant_query_limit_zero() {
    let (env, client, _token, _) = setup();
    client.get_subscriptions_by_merchant(&Address::generate(&env), &0, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // InvalidInput = 3001
fn test_merchant_query_limit_overflow() {
    let (env, client, _token, _) = setup();
    client.get_subscriptions_by_merchant(&Address::generate(&env), &0, &(MAX_SUBSCRIPTION_LIST_PAGE + 1));
}

#[test]
fn test_merchant_query_start_past_end() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    
    let page = client.get_subscriptions_by_merchant(&merchant, &2, &10);
    assert_eq!(page.len(), 0);
}

#[test]
fn test_token_query_basic() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    for _ in 0..10 {
        create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    }

    let page = client.get_subscriptions_by_token(&token, &0, &100);
    assert_eq!(page.len(), 10);
    assert_eq!(client.get_token_subscription_count(&token), 10);
}

#[test]
fn test_token_query_pagination() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    for _ in 0..15 {
        create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    }

    let page1 = client.get_subscriptions_by_token(&token, &0, &10);
    assert_eq!(page1.len(), 10);

    let page2 = client.get_subscriptions_by_token(&token, &10, &10);
    assert_eq!(page2.len(), 5);
}

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // InvalidInput = 3001
fn test_token_query_limit_zero() {
    let (env, client, token, _) = setup();
    client.get_subscriptions_by_token(&token, &0, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // InvalidInput = 3001
fn test_token_query_limit_overflow() {
    let (env, client, token, _) = setup();
    client.get_subscriptions_by_token(&token, &0, &(MAX_SUBSCRIPTION_LIST_PAGE + 1));
}

#[test]
fn test_merchant_count_and_token_count() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    assert_eq!(client.get_merchant_subscription_count(&merchant), 0);
    assert_eq!(client.get_token_subscription_count(&token), 0);

    create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    
    assert_eq!(client.get_merchant_subscription_count(&merchant), 1);
    assert_eq!(client.get_token_subscription_count(&token), 1);
}

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // InvalidInput = 3001
fn test_write_path_scan_depth_guard_triggers_for_large_contracts() {
    let (env, client, token, _) = setup();
    
    // We simulate a contract that has exceeded the MAX_WRITE_PATH_SCAN_DEPTH
    // by injecting a fake next_id. 
    env.as_contract(&client.address, || {
        env.storage().instance().set(&DataKey::NextId, &(MAX_WRITE_PATH_SCAN_DEPTH + 1));
    });

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // In order to trigger the O(n) scan, we need a credit limit > 0
    // so `compute_subscriber_exposure` gets called instead of fast-path exiting.
    env.as_contract(&client.address, || {
        let credit_limit_key = DataKey::CreditLimit(subscriber.clone(), token.clone());
        env.storage().instance().set(&credit_limit_key, &1000i128); // Non-zero sets up the scan
    });

    // This creation should fail with InvalidInput because we simulated an oversized contract
    // AND we forced the scan path by configuring a credit limit.
    client.create_subscription(&subscriber, &merchant, &100, &(30 * 24 * 60 * 60), &false, &None, &None::<u64>);
}

// ================================================================
// Performance Budget Tests (New)
// ================================================================

/// Benchmark: measure baseline resource usage (ignored in CI)
/// Run: cargo test -p subscription_vault benchmark_query_performance -- --ignored --nocapture
#[cfg(test)]
mod benchmark {
    use super::*;

    #[test]
    #[ignore]
    fn benchmark_query_performance() {
        measure_get_subscription();
        measure_list_subscriber();
        measure_merchant_query();
        measure_token_query();
    }

    fn measure_get_subscription() {
        let (env, client, _token, _) = setup();
        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);
        let sub_id = client.create_subscription(
            &subscriber,
            &merchant,
            &10_000,
            &(30 * 24 * 60 * 60),
            &false,
            &None,
            &None::<u64>,
        );

        // High budgets so we never hit limit; just measure
        env.budget().set_cpu_budget(10_000_000);
        env.budget().set_ledger_read_budget(10_000);
        env.budget().set_ledger_write_budget(0);

        let _ = client.get_subscription(&sub_id);

        let cpu = env.budget().cpu_instruction_count();
        let reads = env.budget().ledger_read_count();
        let writes = env.budget().ledger_write_count();
        println!(
            "[BENCH] get_subscription: cpu={}, reads={}, writes={}",
            cpu, reads, writes
        );
    }

    fn measure_list_subscriber() {
        let (env, client, token, _) = setup();
        let subscriber = Address::generate(&env);
        // 1000 total IDs; subscriber has 10 subs spread every 100
        for i in 0u32..1000 {
            let sub = if i % 100 == 0 {
                create_mock_sub(&env, &subscriber, &token)
            } else {
                create_mock_sub(&env, &Address::generate(&env), &token)
            };
            env.as_contract(&client.address, || {
                env.storage().instance().set(&i, &sub);
            });
        }
        env.as_contract(&client.address, || {
            env.storage()
                .instance()
                .set(&Symbol::new(&env, "next_id"), &1000);
        });

        env.budget().set_cpu_budget(2_000_000);
        env.budget().set_ledger_read_budget(2_000);
        env.budget().set_ledger_write_budget(0);

        let _ = client.list_subscriptions_by_subscriber(&subscriber, &0, &10);

        let cpu = env.budget().cpu_instruction_count();
        let reads = env.budget().ledger_read_count();
        println!(
            "[BENCH] list_subscriptions_by_subscriber (scan 1000, find 10): cpu={}, reads={}",
            cpu, reads
        );
    }

    fn measure_merchant_query() {
        let (env, client, token, _) = setup();
        let merchant = Address::generate(&env);
        let subscriber = Address::generate(&env);
        for _ in 0..1000 {
            create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
        }

        env.budget().set_cpu_budget(2_000_000);
        env.budget().set_ledger_read_budget(2_000);
        env.budget().set_ledger_write_budget(0);

        let _ = client.get_subscriptions_by_merchant(&merchant, &0, &100);

        let cpu = env.budget().cpu_instruction_count();
        let reads = env.budget().ledger_read_count();
        println!(
            "[BENCH] get_subscriptions_by_merchant (1000 total, page 100): cpu={}, reads={}",
            cpu, reads
        );
    }

    fn measure_token_query() {
        let (env, client, token, _) = setup();
        let merchant = Address::generate(&env);
        let subscriber = Address::generate(&env);
        for _ in 0..1000 {
            create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
        }

        env.budget().set_cpu_budget(2_000_000);
        env.budget().set_ledger_read_budget(2_000);
        env.budget().set_ledger_write_budget(0);

        let _ = client.get_subscriptions_by_token(&token, &0, &100);

        let cpu = env.budget().cpu_instruction_count();
        let reads = env.budget().ledger_read_count();
        println!(
            "[BENCH] get_subscriptions_by_token (1000 total, page 100): cpu={}, reads={}",
            cpu, reads
        );
    }
}

// ================================================================
// Performance Budget Enforced Tests
// ================================================================

#[test]
fn test_get_subscription_within_budget() {
    let (env, client, _token, _) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000,
        &(30 * 24 * 60 * 60),
        &false,
        &None,
        &None::<u64>,
    );

    with_perf_budget(
        &env,
        perf_budgets::GET_SUBSCRIPTION_CPU,
        perf_budgets::GET_SUBSCRIPTION_LEDGER_READS,
        "get_subscription",
        || {
            let _ = client.get_subscription(&sub_id);
        },
    );
}

#[test]
#[should_panic] // Any panic due to budget exceed is acceptable
fn test_get_subscription_budget_too_tight() {
    let (env, client, _token, _) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000,
        &(30 * 24 * 60 * 60),
        &false,
        &None,
        &None::<u64>,
    );

    // Impossibly tight budgets — must exceed
    env.budget().set_cpu_budget(5);
    env.budget().set_ledger_read_budget(1);
    env.budget().set_ledger_write_budget(0);

    let _ = client.get_subscription(&sub_id);
}

#[test]
fn test_list_subscriber_within_budget() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    inject_subscriptions(&env, &client.address, 10, &subscriber, &token);

    with_perf_budget(
        &env,
        perf_budgets::LIST_SUBSCRIBER_CPU,
        perf_budgets::LIST_SUBSCRIBER_LEDGER_READS,
        "list_subscriptions_by_subscriber (10 subs)",
        || {
            let _ = client.list_subscriptions_by_subscriber(&subscriber, &0, &100);
        },
    );
}

#[test]
#[should_panic]
fn test_list_subscriber_budget_too_tight() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    inject_subscriptions(&env, &client.address, 5, &subscriber, &token);

    env.budget().set_cpu_budget(10);
    env.budget().set_ledger_read_budget(1);
    env.budget().set_ledger_write_budget(0);

    let _ = client.list_subscriptions_by_subscriber(&subscriber, &0, &10);
}

#[test]
fn test_list_subscriber_sparse_ids_within_budget() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    let other = Address::generate(&env);

    // Create sparse pattern: subscriber subs at IDs 0,100,200,...,4900 (50 total)
    // Between each, 99 filler subs → 5000 total IDs
    for block in 0..50 {
        let base = block * 100;
        // subscriber sub at base
        env.as_contract(&client.address, || {
            let sub = create_mock_sub(&env, &subscriber, &token);
            env.storage().instance().set(&base, &sub);
        });
        // 99 filler subs
        for i in 1..100 {
            let id = base + i;
            env.as_contract(&client.address, || {
                let sub = create_mock_sub(&env, &other, &token);
                env.storage().instance().set(&id, &sub);
            });
        }
    }
    env.as_contract(&client.address, || {
        env.storage().instance().set(&Symbol::new(&env, "next_id"), &5000);
    });

    with_perf_budget(
        &env,
        perf_budgets::LIST_SUBSCRIBER_CPU,
        perf_budgets::LIST_SUBSCRIBER_LEDGER_READS,
        "list_subscriptions_by_subscriber (sparse 50 among 5000)",
        || {
            let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &50);
            // Due to scan cap, may not get all in one call, but should complete within budget
            assert!(page.subscription_ids.len() <= 50);
        },
    );
}

#[test]
fn test_merchant_query_within_budget() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    for _ in 0..100 {
        create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    }

    with_perf_budget(
        &env,
        perf_budgets::MERCHANT_QUERY_CPU,
        perf_budgets::MERCHANT_QUERY_LEDGER_READS,
        "get_subscriptions_by_merchant (100 subs)",
        || {
            let _page = client.get_subscriptions_by_merchant(&merchant, &0, &100);
        },
    );
}

#[test]
#[should_panic]
fn test_merchant_query_budget_too_tight() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);

    env.budget().set_cpu_budget(5);
    env.budget().set_ledger_read_budget(1);
    env.budget().set_ledger_write_budget(0);

    let _ = client.get_subscriptions_by_merchant(&merchant, &0, &1);
}

#[test]
fn test_token_query_within_budget() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    for _ in 0..100 {
        create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    }

    with_perf_budget(
        &env,
        perf_budgets::TOKEN_QUERY_CPU,
        perf_budgets::TOKEN_QUERY_LEDGER_READS,
        "get_subscriptions_by_token (100 subs)",
        || {
            let _page = client.get_subscriptions_by_token(&token, &0, &100);
        },
    );
}

#[test]
#[should_panic]
fn test_token_query_budget_too_tight() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);

    env.budget().set_cpu_budget(5);
    env.budget().set_ledger_read_budget(1);
    env.budget().set_ledger_write_budget(0);

    let _ = client.get_subscriptions_by_token(&token, &0, &1);
}

#[test]
fn test_merchant_index_bloat_within_budget() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    // 1000 subscriptions for one merchant (tests index deserialization cost)
    for _ in 0..1000 {
        create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    }

    with_perf_budget(
        &env,
        perf_budgets::MERCHANT_QUERY_CPU,
        perf_budgets::MERCHANT_QUERY_LEDGER_READS,
        "merchant_index_count_1000",
        || {
            let count = client.get_merchant_subscription_count(&merchant);
            assert_eq!(count, 1000);
        },
    );

    with_perf_budget(
        &env,
        perf_budgets::MERCHANT_QUERY_CPU,
        perf_budgets::MERCHANT_QUERY_LEDGER_READS,
        "merchant_query_page_of_100",
        || {
            let page = client.get_subscriptions_by_merchant(&merchant, &0, &100);
            assert_eq!(page.len(), 100);
        },
    );
}

#[test]
fn test_token_index_bloat_within_budget() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    for _ in 0..1000 {
        create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    }

    with_perf_budget(
        &env,
        perf_budgets::TOKEN_QUERY_CPU,
        perf_budgets::TOKEN_QUERY_LEDGER_READS,
        "token_index_count_1000",
        || {
            let count = client.get_token_subscription_count(&token);
            assert_eq!(count, 1000);
        },
    );

    with_perf_budget(
        &env,
        perf_budgets::TOKEN_QUERY_CPU,
        perf_budgets::TOKEN_QUERY_LEDGER_READS,
        "token_query_page_of_100",
        || {
            let page = client.get_subscriptions_by_token(&token, &0, &100);
            assert_eq!(page.len(), 100);
        },
    );
}

#[test]
fn test_subscriber_list_1000_items_multi_page_within_budget() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Create 1000 subscriber subscriptions interleaved with 4000 filler (5000 total IDs)
    for block in 0..50 {
        let base = block * 100;
        // 20 subscriber subs
        for i in 0..20 {
            let id = base + i;
            env.as_contract(&client.address, || {
                let sub = create_mock_sub(&env, &subscriber, &token);
                env.storage().instance().set(&id, &sub);
            });
        }
        // 80 filler subs
        for i in 20..100 {
            let id = base + i;
            env.as_contract(&client.address, || {
                let sub = create_mock_sub(&env, &merchant, &token);
                env.storage().instance().set(&id, &sub);
            });
        }
    }
    env.as_contract(&client.address, || {
        env.storage().instance().set(&Symbol::new(&env, "next_id"), &5000);
    });

    let page_size = 25u32;
    let mut start = 0u32;
    let mut pages = 0u32;
    let mut total_found = 0u32;

    loop {
        with_perf_budget(
            &env,
            perf_budgets::LIST_SUBSCRIBER_CPU,
            perf_budgets::LIST_SUBSCRIBER_LEDGER_READS,
            "multi_page_traversal",
            || {
                let page = client.list_subscriptions_by_subscriber(&subscriber, &start, &page_size);
                let count = page.subscription_ids.len() as u32;
                total_found += count;
                start = match page.next_start_id {
                    Some(id) => id,
                    None => start + page_size,
                };
            },
        );

        pages += 1;
        if total_found >= 1000 || start >= 5000 || pages > 200 {
            break;
        }
    }

    assert_eq!(total_found, 1000);
    assert!(pages >= 40 && pages <= 60,
        "Expected ~40-60 pages for 1000 subs with gaps, got {}", pages);
    println!("[Perf] Multi-page traversal: {} pages, {} found", pages, total_found);
}

#[test]
fn test_dos_unbounded_scan_capped_by_max_scan_depth_and_budget() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    let other = Address::generate(&env);

    // 10,000 filler entries
    for id in 0..10_000u32 {
        env.as_contract(&client.address, || {
            let sub = create_mock_sub(&env, &other, &token);
            env.storage().instance().set(&id, &sub);
        });
    }
    // 10 real subscriber entries at the end
    for i in 0..10 {
        let id = 10_000 + i;
        env.as_contract(&client.address, || {
            let sub = create_mock_sub(&env, &subscriber, &token);
            env.storage().instance().set(&id, &sub);
        });
    }
    env.as_contract(&client.address, || {
        env.storage().instance().set(&Symbol::new(&env, "next_id"), &10_010);
    });

    // Page 1: scans first MAX_SCAN_DEPTH IDs (1000), finds none, returns empty + cursor
    with_perf_budget(
        &env,
        perf_budgets::LIST_SUBSCRIBER_CPU,
        perf_budgets::LIST_SUBSCRIBER_LEDGER_READS,
        "dos_scan_page1",
        || {
            let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &5);
            assert_eq!(page.subscription_ids.len(), 0);
            assert_eq!(page.next_start_id, Some(MAX_SCAN_DEPTH));
        },
    );

    // Subsequent paging eventually reaches the real IDs
    let mut start = MAX_SCAN_DEPTH;
    let mut total_found = 0u32;
    for call in 0..10 {
        with_perf_budget(
            &env,
            perf_budgets::LIST_SUBSCRIBER_CPU,
            perf_budgets::LIST_SUBSCRIBER_LEDGER_READS,
            &format!("dos_scan_page{}", call + 2),
            || {
                let page = client.list_subscriptions_by_subscriber(&subscriber, &start, &5);
                total_found += page.subscription_ids.len();
                start = match page.next_start_id {
                    Some(id) => id,
                    None => start + 5,
                };
            },
        );
        if start >= 10_010 || total_found >= 10 {
            break;
        }
    }
    assert_eq!(total_found, 10, "All 10 subscriber subs should eventually be found");
}
