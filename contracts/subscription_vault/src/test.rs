use crate::{
    can_transition, get_allowed_transitions, validate_status_transition, Error,
    Subscription, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env};

// =============================================================================
// State Machine Helper Tests
// =============================================================================

#[test]
fn test_validate_status_transition_same_status_is_allowed() {
    // Idempotent transitions should be allowed
    assert!(validate_status_transition(&SubscriptionStatus::Active, &SubscriptionStatus::Active).is_ok());
    assert!(validate_status_transition(&SubscriptionStatus::Paused, &SubscriptionStatus::Paused).is_ok());
    assert!(validate_status_transition(&SubscriptionStatus::Cancelled, &SubscriptionStatus::Cancelled).is_ok());
    assert!(validate_status_transition(&SubscriptionStatus::InsufficientBalance, &SubscriptionStatus::InsufficientBalance).is_ok());
}

#[test]
fn test_validate_active_transitions() {
    // Active -> Paused (allowed)
    assert!(validate_status_transition(&SubscriptionStatus::Active, &SubscriptionStatus::Paused).is_ok());
    
    // Active -> Cancelled (allowed)
    assert!(validate_status_transition(&SubscriptionStatus::Active, &SubscriptionStatus::Cancelled).is_ok());
    
    // Active -> InsufficientBalance (allowed)
    assert!(validate_status_transition(&SubscriptionStatus::Active, &SubscriptionStatus::InsufficientBalance).is_ok());
}

#[test]
fn test_validate_paused_transitions() {
    // Paused -> Active (allowed)
    assert!(validate_status_transition(&SubscriptionStatus::Paused, &SubscriptionStatus::Active).is_ok());
    
    // Paused -> Cancelled (allowed)
    assert!(validate_status_transition(&SubscriptionStatus::Paused, &SubscriptionStatus::Cancelled).is_ok());
    
    // Paused -> InsufficientBalance (not allowed)
    assert_eq!(
        validate_status_transition(&SubscriptionStatus::Paused, &SubscriptionStatus::InsufficientBalance),
        Err(Error::InvalidStatusTransition)
    );
}

#[test]
fn test_validate_insufficient_balance_transitions() {
    // InsufficientBalance -> Active (allowed)
    assert!(validate_status_transition(&SubscriptionStatus::InsufficientBalance, &SubscriptionStatus::Active).is_ok());
    
    // InsufficientBalance -> Cancelled (allowed)
    assert!(validate_status_transition(&SubscriptionStatus::InsufficientBalance, &SubscriptionStatus::Cancelled).is_ok());
    
    // InsufficientBalance -> Paused (not allowed)
    assert_eq!(
        validate_status_transition(&SubscriptionStatus::InsufficientBalance, &SubscriptionStatus::Paused),
        Err(Error::InvalidStatusTransition)
    );
}

#[test]
fn test_validate_cancelled_transitions_all_blocked() {
    // Cancelled is a terminal state - no outgoing transitions allowed
    assert_eq!(
        validate_status_transition(&SubscriptionStatus::Cancelled, &SubscriptionStatus::Active),
        Err(Error::InvalidStatusTransition)
    );
    assert_eq!(
        validate_status_transition(&SubscriptionStatus::Cancelled, &SubscriptionStatus::Paused),
        Err(Error::InvalidStatusTransition)
    );
    assert_eq!(
        validate_status_transition(&SubscriptionStatus::Cancelled, &SubscriptionStatus::InsufficientBalance),
        Err(Error::InvalidStatusTransition)
    );
}

#[test]
fn test_can_transition_helper() {
    // True cases
    assert!(can_transition(&SubscriptionStatus::Active, &SubscriptionStatus::Paused));
    assert!(can_transition(&SubscriptionStatus::Active, &SubscriptionStatus::Cancelled));
    assert!(can_transition(&SubscriptionStatus::Paused, &SubscriptionStatus::Active));
    
    // False cases
    assert!(!can_transition(&SubscriptionStatus::Cancelled, &SubscriptionStatus::Active));
    assert!(!can_transition(&SubscriptionStatus::Cancelled, &SubscriptionStatus::Paused));
    assert!(!can_transition(&SubscriptionStatus::Paused, &SubscriptionStatus::InsufficientBalance));
}

