use super::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::Env;

const MIN_TOPUP: i128 = 1_000_000;

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    client.init(&admin, &token, &MIN_TOPUP);
    (env, client, admin, token)
}

#[test]
fn version_is_zero() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    assert_eq!(client.version(), 0);
}

// --- init / config persistence ----------------------------------------------

#[test]
fn test_get_min_topup_returns_init_value() {
    let (_env, client, _admin, _token) = setup();
    let result = client.get_min_topup();
    assert_eq!(result, MIN_TOPUP);
}

#[test]
fn test_double_init_rejected() {
    let (_env, client, admin, token) = setup();
    let result = client.try_init(&admin, &token, &MIN_TOPUP);
    assert_eq!(result, Err(Ok(Error::AlreadyInitialized)));
}

#[test]
fn test_double_init_with_different_admin_rejected() {
    let (_env, client, _admin, _token) = setup();
    let other_admin = Address::generate(&_env);
    let other_token = Address::generate(&_env);
    let result = client.try_init(&other_admin, &other_token, &500_000i128);
    assert_eq!(result, Err(Ok(Error::AlreadyInitialized)));
}

#[test]
fn test_init_without_mock_auth_still_works() {
    // init doesn't call require_auth, so this works even without mock_all_auths
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let result = client.try_init(&admin, &token, &MIN_TOPUP);
    assert!(result.is_ok());
}

// --- storage key isolation --------------------------------------------------

#[test]
fn test_config_readable_after_subscription_creation() {
    let (_env, client, _admin, _token) = setup();
    let sub = Address::generate(&_env);
    let merchant = Address::generate(&_env);

    // Create subscription id 0 — stored at u32 key 0
    let id = client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &None);
    assert_eq!(id, 0);

    // Config keys (stored as Symbols) must remain intact.
    let min_topup = client.get_min_topup();
    assert_eq!(min_topup, MIN_TOPUP);
}

#[test]
fn test_subscription_id_zero_does_not_overwrite_config() {
    let (_env, client, _admin, _token) = setup();
    let sub = Address::generate(&_env);
    let merchant = Address::generate(&_env);

    let id = client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &None);
    assert_eq!(id, 0);

    // Reading config by Symbol key should still work
    let topup = client.get_min_topup();
    assert_eq!(topup, MIN_TOPUP);

    // Subscription at u32(0) should match what we stored
    let stored = client.get_subscription(&id);
    assert_eq!(stored.subscriber, sub);
    assert_eq!(stored.amount, 1000);
    // u32 key 0 and Symbol "min_topup" are distinct storage keys
}

#[test]
fn test_multiple_subscriptions_config_persists() {
    let (_env, client, _admin, _token) = setup();
    let sub = Address::generate(&_env);
    let merchant = Address::generate(&_env);

    for i in 0..5 {
        let id = client.create_subscription(
            &sub,
            &merchant,
            &(1000 * (i + 1)),
            &3600u64,
            &false,
            &None,
        );
        assert_eq!(id, i as u32);
    }

    let topup = client.get_min_topup();
    assert_eq!(topup, MIN_TOPUP);
}

// --- ID sequencing -----------------------------------------------------------

#[test]
fn test_id_starts_at_zero() {
    let (_env, client, _admin, _token) = setup();
    let sub = Address::generate(&_env);
    let merchant = Address::generate(&_env);

    let id = client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &None);
    assert_eq!(id, 0);
}

#[test]
fn test_ids_are_monotonically_increasing() {
    let (_env, client, _admin, _token) = setup();
    let sub = Address::generate(&_env);
    let merchant = Address::generate(&_env);

    for expected in 0..10 {
        let id = client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &None);
        assert_eq!(id, expected);
    }
}

#[test]
fn test_get_subscription_count_matches_creations() {
    let (_env, client, _admin, _token) = setup();
    let sub = Address::generate(&_env);
    let merchant = Address::generate(&_env);

    assert_eq!(client.get_subscription_count(), 0);
    client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &None);
    assert_eq!(client.get_subscription_count(), 1);
    client.create_subscription(&sub, &merchant, &2000i128, &7200u64, &false, &None);
    assert_eq!(client.get_subscription_count(), 2);
}

// --- get_subscription round-trip --------------------------------------------

#[test]
fn test_get_subscription_returns_matching_fields() {
    let (env, client, _admin, _token) = setup();
    let sub = Address::generate(&env);
    let merchant = Address::generate(&env);
    let now = env.ledger().timestamp();

    let id = client.create_subscription(&sub, &merchant, &5000i128, &7200u64, &true, &None);
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
    let (env, client, _admin, _token) = setup();
    let sub = Address::generate(&env);
    let merchant = Address::generate(&env);
    let now = env.ledger().timestamp();
    let future = now + 86400;

    let id = client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &Some(future));
    let stored = client.get_subscription(&id);
    assert_eq!(stored.expires_at, Some(future));
    assert_eq!(stored.last_payment_timestamp, now);
}

// --- NotFound ---------------------------------------------------------------

#[test]
fn test_get_subscription_unknown_id_returns_not_found() {
    let (_env, client, _admin, _token) = setup();
    let result = client.try_get_subscription(&999u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn test_get_subscription_after_creation_other_ids_still_not_found() {
    let (_env, client, _admin, _token) = setup();
    let sub = Address::generate(&_env);
    let merchant = Address::generate(&_env);

    let id = client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &None);
    assert_eq!(id, 0);
    let result = client.try_get_subscription(&1u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

// --- Input validation -------------------------------------------------------

#[test]
fn test_create_subscription_zero_amount_rejected() {
    let (_env, client, _admin, _token) = setup();
    let sub = Address::generate(&_env);
    let merchant = Address::generate(&_env);

    let result = client.try_create_subscription(&sub, &merchant, &0i128, &3600u64, &false, &None);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_negative_amount_rejected() {
    let (_env, client, _admin, _token) = setup();
    let sub = Address::generate(&_env);
    let merchant = Address::generate(&_env);

    let result = client.try_create_subscription(&sub, &merchant, &(-1i128), &3600u64, &false, &None);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_zero_interval_rejected() {
    let (_env, client, _admin, _token) = setup();
    let sub = Address::generate(&_env);
    let merchant = Address::generate(&_env);

    let result = client.try_create_subscription(&sub, &merchant, &1000i128, &0u64, &false, &None);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_past_expiration_rejected() {
    let (_env, client, _admin, _token) = setup();
    let sub = Address::generate(&_env);
    let merchant = Address::generate(&_env);

    let now = _env.ledger().timestamp();
    let result = client.try_create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &Some(now));
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}
