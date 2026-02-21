use crate::{
    Error, Subscription, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
    MAX_SUBSCRIPTION_ID,
};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, Symbol};

// ── helpers ──────────────────────────────────────────────────────────────────

fn setup_contract(env: &Env) -> (SubscriptionVaultClient, Address, Address) {
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    let token = Address::generate(env);
    let admin = Address::generate(env);
    client.init(&token, &admin, &1_000000i128); // 1 USDC min_topup
    (client, token, admin)
}

fn make_subscription(
    env: &Env,
    client: &SubscriptionVaultClient,
    expiration: Option<u64>,
) -> u32 {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    client.create_subscription(
        &subscriber,
        &merchant,
        &10_000000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &expiration,
    )
}

/// Seed the internal `next_id` counter to an arbitrary value via instance storage.
/// This lets us simulate near-overflow conditions without creating millions of real subscriptions.
fn seed_counter(env: &Env, contract_id: &Address, value: u32) {
    env.as_contract(contract_id, || {
        env.storage()
            .instance()
            .set(&Symbol::new(env, "next_id"), &value);
    });
}

// ── existing tests (updated for new expiration field & _next_id signature) ───

#[test]
fn test_init_and_struct() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let min_topup = 1_000000i128;
    client.init(&token, &admin, &min_topup);

    assert_eq!(client.get_min_topup(), min_topup);
}

#[test]
fn test_subscription_struct() {
    let env = Env::default();
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        amount: 10_000_0000,
        interval_seconds: 30 * 24 * 60 * 60,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: 50_000_0000,
        usage_enabled: false,
        expiration: None,
    };
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(sub.expiration, None);
}

#[test]
fn test_subscription_struct_with_expiration() {
    let env = Env::default();
    let exp_ts: u64 = 1_800_000_000;
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        amount: 10_000_0000,
        interval_seconds: 30 * 24 * 60 * 60,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: 50_000_0000,
        usage_enabled: false,
        expiration: Some(exp_ts),
    };
    assert_eq!(sub.expiration, Some(exp_ts));
}

#[test]
fn test_min_topup_below_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let min_topup = 5_000000i128;

    client.init(&token, &admin, &min_topup);

    let result = client.try_deposit_funds(&0, &subscriber, &4_999999);
    assert!(result.is_err());
}

#[test]
fn test_min_topup_exactly_at_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let min_topup = 5_000000i128;

    client.init(&token, &admin, &min_topup);

    let result = client.try_deposit_funds(&0, &subscriber, &min_topup);
    assert!(result.is_ok());
}

#[test]
fn test_min_topup_above_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let min_topup = 5_000000i128;

    client.init(&token, &admin, &min_topup);

    let result = client.try_deposit_funds(&0, &subscriber, &10_000000);
    assert!(result.is_ok());
}

#[test]
fn test_set_min_topup_by_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let initial_min = 1_000000i128;
    let new_min = 10_000000i128;

    client.init(&token, &admin, &initial_min);
    assert_eq!(client.get_min_topup(), initial_min);

    client.set_min_topup(&admin, &new_min);
    assert_eq!(client.get_min_topup(), new_min);
}

#[test]
fn test_set_min_topup_unauthorized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let non_admin = Address::generate(&env);
    let min_topup = 1_000000i128;

    client.init(&token, &admin, &min_topup);

    let result = client.try_set_min_topup(&non_admin, &5_000000);
    assert!(result.is_err());
}

// ── expiration tests ──────────────────────────────────────────────────────────

#[test]
fn test_create_subscription_no_expiration() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let id = make_subscription(&env, &client, None);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.expiration, None);
}

#[test]
fn test_create_subscription_with_expiration() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let exp_ts: u64 = 90 * 24 * 60 * 60;
    let id = make_subscription(&env, &client, Some(exp_ts));
    let sub = client.get_subscription(&id);
    assert_eq!(sub.expiration, Some(exp_ts));
}

#[test]
fn test_charge_expired_subscription() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let exp_ts: u64 = 1_000;
    let id = make_subscription(&env, &client, Some(exp_ts));
    env.ledger().set_timestamp(exp_ts + 1);
    let result = client.try_charge_subscription(&id);
    assert!(
        matches!(result, Err(Ok(Error::SubscriptionExpired))),
        "expected SubscriptionExpired, got {:?}",
        result
    );
}

#[test]
fn test_charge_at_exact_expiration_boundary() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let exp_ts: u64 = 5_000;
    let id = make_subscription(&env, &client, Some(exp_ts));
    env.ledger().set_timestamp(exp_ts);
    let result = client.try_charge_subscription(&id);
    assert!(
        matches!(result, Err(Ok(Error::SubscriptionExpired))),
        "expected SubscriptionExpired at boundary, got {:?}",
        result
    );
}