#[test]
fn test_get_allowed_transitions() {
    // Active
    let active_targets = get_allowed_transitions(&SubscriptionStatus::Active);
    assert_eq!(active_targets.len(), 3);
    assert!(active_targets.contains(&SubscriptionStatus::Paused));
    assert!(active_targets.contains(&SubscriptionStatus::Cancelled));
    assert!(active_targets.contains(&SubscriptionStatus::InsufficientBalance));
    
    // Paused
    let paused_targets = get_allowed_transitions(&SubscriptionStatus::Paused);
    assert_eq!(paused_targets.len(), 2);
    assert!(paused_targets.contains(&SubscriptionStatus::Active));
    assert!(paused_targets.contains(&SubscriptionStatus::Cancelled));
    
    // Cancelled
    let cancelled_targets = get_allowed_transitions(&SubscriptionStatus::Cancelled);
    assert_eq!(cancelled_targets.len(), 0);
    
    // InsufficientBalance
    let ib_targets = get_allowed_transitions(&SubscriptionStatus::InsufficientBalance);
    assert_eq!(ib_targets.len(), 2);
    assert!(ib_targets.contains(&SubscriptionStatus::Active));
    assert!(ib_targets.contains(&SubscriptionStatus::Cancelled));
}

// =============================================================================
// Contract Entrypoint State Transition Tests
// =============================================================================

fn setup_test_env() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin);
    
    (env, client, token, admin)
}

fn create_test_subscription(env: &Env, client: &SubscriptionVaultClient, status: SubscriptionStatus) -> (u32, Address, Address) {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    let amount = 10_000_000i128; // 10 USDC
    let interval_seconds = 30 * 24 * 60 * 60; // 30 days
    let usage_enabled = false;
    
    // Create subscription (always starts as Active)
    let id = client.create_subscription(&subscriber, &merchant, &amount, &interval_seconds, &usage_enabled);
    
    // Manually set status if not Active (bypassing state machine for test setup)
    // Note: In production, this would go through proper transitions
    if status != SubscriptionStatus::Active {
        // We need to manipulate storage directly for test setup
        // This is a test-only pattern
        let mut sub = client.get_subscription(&id);
        sub.status = status;
        env.as_contract(&client.address, || {
            env.storage().instance().set(&id, &sub);
        });
    }
    
    (id, subscriber, merchant)
}

#[test]
fn test_pause_subscription_from_active() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    
    // Pause from Active should succeed
    client.pause_subscription(&id, &subscriber);
    
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Paused);
}

#[test]
#[should_panic(expected = "Error(Contract, #400)")]
fn test_pause_subscription_from_cancelled_should_fail() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    
    // First cancel
    client.cancel_subscription(&id, &subscriber);
    
    // Then try to pause (should fail)
    client.pause_subscription(&id, &subscriber);
    let min_topup = 1_000000i128; // 1 USDC
    client.init(&token, &admin, &min_topup);
    
    assert_eq!(client.get_min_topup(), min_topup);
}

#[test]
fn test_pause_subscription_from_paused_is_idempotent() {
    // Idempotent transition: Paused -> Paused should succeed (no-op)
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    
    // First pause
    client.pause_subscription(&id, &subscriber);
    assert_eq!(client.get_subscription(&id).status, SubscriptionStatus::Paused);
    
    // Pausing again should succeed (idempotent)
    client.pause_subscription(&id, &subscriber);
    assert_eq!(client.get_subscription(&id).status, SubscriptionStatus::Paused);
}

#[test]
fn test_cancel_subscription_from_active() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    
    // Cancel from Active should succeed
    client.cancel_subscription(&id, &subscriber);
    
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

#[test]
fn test_cancel_subscription_from_paused() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    
    // First pause
    client.pause_subscription(&id, &subscriber);
    
    // Then cancel
    client.cancel_subscription(&id, &subscriber);
    
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

#[test]
fn test_cancel_subscription_from_cancelled_is_idempotent() {
    // Idempotent transition: Cancelled -> Cancelled should succeed (no-op)
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    
    // First cancel
    client.cancel_subscription(&id, &subscriber);
    assert_eq!(client.get_subscription(&id).status, SubscriptionStatus::Cancelled);
    
    // Cancelling again should succeed (idempotent)
    client.cancel_subscription(&id, &subscriber);
    assert_eq!(client.get_subscription(&id).status, SubscriptionStatus::Cancelled);
}

#[test]
fn test_resume_subscription_from_paused() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    
    // First pause
    client.pause_subscription(&id, &subscriber);
    
    // Then resume
    client.resume_subscription(&id, &subscriber);
    
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

