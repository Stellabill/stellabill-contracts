use crate::{Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{token::StellarAssetClient as TokenAdminClient, Address, Env};

fn setup() -> (
    Env,
    Address,
    SubscriptionVaultClient<'static>,
    TokenAdminClient<'static>,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = TokenAdminClient::new(&env, &token);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    client.init(
        &token,
        &7u32,
        &admin,
        &1_000_000i128,
        &(7 * 24 * 60 * 60u64),
    );

    (env, contract_id, client, token_admin, token)
}

const T0: u64 = 1_000_000;
const INTERVAL: u64 = 30 * 24 * 3600;
const AMOUNT: i128 = 10_000_000;
const PREPAID: i128 = 50_000_000;

fn create_active_sub(
    env: &Env,
    client: &SubscriptionVaultClient,
    token_admin: &TokenAdminClient,
) -> (u32, Address, Address, Address) {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    token_admin.mint(&subscriber, &(PREPAID * 2));
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
);
    client.deposit_funds(&id, &subscriber, &PREPAID, &None);
    (id, subscriber, merchant, client.get_subscription(&id).token)
}

// --- schedule_cancel ---

#[test]
fn test_schedule_cancel_happy_path() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    let cancel_at = T0 + INTERVAL;
    client.schedule_cancel(&sub_id, &subscriber, &cancel_at);

    // subscription is still Active — cancel_at hasn't fired yet
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.cancel_at, Some(cancel_at));
}

#[test]
fn test_schedule_cancel_past_timestamp_rejected() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    let result = client.try_schedule_cancel(&sub_id, &subscriber, &(T0 - 1));
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

#[test]
fn test_schedule_cancel_now_rejected() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    let result = client.try_schedule_cancel(&sub_id, &subscriber, &T0);
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

#[test]
fn test_schedule_cancel_forbidden_for_stranger() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, _subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);
    let stranger = Address::generate(&env);

    let result = client.try_schedule_cancel(&sub_id, &stranger, &(T0 + INTERVAL));
    assert_eq!(result, Err(Ok(Error::Forbidden)));
}

#[test]
fn test_schedule_cancel_merchant_allowed() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, _subscriber, merchant, _token) = create_active_sub(&env, &client, &token_admin);

    // merchant can also schedule
    client.schedule_cancel(&sub_id, &merchant, &(T0 + INTERVAL));
    let sub = client.get_subscription(&sub_id);
    assert!(sub.cancel_at.is_some());
}

// --- unschedule_cancel ---

#[test]
fn test_unschedule_cancel_clears_schedule() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    client.schedule_cancel(&sub_id, &subscriber, &(T0 + INTERVAL));
    client.unschedule_cancel(&sub_id, &subscriber);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.cancel_at, None);
}

#[test]
fn test_unschedule_cancel_idempotent() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    // No prior schedule — should not error
    client.unschedule_cancel(&sub_id, &subscriber);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.cancel_at, None);
}

// --- charge_one fires scheduled cancellation ---

#[test]
fn test_charge_one_fires_scheduled_cancellation_when_due() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    // Schedule cancel for exactly one interval from now
    let cancel_at = T0 + INTERVAL;
    client.schedule_cancel(&sub_id, &subscriber, &cancel_at);

    // Advance time to cancel_at
    env.ledger().set_timestamp(cancel_at);
    client.charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, crate::SubscriptionStatus::Cancelled);
    assert_eq!(sub.cancel_at, None);
}

#[test]
fn test_charge_one_does_not_fire_when_cancel_at_in_future() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    // Schedule cancel well in the future
    client.schedule_cancel(&sub_id, &subscriber, &(T0 + INTERVAL * 10));

    // Advance one interval — should charge normally, NOT cancel
    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, crate::SubscriptionStatus::Active);
}

