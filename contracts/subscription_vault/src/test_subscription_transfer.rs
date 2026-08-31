#![cfg(test)]

use crate::{SubscriptionVault, SubscriptionVaultClient};
use crate::types::{Error, SubscriptionStatus};
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
    let subscriber1 = Address::generate(&env);
    let subscriber2 = Address::generate(&env);

    let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_client.mint(&subscriber1, &1000_000_000);
    token_client.mint(&subscriber2, &1000_000_000);

    (env, client, admin, token, merchant, subscriber1, subscriber2)
}

#[test]
fn test_happy_path_transfer() {
    let (env, client, _admin, token, merchant, sub1, sub2) = setup();

    let sub_id = client.create_subscription_with_token(
        &sub1,
        &merchant,
        &token,
        &10_000,
        &86400,
        &false,
        &None,
        &None,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    client.deposit_funds(&sub_id, &sub1, &50_000, &None);

    let expires_at = env.ledger().timestamp() + 3600;
    client.initiate_transfer(&sub_id, &sub1, &sub2, &expires_at);

    client.accept_transfer(&sub_id, &sub2);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.subscriber, sub2);
    assert_eq!(sub.prepaid_balance, 50_000);
}

#[test]
fn test_merchant_veto_before_acceptance() {
    let (env, client, _admin, token, merchant, sub1, sub2) = setup();

    let sub_id = client.create_subscription_with_token(
        &sub1,
        &merchant,
        &token,
        &10_000,
        &86400,
        &false,
        &None,
        &None,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    let expires_at = env.ledger().timestamp() + 3600;
    client.initiate_transfer(&sub_id, &sub1, &sub2, &expires_at);

    // Merchant vetoes
    client.veto_transfer(&sub_id, &merchant);

    // Accept fails because intent is gone
    let res = client.try_accept_transfer(&sub_id, &sub2);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::TransferIntentNotFound.to_code());
}

#[test]
fn test_merchant_veto_after_acceptance() {
    let (env, client, _admin, token, merchant, sub1, sub2) = setup();

    let sub_id = client.create_subscription_with_token(
        &sub1,
        &merchant,
        &token,
        &10_000,
        &86400,
        &false,
        &None,
        &None,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    let expires_at = env.ledger().timestamp() + 3600;
    client.initiate_transfer(&sub_id, &sub1, &sub2, &expires_at);

    client.accept_transfer(&sub_id, &sub2);

    // Merchant tries to veto after acceptance
    let res = client.try_veto_transfer(&sub_id, &merchant);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::TransferIntentNotFound.to_code());
}

#[test]
fn test_expired_intent() {
    let (env, client, _admin, token, merchant, sub1, sub2) = setup();

    let sub_id = client.create_subscription_with_token(
        &sub1,
        &merchant,
        &token,
        &10_000,
        &86400,
        &false,
        &None,
        &None,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    let expires_at = env.ledger().timestamp() + 3600;
    client.initiate_transfer(&sub_id, &sub1, &sub2, &expires_at);

    // Time passes past expiry
    env.ledger().set_timestamp(expires_at + 1);

    // Accept fails due to expiry
    let res = client.try_accept_transfer(&sub_id, &sub2);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::TransferIntentExpired.to_code());
}

#[test]
fn test_transfer_to_self() {
    let (env, client, _admin, token, merchant, sub1, _sub2) = setup();

    let sub_id = client.create_subscription_with_token(
        &sub1,
        &merchant,
        &token,
        &10_000,
        &86400,
        &false,
        &None,
        &None,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    let expires_at = env.ledger().timestamp() + 3600;
    let res = client.try_initiate_transfer(&sub_id, &sub1, &sub1, &expires_at);
    assert_eq!(res.unwrap_err().unwrap().to_code(), Error::InvalidTransferTarget.to_code());
}
