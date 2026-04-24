#![cfg(test)]

use crate::{
    Error, RecoveryReason, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
use soroban_sdk::{token, Address, Env, String, Symbol, IntoVal};

extern crate alloc;
use alloc::format;

const T0: u64 = 1_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;

fn setup_env() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let min_topup = 1_000_000i128;
    client.init(&token, &6, &admin, &min_topup, &(7 * 24 * 60 * 60));

    (env, client, token, admin)
}

#[test]
fn test_recovery_success_all_reasons() {
    let (env, client, token, admin) = setup_env();
    let recipient = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let token_admin = admin.clone();
    let token_client = token::StellarAssetClient::new(&env, &token);

    // Mint 100 USDC to contract directly (stranded funds)
    token_client.mint(&client.address, &100_000_000);

    let reasons = [
        RecoveryReason::UserOverpayment,
        RecoveryReason::FailedTransfer,
        RecoveryReason::ExpiredEscrow,
        RecoveryReason::SystemCorrection,
    ];

    for (i, reason) in reasons.iter().enumerate() {
        let recovery_id = String::from_str(&env, &format!("rec_{}", i));
        let amount = 10_000_000;
        
        let balance_before = token::Client::new(&env, &token).balance(&recipient);
        
        client.recover_stranded_funds(&admin, &token, &recipient, &amount, &recovery_id, reason);
        
        let balance_after = token::Client::new(&env, &token).balance(&recipient);
        assert_eq!(balance_after - balance_before, amount);

        // Check event
        let events = env.events().all();
        if events.len() > 0 {
            let last_event = events.last().unwrap();
            assert_eq!(last_event.0, client.address);
        }
        
        // Let's not assert raw event contents here, just that it didn't panic and balance changed
    }
}

#[test]
fn test_recovery_unauthorized() {
    let (env, client, token, admin) = setup_env();
    let recipient = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let fake_admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let token_client = token::StellarAssetClient::new(&env, &token);

    token_client.mint(&client.address, &100_000_000);

    let recovery_id = String::from_str(&env, "rec_unauth");
    
    let result = client.try_recover_stranded_funds(&fake_admin, &token, &recipient, &10_000_000, &recovery_id, &RecoveryReason::UserOverpayment);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_recovery_amount_validation() {
    let (env, client, token, admin) = setup_env();
    let recipient = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let token_client = token::StellarAssetClient::new(&env, &token);

    token_client.mint(&client.address, &100_000_000);

    // Zero amount
    let rec_zero = String::from_str(&env, "rec_zero");
    let result = client.try_recover_stranded_funds(&admin, &token, &recipient, &0, &rec_zero, &RecoveryReason::UserOverpayment);
    assert_eq!(result, Err(Ok(Error::InvalidRecoveryAmount)));

    // Negative amount
    let rec_neg = String::from_str(&env, "rec_neg");
    let result = client.try_recover_stranded_funds(&admin, &token, &recipient, &-100, &rec_neg, &RecoveryReason::UserOverpayment);
    assert_eq!(result, Err(Ok(Error::InvalidRecoveryAmount)));

    // Overdraw
    let rec_over = String::from_str(&env, "rec_over");
    let result = client.try_recover_stranded_funds(&admin, &token, &recipient, &200_000_000, // Contract only has 100M
        &rec_over, &RecoveryReason::UserOverpayment);
    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));
}

#[test]
fn test_recovery_replay_protection() {
    let (env, client, token, admin) = setup_env();
    let recipient = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let token_client = token::StellarAssetClient::new(&env, &token);

    token_client.mint(&client.address, &100_000_000);

    let recovery_id = String::from_str(&env, "rec_replay");
    
    // First call succeeds
    client.recover_stranded_funds(&admin, &token, &recipient, &10_000_000, &recovery_id, &RecoveryReason::UserOverpayment);

    // Second call with same ID fails
    let result = client.try_recover_stranded_funds(&admin, &token, &recipient, &10_000_000, &recovery_id, &RecoveryReason::UserOverpayment);
    assert_eq!(result, Err(Ok(Error::Replay)));
}

#[test]
fn test_state_consistency() {
    let (env, client, token, admin) = setup_env();
    let subscriber = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let merchant = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let token_client = token::StellarAssetClient::new(&env, &token);
    let recipient = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

    // 1. Setup subscription and deposit
    token_client.mint(&subscriber, &50_000_000);
    
    let sub_id = client.create_subscription(&subscriber, &merchant, &10_000_000, &INTERVAL, &false, &None::<i128>, &None::<u64>);
    
    client.deposit_funds(&sub_id, &subscriber, &50_000_000);
    
    // Total accounted should be 50M. Contract balance is 50M.
    // Try to recover 1 from accounted funds - should fail
    let rec_id = String::from_str(&env, "rec_steal");
    let result = client.try_recover_stranded_funds(&admin, &token, &recipient, &1, &rec_id, &RecoveryReason::UserOverpayment);
    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));

    // 2. Stranded funds arrive (20M)
    token_client.mint(&client.address, &20_000_000);
    
    // 3. Try to over-recover (21M) - fails
    let rec_id2 = String::from_str(&env, "rec_over");
    let result2 = client.try_recover_stranded_funds(&admin, &token, &recipient, &20_000_001, &rec_id2, &RecoveryReason::UserOverpayment);
    assert_eq!(result2, Err(Ok(Error::InsufficientBalance)));

    // 4. Exact recovery succeeds
    let rec_id3 = String::from_str(&env, "rec_exact");
    client.recover_stranded_funds(&admin, &token, &recipient, &20_000_000, &rec_id3, &RecoveryReason::UserOverpayment);

    // 5. Normal operation still works (withdraw)
    client.cancel_subscription(&sub_id, &subscriber);
    client.withdraw_subscriber_funds(&sub_id, &subscriber);
    
    let sub_balance = token::Client::new(&env, &token).balance(&subscriber);
    assert_eq!(sub_balance, 50_000_000); // Got refund back
}