#[test]
fn test_unschedule_before_fire_prevents_cancellation() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    let cancel_at = T0 + INTERVAL;
    client.schedule_cancel(&sub_id, &subscriber, &cancel_at);
    client.unschedule_cancel(&sub_id, &subscriber);

    // Advance to what would have been the fire time
    env.ledger().set_timestamp(cancel_at);
    client.charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>);

    let sub = client.get_subscription(&sub_id);
    // Should have charged normally, not cancelled
    assert_eq!(sub.status, crate::SubscriptionStatus::Active);
}
;
    client.deposit_funds(&id, &subscriber, &PREPAID, &None);
    (id, subscriber, merchant, client.get_subscription(&id).token)
}

// --- schedule_cancel ---

#[test]
fn test_schedule_cancel_happy_path() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    let cancel_at = T0 + INTERVAL;
    client.schedule_cancel(&sub_id, &subscriber, &cancel_at);

    // subscription is still Active — cancel_at hasn't fired yet
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.cancel_at, Some(cancel_at));
}

#[test]
fn test_schedule_cancel_past_timestamp_rejected() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    let result = client.try_schedule_cancel(&sub_id, &subscriber, &(T0 - 1));
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

#[test]
fn test_schedule_cancel_now_rejected() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    let result = client.try_schedule_cancel(&sub_id, &subscriber, &T0);
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

#[test]
fn test_schedule_cancel_forbidden_for_stranger() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, _subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);
    let stranger = Address::generate(&env);

    let result = client.try_schedule_cancel(&sub_id, &stranger, &(T0 + INTERVAL));
    assert_eq!(result, Err(Ok(Error::Forbidden)));
}

#[test]
fn test_schedule_cancel_merchant_allowed() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, _subscriber, merchant, _token) = create_active_sub(&env, &client, &token_admin);

    // merchant can also schedule
    client.schedule_cancel(&sub_id, &merchant, &(T0 + INTERVAL));
    let sub = client.get_subscription(&sub_id);
    assert!(sub.cancel_at.is_some());
}

// --- unschedule_cancel ---

#[test]
fn test_unschedule_cancel_clears_schedule() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    client.schedule_cancel(&sub_id, &subscriber, &(T0 + INTERVAL));
    client.unschedule_cancel(&sub_id, &subscriber);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.cancel_at, None);
}

#[test]
fn test_unschedule_cancel_idempotent() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    // No prior schedule — should not error
    client.unschedule_cancel(&sub_id, &subscriber);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.cancel_at, None);
}

// --- charge_one fires scheduled cancellation ---

#[test]
fn test_charge_one_fires_scheduled_cancellation_when_due() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    // Schedule cancel for exactly one interval from now
    let cancel_at = T0 + INTERVAL;
    client.schedule_cancel(&sub_id, &subscriber, &cancel_at);

    // Advance time to cancel_at
    env.ledger().set_timestamp(cancel_at);
    client.charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, crate::SubscriptionStatus::Cancelled);
    assert_eq!(sub.cancel_at, None);
}

#[test]
fn test_charge_one_does_not_fire_when_cancel_at_in_future() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    // Schedule cancel well in the future
    client.schedule_cancel(&sub_id, &subscriber, &(T0 + INTERVAL * 10));

    // Advance one interval — should charge normally, NOT cancel
    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, crate::SubscriptionStatus::Active);
}

#[test]
fn test_unschedule_before_fire_prevents_cancellation() {
    let (env, _, client, token_admin, _token) = setup();
    env.ledger().set_timestamp(T0);
    let (sub_id, subscriber, _merchant, _token) = create_active_sub(&env, &client, &token_admin);

    let cancel_at = T0 + INTERVAL;
    client.schedule_cancel(&sub_id, &subscriber, &cancel_at);
    client.unschedule_cancel(&sub_id, &subscriber);

    // Advance to what would have been the fire time
    env.ledger().set_timestamp(cancel_at);
    client.charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>);

    let sub = client.get_subscription(&sub_id);
    // Should have charged normally, not cancelled
    assert_eq!(sub.status, crate::SubscriptionStatus::Active);
}