#[test]
fn test_charge_one_second_before_expiration() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let exp_ts: u64 = 5_000;
    let id = make_subscription(&env, &client, Some(exp_ts));
    env.ledger().set_timestamp(exp_ts - 1);
    let result = client.try_charge_subscription(&id);
    assert!(result.is_ok(), "expected Ok before expiration, got {:?}", result);
}

#[test]
fn test_charge_no_expiration_always_allowed() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let id = make_subscription(&env, &client, None);
    env.ledger().set_timestamp(u64::MAX / 2);
    let result = client.try_charge_subscription(&id);
    assert!(result.is_ok(), "expected Ok for open-ended subscription, got {:?}", result);
}

#[test]
fn test_charge_nonexistent_subscription() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let result = client.try_charge_subscription(&999);
    assert!(
        matches!(result, Err(Ok(Error::NotFound))),
        "expected NotFound, got {:?}",
        result
    );
}

#[test]
fn test_long_running_no_expiration() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let id = make_subscription(&env, &client, None);
    let one_month: u64 = 30 * 24 * 60 * 60;
    for month in 1u64..=60 {
        env.ledger().set_timestamp(month * one_month);
        let result = client.try_charge_subscription(&id);
        assert!(result.is_ok(), "month {} failed: {:?}", month, result);
    }
}

// ── ID hardening tests ────────────────────────────────────────────────────────

/// The very first subscription always receives ID 0.
#[test]
fn test_id_starts_at_zero() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let id = make_subscription(&env, &client, None);
    assert_eq!(id, 0, "first subscription must have ID 0");
}

/// Consecutive subscriptions receive strictly increasing IDs (0, 1, 2, …).
#[test]
fn test_ids_are_monotonically_increasing() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    for expected in 0u32..10 {
        let id = make_subscription(&env, &client, None);
        assert_eq!(id, expected, "expected monotone ID {expected}, got {id}");
    }
}

/// 100 consecutive subscriptions produce 100 pairwise-distinct IDs.
#[test]
fn test_ids_are_unique() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let mut ids: soroban_sdk::Vec<u32> = soroban_sdk::Vec::new(&env);
    for _ in 0..100 {
        let id = make_subscription(&env, &client, None);
        // Verify the new ID is not already in our collected set.
        assert!(
            !ids.contains(id),
            "duplicate ID detected: {id}"
        );
        ids.push_back(id);
    }
    assert_eq!(ids.len(), 100);
}

/// `get_subscription_count` reflects the total number ever created.
#[test]
fn test_get_subscription_count() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    assert_eq!(client.get_subscription_count(), 0, "count must be 0 before any subscription");
    for expected_count in 1u32..=5 {
        make_subscription(&env, &client, None);
        assert_eq!(
            client.get_subscription_count(),
            expected_count,
            "count mismatch after {expected_count} subscription(s)"
        );
    }
}

/// Allocation at counter = MAX_SUBSCRIPTION_ID - 1 succeeds and returns that value.
#[test]
fn test_id_at_max_minus_one_succeeds() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    // Seed counter to one below the ceiling.
    let high_id = MAX_SUBSCRIPTION_ID - 1;
    seed_counter(&env, &contract_id, high_id);

    let id = make_subscription(&env, &client, None);
    assert_eq!(
        id, high_id,
        "expected ID {high_id} at counter MAX-1, got {id}"
    );
    // Counter should now be at MAX_SUBSCRIPTION_ID.
    assert_eq!(client.get_subscription_count(), MAX_SUBSCRIPTION_ID);
}

/// When the counter is already at MAX_SUBSCRIPTION_ID, allocation returns SubscriptionLimitReached.
#[test]
fn test_id_at_max_returns_limit_reached() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    // Seed counter directly to the ceiling.
    seed_counter(&env, &contract_id, MAX_SUBSCRIPTION_ID);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let result = client.try_create_subscription(
        &subscriber,
        &merchant,
        &10_000000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &None,
    );
    assert!(
        matches!(result, Err(Ok(Error::SubscriptionLimitReached))),
        "expected SubscriptionLimitReached, got {:?}",
        result
    );
}

/// Repeated calls after the limit is reached all return SubscriptionLimitReached (no wrap).
#[test]
fn test_no_id_reuse_after_limit() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    seed_counter(&env, &contract_id, MAX_SUBSCRIPTION_ID);

    for attempt in 0..5 {
        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);
        let result = client.try_create_subscription(
            &subscriber,
            &merchant,
            &10_000000i128,
            &(30 * 24 * 60 * 60u64),
            &false,
            &None,
        );
        assert!(
            matches!(result, Err(Ok(Error::SubscriptionLimitReached))),
            "attempt {attempt}: expected SubscriptionLimitReached, got {:?}",
            result
        );
        // Counter must remain at MAX — no wrap to 0.
        assert_eq!(
            client.get_subscription_count(),
            MAX_SUBSCRIPTION_ID,
            "counter must not change after limit is reached"
        );
    }
}
