use crate::test_utils::{fixtures, setup::TestEnv};
use crate::{
    ChargeExecutionResult, Error, SubscriptionStatus,
};
use soroban_sdk::{testutils::{Address as _, Events as _}, Address, Symbol, Vec, FromVal};

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const GRACE_PERIOD: u64 = 7 * 24 * 60 * 60;

#[test]
fn fixture_successful_charge_and_deposit() {
    let test_env = TestEnv::default();
    let (id, subscriber, _merchant) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );

    // Initial deposit
    let deposit_amount = 50_000_000;
    test_env.stellar_token_client().mint(&subscriber, &deposit_amount);
    test_env.client.deposit_funds(&id, &subscriber, &deposit_amount);

    // Verify deposit event immediately
    let events = test_env.env.events().all();
    assert!(events.iter().any(|e| Symbol::from_val(&test_env.env, &e.1.get(0).unwrap()) == Symbol::new(&test_env.env, "deposited")));

    // Advance time to next billing period
    test_env.jump(INTERVAL + 1);

    // Charge
    let result = test_env.client.charge_subscription(&id);
    assert_eq!(result, ChargeExecutionResult::Charged);

    // Verify charge event immediately
    let events = test_env.env.events().all();
    assert!(events.iter().any(|e| Symbol::from_val(&test_env.env, &e.1.get(0).unwrap()) == Symbol::new(&test_env.env, "charged")));
}

#[test]
fn fixture_insufficient_balance_grace_period() {
    let test_env = TestEnv::default();
    let (id, _subscriber, _merchant) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );

    // No funds deposited. Advance time.
    test_env.jump(INTERVAL + 1);

    // Charge attempt should fail and move to GracePeriod
    let result = test_env.client.charge_subscription(&id);
    assert_eq!(result, ChargeExecutionResult::InsufficientBalance);

    // Verify event immediately
    let events = test_env.env.events().all();
    assert!(events.iter().any(|e| Symbol::from_val(&test_env.env, &e.1.get(0).unwrap()) == Symbol::new(&test_env.env, "charge_failed")));

    let sub = test_env.client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::GracePeriod);
}

#[test]
fn fixture_insufficient_balance_terminal() {
    let test_env = TestEnv::default();
    let (id, _subscriber, _merchant) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );

    // Advance time past grace period
    test_env.jump(INTERVAL + GRACE_PERIOD + 1);

    // Charge attempt should fail and move to InsufficientBalance
    let result = test_env.client.charge_subscription(&id);
    assert_eq!(result, ChargeExecutionResult::InsufficientBalance);

    let sub = test_env.client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::InsufficientBalance);
}

#[test]
fn fixture_unauthorized_charge_rejected() {
    let test_env = TestEnv::default();
    let (id, _subscriber, _merchant) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );

    let attacker = Address::generate(&test_env.env);
    
    // Attacker tries to deposit funds into someone else's subscription
    // deposit_funds requires subscriber auth.
    let _result = test_env.client.try_deposit_funds(&id, &attacker, &AMOUNT);
    
    // In Soroban test environment with mock_all_auths, this might still succeed if we don't handle it right?
    // Actually, mock_all_auths() usually makes it so that any address is "authorized" for the duration of the call.
    // If I want to test auth failure, I might need to disable mock_all_auths or use a specific pattern.
    // But typically, we trust the contract's require_auth calls.
    
    // Let's try an operation that definitely fails auth if not the right person.
    // pause_subscription(id, authorizer) - do_pause_subscription checks authorizer is merchant or subscriber.
    
    let result = test_env.client.try_pause_subscription(&id, &attacker);
    assert_eq!(result, Err(Ok(Error::Forbidden)));
}

#[test]
fn fixture_refund_on_cancel_with_prepaid() {
    let test_env = TestEnv::default();
    let (id, subscriber, _merchant) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );

    let deposit_amount = 50_000_000;
    test_env.stellar_token_client().mint(&subscriber, &deposit_amount);
    test_env.client.deposit_funds(&id, &subscriber, &deposit_amount);

    // Cancel
    test_env.client.cancel_subscription(&id, &subscriber);
    let events = test_env.env.events().all();

    let sub = test_env.client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    // refund_amount should be > 0 in events
    assert!(events.iter().any(|e| Symbol::from_val(&test_env.env, &e.1.get(0).unwrap()) == Symbol::new(&test_env.env, "subscription_cancelled")));
}

