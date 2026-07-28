#![cfg(test)]
extern crate std;

use super::*;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{token, Address, Env};

const T0: u64 = 1_000_000;
const INTERVAL: u64 = 60; // minimum valid interval

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

    let token_admin = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(token_admin.clone());
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

#[test]
fn test_expiration_timing_and_charging() {
    let (env, client, token, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let amount = 1_000_000i128; // at min_topup threshold
    let interval = INTERVAL;
    let expires_at = T0 + 2 * INTERVAL; // expires after 2 intervals

    let min_topup = 1_000_000i128;
    token_admin.mint(&subscriber, &(min_topup * 5));

    let sub_id = client.create_subscription_with_token(&subscriber, &merchant, &token.address, &amount, &interval, &false, &None::<i128>, &Some(expires_at));

    client.deposit_funds(&sub_id, &subscriber, &(min_topup * 5));

    // Before expiry (T0 + INTERVAL), should charge normally
    env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    client.charge_subscription(&sub_id);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.lifetime_charged, amount);

    // At expiry boundary
    env.ledger().with_mut(|l| l.timestamp = T0 + 2 * INTERVAL);
    let res = client.try_charge_subscription(&sub_id);
    assert!(res.is_err()); // Should reject as expired

    // Subscription should have expires_at set
    let sub_expired = client.get_subscription(&sub_id);
    assert!(sub_expired.expires_at.is_some());

    // After expiry
    env.ledger().with_mut(|l| l.timestamp = T0 + 3 * INTERVAL);
    let res2 = client.try_charge_subscription(&sub_id);
    assert!(res2.is_err()); // Still rejects

    // Check withdrawal behavior after expiry
    let initial_balance = token.balance(&subscriber);
    client.withdraw_subscriber_funds(&sub_id, &subscriber);
    let final_balance = token.balance(&subscriber);
    assert!(final_balance > initial_balance);
}

#[test]
fn test_cleanup_and_archival() {
    let (env, client, token, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let min_topup = 1_000_000i128;
    token_admin.mint(&subscriber, &(min_topup * 5));

    let sub_id = client.create_subscription_with_token(&subscriber, &merchant, &token.address, &100, &10, &false, &None::<i128>, &Some(1050));

    client.deposit_funds(&sub_id, &subscriber, &(min_topup * 5));

    // Try cleanup before expiry/cancel - should fail
    let res = client.try_cleanup_subscription(&sub_id, &subscriber);
    assert!(res.is_err());

    // Expire it
    env.ledger().with_mut(|l| l.timestamp = T0 + 2 * INTERVAL);

    // Cleanup now should succeed
    client.cleanup_subscription(&sub_id, &subscriber);

    let sub_archived = client.get_subscription(&sub_id);
    assert_eq!(sub_archived.status, SubscriptionStatus::Archived);

    // Archival reads - can still read it
    assert_eq!(sub_archived.amount, 1_000_000);

    // Ensure funds can be withdrawn (already done by cleanup_subscription in some impls,
    // or via explicit withdraw)
    let deposit_balance = (min_topup * 5) - 0; // no charges made before expiry
    let sub_balance = sub_archived.prepaid_balance;
    if sub_balance > 0 {
        let initial_balance = token.balance(&subscriber);
        client.withdraw_subscriber_funds(&sub_id, &subscriber);
        assert!(token.balance(&subscriber) > initial_balance);
    }
}

#[test]
fn test_expiration_vs_cancellation() {
    let (env, client, token, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let expires_at = T0 + 2 * INTERVAL;

    // Scenario 1: Cancel before expiry
    let sub_id1 = client.create_subscription_with_token(&subscriber, &merchant, &token.address, &100, &10, &false, &None::<i128>, &Some(1050));
    
    client.cancel_subscription(&sub_id1, &subscriber);
    assert_eq!(
        client.get_subscription(&sub_id1).status,
        SubscriptionStatus::Cancelled
    );

    env.ledger().with_mut(|l| l.timestamp = T0 + 3 * INTERVAL);
    // Should stay cancelled
    assert_eq!(
        client.get_subscription(&sub_id1).status,
        SubscriptionStatus::Cancelled
    );
    // Can be archived from Cancelled
    client.cleanup_subscription(&sub_id1, &subscriber);
    assert_eq!(
        client.get_subscription(&sub_id1).status,
        SubscriptionStatus::Archived
    );

    // Scenario 2: Expire without cancel
    let sub_id2 = client.create_subscription_with_token(&subscriber, &merchant, &token.address, &100, &10, &false, &None::<i128>, &Some(1050));
    
    // Trigger expiration
    env.ledger().with_mut(|l| l.timestamp = 1060);
    let res = client.try_cancel_subscription(&sub_id2, &subscriber);
    assert!(res.is_err()); // Cannot cancel an expired subscription directly, it is already expired

    // Archiving should work
    client.cleanup_subscription(&sub_id2, &subscriber);
    assert_eq!(
        client.get_subscription(&sub_id2).status,
        SubscriptionStatus::Archived
    );
}

#[test]
fn test_deposit_rejected_when_expired() {
    let (env, client, _token, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let min_topup = 1_000_000i128;
    token_admin.mint(&subscriber, &(min_topup * 5));

    let sub_id = client.create_subscription_with_token(&subscriber, &merchant, &token.address, &100, &10, &false, &None::<i128>, &Some(1050));

    // Advance past expiry
    env.ledger().with_mut(|l| l.timestamp = T0 + 3 * INTERVAL);
    // Trigger the expiration by attempting a charge
    let _ = client.try_charge_subscription(&sub_id);

    // Deposit after expiry should be rejected
    let res = client.try_deposit_funds(&sub_id, &subscriber, &min_topup);
    assert!(res.is_err());
}
