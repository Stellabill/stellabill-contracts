
use super::*;
use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
use soroban_sdk::{token, Address, Env};

const T0: u64 = 1_000_000;
const INTERVAL: u64 = 60; // minimum valid interval (MIN_SUBSCRIPTION_INTERVAL_SECONDS)

fn setup_test_env() -> (
    Env,
    SubscriptionVaultClient<'static>,
    token::Client<'static>,
    token::StellarAssetClient<'static>,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = T0);

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token_admin_addr = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(token_admin_addr.clone());
    let token_client = token::Client::new(&env, &token_id.address());
    let token_admin_client = token::StellarAssetClient::new(&env, &token_id.address());

    let min_topup = 1_000_000i128;
    client.init(
        &token_id.address(),
        &6,
        &admin,
        &min_topup,
        &(7 * 24 * 60 * 60),
    );

    (env, client, token_client, token_admin_client, admin)
}

// doc 3: charge_subscription rejected at expiry boundary and after;
// withdrawal allowed after expiry (doc 5, Flow 1 steps 1-3, 6)
#[test]
fn test_expiration_timing_and_charging() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let amount = 1_000_000i128;
    let interval = INTERVAL;
    let expires_at = T0 + 2 * INTERVAL;

    let min_topup = 1_000_000i128;
    token_admin.mint(&subscriber, &(min_topup * 5));

    let sub_id = client.create_subscription_with_token(&subscriber, &merchant, &token_client.address, &amount, &interval, &false, &None::<i128>, &Some(expires_at));

    token_admin.mint(&subscriber, &(amount * 5));

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token.address,
        &amount,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(expires_at),
    );
    client.deposit_funds(&sub_id, &subscriber, &(amount * 5));

    // Before expiry: charge succeeds
    env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    client.charge_subscription(&sub_id);
    assert_eq!(client.get_subscription(&sub_id).lifetime_charged, amount);

    // At expiry boundary — subscription expires_at = T0 + 2*INTERVAL
    env.ledger().with_mut(|l| l.timestamp = T0 + 2 * INTERVAL);
    let res = client.try_charge_subscription(&sub_id);
    assert!(res.is_err(), "charge at expiry should be rejected");

    // expires_at field is preserved on the subscription
    assert!(client.get_subscription(&sub_id).expires_at.is_some());

    // After expiry — still rejects
    env.ledger().with_mut(|l| l.timestamp = T0 + 3 * INTERVAL);
    let res2 = client.try_charge_subscription(&sub_id);
    assert!(res2.is_err(), "charge after expiry should be rejected");

    // Check withdrawal behavior after expiry
    let initial_balance = token_client.balance(&subscriber);
    client.withdraw_subscriber_funds(&sub_id, &subscriber);
    let final_balance = token_client.balance(&subscriber);
    assert!(final_balance > initial_balance);
}

// doc 3: charge_usage rejected when expired
#[test]
fn test_cleanup_and_archival() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let amount = 1_000_000i128;
    let expires_at = T0 + 2 * INTERVAL;
    let min_topup = 1_000_000i128;
    token_admin.mint(&subscriber, &(min_topup * 5));

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &min_topup,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(T0 + INTERVAL),
    );

    // Try cleanup before expiry — should fail
    let res = client.try_cleanup_subscription(&sub_id, &subscriber);
    assert!(res.is_err(), "cleanup before expiry should fail");

    // Advance past expiry and trigger it via a charge attempt
    env.ledger().with_mut(|l| l.timestamp = T0 + 2 * INTERVAL);
    let _ = client.try_charge_subscription(&sub_id); // transitions to Expired

    // Perform cleanup which archives the subscription
    client.cleanup_subscription(&sub_id, &subscriber);

    let sub_archived = client.get_subscription(&sub_id);
    assert_eq!(sub_archived.status, SubscriptionStatus::Archived);
    assert_eq!(sub_archived.amount, min_topup);

    // Ensure funds can be withdrawn (already done by cleanup_subscription in some impls,
    // or via explicit withdraw)
    let sub_balance = sub_archived.prepaid_balance;
    assert_eq!(sub_balance, 0, "Funds should have been returned during cleanup");
}

// doc 2, 4, Flow 2: cancel before expiry -> Cancelled -> Archived;
// expired path: cancel rejected, cleanup -> Archived
#[test]
fn test_expiration_vs_cancellation() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let min_topup = 1_000_000i128;
    token_admin.mint(&subscriber, &(min_topup * 5));

    let expires_at = T0 + 2 * INTERVAL;

    // Scenario 1: Cancel before expiry
    let sub_id1 = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(expires_at),
    );
    
    client.cancel_subscription(&sub_id1, &subscriber);
    assert_eq!(
        client.get_subscription(&sub_id1).status,
        SubscriptionStatus::Cancelled
    );
    client.cancel_subscription(&sub_id1, &subscriber);
    assert_eq!(client.get_subscription(&sub_id1).status, SubscriptionStatus::Cancelled);

    // Status stays Cancelled even after the would-be expiry time passes
    env.ledger().with_mut(|l| l.timestamp = T0 + 4 * INTERVAL);
    assert_eq!(client.get_subscription(&sub_id1).status, SubscriptionStatus::Cancelled);

    env.ledger().with_mut(|l| l.timestamp = T0 + 3 * INTERVAL);
    assert_eq!(
        client.get_subscription(&sub_id1).status,
        SubscriptionStatus::Cancelled,
        "status stays Cancelled after expiry time has passed"
    );
    // Can be archived from Cancelled
    client.cleanup_subscription(&sub_id1, &subscriber);
    assert_eq!(client.get_subscription(&sub_id1).status, SubscriptionStatus::Archived);

    // Flow 1: expire without cancel -> cancel rejected -> cleanup -> Archived
    let sub_id2 = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(T0 + 2 * INTERVAL),
    );

    // Scenario 2: Expire without cancel
    let sub_id2 = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(expires_at),
    );
    
    // Trigger expiration
    env.ledger().with_mut(|l| l.timestamp = expires_at + 1);
    let res = client.try_cancel_subscription(&sub_id2, &subscriber);
    assert_eq!(res, Err(Ok(Error::SubscriptionExpired)));

    client.cleanup_subscription(&sub_id2, &subscriber);
    assert_eq!(client.get_subscription(&sub_id2).status, SubscriptionStatus::Archived);
}

// doc 3: deposit_funds rejected when expired
#[test]
fn test_deposit_rejected_when_expired() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let min_topup = 1_000_000i128;
    let expires_at = T0 + 2 * INTERVAL;
    token_admin.mint(&subscriber, &(min_topup * 5));

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &min_topup,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(T0 + INTERVAL),
    );

    // Advance past expiry
    env.ledger().with_mut(|l| l.timestamp = T0 + 100);
    // Trigger the expiration by attempting a charge
    let _ = client.try_charge_subscription(&sub_id);

    // subscription.is_expired(now) is true; deposit should be rejected
    let res = client.try_deposit_funds(&sub_id, &subscriber, &min_topup);
    assert_eq!(res, Err(Ok(Error::SubscriptionExpired)));
}