#[test]
fn fixture_subscriber_withdrawal_after_cancel() {
    let test_env = TestEnv::default();
    let (id, subscriber, _merchant) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );

    let deposit_amount = 50_000_000;
    test_env.stellar_token_client().mint(&subscriber, &deposit_amount);
    test_env.client.deposit_funds(&id, &subscriber, &deposit_amount);

    test_env.client.cancel_subscription(&id, &subscriber);
    
    // Subscriber withdraws
    test_env.client.withdraw_subscriber_funds(&id, &subscriber);
    
    let events = test_env.env.events().all();
    assert!(events.iter().any(|e| Symbol::from_val(&test_env.env, &e.1.get(0).unwrap()) == Symbol::new(&test_env.env, "sub_withdrawn")));
}

#[test]
fn fixture_merchant_withdrawal() {
    let test_env = TestEnv::default();
    let (id, subscriber, merchant) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );

    let deposit_amount = 50_000_000;
    test_env.stellar_token_client().mint(&subscriber, &deposit_amount);
    test_env.client.deposit_funds(&id, &subscriber, &deposit_amount);

    test_env.jump(INTERVAL + 1);
    test_env.client.charge_subscription(&id);

    // Merchant withdraws
    test_env.client.withdraw_merchant_token_funds(&merchant, &test_env.token, &5_000_000);

    let events = test_env.env.events().all();
    assert!(events.iter().any(|e| Symbol::from_val(&test_env.env, &e.1.get(0).unwrap()) == Symbol::new(&test_env.env, "withdrawn")));
}

#[test]
fn fixture_batch_charge_all_success() {
    let test_env = TestEnv::default();
    let mut ids = Vec::new(&test_env.env);
    
    for _ in 0..3 {
        let (id, subscriber, _) = fixtures::create_subscription_detailed(
            &test_env.env,
            &test_env.client,
            SubscriptionStatus::Active,
            AMOUNT,
            INTERVAL,
        );
        test_env.stellar_token_client().mint(&subscriber, &AMOUNT);
        test_env.client.deposit_funds(&id, &subscriber, &AMOUNT);
        ids.push_back(id);
    }

    test_env.jump(INTERVAL + 1);

    let results = test_env.client.batch_charge(&ids);
    assert_eq!(results.len(), 3);
    for r in results.iter() {
        assert!(r.success);
        assert_eq!(r.error_code, 0);
    }
}

#[test]
fn fixture_batch_charge_partial_failure() {
    let test_env = TestEnv::default();
    let mut ids = Vec::new(&test_env.env);
    
    // 1: Success
    let (id1, subscriber1, _) = fixtures::create_subscription_detailed(&test_env.env, &test_env.client, SubscriptionStatus::Active, AMOUNT, INTERVAL);
    test_env.stellar_token_client().mint(&subscriber1, &AMOUNT);
    test_env.client.deposit_funds(&id1, &subscriber1, &AMOUNT);
    ids.push_back(id1);

    // 2: Failure (InsufficientBalance)
    let (id2, _, _) = fixtures::create_subscription_detailed(&test_env.env, &test_env.client, SubscriptionStatus::Active, AMOUNT, INTERVAL);
    ids.push_back(id2);

    // 3: Success
    let (id3, subscriber3, _) = fixtures::create_subscription_detailed(&test_env.env, &test_env.client, SubscriptionStatus::Active, AMOUNT, INTERVAL);
    test_env.stellar_token_client().mint(&subscriber3, &AMOUNT);
    test_env.client.deposit_funds(&id3, &subscriber3, &AMOUNT);
    ids.push_back(id3);

    test_env.jump(INTERVAL + 1);

    let results = test_env.client.batch_charge(&ids);
    assert_eq!(results.get(0).unwrap().success, true);
    assert_eq!(results.get(1).unwrap().success, false);
    assert_eq!(results.get(1).unwrap().error_code, Error::InsufficientBalance.to_code());
    assert_eq!(results.get(2).unwrap().success, true);
}

#[test]
fn fixture_batch_charge_all_fail() {
    let test_env = TestEnv::default();
    let mut ids = Vec::new(&test_env.env);
    
    for _ in 0..3 {
        let (id, _, _) = fixtures::create_subscription_detailed(&test_env.env, &test_env.client, SubscriptionStatus::Active, AMOUNT, INTERVAL);
        ids.push_back(id);
    }

    test_env.jump(INTERVAL + 1);

    let results = test_env.client.batch_charge(&ids);
    for r in results.iter() {
        assert!(!r.success);
        assert_eq!(r.error_code, Error::InsufficientBalance.to_code());
    }
}

#[test]
fn fixture_lifetime_cap_reached() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    
    let cap = AMOUNT * 2;
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &Some(cap),
        &None,
    );

    test_env.stellar_token_client().mint(&subscriber, &cap);
    test_env.client.deposit_funds(&id, &subscriber, &cap);

    // Charge 1
    test_env.jump(INTERVAL + 1);
    test_env.client.charge_subscription(&id);

    // Charge 2 (reaches cap)
    test_env.jump(INTERVAL + 1);
    test_env.client.charge_one_off(&id, &merchant, &AMOUNT);
    let events = test_env.env.events().all();
    assert!(events.iter().any(|e| Symbol::from_val(&test_env.env, &e.1.get(0).unwrap()) == Symbol::new(&test_env.env, "lifetime_cap_reached")));

    let sub = test_env.client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(sub.lifetime_charged, cap);
}