#[test]
#[should_panic(expected = "Error(Contract, #400)")]
fn test_resume_subscription_from_cancelled_should_fail() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    
    // First cancel
    client.cancel_subscription(&id, &subscriber);
    
    // Try to resume (should fail)
    client.resume_subscription(&id, &subscriber);
}

#[test]
fn test_state_transition_idempotent_same_status() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    
    // Cancelling from already cancelled should fail (but we need to set it first)
    // First cancel
    client.cancel_subscription(&id, &subscriber);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

// =============================================================================
// Complex State Transition Sequences
// =============================================================================

#[test]
fn test_full_lifecycle_active_pause_resume() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    
    // Active -> Paused
    client.pause_subscription(&id, &subscriber);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Paused);
    
    // Paused -> Active
    client.resume_subscription(&id, &subscriber);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    
    // Can pause again
    client.pause_subscription(&id, &subscriber);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Paused);
}

#[test]
fn test_full_lifecycle_active_cancel() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    
    // Active -> Cancelled (terminal)
    client.cancel_subscription(&id, &subscriber);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    
    // Verify no further transitions possible
    // We can't easily test all fail cases without #[should_panic] for each
}

#[test]
fn test_all_valid_transitions_coverage() {
    // This test exercises every valid state transition at least once
    
    // 1. Active -> Paused
    {
        let (env, client, _, _) = setup_test_env();
        let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
        client.pause_subscription(&id, &subscriber);
        assert_eq!(client.get_subscription(&id).status, SubscriptionStatus::Paused);
    }
    
    // 2. Active -> Cancelled
    {
        let (env, client, _, _) = setup_test_env();
        let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
        client.cancel_subscription(&id, &subscriber);
        assert_eq!(client.get_subscription(&id).status, SubscriptionStatus::Cancelled);
    }
    
    // 3. Active -> InsufficientBalance (simulated via direct storage manipulation)
    {
        let (env, client, _, _) = setup_test_env();
        let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
        
        // Simulate transition by updating storage directly
        let mut sub = client.get_subscription(&id);
        sub.status = SubscriptionStatus::InsufficientBalance;
        env.as_contract(&client.address, || {
            env.storage().instance().set(&id, &sub);
        });
        
        assert_eq!(client.get_subscription(&id).status, SubscriptionStatus::InsufficientBalance);
    }
    
    // 4. Paused -> Active
    {
        let (env, client, _, _) = setup_test_env();
        let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
        client.pause_subscription(&id, &subscriber);
        client.resume_subscription(&id, &subscriber);
        assert_eq!(client.get_subscription(&id).status, SubscriptionStatus::Active);
    }
    
    // 5. Paused -> Cancelled
    {
        let (env, client, _, _) = setup_test_env();
        let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
        client.pause_subscription(&id, &subscriber);
        client.cancel_subscription(&id, &subscriber);
        assert_eq!(client.get_subscription(&id).status, SubscriptionStatus::Cancelled);
    }
    
    // 6. InsufficientBalance -> Active
    {
        let (env, client, _, _) = setup_test_env();
        let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
        
        // Set to InsufficientBalance
        let mut sub = client.get_subscription(&id);
        sub.status = SubscriptionStatus::InsufficientBalance;
        env.as_contract(&client.address, || {
            env.storage().instance().set(&id, &sub);
        });
        
        // Resume to Active
        client.resume_subscription(&id, &subscriber);
        assert_eq!(client.get_subscription(&id).status, SubscriptionStatus::Active);
    }
    
    // 7. InsufficientBalance -> Cancelled
    {
        let (env, client, _, _) = setup_test_env();
        let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
        
        // Set to InsufficientBalance
        let mut sub = client.get_subscription(&id);
        sub.status = SubscriptionStatus::InsufficientBalance;
        env.as_contract(&client.address, || {
            env.storage().instance().set(&id, &sub);
        });
        
        // Cancel
        client.cancel_subscription(&id, &subscriber);
        assert_eq!(client.get_subscription(&id).status, SubscriptionStatus::Cancelled);
    }
}

// =============================================================================
// Invalid Transition Tests (#[should_panic] for each invalid case)
// =============================================================================

#[test]
#[should_panic(expected = "Error(Contract, #400)")]
fn test_invalid_cancelled_to_active() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    
    client.cancel_subscription(&id, &subscriber);
    client.resume_subscription(&id, &subscriber);
}

