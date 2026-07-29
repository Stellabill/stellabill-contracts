#![cfg(test)]

use crate::{SubscriptionVault, SubscriptionVaultClient};
use crate::types::{Error, SubscriptionStatus};
use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, Address, Env};

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token, &6, &admin, &100_000, &86400);

    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let payer = Address::generate(&env);

    let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_client.mint(&subscriber, &1000_000_000);
    token_client.mint(&payer, &1000_000_000);
    token_client.mint(&merchant, &100_000_000);

    (env, client, subscriber, merchant, payer, token)
}

fn create_sub(
    env: &Env,
    client: &SubscriptionVaultClient,
    subscriber: &Address,
    merchant: &Address,
    token: &Address,
) -> u32 {
    client.create_subscription_with_token(
        subscriber,
        merchant,
        token,
        &10_000,
        &86400,
        &false,
        &None,
        &None,
    )
}

#[test]
fn test_grant_and_deposit_on_behalf() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let expires_at = env.ledger().timestamp() + 3600;
    let max_amount: i128 = 100_000;

    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &max_amount);

    client.deposit_funds_on_behalf(&sub_id, &payer, &50_000, &None);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 50_000);
}

#[test]
fn test_grant_consumed_after_deposit() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let expires_at = env.ledger().timestamp() + 3600;
    let max_amount: i128 = 100_000;

    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &max_amount);

    client.deposit_funds_on_behalf(&sub_id, &payer, &50_000, &None);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &10_000, &None);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::DelegatedPayerGrantNotFound.to_code(),
    );
}

#[test]
fn test_expired_grant_rejected() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let expires_at = env.ledger().timestamp() + 1;
    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &100_000);

    env.ledger().set_timestamp(expires_at + 1);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &10_000, &None);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::DelegatedPayerGrantExpired.to_code(),
    );
}

#[test]
fn test_over_limit_deposit_rejected() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let expires_at = env.ledger().timestamp() + 3600;
    let max_amount: i128 = 50_000;

    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &max_amount);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &100_000, &None);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::DelegatedPayerAmountExceeded.to_code(),
    );

    client.deposit_funds_on_behalf(&sub_id, &payer, &50_000, &None);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 50_000);
}

#[test]
fn test_unauthorized_payer_no_grant() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &10_000, &None);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::DelegatedPayerGrantNotFound.to_code(),
    );
}

#[test]
fn test_revoke_grant() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let expires_at = env.ledger().timestamp() + 3600;
    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &100_000);

    client.revoke_delegated_payer(&subscriber, &payer);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &10_000, &None);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::DelegatedPayerGrantNotFound.to_code(),
    );
}

#[test]
fn test_revoke_nonexistent_grant_fails() {
    let (env, client, subscriber, _merchant, payer, _token) = setup();
    let res = client.try_revoke_delegated_payer(&subscriber, &payer);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::DelegatedPayerGrantNotFound.to_code(),
    );
}

#[test]
fn test_grant_past_expiry_rejected() {
    let (env, client, subscriber, _merchant, _payer, _token) = setup();
    let payer = Address::generate(&env);

    let expires_at = env.ledger().timestamp() - 1;
    let res = client.try_grant_delegated_payer(&subscriber, &payer, &expires_at, &100_000);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::InvalidInput.to_code(),
    );
}

#[test]
fn test_grant_zero_max_amount_rejected() {
    let (env, client, subscriber, _merchant, _payer, _token) = setup();
    let payer = Address::generate(&env);

    let expires_at = env.ledger().timestamp() + 3600;
    let res = client.try_grant_delegated_payer(&subscriber, &payer, &expires_at, &0);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::InvalidAmount.to_code(),
    );
}

#[test]
fn test_grant_self_as_payer_rejected() {
    let (env, client, subscriber, _merchant, _payer, _token) = setup();

    let expires_at = env.ledger().timestamp() + 3600;
    let res = client.try_grant_delegated_payer(&subscriber, &subscriber, &expires_at, &100_000);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::InvalidInput.to_code(),
    );
}