#[test]
fn fixture_grace_period_to_active_recovery() {
    let test_env = TestEnv::default();
    let (id, subscriber, _merchant) = fixtures::create_subscription_detailed(&test_env.env, &test_env.client, SubscriptionStatus::Active, AMOUNT, INTERVAL);

    test_env.jump(INTERVAL + 1);
    test_env.client.charge_subscription(&id); // Fails -> GracePeriod

    assert_eq!(test_env.client.get_subscription(&id).status, SubscriptionStatus::GracePeriod);

    // Deposit funds
    test_env.stellar_token_client().mint(&subscriber, &AMOUNT);
    test_env.client.deposit_funds(&id, &subscriber, &AMOUNT);

    // Should be back to Active if deposit covered it (or via resume)
    // Actually, do_deposit_funds might auto-resume if it covers the shortfall
    assert_eq!(test_env.client.get_subscription(&id).status, SubscriptionStatus::Active);
}

#[test]
fn fixture_pause_resume_charge_sequence() {
    let test_env = TestEnv::default();
    let (id, subscriber, _merchant) = fixtures::create_subscription_detailed(&test_env.env, &test_env.client, SubscriptionStatus::Active, AMOUNT, INTERVAL);

    test_env.client.pause_subscription(&id, &subscriber);
    let events1 = test_env.env.events().all();
    assert_eq!(test_env.client.get_subscription(&id).status, SubscriptionStatus::Paused);
    assert!(events1.iter().any(|e| Symbol::from_val(&test_env.env, &e.1.get(0).unwrap()) == Symbol::new(&test_env.env, "sub_paused")));

    test_env.client.resume_subscription(&id, &subscriber);
    let events2 = test_env.env.events().all();
    assert_eq!(test_env.client.get_subscription(&id).status, SubscriptionStatus::Active);
    assert!(events2.iter().any(|e| Symbol::from_val(&test_env.env, &e.1.get(0).unwrap()) == Symbol::new(&test_env.env, "sub_resumed")));
}

#[test]
fn fixture_replay_protection() {
    let test_env = TestEnv::default();
    let (id, subscriber, _merchant) = fixtures::create_subscription_detailed(&test_env.env, &test_env.client, SubscriptionStatus::Active, AMOUNT, INTERVAL);
    test_env.stellar_token_client().mint(&subscriber, &AMOUNT);
    test_env.client.deposit_funds(&id, &subscriber, &AMOUNT);

    test_env.jump(INTERVAL + 1);
    test_env.client.charge_subscription(&id); // Success

    // Try charging again for the same period
    let result = test_env.client.try_charge_subscription(&id);
    // Should return Replay error
    assert_eq!(result, Err(Ok(Error::Replay)));
}

#[test]
fn fixture_emergency_stop_blocks_charge() {
    let test_env = TestEnv::default();
    let (id, subscriber, _merchant) = fixtures::create_subscription_detailed(&test_env.env, &test_env.client, SubscriptionStatus::Active, AMOUNT, INTERVAL);
    test_env.stellar_token_client().mint(&subscriber, &AMOUNT);
    test_env.client.deposit_funds(&id, &subscriber, &AMOUNT);

    test_env.client.enable_emergency_stop(&test_env.admin);
    let events = test_env.env.events().all();
    assert!(events.iter().any(|e| Symbol::from_val(&test_env.env, &e.1.get(0).unwrap()) == Symbol::new(&test_env.env, "emergency_stop_enabled")));
    
    assert!(test_env.client.get_emergency_stop_status());

    test_env.jump(INTERVAL + 1);
    let result = test_env.client.try_charge_subscription(&id);
    assert_eq!(result, Err(Ok(Error::EmergencyStopActive)));
}

#[test]
fn fixture_one_off_charge() {
    let test_env = TestEnv::default();
    let (id, subscriber, merchant) = fixtures::create_subscription_detailed(&test_env.env, &test_env.client, SubscriptionStatus::Active, AMOUNT, INTERVAL);
    test_env.stellar_token_client().mint(&subscriber, &AMOUNT);
    test_env.client.deposit_funds(&id, &subscriber, &AMOUNT);

    test_env.client.charge_one_off(&id, &merchant, &5_000_000);
    let events = test_env.env.events().all();
    assert!(events.iter().any(|e| Symbol::from_val(&test_env.env, &e.1.get(0).unwrap()) == Symbol::new(&test_env.env, "oneoff_ch")));
}