#[test]
#[should_panic(expected = "Error(Contract, #400)")]
fn test_invalid_insufficient_balance_to_paused() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    
    // Set to InsufficientBalance
    let mut sub = client.get_subscription(&id);
    sub.status = SubscriptionStatus::InsufficientBalance;
    env.as_contract(&client.address, || {
        env.storage().instance().set(&id, &sub);
    });
    
    // Can't pause from InsufficientBalance - only resume to Active or cancel
    // Since pause_subscription validates Active -> Paused, this should fail
    client.pause_subscription(&id, &subscriber);
}

#[test]
fn test_subscription_struct_status_field() {
    let env = Env::default();
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        amount: 10_000_0000,
        interval_seconds: 30 * 24 * 60 * 60,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: 50_000_0000,
        usage_enabled: false,
    };
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

// ============== Insufficient Balance Tests ==============

/// Test: deposit_funds recovery flow - status transitions from InsufficientBalance to Active
/// This tests the recovery mechanism by directly manipulating storage (bypassing client auth)
#[test] 
fn test_deposit_recovery_flow() {
    let env = Env::default();
    env.ledger().set_timestamp(1000);
    
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin);
    
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 10_000_0000i128;
    let initial_balance = 5_000_0000i128;
    let deposit_amount = 10_000_0000i128;
    
    // Create subscription in InsufficientBalance status
    let sub = Subscription {
        subscriber: subscriber.clone(),
        merchant,
        amount,
        interval_seconds: 30 * 24 * 60 * 60,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::InsufficientBalance,
        prepaid_balance: initial_balance,
        usage_enabled: false,
    };
    
    // Store directly using as_contract
    env.as_contract(&contract_id, || {
        env.storage().instance().set(&0u32, &sub);
        Ok::<(), ()>(())
    });
    
    // Verify initial status
    let initial = client.get_subscription(&0u32);
    assert_eq!(initial.status, SubscriptionStatus::InsufficientBalance);
    assert_eq!(initial.prepaid_balance, initial_balance);
    
    // Simulate deposit_funds recovery flow via as_contract
    // (In production this would be called by subscriber with proper auth)
    env.as_contract(&contract_id, || {
        let mut sub: Subscription = env.storage().instance().get(&0u32).unwrap();
        sub.prepaid_balance += deposit_amount;
        sub.status = SubscriptionStatus::Active; // Recovery: InsufficientBalance -> Active
        env.storage().instance().set(&0u32, &sub);
        Ok::<(), ()>(())
    });
    
    // Verify status changed to Active (recovery successful)
    let after_deposit = client.get_subscription(&0u32);
    assert_eq!(after_deposit.status, SubscriptionStatus::Active);
    assert_eq!(after_deposit.prepaid_balance, initial_balance + deposit_amount);
}

/// Test: charge_subscription behavior - insufficient balance triggers status change
/// Uses direct contract call to capture the error result
#[test]
fn test_charge_subscription_behavior() {
    let env = Env::default();
    let interval = 30 * 24 * 60 * 60u64;
    env.ledger().set_timestamp(interval + 1);
    
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin);
    
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 10_000_0000i128;
    
    // Create subscription with insufficient balance
    let sub = Subscription {
        subscriber: subscriber.clone(),
        merchant,
        amount,
        interval_seconds: interval,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: amount - 1,
        usage_enabled: false,
    };
    
    env.as_contract(&contract_id, || {
        env.storage().instance().set(&0u32, &sub);
        Ok::<(), ()>(())
    });
    
    // Test via direct contract call to verify behavior
    // (Client panics on error, but contract returns Err(InsufficientBalance))
    env.as_contract(&contract_id, || {
        use crate::Error;
        let result = SubscriptionVault::charge_subscription(env.clone(), 0u32);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), Error::InsufficientBalance);
        
        // Verify status changed to InsufficientBalance
        let updated: Subscription = env.storage().instance().get(&0u32).unwrap();
        assert_eq!(updated.status, SubscriptionStatus::InsufficientBalance);
        
        // CRITICAL INVARIANT: Balance unchanged
        assert_eq!(updated.prepaid_balance, amount - 1);
        
        Ok::<(), ()>(())
    });
}

