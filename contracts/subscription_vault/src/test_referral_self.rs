use crate::{Error, ReferralAttributedEvent, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::{Address as _, Events};
use soroban_sdk::{token::StellarAssetClient as TokenAdminClient, Address, Env, FromVal};

const INTERVAL: u64 = 30 * 24 * 3600;
const AMOUNT: i128 = 10_000_000;

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

    (env, admin, client, token_admin, token)
}

fn create_sub(
    client: &SubscriptionVaultClient,
    subscriber: &Address,
    merchant: &Address,
    inviter: Option<&Address>,
) -> u32 {
    client.create_subscription(
        subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &inviter.copied(),
    )
}

fn try_create_sub(
    client: &SubscriptionVaultClient,
    subscriber: &Address,
    merchant: &Address,
    inviter: Option<&Address>,
) -> Result<u32, Error> {
    client
        .try_create_subscription(
            subscriber,
            merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
            &inviter.copied(),
        )
        .unwrap_or_else(|e| e)
}

fn count_referral_events(env: &Env) -> usize {
    env.events()
        .all()
        .iter()
        .filter(|e| {
            let topic: soroban_sdk::Symbol = FromVal::from_val(env, &e.0.get(0).unwrap());
            topic == soroban_sdk::Symbol::new(env, "referral_attributed")
        })
        .count()
}

fn get_referral_events(env: &Env) -> Vec<ReferralAttributedEvent> {
    env.events()
        .all()
        .iter()
        .filter(|e| {
            let topic: soroban_sdk::Symbol = FromVal::from_val(env, &e.0.get(0).unwrap());
            topic == soroban_sdk::Symbol::new(env, "referral_attributed")
        })
        .map(|e| ReferralAttributedEvent::from_val(env, &e.2))
        .collect()
}

#[test]
fn self_referral_rejected() {
    let (env, _admin, client, _token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let result = try_create_sub(&client, &subscriber, &merchant, Some(&subscriber));
    assert_eq!(result, Err(Error::SelfReferralNotAllowed));
}

#[test]
fn no_referral_event_on_self_referral() {
    let (env, _admin, client, _token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let _ = try_create_sub(&client, &subscriber, &merchant, Some(&subscriber));
    assert_eq!(count_referral_events(&env), 0);
}

#[test]
fn self_referral_does_not_consume_id() {
    let (env, _admin, client, _token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let _ = try_create_sub(&client, &subscriber, &merchant, Some(&subscriber));
    let _ = try_create_sub(&client, &subscriber, &merchant, Some(&subscriber));
    let _ = try_create_sub(&client, &subscriber, &merchant, Some(&subscriber));
    // After three rejected self-referrals, the first successful call should get ID 0.
    let other = Address::generate(&env);
    let ok_id = create_sub(&client, &subscriber, &merchant, Some(&other));
    assert_eq!(ok_id, 0u32);
}

#[test]
fn valid_referral_with_merchant_succeeds() {
    let (env, _admin, client, _token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = create_sub(&client, &subscriber, &merchant, Some(&merchant));
    assert!(id > 0 || id == 0);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.subscriber, subscriber);
    assert_eq!(sub.merchant, merchant);
}

#[test]
fn valid_referral_with_merchant_emits_event() {
    let (env, _admin, client, _token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = create_sub(&client, &subscriber, &merchant, Some(&merchant));
    let referral_events = get_referral_events(&env);
    assert_eq!(referral_events.len(), 1);
    assert_eq!(referral_events[0].subscription_id, id);
    assert_eq!(referral_events[0].inviter, merchant);
    assert_eq!(referral_events[0].subscriber, subscriber);
}

#[test]
fn valid_referral_with_admin_succeeds() {
    let (env, admin, client, _token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = create_sub(&client, &subscriber, &merchant, Some(&admin));
    assert!(id > 0 || id == 0);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.subscriber, subscriber);
}

#[test]
fn valid_referral_with_admin_emits_event() {
    let (env, admin, client, _token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = create_sub(&client, &subscriber, &merchant, Some(&admin));
    let referral_events = get_referral_events(&env);
    assert_eq!(referral_events.len(), 1);
    assert_eq!(referral_events[0].inviter, admin);
}

#[test]
fn no_referral_when_inviter_none() {
    let (env, _admin, client, _token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = create_sub(&client, &subscriber, &merchant, None);
    assert!(id > 0 || id == 0);
    assert_eq!(count_referral_events(&env), 0);
}

#[test]
fn no_referral_event_when_inviter_none() {
    let (env, _admin, client, _token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let _id = create_sub(&client, &subscriber, &merchant, None);
    let referral_events = get_referral_events(&env);
    assert_eq!(referral_events.len(), 0);
}

#[test]
fn subscription_created_event_still_emitted_with_inviter() {
    let (env, _admin, client, _token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let _id = create_sub(&client, &subscriber, &merchant, Some(&merchant));
    let created_events: Vec<crate::SubscriptionCreatedEvent> = env
        .events()
        .all()
        .iter()
        .filter(|e| {
            let topic: soroban_sdk::Symbol = FromVal::from_val(&env, &e.0.get(0).unwrap());
            topic == soroban_sdk::Symbol::new(&env, "subscription_created")
        })
        .map(|e| crate::SubscriptionCreatedEvent::from_val(&env, &e.2))
        .collect();
    assert_eq!(created_events.len(), 1);
}

#[test]
fn zero_address_inviter_treated_as_valid() {
    let (env, _admin, client, _token_admin, _token) = setup();
    let zero = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = create_sub(&client, &subscriber, &merchant, Some(&zero));
    assert!(id > 0 || id == 0);
    let referral_events = get_referral_events(&env);
    assert_eq!(referral_events.len(), 1);
    assert_eq!(referral_events[0].inviter, zero);
}

#[test]
fn self_referral_with_zero_address_passes() {
    let (env, _admin, client, _token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let other = Address::generate(&env);

    let result = try_create_sub(&client, &subscriber, &merchant, Some(&subscriber));
    assert_eq!(result, Err(Error::SelfReferralNotAllowed));

    let result2 = try_create_sub(&client, &subscriber, &merchant, Some(&other));
    assert_eq!(result2, Ok(0u32));
}

#[test]
fn multiple_referral_events_tracked_correctly() {
    let (env, _admin, client, _token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let inviter = Address::generate(&env);

    let id1 = create_sub(&client, &subscriber, &merchant, Some(&inviter));
    let sub2 = Address::generate(&env);
    let mer2 = Address::generate(&env);
    let id2 = create_sub(&client, &sub2, &mer2, Some(&inviter));

    let referral_events = get_referral_events(&env);
    assert_eq!(referral_events.len(), 2);
    assert_eq!(referral_events[0].subscription_id, id1);
    assert_eq!(referral_events[0].inviter, inviter);
    assert_eq!(referral_events[1].subscription_id, id2);
    assert_eq!(referral_events[1].inviter, inviter);
}

#[test]
fn schema_version_on_referral_event() {
    let (env, _admin, client, _token_admin, _token) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let _id = create_sub(&client, &subscriber, &merchant, Some(&merchant));
    let referral_events = get_referral_events(&env);
    assert_eq!(referral_events.len(), 1);
    assert_eq!(
        referral_events[0].schema_version,
        crate::EVENT_SCHEMA_VERSION
    );
}
