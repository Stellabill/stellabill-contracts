use super::*;
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::Env;

const MIN_TOPUP: i128 = 1_000_000;

fn setup() -> (Env, SubscriptionVaultClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    client.init(&admin, &token, &MIN_TOPUP);
    (env, client)
}

fn make_sub(env: &Env, client: &SubscriptionVaultClient<'static>, expires_at: Option<u64>) -> u32 {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    client.create_subscription(&subscriber, &merchant, &1000i128, &3600u64, &false, &expires_at)
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
    let (_env, client) = setup();
    assert_eq!(client.get_min_topup(), MIN_TOPUP);
}

#[test]
fn test_double_init_rejected() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let result = client.try_init(&admin, &token, &MIN_TOPUP);
    assert_eq!(result, Err(Ok(Error::AlreadyInitialized)));
}

// --- storage key isolation --------------------------------------------------

#[test]
fn test_config_readable_after_subscription_creation() {
    let (env, client) = setup();
    let id = make_sub(&env, &client, None);
    assert_eq!(id, 0);
    assert_eq!(client.get_min_topup(), MIN_TOPUP);
}

// --- charge_subscription expiration guard -----------------------------------

#[test]
fn test_charge_before_expiration_succeeds() {
    let (env, client) = setup();
    let now = env.ledger().timestamp();
    let future = now + 100;
    let id = make_sub(&env, &client, Some(future));

    // Jump to one second before expiration.
    env.ledger().set_timestamp(future - 1);
    let result = client.try_charge_subscription(&id);
    assert!(result.is_ok());
}

#[test]
fn test_charge_at_exact_expiration_rejected() {
    let (env, client) = setup();
    let now = env.ledger().timestamp();
    let future = now + 100;
    let id = make_sub(&env, &client, Some(future));

    // Jump to exactly the expiration timestamp.
    env.ledger().set_timestamp(future);
    let result = client.try_charge_subscription(&id);
    assert_eq!(result, Err(Ok(Error::SubscriptionExpired)));
}

#[test]
fn test_charge_after_expiration_rejected() {
    let (env, client) = setup();
    let now = env.ledger().timestamp();
    let future = now + 100;
    let id = make_sub(&env, &client, Some(future));

    // Jump to one second after expiration.
    env.ledger().set_timestamp(future + 1);
    let result = client.try_charge_subscription(&id);
    assert_eq!(result, Err(Ok(Error::SubscriptionExpired)));
}

#[test]
fn test_charge_long_after_expiration_rejected() {
    let (env, client) = setup();
    let now = env.ledger().timestamp();
    let future = now + 100;
    let id = make_sub(&env, &client, Some(future));

    // Far in the future.
    env.ledger().set_timestamp(future + 100_000);
    let result = client.try_charge_subscription(&id);
    assert_eq!(result, Err(Ok(Error::SubscriptionExpired)));
}

#[test]
fn test_charge_open_ended_never_expires() {
    let (env, client) = setup();
    let id = make_sub(&env, &client, None);

    // Far in the future — no expiration set, so charge should succeed.
    env.ledger().set_timestamp(u64::MAX);
    let result = client.try_charge_subscription(&id);
    assert!(result.is_ok());
}

#[test]
fn test_charge_open_ended_at_creation_time_succeeds() {
    let (env, client) = setup();
    let id = make_sub(&env, &client, None);

    // At creation timestamp.
    let result = client.try_charge_subscription(&id);
    assert!(result.is_ok());
}

#[test]
fn test_charge_subscription_not_found() {
    let (_env, client) = setup();
    let result = client.try_charge_subscription(&999u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn test_charge_expired_subscription_id_does_not_affect_other_ids() {
    let (env, client) = setup();
    let now = env.ledger().timestamp();
    let future = now + 100;

    // Create one expiring and one open-ended subscription.
    let expired_id = make_sub(&env, &client, Some(future));
    let open_id = make_sub(&env, &client, None);

    env.ledger().set_timestamp(future + 1);

    let result_expired = client.try_charge_subscription(&expired_id);
    assert_eq!(result_expired, Err(Ok(Error::SubscriptionExpired)));

    let result_open = client.try_charge_subscription(&open_id);
    assert!(result_open.is_ok());
}

// --- ID sequencing -----------------------------------------------------------

#[test]
fn test_id_starts_at_zero() {
    let (env, client) = setup();
    let id = make_sub(&env, &client, None);
    assert_eq!(id, 0);
}

#[test]
fn test_ids_are_monotonically_increasing() {
    let (env, client) = setup();
    for expected in 0..10 {
        let id = make_sub(&env, &client, None);
        assert_eq!(id, expected);
    }
}

#[test]
fn test_get_subscription_count_matches_creations() {
    let (env, client) = setup();
    assert_eq!(client.get_subscription_count(), 0);
    make_sub(&env, &client, None);
    assert_eq!(client.get_subscription_count(), 1);
    make_sub(&env, &client, None);
    assert_eq!(client.get_subscription_count(), 2);
}

// --- get_subscription round-trip --------------------------------------------

#[test]
fn test_get_subscription_returns_matching_fields() {
    let (env, client) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let now = env.ledger().timestamp();
    let future = now + 86400;

    let id = client.create_subscription(
        &subscriber, &merchant, &5000i128, &7200u64, &true, &Some(future),
    );
    let stored = client.get_subscription(&id);
    assert_eq!(stored.subscriber, subscriber);
    assert_eq!(stored.merchant, merchant);
    assert_eq!(stored.amount, 5000);
    assert_eq!(stored.interval_seconds, 7200);
    assert_eq!(stored.last_payment_timestamp, now);
    assert_eq!(stored.status, SubscriptionStatus::Active);
    assert_eq!(stored.prepaid_balance, 0);
    assert!(stored.usage_enabled);
    assert_eq!(stored.expires_at, Some(future));
}

#[test]
fn test_get_subscription_without_expiration() {
    let (env, client) = setup();
    let id = make_sub(&env, &client, None);
    let stored = client.get_subscription(&id);
    assert_eq!(stored.expires_at, None);
}

// --- NotFound ---------------------------------------------------------------

#[test]
fn test_get_subscription_unknown_id_returns_not_found() {
    let (_env, client) = setup();
    let result = client.try_get_subscription(&999u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

// --- Input validation -------------------------------------------------------

#[test]
fn test_create_subscription_zero_amount_rejected() {
    let (env, client) = setup();
    let sub = Address::generate(&env);
    let merchant = Address::generate(&env);

    let result = client.try_create_subscription(&sub, &merchant, &0i128, &3600u64, &false, &None);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_negative_amount_rejected() {
    let (env, client) = setup();
    let sub = Address::generate(&env);
    let merchant = Address::generate(&env);

    let result = client.try_create_subscription(&sub, &merchant, &(-1i128), &3600u64, &false, &None);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_zero_interval_rejected() {
    let (env, client) = setup();
    let sub = Address::generate(&env);
    let merchant = Address::generate(&env);

    let result = client.try_create_subscription(&sub, &merchant, &1000i128, &0u64, &false, &None);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_past_expiration_rejected() {
    let (env, client) = setup();
    let sub = Address::generate(&env);
    let merchant = Address::generate(&env);

    let now = env.ledger().timestamp();
    let result = client.try_create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &Some(now));
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}