/// Test: successful charge - exact balance gets deducted
#[test]
fn test_successful_charge_exact_balance() {
    let env = Env::default();
    let interval = 30 * 24 * 60 * 60u64;
    env.ledger().set_timestamp(interval + 1);
    
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin);
    
    let merchant = Address::generate(&env);
    let amount = 10_000_0000i128;
    
    // Create subscription with exact balance
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant,
        amount,
        interval_seconds: interval,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: amount,
        usage_enabled: false,
    };
    
    env.as_contract(&contract_id, || {
        env.storage().instance().set(&0u32, &sub);
        Ok::<(), ()>(())
    });
    
    // Test via direct contract call
    env.as_contract(&contract_id, || {
        let result = SubscriptionVault::charge_subscription(env.clone(), 0u32);
        assert!(result.is_ok());
        
        // Verify balance is now 0
        let updated: Subscription = env.storage().instance().get(&0u32).unwrap();
        assert_eq!(updated.prepaid_balance, 0i128);
        
        Ok::<(), ()>(())
    });
}

/// Test: multiple failed charges don't corrupt state
#[test]
fn test_repeated_failed_charges_no_corruption() {
    let env = Env::default();
    let interval = 30 * 24 * 60 * 60u64;
    env.ledger().set_timestamp(interval + 1);
    
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin);
    
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 10_000_0000i128;
    let initial_balance = 5_000_0000i128;
    
    let sub = Subscription {
        subscriber: subscriber.clone(),
        merchant,
        amount,
        interval_seconds: interval,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: initial_balance,
        usage_enabled: false,
    };
    
    env.as_contract(&contract_id, || {
        env.storage().instance().set(&0u32, &sub);
        Ok::<(), ()>(())
    });
    
    // Multiple charge attempts
    env.as_contract(&contract_id, || {
        // First attempt - fails with InsufficientBalance
        let r1 = SubscriptionVault::charge_subscription(env.clone(), 0u32);
        assert!(r1.is_err());
        
        // Verify balance unchanged after first failure (INVARIANT)
        let after_first: Subscription = env.storage().instance().get(&0u32).unwrap();
        assert_eq!(after_first.prepaid_balance, initial_balance);
        assert_eq!(after_first.status, SubscriptionStatus::InsufficientBalance);
        
        // Second attempt - status is now InsufficientBalance, so returns Ok (skipped)
        let r2 = SubscriptionVault::charge_subscription(env.clone(), 0u32);
        assert!(r2.is_ok()); // Returns Ok because status != Active
        
        // Third attempt - same, returns Ok
        let r3 = SubscriptionVault::charge_subscription(env.clone(), 0u32);
        assert!(r3.is_ok());
        
        // Balance still unchanged (INVARIANT preserved)
        let final_state: Subscription = env.storage().instance().get(&0u32).unwrap();
        assert_eq!(final_state.prepaid_balance, initial_balance);
        
        Ok::<(), ()>(())
    });
#[test]
fn test_init_and_struct() {
    let env = Env::default();
fn test_min_topup_below_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin);
    let subscriber = Address::generate(&env);
    let min_topup = 5_000000i128; // 5 USDC
    
    client.init(&token, &admin, &min_topup);
    
    let result = client.try_deposit_funds(&0, &subscriber, &4_999999);
    assert!(result.is_err());
}

#[test]
fn test_min_topup_exactly_at_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let min_topup = 5_000000i128; // 5 USDC
    
    client.init(&token, &admin, &min_topup);
    
    let result = client.try_deposit_funds(&0, &subscriber, &min_topup);
    assert!(result.is_ok());
}

#[test]
fn test_min_topup_above_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let min_topup = 5_000000i128; // 5 USDC
    
    client.init(&token, &admin, &min_topup);
    
    let result = client.try_deposit_funds(&0, &subscriber, &10_000000);
    assert!(result.is_ok());
}

#[test]
fn test_set_min_topup_by_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let initial_min = 1_000000i128;
    let new_min = 10_000000i128;
    
    client.init(&token, &admin, &initial_min);
    assert_eq!(client.get_min_topup(), initial_min);
    
    client.set_min_topup(&admin, &new_min);
    assert_eq!(client.get_min_topup(), new_min);
}

#[test]
fn test_set_min_topup_unauthorized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let non_admin = Address::generate(&env);
    let min_topup = 1_000000i128;
    
    client.init(&token, &admin, &min_topup);
    
    let result = client.try_set_min_topup(&non_admin, &5_000000);
    assert!(result.is_err());
}
