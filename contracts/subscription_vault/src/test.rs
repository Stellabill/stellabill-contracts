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
fn test_create_subscription_ok() {
    let (env, client, _default_token) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    let id = client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &None);
    assert_eq!(id, 0);
}

#[test]
fn test_create_subscription_with_expires_at_ok() {
    let (env, client, _default_token) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    let now = env.ledger().timestamp();
    let future = now + 86400;

    let id = client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &Some(future));
    assert_eq!(id, 0);
}

#[test]
fn test_create_subscription_increments_id() {
    let (env, client, _default_token) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    let id1 = client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &None);
    let id2 = client.create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &None);
    assert_eq!(id1, 0);
    assert_eq!(id2, 1);
}

#[test]
fn test_create_subscription_zero_amount_rejected() {
    let (env, client, _default_token) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    let result = client.try_create_subscription(&sub, &merchant, &0i128, &3600u64, &false, &None);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_negative_amount_rejected() {
    let (env, client, _default_token) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    let result = client.try_create_subscription(&sub, &merchant, &(-1i128), &3600u64, &false, &None);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_zero_interval_rejected() {
    let (env, client, _default_token) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    let result = client.try_create_subscription(&sub, &merchant, &1000i128, &0u64, &false, &None);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_past_expiration_rejected() {
    let (env, client, _default_token) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    let now = env.ledger().timestamp();
    let result = client.try_create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &Some(now));
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_expiration_before_now_rejected() {
    let (env, client, _default_token) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);

    let now = env.ledger().timestamp();
    let past = now.saturating_sub(1);
    let result = client.try_create_subscription(&sub, &merchant, &1000i128, &3600u64, &false, &Some(past));
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_with_token_ok() {
    let (env, client, _default_token) = setup();
    let sub = make_account(&env);
    let token = make_account(&env);
    let merchant = make_account(&env);

    let id = client.create_subscription_with_token(&sub, &token, &merchant, &1000i128, &3600u64, &false, &None);
    assert_eq!(id, 0);
}

#[test]
fn test_create_subscription_with_token_zero_amount_rejected() {
    let (env, client, _default_token) = setup();
    let sub = make_account(&env);
    let token = make_account(&env);
    let merchant = make_account(&env);

    let result = client.try_create_subscription_with_token(&sub, &token, &merchant, &0i128, &3600u64, &false, &None);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_with_token_negative_amount_rejected() {
    let (env, client, _default_token) = setup();
    let sub = make_account(&env);
    let token = make_account(&env);
    let merchant = make_account(&env);

    let result = client.try_create_subscription_with_token(&sub, &token, &merchant, &(-5i128), &3600u64, &false, &None);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_with_token_zero_interval_rejected() {
    let (env, client, _default_token) = setup();
    let sub = make_account(&env);
    let token = make_account(&env);
    let merchant = make_account(&env);

    let result = client.try_create_subscription_with_token(&sub, &token, &merchant, &1000i128, &0u64, &false, &None);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_with_token_past_expiration_rejected() {
    let (env, client, _default_token) = setup();
    let sub = make_account(&env);
    let token = make_account(&env);
    let merchant = make_account(&env);

    let now = env.ledger().timestamp();
    let result = client.try_create_subscription_with_token(&sub, &token, &merchant, &1000i128, &3600u64, &false, &Some(now));
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn test_create_subscription_with_token_future_expiration_ok() {
    let (env, client, _default_token) = setup();
    let sub = make_account(&env);
    let token = make_account(&env);
    let merchant = make_account(&env);

    let now = env.ledger().timestamp();
    let future = now + 7200;
    let id = client.create_subscription_with_token(&sub, &token, &merchant, &1000i128, &3600u64, &false, &Some(future));
    assert_eq!(id, 0);
}

#[test]
fn test_subscription_stored_correctly() {
    let (env, client, default_token) = setup();
    let sub = make_account(&env);
    let merchant = make_account(&env);
    let now = env.ledger().timestamp();
    let future = now + 86400;

    let id = client.create_subscription(&sub, &merchant, &5000i128, &7200u64, &true, &Some(future));

    let stored = client.get_subscription(&id);
    assert_eq!(stored.subscriber, sub);
    assert_eq!(stored.token, default_token);
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
fn test_create_subscription_with_token_stores_correctly() {
    let (env, client, _default_token) = setup();
    let sub = make_account(&env);
    let token = make_account(&env);
    let merchant = make_account(&env);
    let now = env.ledger().timestamp();
    let future = now + 86400;

    let id = client.create_subscription_with_token(&sub, &token, &merchant, &5000i128, &7200u64, &true, &Some(future));

    let stored = client.get_subscription(&id);
    assert_eq!(stored.subscriber, sub);
    assert_eq!(stored.token, token);
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
fn test_get_subscription_not_found() {
    let (_env, client, _default_token) = setup();
    let result = client.try_get_subscription(&999u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}
