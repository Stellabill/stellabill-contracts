//! Ignored 100k-subscription soak tests for query budget guardrails.
//!
//! Run with:
//! `cargo test --release -p subscription_vault --test soak_100k soak_100k -- --ignored --nocapture`

use std::time::Instant;

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, Vec as SorobanVec,
};
use subscription_vault::{
    DataKey, Subscription, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
    MAX_SCAN_DEPTH, MAX_SUBSCRIPTION_LIST_PAGE,
};

const TOTAL_SUBSCRIPTIONS: u32 = 100_000;
const MERCHANT_COUNT: usize = 1_000;
const DENSE_MERCHANT_SUBS: u32 = 1_000;
const SINGLE_MERCHANT_ID: u32 = DENSE_MERCHANT_SUBS;
const SINGLE_MERCHANT_INDEX: usize = MERCHANT_COUNT - 1;
const T0: u64 = 1_700_000_000;

const MERCHANT_QUERY_CPU_LIMIT: i64 = 500_000;
const MERCHANT_QUERY_READ_LIMIT: u32 = 200;
const SUBSCRIBER_QUERY_CPU_LIMIT: i64 = 200_000;
const SUBSCRIBER_QUERY_READ_LIMIT: u32 = 1_500;

struct SoakDataset {
    dense_merchant: Address,
    single_subscription_merchant: Address,
    first_window_subscriber: Address,
    middle_window_subscriber: Address,
    tail_window_subscriber: Address,
}

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    env.cost_estimate().budget().reset_unlimited();

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, client, token)
}

fn merchant_index_for(id: u32) -> usize {
    if id < DENSE_MERCHANT_SUBS {
        0
    } else if id == SINGLE_MERCHANT_ID {
        SINGLE_MERCHANT_INDEX
    } else {
        let spread = (id - DENSE_MERCHANT_SUBS - 1) as usize;
        1 + (spread % (MERCHANT_COUNT - 2))
    }
}

