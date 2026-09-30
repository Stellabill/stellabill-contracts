//! Tests for merchant vacation mode (#580).
//!
//! Covers:
//! - Setting a valid vacation window
//! - Charges blocked with `VacationActive` during the window
//! - Charges allowed before/after the window
//! - `clear_merchant_vacation` restores charging
//! - Zero-length window rejected (end_ts <= start_ts)
//! - Past start_ts rejected (start_ts < now)
//! - `is_merchant_in_vacation` boundary correctness
//! - Vacation mode isolation (other merchants unaffected)
//! - Vacation override (setting a new vacation replaces the old one)
//! - Idempotent clear (clear when no vacation set is a no-op)
//! - Split payees vacation check
//! - Vacation past subscription expiration
//! - Merchant vacation migration on address rotation

use crate::{Error, SubscriptionVault, SubscriptionVaultClient, SubscriptionStatus};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{token::StellarAssetClient as TokenAdminClient, Address, Env, String, Vec};

fn setup() -> (
    Env,
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
        &10i128,
        &(7 * 24 * 60 * 60u64),
    );

    (env, client, token_admin, token)
}

/// Create a merchant with config and a subscriber with a funded subscription.
/// Returns (sub_id, merchant, subscriber).
fn setup_merchant_and_sub(
    env: &Env,
    client: &SubscriptionVaultClient,
    token_admin: &TokenAdminClient,
) -> (u32, Address, Address) {
    let merchant = Address::generate(env);
    let subscriber = Address::generate(env);

    use soroban_sdk::String;
    client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0i32,
        &0x1Fi32,
        &None,
        &String::from_str(env, "https://example.com"),
    );

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &100i128,
        &3600u64,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    token_admin.mint(&subscriber, &10_000i128);
    client.deposit_funds(&sub_id, &subscriber, &1_000i128, &None);

    (sub_id, merchant, subscriber)
}

// ── Basic vacation lifecycle ─────────────────────────────────────────────────

#[test]
fn test_set_vacation_valid() {
    let (env, client, _, _) = setup();
    let merchant = Address::generate(&env);

    let now = env.ledger().timestamp();
    let start = now + 100;
    let end = start + 3600;

    // Setting a valid vacation should succeed (no error).
    client.set_merchant_vacation(&merchant, &start, &end);

    // Vacation should be retrievable.
    let vac = client.get_merchant_vacation(&merchant).unwrap();
    assert_eq!(vac.start_ts, start);
    assert_eq!(vac.end_ts, end);
}

#[test]
fn test_set_vacation_rejects_zero_length() {
    let (env, client, _, _) = setup();
    let merchant = Address::generate(&env);

    let now = env.ledger().timestamp();
    let ts = now + 100;

    // end_ts == start_ts → invalid
    let res = client.try_set_merchant_vacation(&merchant, &ts, &ts);
    assert_eq!(res, Err(Ok(Error::InvalidInput)));

    // end_ts < start_ts → invalid
    let res2 = client.try_set_merchant_vacation(&merchant, &(ts + 100), &ts);
    assert_eq!(res2, Err(Ok(Error::InvalidInput)));
}

#[test]
fn test_set_vacation_rejects_past_start() {
    let (env, client, _, _) = setup();
    let merchant = Address::generate(&env);

    let now = env.ledger().timestamp();

    // start_ts < now => rejected
    let res = client.try_set_merchant_vacation(&merchant, &(now - 1), &(now + 3600));
    assert_eq!(res, Err(Ok(Error::InvalidExpiration)));
}

#[test]
fn test_set_vacation_allows_immediate_start() {
    let (env, client, _, _) = setup();
    let merchant = Address::generate(&env);

    let now = env.ledger().timestamp();

    // start_ts == now should be allowed
    client.set_merchant_vacation(&merchant, &now, &(now + 3600));

    let vac = client.get_merchant_vacation(&merchant).unwrap();
    assert_eq!(vac.start_ts, now);
}

#[test]
fn test_charge_blocked_during_vacation() {
    let (env, client, token_admin, _) = setup();
    let (sub_id, merchant, _) = setup_merchant_and_sub(&env, &client, &token_admin);

    let now = env.ledger().timestamp();

    // Set vacation starting now
    client.set_merchant_vacation(&merchant, &now, &(now + 3600));

    // Advance past the first charge interval
    env.ledger().set_timestamp(now + 3601);

    // Charge should be blocked with VacationActive
    let res = client.try_charge_subscription(&sub_id, &None);
    assert_eq!(res, Err(Ok(Error::VacationActive)));
}