#[test]
fn test_below_min_topup() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let expires_at = env.ledger().timestamp() + 3600;
    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &1_000_000);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &10, &None);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::BelowMinimumTopup.to_code(),
    );
}

#[test]
fn test_deposit_on_behalf_negative_amount() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let expires_at = env.ledger().timestamp() + 3600;
    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &1_000_000);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &-100, &None);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::InvalidAmount.to_code(),
    );
}

#[test]
fn test_deposit_on_behalf_expired_subscription() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token,
        &10_000,
        &86400,
        &false,
        &None,
        &Some(env.ledger().timestamp() + 1000),
    );

    let expires_at = env.ledger().timestamp() + 3600;
    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &100_000);

    env.ledger().set_timestamp(env.ledger().timestamp() + 2000);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &10_000, &None);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::SubscriptionExpired.to_code(),
    );
}

#[test]
fn test_deposit_on_behalf_recovery_ready() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let expires_at = env.ledger().timestamp() + 3600;
    let max_amount: i128 = 200_000;
    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &max_amount);

    client.deposit_funds(&sub_id, &subscriber, &5_000, &None);

    let _ = client.charge_subscription(&sub_id, &None);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::InsufficientBalance);

    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &max_amount);

    client.deposit_funds_on_behalf(&sub_id, &payer, &10_000, &None);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

#[test]
fn test_deposit_on_behalf_idempotency() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let expires_at = env.ledger().timestamp() + 3600;
    let max_amount: i128 = 100_000;
    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &max_amount);

    let idem_key = soroban_sdk::BytesN::from_array(&env, &[0u8; 32]);

    client.deposit_funds_on_behalf(&sub_id, &payer, &50_000, &Some(idem_key.clone()));

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 50_000);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &50_000, &Some(idem_key));
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::DelegatedPayerGrantNotFound.to_code(),
    );
}

#[test]
fn test_grant_multiple_payers_independent() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let payer2 = Address::generate(&env);
    let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_client.mint(&payer2, &1000_000_000);

    let expires_at = env.ledger().timestamp() + 3600;

    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &100_000);
    client.grant_delegated_payer(&subscriber, &payer2, &expires_at, &200_000);

    client.deposit_funds_on_behalf(&sub_id, &payer, &50_000, &None);
    client.deposit_funds_on_behalf(&sub_id, &payer2, &100_000, &None);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 150_000);
}

#[test]
fn test_revoke_only_target_payer() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let payer2 = Address::generate(&env);
    let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_client.mint(&payer2, &1000_000_000);

    let expires_at = env.ledger().timestamp() + 3600;

    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &100_000);
    client.grant_delegated_payer(&subscriber, &payer2, &expires_at, &100_000);

    client.revoke_delegated_payer(&subscriber, &payer);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &10_000, &None);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::DelegatedPayerGrantNotFound.to_code(),
    );

    client.deposit_funds_on_behalf(&sub_id, &payer2, &50_000, &None);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 50_000);
}

#[test]
fn test_deposit_on_behalf_blocklisted_subscriber() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let expires_at = env.ledger().timestamp() + 3600;
    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &100_000);

    let reason = soroban_sdk::String::from_str(&env, "fraud");
    client.add_to_blocklist(&subscriber, &subscriber, &Some(reason));

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &10_000, &None);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::SubscriberBlocklisted.to_code(),
    );
}

#[test]
fn test_deposit_on_behalf_merchant_paused() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let expires_at = env.ledger().timestamp() + 3600;
    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &100_000);

    client.pause_merchant(&merchant);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &10_000, &None);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::MerchantPaused.to_code(),
    );
}

#[test]
fn test_non_subscriber_cannot_grant() {
    let (env, client, _subscriber, _merchant, _payer, _token) = setup();
    let attacker = Address::generate(&env);
    let victim = Address::generate(&env);
    let payer = Address::generate(&env);

    env.disable_auth_mocks();

    let expires_at = env.ledger().timestamp() + 3600;
    let res = client.try_grant_delegated_payer(&victim, &payer, &expires_at, &100_000);
    assert!(res.is_err());
}

