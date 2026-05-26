use super::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::Env;

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let default_token = Address::generate(&env);
    client.init(&admin, &default_token);
    (env, client, default_token)
}

fn make_account(env: &Env) -> Address {
    Address::generate(&env)
}

#[test]
fn version_is_zero() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    assert_eq!(client.version(), 0);
}

// --- ID sequencing -----------------------------------------------------------

#[test]
fn test_id_starts_at_zero() {
    let (env, client, _) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    let id = client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &None);
    assert_eq!(id, 0);
}

#[test]
fn test_ids_are_monotonically_increasing() {
    let (env, client, _) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    for expected in 0..10 {
        let id = client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &None);
        assert_eq!(id, expected);
    }
}

#[test]
fn test_ids_are_unique() {
    let (env, client, _) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    let mut seen = soroban_sdk::Vec::new(&env);
    for i in 0..100 {
        let id = client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &None);
        assert_eq!(id, i);
        seen.push_back(id);
    }
    // All 100 IDs are distinct because they are 0..99 with no gaps.
    assert_eq!(seen.len(), 100);
}

#[test]
fn test_get_subscription_count_on_empty() {
    let (_env, client, _) = setup();
    assert_eq!(client.get_subscription_count(), 0);
}

#[test]
fn test_get_subscription_count_matches_creations() {
    let (env, client, _) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    assert_eq!(client.get_subscription_count(), 0);

    client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &None);
    assert_eq!(client.get_subscription_count(), 1);

    client.create_subscription(&sub, &merchant, &2000i128, &7200u64, &false, &None);
    assert_eq!(client.get_subscription_count(), 2);

    client.create_subscription(&sub, &merchant, &3000i128, &14400u64, &true, &None);
    assert_eq!(client.get_subscription_count(), 3);
}

// --- get_subscription round-trip --------------------------------------------

#[test]
fn test_get_subscription_returns_matching_fields() {
    let (env, client, _) = setup();
    let sub = Address::generate(&env);
    let merchant = Address::generate(&env);
    let now = env.ledger().timestamp();

    let id = client.create_subscription(
        &sub,
        &merchant,
        &5000i128,
        &7200u64,
        &true,
        &None,
    );

    let stored = client.get_subscription(&id);
    assert_eq!(stored.subscriber, sub);
    assert_eq!(stored.merchant, merchant);
    assert_eq!(stored.amount, 5000);
    assert_eq!(stored.interval_seconds, 7200);
    assert_eq!(stored.last_payment_timestamp, now);
    assert_eq!(stored.status, SubscriptionStatus::Active);
    assert_eq!(stored.prepaid_balance, 0);
    assert!(stored.usage_enabled);
    assert_eq!(stored.expires_at, None);
}

#[test]
fn test_get_subscription_with_expires_at_round_trip() {
    let (env, client, _) = setup();
    let sub = Address::generate(&env);
    let merchant = Address::generate(&env);
    let now = env.ledger().timestamp();
    let future = now + 86400;

    let id = client.create_subscription(
        &sub,
        &merchant,
        &1000i128,
        &3600u64,
        &false,
        &Some(future),
    );

    let stored = client.get_subscription(&id);
    assert_eq!(stored.expires_at, Some(future));
    assert_eq!(stored.last_payment_timestamp, now);
}

#[test]
fn test_get_subscription_stores_token() {
    let (env, client, default_token) = setup();
    let sub = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &None);
    let stored = client.get_subscription(&id);
    assert_eq!(stored.token, default_token);
}

// --- NotFound ---------------------------------------------------------------

#[test]
fn test_get_subscription_unknown_id_returns_not_found() {
    let (_env, client, _) = setup();
    // No subscriptions created yet.
    let result = client.try_get_subscription(&999u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn test_get_subscription_after_creation_other_ids_still_not_found() {
    let (env, client, _) = setup();
    let sub = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &None);
    assert_eq!(id, 0);

    // id 1 was never created.
    let result = client.try_get_subscription(&1u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn test_get_subscription_multiple_ids_each_round_trips() {
    let (env, client, _) = setup();
    let sub = Address::generate(&env);
    let merchant = Address::generate(&env);

    let amounts = [1000i128, 2000, 3000, 4000, 5000];
    for &amount in &amounts {
        let id = client.create_subscription(&sub, &merchant, &amount, &3600u64, &false, &None);
        let stored = client.get_subscription(&id);
        assert_eq!(stored.amount, amount);
    }
}

// --- Input validation (complementary) ---------------------------------------

#[test]
fn test_create_subscription_zero_amount_rejected() {
    let (env, client, _) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    let result = client.try_create_subscription(&sub, &merchant, &0i128, &3600u64, &false, &None);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_negative_amount_rejected() {
    let (env, client, _) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    let result = client.try_create_subscription(&sub, &merchant, &(-1i128), &3600u64, &false, &None);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_zero_interval_rejected() {
    let (env, client, _) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    let result = client.try_create_subscription(&sub, &merchant, &1000i128, &0u64, &false, &None);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_past_expiration_rejected() {
    let (env, client, _) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    let now = env.ledger().timestamp();
    let result = client.try_create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &Some(now));
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}