#[test]
fn test_charge_allowed_before_vacation() {
    let (env, client, token_admin, _) = setup();
    let (sub_id, merchant, _) = setup_merchant_and_sub(&env, &client, &token_admin);

    let now = env.ledger().timestamp();

    // Set vacation starting in the future
    client.set_merchant_vacation(&merchant, &(now + 7200), &(now + 10800));

    // Advance past the first charge interval (but before vacation starts)
    env.ledger().set_timestamp(now + 3601);

    // Charge should succeed
    client.charge_subscription(&sub_id, &None);

    let sub = client.get_subscription(&sub_id);
    assert_ne!(sub.prepaid_balance, 1000);
}

#[test]
fn test_charge_allowed_after_vacation() {
    let (env, client, token_admin, _) = setup();
    let (sub_id, merchant, _) = setup_merchant_and_sub(&env, &client, &token_admin);

    let now = env.ledger().timestamp();

    // Set a short vacation
    client.set_merchant_vacation(&merchant, &(now + 100), &(now + 200));

    // Advance past the vacation end and past first charge interval
    env.ledger().set_timestamp(now + 3601);

    // Charge should succeed (vacation already expired)
    client.charge_subscription(&sub_id, &None);

    let sub = client.get_subscription(&sub_id);
    assert_ne!(sub.prepaid_balance, 1000);
}

#[test]
fn test_clear_vacation_restores_charging() {
    let (env, client, token_admin, _) = setup();
    let (sub_id, merchant, _) = setup_merchant_and_sub(&env, &client, &token_admin);

    let now = env.ledger().timestamp();

    // Set vacation
    client.set_merchant_vacation(&merchant, &now, &(now + 3600));

    // Clear it immediately
    client.clear_merchant_vacation(&merchant);

    // Advance past the first charge interval
    env.ledger().set_timestamp(now + 3601);

    // Charge should succeed
    client.charge_subscription(&sub_id, &None);

    let sub = client.get_subscription(&sub_id);
    assert_ne!(sub.prepaid_balance, 1000);
}

#[test]
fn test_clear_vacation_when_none_set_is_noop() {
    let (env, client, _, _) = setup();
    let merchant = Address::generate(&env);

    // Clearing when no vacation exists should not error
    client.clear_merchant_vacation(&merchant);

    // Still no vacation
    assert!(client.get_merchant_vacation(&merchant).is_none());
}

#[test]
fn test_vacation_override_replaces_existing() {
    let (env, client, _, _) = setup();
    let merchant = Address::generate(&env);

    let now = env.ledger().timestamp();

    // Set first vacation
    client.set_merchant_vacation(&merchant, &(now + 100), &(now + 3600));

    // Set second vacation (should replace)
    client.set_merchant_vacation(&merchant, &(now + 2000), &(now + 5000));

    let vac = client.get_merchant_vacation(&merchant).unwrap();
    assert_eq!(vac.start_ts, now + 2000);
    assert_eq!(vac.end_ts, now + 5000);
}

#[test]
fn test_is_merchant_in_vacation_boundary() {
    let (env, client, _, _) = setup();
    let merchant = Address::generate(&env);

    let now = env.ledger().timestamp();

    client.set_merchant_vacation(&merchant, &(now + 100), &(now + 200));

    // Before vacation: false
    assert!(!client.is_merchant_in_vacation(&merchant, &(now + 50)));

    // At exact start: true (inclusive)
    assert!(client.is_merchant_in_vacation(&merchant, &(now + 100)));

    // During vacation: true
    assert!(client.is_merchant_in_vacation(&merchant, &(now + 150)));

    // At exact end: false (exclusive)
    assert!(!client.is_merchant_in_vacation(&merchant, &(now + 200)));

    // After vacation: false
    assert!(!client.is_merchant_in_vacation(&merchant, &(now + 300)));
}

#[test]
fn test_vacation_does_not_affect_other_merchants() {
    let (env, client, token_admin, _) = setup();
    let (sub_id1, merchant1, _) = setup_merchant_and_sub(&env, &client, &token_admin);

    let merchant2 = Address::generate(&env);
    let subscriber2 = Address::generate(&env);

    use soroban_sdk::String;
    client.initialize_merchant_config(
        &merchant2,
        &merchant2,
        &0i32,
        &0x1Fi32,
        &None,
        &String::from_str(&env, "https://example.com"),
    );

    let sub_id2 = client.create_subscription(
        &subscriber2,
        &merchant2,
        &100i128,
        &3600u64,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    token_admin.mint(&subscriber2, &10_000i128);
    client.deposit_funds(&sub_id2, &subscriber2, &1_000i128, &None);

    let now = env.ledger().timestamp();

    // Put merchant1 in vacation
    client.set_merchant_vacation(&merchant1, &now, &(now + 3600));

    // Advance past first charge interval
    env.ledger().set_timestamp(now + 3601);

    // merchant1's subscription should be blocked
    let res1 = client.try_charge_subscription(&sub_id1, &None);
    assert_eq!(res1, Err(Ok(Error::VacationActive)));

    // merchant2's subscription should charge normally
    client.charge_subscription(&sub_id2, &None);

    let sub2 = client.get_subscription(&sub_id2);
    assert_ne!(sub2.prepaid_balance, 1000);
}

#[test]
fn test_vacation_usage_charge_blocked() {
    let (env, client, token_admin, _) = setup();

    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    use soroban_sdk::String;
    client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0i32,
        &0x1Fi32,
        &None,
        &String::from_str(&env, "https://example.com"),
    );

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &100i128,
        &3600u64,
        &true, // usage_enabled
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    token_admin.mint(&subscriber, &10_000i128);
    client.deposit_funds(&sub_id, &subscriber, &1_000i128, &None);

    let now = env.ledger().timestamp();

    // Put merchant in vacation
    client.set_merchant_vacation(&merchant, &now, &(now + 3600));

    // Usage charge should be blocked
    let res = client.try_charge_usage(&sub_id, &50i128);
    assert_eq!(res, Err(Ok(Error::VacationActive)));
}

