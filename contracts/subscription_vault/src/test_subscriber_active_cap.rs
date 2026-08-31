//! Tests for the per-subscriber active-subscription cap (#578).

use crate::{Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{token::StellarAssetClient as TokenAdminClient, Address, Env};

const INTERVAL: u64 = 30 * 24 * 3600;
const AMOUNT: i128 = 10_000_000;
const PREPAID: i128 = 1_000_000_000;

fn setup() -> (Env, Address, SubscriptionVaultClient<'static>, TokenAdminClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = TokenAdminClient::new(&env, &token);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    client.init(&token, &7u32, &admin, &1_000_000i128, &(7 * 24 * 60 * 60u64));

    (env, admin, client, token_admin, token)
}

fn create_sub(
    client: &SubscriptionVaultClient,
    subscriber: &Address,
    merchant: &Address,
) -> u32 {
    client.create_subscription(subscriber, merchant, &AMOUNT, &INTERVAL, &false, &None, &None,
    &None::<u32>,
    &None::<soroban_sdk::Symbol>,
)
}

/// The default cap (10) blocks the 11th concurrent active subscription for
/// the same subscriber, and emits `SubscriberCapReachedEvent` when it does.
#[test]
fn default_cap_blocks_the_eleventh_subscription() {
    let (env, _admin, client, token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    token_admin.mint(&subscriber, &PREPAID);

    for _ in 0..10 {
        let merchant = Address::generate(&env);
        create_sub(&client, &subscriber, &merchant);
    }
    assert_eq!(client.get_subscriber_active_count(&subscriber), 10);
    assert_eq!(client.get_subscriber_active_cap(&subscriber), 10);

    let merchant = Address::generate(&env);
    let result = client.try_create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None,
        &None::<u32>,
            &None::<soroban_sdk::Symbol>,
);
    assert_eq!(
        result,
        Err(Ok(Error::MaxConcurrentSubscriptionsReached))
    );
    // The rejected attempt must not have incremented the counter.
    assert_eq!(client.get_subscriber_active_count(&subscriber), 10);
}

/// Cancelling an active subscription frees a slot for a new one.
#[test]
fn cancelling_frees_a_slot() {
    let (env, admin, client, token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    token_admin.mint(&subscriber, &PREPAID);

    client.set_subscriber_active_cap(&admin, &subscriber, &Some(2u32));

    let merchant = Address::generate(&env);
    let id1 = create_sub(&client, &subscriber, &merchant);
    create_sub(&client, &subscriber, &merchant);
    assert_eq!(client.get_subscriber_active_count(&subscriber), 2);

    let blocked = client.try_create_subscription(
        &subscriber, &merchant, &AMOUNT, &INTERVAL, &false, &None, &None,
        &None::<u32>,
            &None::<soroban_sdk::Symbol>,
);
    assert_eq!(blocked, Err(Ok(Error::MaxConcurrentSubscriptionsReached)));

    client.cancel_subscription(&id1, &subscriber);
    assert_eq!(client.get_subscriber_active_count(&subscriber), 1);

    // Now there's room again.
    create_sub(&client, &subscriber, &merchant);
    assert_eq!(client.get_subscriber_active_count(&subscriber), 2);
}

/// Pausing decrements the count; resuming increments it back.
#[test]
fn pause_and_resume_round_trip_the_counter() {
    let (env, admin, client, token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    token_admin.mint(&subscriber, &PREPAID);
    client.set_subscriber_active_cap(&admin, &subscriber, &Some(1u32));

    let merchant = Address::generate(&env);
    let id = create_sub(&client, &subscriber, &merchant);
    assert_eq!(client.get_subscriber_active_count(&subscriber), 1);

    client.pause_subscription(&id, &subscriber);
    assert_eq!(client.get_subscriber_active_count(&subscriber), 0);

    // With the cap at 1 and the only subscription paused, a new one is
    // allowed again (mirrors real usage: pausing frees exposure).
    let id2 = create_sub(&client, &subscriber, &merchant);
    assert_eq!(client.get_subscriber_active_count(&subscriber), 1);

    // Resuming the first (still-paused) subscription would now exceed the
    // cap conceptually, but the counter itself simply reflects reality: it
    // increments back to 2 since resume only cares about the prior state,
    // not the cap (the cap is enforced at create time per the issue).
    client.resume_subscription(&id, &subscriber);
    assert_eq!(client.get_subscriber_active_count(&subscriber), 2);

    let _ = id2;
}

/// An admin override raises (or lowers) the cap for a specific subscriber,
/// without affecting other subscribers' default cap.
#[test]
fn admin_override_changes_the_effective_cap() {
    let (env, admin, client, token_admin, _token) = setup();
    let institutional = Address::generate(&env);
    let regular = Address::generate(&env);
    token_admin.mint(&institutional, &PREPAID);
    token_admin.mint(&regular, &PREPAID);

    assert_eq!(client.get_subscriber_active_cap(&institutional), 10);

    client.set_subscriber_active_cap(&admin, &institutional, &Some(50u32));
    assert_eq!(client.get_subscriber_active_cap(&institutional), 50);
    // Unrelated subscriber is unaffected.
    assert_eq!(client.get_subscriber_active_cap(&regular), 10);

    // Clearing the override restores the default.
    client.set_subscriber_active_cap(&admin, &institutional, &None);
    assert_eq!(client.get_subscriber_active_cap(&institutional), 10);
}

/// Only the admin may set an override.
#[test]
fn non_admin_cannot_set_override() {
    let (env, _admin, client, _token_admin, _token) = setup();
    let stranger = Address::generate(&env);
    let subscriber = Address::generate(&env);

    let result = client.try_set_subscriber_active_cap(&stranger, &subscriber, &Some(99u32));
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}