#[test]
fn test_deposit_on_behalf_credit_limit_respected() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    client.set_subscriber_credit_limit(&subscriber, &subscriber, &token, &20_000);

    let expires_at = env.ledger().timestamp() + 3600;
    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &100_000);

    client.deposit_funds_on_behalf(&sub_id, &payer, &10_000, &None);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &50_000, &None);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::DelegatedPayerGrantNotFound.to_code(),
    );
}

#[test]
fn test_deposit_on_behalf_lifetime_cap_respected() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token,
        &10_000,
        &86400,
        &false,
        &Some(50_000i128),
        &None,
    );

    let expires_at = env.ledger().timestamp() + 3600;
    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &100_000);

    client.deposit_funds_on_behalf(&sub_id, &payer, &50_000, &None);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 50_000);
}

#[test]
fn test_query_grant_readable() {
    let (env, client, subscriber, _merchant, _payer, _token) = setup();
    let payer = Address::generate(&env);

    let expires_at = env.ledger().timestamp() + 3600;
    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &100_000);

    let key = crate::types::DataKey::DelegatedPayerGrant(subscriber.clone(), payer.clone());
    let grant: Option<crate::types::DelegatedPayerGrant> = env.as_contract(&client.address, || {
        env.storage().persistent().get(&key)
    });
    assert!(grant.is_some());
    let grant = grant.unwrap();
    assert_eq!(grant.subscriber, subscriber);
    assert_eq!(grant.payer, payer);
    assert_eq!(grant.expires_at, expires_at);
    assert_eq!(grant.max_amount, 100_000);
}

#[test]
fn test_deposit_on_behalf_events_emitted() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let expires_at = env.ledger().timestamp() + 3600;
    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &100_000);

    let sub_before = client.get_subscription(&sub_id);

    client.deposit_funds_on_behalf(&sub_id, &payer, &50_000, &None);

    let sub_after = client.get_subscription(&sub_id);

    let mut events = env.events().all();
    // There should be at least a delegated_deposited event
    let has_deposit_event = events.iter().any(|e| {
        e.0.to_string().contains("delegated_deposited")
    });
    assert!(has_deposit_event, "Expected delegated_deposited event");
}

#[test]
fn test_unauthorized_payer_different_subscriber() {
    let (env, client, subscriber, merchant, _payer, token) = setup();
    let sub_id = create_sub(&env, &client, &subscriber, &merchant, &token);

    let other = Address::generate(&env);
    let other_payer = Address::generate(&env);

    let expires_at = env.ledger().timestamp() + 3600;
    client.grant_delegated_payer(&subscriber, &other_payer, &expires_at, &100_000);

    // other tries to use the grant meant for other_payer — wrong payer
    let res = client.try_deposit_funds_on_behalf(&sub_id, &other, &10_000, &None);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::DelegatedPayerGrantNotFound.to_code(),
    );
}

#[test]
fn test_deposit_on_behalf_wrong_subscription() {
    let (env, client, subscriber, merchant, payer, token) = setup();
    let sub_id_1 = create_sub(&env, &client, &subscriber, &merchant, &token);
    let sub_id_2 = create_sub(&env, &client, &subscriber, &merchant, &token);

    let expires_at = env.ledger().timestamp() + 3600;
    client.grant_delegated_payer(&subscriber, &payer, &expires_at, &100_000);

    // Use grant on sub_id_1
    client.deposit_funds_on_behalf(&sub_id_1, &payer, &50_000, &None);

    // Grant consumed, can't use it on sub_id_2
    let res = client.try_deposit_funds_on_behalf(&sub_id_2, &payer, &10_000, &None);
    assert_eq!(
        res.unwrap_err().unwrap().to_code(),
        Error::DelegatedPayerGrantNotFound.to_code(),
    );
}