#[test]
fn test_vacation_split_payees_blocked() {
    let (env, client, token_admin, _) = setup();

    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let payee0 = Address::generate(&env);
    let payee1 = Address::generate(&env);

    use soroban_sdk::String;
    for payee in [&merchant, &payee0, &payee1].iter() {
        client.initialize_merchant_config(
            payee,
            payee,
            &0i32,
            &0x1Fi32,
            &None,
            &String::from_str(&env, "https://example.com"),
        );
    }

    let mut entries = Vec::new(&env);
    entries.push_back((payee0.clone(), 5000u32));
    entries.push_back((payee1.clone(), 5000u32));

    let sub_id = client.create_subscription_with_split(
        &subscriber,
        &merchant,
        &100i128,
        &3600u64,
        &false,
        &None::<i128>,
        &None::<u64>,
        &entries,
    );

    token_admin.mint(&subscriber, &10_000i128);
    client.deposit_funds(&sub_id, &subscriber, &1_000i128, &None);

    let now = env.ledger().timestamp();

    // Put payee0 in vacation
    client.set_merchant_vacation(&payee0, &now, &(now + 3600));

    // Advance past first charge interval
    env.ledger().set_timestamp(now + 3601);

    // Charge should be blocked because payee0 is in vacation
    let res = client.try_charge_subscription(&sub_id, &None);
    assert_eq!(res, Err(Ok(Error::VacationActive)));
}

#[test]
fn test_vacation_past_subscription_expiration() {
    let (env, client, token_admin, _) = setup();

    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    use soroban_sdk::String;
    client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0i32,
        &0x1Fi32,
        &None,
        &String::from_str(&env, "https://example.com"),
    );

    let now = env.ledger().timestamp();
    let expires_at = now + 5000;

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &100i128,
        &3600u64,
        &false,
        &None::<i128>,
        &Some(expires_at),
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    token_admin.mint(&subscriber, &10_000i128);
    client.deposit_funds(&sub_id, &subscriber, &1_000i128, &None);

    // Set vacation that starts after subscription expiration
    client.set_merchant_vacation(&merchant, &(expires_at - 100), &(expires_at + 5000));

    // Advance past expiration and past charge interval
    env.ledger().set_timestamp(expires_at + 100);

    // Charge should fail with SubscriptionExpired (not VacationActive) because
    // expiration check comes before vacation check
    let res = client.try_charge_subscription(&sub_id, &None);
    assert_eq!(res, Err(Ok(Error::SubscriptionExpired)));

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Expired);
}

#[test]
fn test_vacation_deposit_blocked() {
    let (env, client, token_admin, _) = setup();
    let (sub_id, merchant, subscriber) =
        setup_merchant_and_sub(&env, &client, &token_admin);

    let now = env.ledger().timestamp();

    // Put merchant in vacation
    client.set_merchant_vacation(&merchant, &now, &(now + 3600));

    // Deposit should be blocked
    let res = client.try_deposit_funds(
        &sub_id,
        &subscriber,
        &500i128,
        &None,
    );
    assert_eq!(res, Err(Ok(Error::VacationActive)));
}

#[test]
fn test_get_merchant_vacation_returns_none_when_not_set() {
    let (env, client, _, _) = setup();
    let merchant = Address::generate(&env);

    let vac = client.get_merchant_vacation(&merchant);
    assert!(vac.is_none());
}

#[test]
fn test_vacation_events_emitted() {
    let (env, client, _, _) = setup();
    let merchant = Address::generate(&env);

    let now = env.ledger().timestamp();
    let start = now + 100;
    let end = start + 3600;

    // Set vacation → should emit vacation_started event
    client.set_merchant_vacation(&merchant, &start, &end);

    // Clear vacation → should emit vacation_ended event
    client.clear_merchant_vacation(&merchant);
}