fn subscription_for(
    env: &Env,
    id: u32,
    merchant: &Address,
    token: &Address,
    first_window_subscriber: &Address,
    middle_window_subscriber: &Address,
    tail_window_subscriber: &Address,
    fallback_subscriber: &Address,
) -> Subscription {
    let subscriber = if id < 250 {
        first_window_subscriber
    } else if (50_000..50_250).contains(&id) {
        middle_window_subscriber
    } else if id >= 99_750 {
        tail_window_subscriber
    } else {
        fallback_subscriber
    };

    Subscription {
        subscriber: subscriber.clone(),
        merchant: merchant.clone(),
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

fn seed_100k_subscriptions(env: &Env, contract_id: &Address, token: &Address) -> SoakDataset {
    env.cost_estimate().budget().reset_unlimited();

    let merchants = (0..MERCHANT_COUNT)
        .map(|_| Address::generate(env))
        .collect::<std::vec::Vec<_>>();
    let first_window_subscriber = Address::generate(env);
    let middle_window_subscriber = Address::generate(env);
    let tail_window_subscriber = Address::generate(env);
    let fallback_subscriber = Address::generate(env);

    let mut merchant_ids = (0..MERCHANT_COUNT)
        .map(|_| std::vec::Vec::<u32>::new())
        .collect::<std::vec::Vec<_>>();

    env.as_contract(contract_id, || {
        for id in 0..TOTAL_SUBSCRIPTIONS {
            let merchant_index = merchant_index_for(id);
            merchant_ids[merchant_index].push(id);

            let sub = subscription_for(
                env,
                id,
                &merchants[merchant_index],
                token,
                &first_window_subscriber,
                &middle_window_subscriber,
                &tail_window_subscriber,
                &fallback_subscriber,
            );
            env.storage().persistent().set(&DataKey::Sub(id), &sub);
        }

        for (merchant_index, ids) in merchant_ids.iter().enumerate() {
            let mut contract_ids = SorobanVec::new(env);
            for id in ids {
                contract_ids.push_back(*id);
            }
            env.storage().instance().set(
                &DataKey::MerchantSubs(merchants[merchant_index].clone()),
                &contract_ids,
            );
        }

        env.storage()
            .instance()
            .set(&DataKey::NextId, &TOTAL_SUBSCRIPTIONS);
    });

    println!(
        "[Soak] seeded subscriptions={} merchants={} dense_merchant_subs={} single_merchant_subs=1",
        TOTAL_SUBSCRIPTIONS, MERCHANT_COUNT, DENSE_MERCHANT_SUBS
    );

    SoakDataset {
        dense_merchant: merchants[0].clone(),
        single_subscription_merchant: merchants[SINGLE_MERCHANT_INDEX].clone(),
        first_window_subscriber,
        middle_window_subscriber,
        tail_window_subscriber,
    }
}

fn run_budgeted<T, F>(env: &Env, label: &str, cpu_limit: i64, read_limit: u32, op: F) -> T
where
    F: FnOnce() -> T,
{
    let start = Instant::now();
    env.cost_estimate()
        .budget()
        .reset_limits(cpu_limit as u64, u64::MAX);

    let result = op();
    let elapsed_ms = start.elapsed().as_millis();
    let resources = env.cost_estimate().resources();
    let cpu = resources.instructions.max(0);
    let reads = resources.read_entries;

    println!(
        "[Soak] {label} cpu={cpu}/{cpu_limit} read_entries={reads}/{read_limit} elapsed_ms={elapsed_ms}"
    );
    assert!(
        cpu <= cpu_limit,
        "[Soak] {label} exceeded CPU budget: {cpu} > {cpu_limit}"
    );
    assert!(
        reads <= read_limit,
        "[Soak] {label} exceeded ledger read budget: {reads} > {read_limit}"
    );

    result
}

fn assert_merchant_page(page: &SorobanVec<Subscription>, expected_merchant: &Address) {
    let mut i = 0;
    while i < page.len() {
        let sub = page.get(i).unwrap();
        assert_eq!(sub.merchant, expected_merchant.clone());
        i += 1;
    }
}

#[test]
#[ignore = "100k direct-storage soak test; run nightly or manually with --ignored"]
fn soak_100k_query_budget_guardrails() {
    let (env, client, token) = setup();
    let dataset = seed_100k_subscriptions(&env, &client.address, &token);

    let dense_count = run_budgeted(
        &env,
        "merchant_dense_count",
        MERCHANT_QUERY_CPU_LIMIT,
        MERCHANT_QUERY_READ_LIMIT,
        || client.get_merchant_subscription_count(&dataset.dense_merchant),
    );
    assert_eq!(dense_count, DENSE_MERCHANT_SUBS);

    let first_merchant_page = run_budgeted(
        &env,
        "merchant_dense_page_first start=0 size=100",
        MERCHANT_QUERY_CPU_LIMIT,
        MERCHANT_QUERY_READ_LIMIT,
        || client.get_subscriptions_by_merchant(&dataset.dense_merchant, &0, &100),
    );
    assert_eq!(first_merchant_page.len(), MAX_SUBSCRIPTION_LIST_PAGE);
    assert_merchant_page(&first_merchant_page, &dataset.dense_merchant);

    let middle_merchant_page = run_budgeted(
        &env,
        "merchant_dense_page_middle start=500 size=100",
        MERCHANT_QUERY_CPU_LIMIT,
        MERCHANT_QUERY_READ_LIMIT,
        || client.get_subscriptions_by_merchant(&dataset.dense_merchant, &500, &100),
    );
    assert_eq!(middle_merchant_page.len(), MAX_SUBSCRIPTION_LIST_PAGE);
    assert_merchant_page(&middle_merchant_page, &dataset.dense_merchant);

    let last_merchant_page = run_budgeted(
        &env,
        "merchant_dense_page_last start=900 size=100",
        MERCHANT_QUERY_CPU_LIMIT,
        MERCHANT_QUERY_READ_LIMIT,
        || client.get_subscriptions_by_merchant(&dataset.dense_merchant, &900, &100),
    );
    assert_eq!(last_merchant_page.len(), MAX_SUBSCRIPTION_LIST_PAGE);
    assert_merchant_page(&last_merchant_page, &dataset.dense_merchant);

    let single_merchant_page = run_budgeted(
        &env,
        "merchant_single_subscription start=0 size=100",
        MERCHANT_QUERY_CPU_LIMIT,
        MERCHANT_QUERY_READ_LIMIT,
        || client.get_subscriptions_by_merchant(&dataset.single_subscription_merchant, &0, &100),
    );
    assert_eq!(single_merchant_page.len(), 1);
    assert_merchant_page(&single_merchant_page, &dataset.single_subscription_merchant);

    let first_subscriber_page = run_budgeted(
        &env,
        "subscriber_page_first start=0 size=100",
        SUBSCRIBER_QUERY_CPU_LIMIT,
        SUBSCRIBER_QUERY_READ_LIMIT,
        || client.list_subscriptions_by_subscriber(&dataset.first_window_subscriber, &0, &100),
    );
    assert_eq!(first_subscriber_page.subscription_ids.len(), 100);
    assert_eq!(first_subscriber_page.subscription_ids.get(0).unwrap(), 0);
    assert_eq!(
        first_subscriber_page.next_start_id,
        Some(MAX_SUBSCRIPTION_LIST_PAGE)
    );

    let resumed_subscriber_page = run_budgeted(
        &env,
        "subscriber_cursor_resume start=100 size=100",
        SUBSCRIBER_QUERY_CPU_LIMIT,
        SUBSCRIBER_QUERY_READ_LIMIT,
        || {
            client.list_subscriptions_by_subscriber(
                &dataset.first_window_subscriber,
                &MAX_SUBSCRIPTION_LIST_PAGE,
                &100,
            )
        },
    );
    assert_eq!(resumed_subscriber_page.subscription_ids.len(), 100);
    assert_eq!(
        resumed_subscriber_page.subscription_ids.get(0).unwrap(),
        100
    );

    let middle_subscriber_page = run_budgeted(
        &env,
        "subscriber_page_middle start=50000 size=100",
        SUBSCRIBER_QUERY_CPU_LIMIT,
        SUBSCRIBER_QUERY_READ_LIMIT,
        || {
            client.list_subscriptions_by_subscriber(
                &dataset.middle_window_subscriber,
                &50_000,
                &100,
            )
        },
    );
    assert_eq!(middle_subscriber_page.subscription_ids.len(), 100);
    assert_eq!(
        middle_subscriber_page.subscription_ids.get(0).unwrap(),
        50_000
    );

    let tail_subscriber_page = run_budgeted(
        &env,
        "subscriber_page_tail start=99900 size=100",
        SUBSCRIBER_QUERY_CPU_LIMIT,
        SUBSCRIBER_QUERY_READ_LIMIT,
        || client.list_subscriptions_by_subscriber(&dataset.tail_window_subscriber, &99_900, &100),
    );
    assert_eq!(tail_subscriber_page.subscription_ids.len(), 100);
    assert_eq!(
        tail_subscriber_page.subscription_ids.get(0).unwrap(),
        99_900
    );
    assert_eq!(
        tail_subscriber_page.subscription_ids.get(99).unwrap(),
        99_999
    );
    assert_eq!(tail_subscriber_page.next_start_id, None);

    let exhausted_cursor_page = run_budgeted(
        &env,
        "subscriber_cursor_exhausted start=100000 size=100",
        SUBSCRIBER_QUERY_CPU_LIMIT,
        SUBSCRIBER_QUERY_READ_LIMIT,
        || {
            client.list_subscriptions_by_subscriber(
                &dataset.tail_window_subscriber,
                &TOTAL_SUBSCRIPTIONS,
                &100,
            )
        },
    );
    assert_eq!(exhausted_cursor_page.subscription_ids.len(), 0);
    assert_eq!(exhausted_cursor_page.next_start_id, None);

    println!(
        "[Soak] complete max_scan_depth={} max_page={} total_subscriptions={}",
        MAX_SCAN_DEPTH, MAX_SUBSCRIPTION_LIST_PAGE, TOTAL_SUBSCRIPTIONS
    );
}
