#![cfg(test)]

use crate::{
    Error, RecoveryReason, SubscriptionStatus
};
use crate::test_utils::{fixtures, setup::TestEnv};
use soroban_sdk::{
    testutils::{Address as _, Events},
    token, String
};

extern crate alloc;
use alloc::format;

const INTERVAL: u64 = 30 * 24 * 60 * 60;

#[test]
fn test_recovery_success_all_reasons() {
    let test_env = TestEnv::with_min_topup(1_000_000);
    let recipient = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);
    let token_client = token::StellarAssetClient::new(&test_env.env, &test_env.token);

    // Mint 100 USDC to contract directly (stranded funds)
    token_client.mint(&test_env.client.address, &100_000_000);

    let reasons = [
        RecoveryReason::UserOverpayment,
        RecoveryReason::FailedTransfer,
        RecoveryReason::ExpiredEscrow,
        RecoveryReason::SystemCorrection,
    ];

    for (i, reason) in reasons.iter().enumerate() {
        let recovery_id = String::from_str(&test_env.env, &format!("rec_{}", i));
        let amount = 10_000_000;

        let balance_before = test_env.token_client().balance(&recipient);

        test_env.client.recover_stranded_funds(&test_env.admin, &test_env.token, &recipient, &amount, &recovery_id, reason);

        let balance_after = test_env.token_client().balance(&recipient);
        assert_eq!(balance_after - balance_before, amount);

        // Check event
        let events = test_env.env.events().all();
        if events.len() > 0 {
            let last_event = events.last().unwrap();
            assert_eq!(last_event.0, test_env.client.address);
        }
    }
}

#[test]
fn test_recovery_unauthorized() {
    let test_env = TestEnv::with_min_topup(1_000_000);
    let recipient = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);
    let fake_admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);
    let token_client = token::StellarAssetClient::new(&test_env.env, &test_env.token);

    token_client.mint(&test_env.client.address, &100_000_000);

    let recovery_id = String::from_str(&test_env.env, "rec_unauth");

    let result = test_env.client.try_recover_stranded_funds(&fake_admin, &test_env.token, &recipient, &10_000_000, &recovery_id, &RecoveryReason::UserOverpayment);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_recovery_amount_validation() {
    let test_env = TestEnv::with_min_topup(1_000_000);
    let recipient = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);
    let token_client = token::StellarAssetClient::new(&test_env.env, &test_env.token);

    token_client.mint(&test_env.client.address, &100_000_000);

    // Zero amount
    let rec_zero = String::from_str(&test_env.env, "rec_zero");
    let result = test_env.client.try_recover_stranded_funds(&test_env.admin, &test_env.token, &recipient, &0, &rec_zero, &RecoveryReason::UserOverpayment);
    assert_eq!(result, Err(Ok(Error::InvalidRecoveryAmount)));

    // Negative amount
    let rec_neg = String::from_str(&test_env.env, "rec_neg");
    let result = test_env.client.try_recover_stranded_funds(&test_env.admin, &test_env.token, &recipient, &-100, &rec_neg, &RecoveryReason::UserOverpayment);
    assert_eq!(result, Err(Ok(Error::InvalidRecoveryAmount)));

    // Overdraw
    let rec_over = String::from_str(&test_env.env, "rec_over");
    let result = test_env.client.try_recover_stranded_funds(&test_env.admin, &test_env.token, &recipient, &200_000_000, // Contract only has 100M
        &rec_over, &RecoveryReason::UserOverpayment);
    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));
}

#[test]
fn test_recovery_replay_protection() {
    let test_env = TestEnv::with_min_topup(1_000_000);
    let recipient = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);
    let token_client = token::StellarAssetClient::new(&test_env.env, &test_env.token);

    token_client.mint(&test_env.client.address, &100_000_000);

    let recovery_id = String::from_str(&test_env.env, "rec_replay");

    // First call succeeds
    test_env.client.recover_stranded_funds(&test_env.admin, &test_env.token, &recipient, &10_000_000, &recovery_id, &RecoveryReason::UserOverpayment);

    // Second call with same ID fails
    let result = test_env.client.try_recover_stranded_funds(&test_env.admin, &test_env.token, &recipient, &10_000_000, &recovery_id, &RecoveryReason::UserOverpayment);
    assert_eq!(result, Err(Ok(Error::Replay)));
}

#[test]
fn test_state_consistency() {
    let test_env = TestEnv::with_min_topup(1_000_000);
    let recipient = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&test_env.env);

    // 1. Setup subscription and deposit
    let (sub_id, subscriber, _) = fixtures::create_test_subscription(
        &test_env.env, &test_env.client, SubscriptionStatus::Active
    );
    test_env.stellar_token_client().mint(&subscriber, &50_000_000);
    test_env.client.deposit_funds(&sub_id, &subscriber, &50_000_000);

    // Total accounted should be 50M. Contract balance is 50M.
    // Try to recover 1 from accounted funds - should fail
    let rec_id = String::from_str(&test_env.env, "rec_steal");
    let result = test_env.client.try_recover_stranded_funds(&test_env.admin, &test_env.token, &recipient, &1, &rec_id, &RecoveryReason::UserOverpayment);
    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));

    // 2. Stranded funds arrive (20M)
    test_env.stellar_token_client().mint(&test_env.client.address, &20_000_000);

    // 3. Try to over-recover (21M) - fails
    let rec_id2 = String::from_str(&test_env.env, "rec_over");
    let result2 = test_env.client.try_recover_stranded_funds(&test_env.admin, &test_env.token, &recipient, &20_000_001, &rec_id2, &RecoveryReason::UserOverpayment);
    assert_eq!(result2, Err(Ok(Error::InsufficientBalance)));

    // 4. Exact recovery succeeds
    let rec_id3 = String::from_str(&test_env.env, "rec_exact");
    test_env.client.recover_stranded_funds(&test_env.admin, &test_env.token, &recipient, &20_000_000, &rec_id3, &RecoveryReason::UserOverpayment);

    // 5. Normal operation still works (withdraw)
    test_env.client.cancel_subscription(&sub_id, &subscriber);
    test_env.client.withdraw_subscriber_funds(&sub_id, &subscriber);

    let sub_balance = test_env.token_client().balance(&subscriber);
    assert_eq!(sub_balance, 50_000_000); // Got refund back
}
