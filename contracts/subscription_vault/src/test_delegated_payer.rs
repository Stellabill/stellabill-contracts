#![cfg(test)]

use crate::{SubscriptionVault, SubscriptionVaultClient};
use crate::types::{DelegatedPayerGrant, Error, SubscriptionStatus};
use soroban_sdk::{testutils::Address as _, testutils::Ledger, Address, Env};

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    let token = env.register_stellar_asset_contract_v2(admin.clone()).address();

    client.init(&token, &6, &admin, &100_000, &86400);

    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let payer = Address::generate(&env);

    let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_client.mint(&subscriber, &100_000_000);
    token_client.mint(&payer, &100_000_000);

    (env, client, admin, token, merchant, subscriber, payer)
}

fn create_active_sub(
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

// ── Grant tests ────────────────────────────────────────────────────────────

#[test]
fn grant_delegated_payer_happy_path() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    let expires_at = env.ledger().timestamp() + 3600;
    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &Some(expires_at), &None);

    let grant = client.get_delegated_payer_grant(&subscriber, &payer);
    assert!(grant.is_some());
    let g = grant.unwrap();
    assert_eq!(g.subscriber, subscriber);
    assert_eq!(g.payer, payer);
    assert_eq!(g.expires_at, Some(expires_at));
    assert_eq!(g.max_amount, None);
}

#[test]
fn grant_delegated_payer_with_max_amount() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &Some(50_000));

    let grant = client.get_delegated_payer_grant(&subscriber, &payer).unwrap();
    assert_eq!(grant.expires_at, None);
    assert_eq!(grant.max_amount, Some(50_000));
}

#[test]
fn grant_delegated_payer_overwrites_previous() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &Some(10_000));
    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &Some(20_000));

    let grant = client.get_delegated_payer_grant(&subscriber, &payer).unwrap();
    assert_eq!(grant.max_amount, Some(20_000));
}

#[test]
fn grant_delegated_payer_wrong_subscriber_fails() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    let other = Address::generate(&env);
    let res = client.try_grant_delegated_payer(&sub_id, &other, &payer, &None, &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::Unauthorized.to_code());
}

#[test]
fn grant_delegated_payer_expired_subscription_fails() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &_token,
        &10_000,
        &86400,
        &false,
        &None,
        &Some(&(env.ledger().timestamp() + 100)),
    );

    // Jump past expiration
    env.ledger().set_timestamp(env.ledger().timestamp() + 200);

    let res = client.try_grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::SubscriptionExpired.to_code());
}

#[test]
fn grant_delegated_payer_invalid_expiry_fails() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    // Expires at now (must be strictly > now)
    let now = env.ledger().timestamp();
    let res = client.try_grant_delegated_payer(&sub_id, &subscriber, &payer, &Some(now), &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::InvalidInput.to_code());

    // Expires in the past
    let res = client.try_grant_delegated_payer(&sub_id, &subscriber, &payer, &Some(now - 1), &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::InvalidInput.to_code());
}

#[test]
fn grant_delegated_payer_zero_max_amount_fails() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    let res = client.try_grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &Some(0));
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::InvalidAmount.to_code());
}

#[test]
fn grant_delegated_payer_negative_max_amount_fails() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    let res = client.try_grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &Some(-1));
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::InvalidAmount.to_code());
}

#[test]
fn grant_delegated_payer_nonexistent_subscription_fails() {
    let (env, client, _admin, _token, _merchant, subscriber, payer) = setup();

    let res = client.try_grant_delegated_payer(&999, &subscriber, &payer, &None, &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::NotFound.to_code());
}

// ── Revoke tests ───────────────────────────────────────────────────────────

#[test]
fn revoke_delegated_payer_happy_path() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &None);
    assert!(client.get_delegated_payer_grant(&subscriber, &payer).is_some());

    client.revoke_delegated_payer(&sub_id, &subscriber, &payer);
    assert!(client.get_delegated_payer_grant(&subscriber, &payer).is_none());
}

#[test]
fn revoke_delegated_payer_nonexistent_is_idempotent() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    // Should succeed without error
    client.revoke_delegated_payer(&sub_id, &subscriber, &payer);
    assert!(client.get_delegated_payer_grant(&subscriber, &payer).is_none());
}

#[test]
fn revoke_delegated_payer_wrong_subscriber_fails() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &None);

    let other = Address::generate(&env);
    let res = client.try_revoke_delegated_payer(&sub_id, &other, &payer);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::Unauthorized.to_code());
}

// ── Deposit on behalf tests ────────────────────────────────────────────────

#[test]
fn deposit_on_behalf_happy_path() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &None);
    client.deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &50_000, &None);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 50_000);
}

#[test]
fn deposit_on_behalf_respects_max_amount() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &Some(10_000));

    // Exactly at max should work
    client.deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &10_000, &None);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 10_000);

    // Exceeding max should fail
    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &10_001, &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::DelegatedDepositExceedsMax.to_code());
}

#[test]
fn deposit_on_behalf_expired_grant_fails() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    let expires_at = env.ledger().timestamp() + 3600;
    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &Some(expires_at), &None);

    // Jump past expiry
    env.ledger().set_timestamp(expires_at + 1);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &10_000, &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::DelegatedGrantExpired.to_code());
}

#[test]
fn deposit_on_behalf_no_grant_fails() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &10_000, &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::DelegatedGrantNotFound.to_code());
}

#[test]
fn deposit_on_behalf_revoked_grant_fails() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &None);
    client.revoke_delegated_payer(&sub_id, &subscriber, &payer);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &10_000, &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::DelegatedGrantNotFound.to_code());
}

#[test]
fn deposit_on_behalf_below_minimum_topup_fails() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &None);

    // Min topup is 100_000, so 50_000 should fail
    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &50_000, &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::BelowMinimumTopup.to_code());
}

#[test]
fn deposit_on_behalf_payer_blocklisted_fails() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &None);

    // Blocklist the payer
    client.add_to_blocklist(&_admin, &payer, &None);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &200_000, &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::SubscriberBlocklisted.to_code());
}

#[test]
fn deposit_on_behalf_subscriber_not_matching_fails() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    let other_sub = Address::generate(&env);
    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &None);

    // Deposit on behalf of the wrong subscriber
    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &other_sub, &200_000, &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::Unauthorized.to_code());
}

#[test]
fn deposit_on_behalf_expired_subscription_fails() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &_token,
        &10_000,
        &86400,
        &false,
        &None,
        &Some(&(env.ledger().timestamp() + 100)),
    );

    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &None);

    // Jump past subscription expiration
    env.ledger().set_timestamp(env.ledger().timestamp() + 200);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &200_000, &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::SubscriptionExpired.to_code());
}

#[test]
fn deposit_on_behalf_payer_never_gets_withdrawal_rights() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &None);
    client.deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &200_000, &None);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 200_000);

    // Cancel subscription to enable withdrawal
    client.cancel_subscription(&sub_id, &subscriber);

    // Payer cannot withdraw
    let res = client.try_withdraw_subscriber_funds(&sub_id, &payer);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::Forbidden.to_code());

    // Subscriber CAN withdraw
    client.withdraw_subscriber_funds(&sub_id, &subscriber);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 0);
}

#[test]
fn deposit_on_behalf_multiple_payers_independent() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    let payer2 = Address::generate(&env);
    let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &_token);
    token_client.mint(&payer2, &100_000_000);

    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &Some(200_000));
    client.grant_delegated_payer(&sub_id, &subscriber, &payer2, &None, &Some(100_000));

    // Each payer is independent
    client.deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &200_000, &None);
    client.deposit_funds_on_behalf(&sub_id, &payer2, &subscriber, &100_000, &None);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 300_000);

    // Revoking one payer doesn't affect the other
    client.revoke_delegated_payer(&sub_id, &subscriber, &payer);

    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &100_000, &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::DelegatedGrantNotFound.to_code());

    // Payer2 still works
    client.deposit_funds_on_behalf(&sub_id, &payer2, &subscriber, &100_000, &None);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 400_000);
}

#[test]
fn deposit_on_behalf_grant_expiry_at_boundary() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    let expires_at = env.ledger().timestamp() + 3600;
    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &Some(expires_at), &None);

    // Just before expiry should work
    env.ledger().set_timestamp(expires_at - 1);
    client.deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &200_000, &None);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 200_000);

    // At expiry should fail
    env.ledger().set_timestamp(expires_at);
    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &200_000, &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::DelegatedGrantExpired.to_code());
}

#[test]
fn deposit_on_behalf_with_max_amount_boundary() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    let max = 150_000i128;
    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &Some(max));

    // Exactly at max works
    client.deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &max, &None);

    // One unit over fails
    let res = client.try_deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &1, &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::DelegatedDepositExceedsMax.to_code());
}

#[test]
fn deposit_on_behalf_no_max_amount_allows_large_deposit() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    // No max_amount limit
    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &None);

    // Large deposit should work (only bounded by lifetime cap)
    client.deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &1_000_000, &None);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 1_000_000);
}

#[test]
fn deposit_on_behalf_idempotent_key() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &None);

    let idem_key = soroban_sdk::BytesN::from_array(&env, &[1u8; 32]);
    client.deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &200_000, &Some(idem_key.clone()));

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 200_000);

    // Same idempotent key should be a no-op
    client.deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &200_000, &Some(idem_key));
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 200_000);
}

#[test]
fn deposit_on_behalf_recovery_ready_event() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &None, &None);

    // Deposit enough to cover one interval
    client.deposit_funds_on_behalf(&sub_id, &payer, &subscriber, &200_000, &None);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(sub.prepaid_balance, 200_000);
}

#[test]
fn grant_get_revoke_lifecycle() {
    let (env, client, _admin, _token, merchant, subscriber, payer) = setup();
    let sub_id = create_active_sub(&env, &client, &subscriber, &merchant, &_token);

    // Initially no grant
    assert!(client.get_delegated_payer_grant(&subscriber, &payer).is_none());

    // Grant
    let expires_at = env.ledger().timestamp() + 7200;
    client.grant_delegated_payer(&sub_id, &subscriber, &payer, &Some(expires_at), &Some(50_000));

    let g = client.get_delegated_payer_grant(&subscriber, &payer).unwrap();
    assert_eq!(g.subscriber, subscriber);
    assert_eq!(g.payer, payer);
    assert_eq!(g.expires_at, Some(expires_at));
    assert_eq!(g.max_amount, Some(50_000));

    // Revoke
    client.revoke_delegated_payer(&sub_id, &subscriber, &payer);
    assert!(client.get_delegated_payer_grant(&subscriber, &payer).is_none());
}

#[test]
fn deposit_on_behalf_nonexistent_subscription_fails() {
    let (env, client, _admin, _token, _merchant, subscriber, payer) = setup();

    client.grant_delegated_payer(&999, &subscriber, &payer, &None, &None);

    let res = client.try_deposit_funds_on_behalf(&999, &payer, &subscriber, &200_000, &None);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::NotFound.to_code());
}
