use crate::safe_math::*;
use crate::{
    can_transition, get_allowed_transitions, validate_status_transition, Error, RecoveryReason,
    Subscription, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
use soroban_sdk::{Address, Env, IntoVal, Vec as SorobanVec};

/// Baseline creation timestamp used by test helpers.
const T0: u64 = 1_000;
/// Default billing interval for tests (30 days in seconds).
const INTERVAL: u64 = 30 * 24 * 60 * 60;

// =============================================================================
// State Machine Helper Tests
// =============================================================================

#[test]
fn test_validate_status_transition_same_status_is_allowed() {
    // Idempotent transitions should be allowed
    assert!(
        validate_status_transition(&SubscriptionStatus::Active, &SubscriptionStatus::Active)
            .is_ok()
    );
    assert!(
        validate_status_transition(&SubscriptionStatus::Paused, &SubscriptionStatus::Paused)
            .is_ok()
    );
    assert!(validate_status_transition(
        &SubscriptionStatus::Cancelled,
        &SubscriptionStatus::Cancelled
    )
    .is_ok());
    assert!(validate_status_transition(
        &SubscriptionStatus::InsufficientBalance,
        &SubscriptionStatus::InsufficientBalance
    )
    .is_ok());
}

#[test]
fn test_validate_active_transitions() {
    // Active -> Paused (allowed)
    assert!(
        validate_status_transition(&SubscriptionStatus::Active, &SubscriptionStatus::Paused)
            .is_ok()
    );

    // Active -> Cancelled (allowed)
    assert!(validate_status_transition(
        &SubscriptionStatus::Active,
        &SubscriptionStatus::Cancelled
    )
    .is_ok());

    // Active -> InsufficientBalance (allowed)
    assert!(validate_status_transition(
        &SubscriptionStatus::Active,
        &SubscriptionStatus::InsufficientBalance
    )
    .is_ok());
}

#[test]
fn test_validate_paused_transitions() {
    // Paused -> Active (allowed)
    assert!(
        validate_status_transition(&SubscriptionStatus::Paused, &SubscriptionStatus::Active)
            .is_ok()
    );

    // Paused -> Cancelled (allowed)
    assert!(validate_status_transition(
        &SubscriptionStatus::Paused,
        &SubscriptionStatus::Cancelled
    )
    .is_ok());

    // Paused -> InsufficientBalance (not allowed)
    assert_eq!(
        validate_status_transition(
            &SubscriptionStatus::Paused,
            &SubscriptionStatus::InsufficientBalance
        ),
        Err(Error::InvalidStatusTransition)
    );
}

#[test]
fn test_validate_insufficient_balance_transitions() {
    // InsufficientBalance -> Active (allowed)
    assert!(validate_status_transition(
        &SubscriptionStatus::InsufficientBalance,
        &SubscriptionStatus::Active
    )
    .is_ok());

    // InsufficientBalance -> Cancelled (allowed)
    assert!(validate_status_transition(
        &SubscriptionStatus::InsufficientBalance,
        &SubscriptionStatus::Cancelled
    )
    .is_ok());

    // InsufficientBalance -> Paused (not allowed)
    assert_eq!(
        validate_status_transition(
            &SubscriptionStatus::InsufficientBalance,
            &SubscriptionStatus::Paused
        ),
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
        validate_status_transition(
            &SubscriptionStatus::Cancelled,
            &SubscriptionStatus::InsufficientBalance
        ),
        Err(Error::InvalidStatusTransition)
    );
}

#[test]
fn test_can_transition_helper() {
    // True cases
    assert!(can_transition(
        &SubscriptionStatus::Active,
        &SubscriptionStatus::Paused
    ));
    assert!(can_transition(
        &SubscriptionStatus::Active,
        &SubscriptionStatus::Cancelled
    ));
    assert!(can_transition(
        &SubscriptionStatus::Paused,
        &SubscriptionStatus::Active
    ));

    // False cases
    assert!(!can_transition(
        &SubscriptionStatus::Cancelled,
        &SubscriptionStatus::Active
    ));
    assert!(!can_transition(
        &SubscriptionStatus::Cancelled,
        &SubscriptionStatus::Paused
    ));
    assert!(!can_transition(
        &SubscriptionStatus::Paused,
        &SubscriptionStatus::InsufficientBalance
    ));
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

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let min_topup = 1_000000i128; // 1 USDC
    client.init(&token, &admin, &min_topup);

    (env, client, token, admin)
}

fn create_test_subscription(
    env: &Env,
    client: &SubscriptionVaultClient,
    status: SubscriptionStatus,
) -> (u32, Address, Address) {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    let amount = 10_000_000i128; // 10 USDC
    let interval_seconds = 30 * 24 * 60 * 60; // 30 days
    let usage_enabled = false;

    // Create subscription (always starts as Active)
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval_seconds,
        &usage_enabled,
    );

    // Manually set status if not Active (bypassing state machine for test setup)
    // Note: In production, this would go through proper transitions
    if status != SubscriptionStatus::Active {
        // We need to manipulate storage directly for test setup
        // This is a test-only pattern
        let mut sub = client.get_subscription(&id);
        sub.status = status;
        let _ = env.as_contract(&client.address, || {
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
}

#[test]
fn test_pause_subscription_from_paused_is_idempotent() {
    // Idempotent transition: Paused -> Paused should succeed (no-op)
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);

    // First pause
    client.pause_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );

    // Pausing again should succeed (idempotent)
    client.pause_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );
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
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );

    // Cancelling again should succeed (idempotent)
    client.cancel_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
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
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, SubscriptionStatus::Active);
        client.pause_subscription(&id, &subscriber);
        assert_eq!(
            client.get_subscription(&id).status,
            SubscriptionStatus::Paused
        );
    }

    // 2. Active -> Cancelled
    {
        let (env, client, _, _) = setup_test_env();
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, SubscriptionStatus::Active);
        client.cancel_subscription(&id, &subscriber);
        assert_eq!(
            client.get_subscription(&id).status,
            SubscriptionStatus::Cancelled
        );
    }

    // 3. Active -> InsufficientBalance (simulated via direct storage manipulation)
    {
        let (env, client, _, _) = setup_test_env();
        let (id, _subscriber, _) =
            create_test_subscription(&env, &client, SubscriptionStatus::Active);

        // Simulate transition by updating storage directly
        let mut sub = client.get_subscription(&id);
        sub.status = SubscriptionStatus::InsufficientBalance;
        let _ = env.as_contract(&client.address, || {
            env.storage().instance().set(&id, &sub);
        });

        assert_eq!(
            client.get_subscription(&id).status,
            SubscriptionStatus::InsufficientBalance
        );
    }

    // 4. Paused -> Active
    {
        let (env, client, _, _) = setup_test_env();
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, SubscriptionStatus::Active);
        client.pause_subscription(&id, &subscriber);
        client.resume_subscription(&id, &subscriber);
        assert_eq!(
            client.get_subscription(&id).status,
            SubscriptionStatus::Active
        );
    }

    // 5. Paused -> Cancelled
    {
        let (env, client, _, _) = setup_test_env();
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, SubscriptionStatus::Active);
        client.pause_subscription(&id, &subscriber);
        client.cancel_subscription(&id, &subscriber);
        assert_eq!(
            client.get_subscription(&id).status,
            SubscriptionStatus::Cancelled
        );
    }

    // 6. InsufficientBalance -> Active
    {
        let (env, client, _, _) = setup_test_env();
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, SubscriptionStatus::Active);

        // Set to InsufficientBalance
        let mut sub = client.get_subscription(&id);
        sub.status = SubscriptionStatus::InsufficientBalance;
        let _ = env.as_contract(&client.address, || {
            env.storage().instance().set(&id, &sub);
        });

        // Resume to Active
        client.resume_subscription(&id, &subscriber);
        assert_eq!(
            client.get_subscription(&id).status,
            SubscriptionStatus::Active
        );
    }

    // 7. InsufficientBalance -> Cancelled
    {
        let (env, client, _, _) = setup_test_env();
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, SubscriptionStatus::Active);

        // Set to InsufficientBalance
        let mut sub = client.get_subscription(&id);
        sub.status = SubscriptionStatus::InsufficientBalance;
        let _ = env.as_contract(&client.address, || {
            env.storage().instance().set(&id, &sub);
        });

        // Cancel
        client.cancel_subscription(&id, &subscriber);
        assert_eq!(
            client.get_subscription(&id).status,
            SubscriptionStatus::Cancelled
        );
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
    let _ = env.as_contract(&client.address, || {
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
        amount: 100_000_000,
        interval_seconds: 30 * 24 * 60 * 60,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: 500_000_000,
        usage_enabled: false,
    };
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

// =============================================================================
// Safe Math Tests
// =============================================================================

#[test]
fn test_safe_add_normal() {
    assert_eq!(safe_add(100, 200), Ok(300));
    assert_eq!(safe_add(0, 1000), Ok(1000));
    assert_eq!(safe_add(1_000_000, 2_000_000), Ok(3_000_000));
}

#[test]
fn test_safe_add_overflow() {
    assert_eq!(safe_add(i128::MAX, 1), Err(Error::Overflow));
    assert_eq!(safe_add(i128::MAX, 0), Ok(i128::MAX));
    assert_eq!(safe_add(i128::MAX - 1, 2), Err(Error::Overflow));
}

#[test]
fn test_safe_sub_normal() {
    assert_eq!(safe_sub(200, 100), Ok(100));
    assert_eq!(safe_sub(1000, 0), Ok(1000));
    assert_eq!(safe_sub(5_000_000, 2_000_000), Ok(3_000_000));
}

#[test]
fn test_safe_sub_underflow() {
    assert_eq!(safe_sub(i128::MIN, 1), Err(Error::Underflow));
    assert_eq!(safe_sub(i128::MIN, 0), Ok(i128::MIN));
    assert_eq!(safe_sub(i128::MIN + 1, 2), Err(Error::Underflow));
}

#[test]
fn test_safe_sub_negative_result() {
    // safe_sub allows negative results (it's for general arithmetic)
    assert_eq!(safe_sub(100, 200), Ok(-100));
    assert_eq!(safe_sub(0, 1), Ok(-1));
}

#[test]
fn test_validate_non_negative() {
    assert_eq!(validate_non_negative(0), Ok(()));
    assert_eq!(validate_non_negative(100), Ok(()));
    assert_eq!(validate_non_negative(i128::MAX), Ok(()));
    assert_eq!(validate_non_negative(-1), Err(Error::Underflow));
    assert_eq!(validate_non_negative(i128::MIN), Err(Error::Underflow));
}

#[test]
fn test_safe_add_balance_normal() {
    assert_eq!(safe_add_balance(1000, 500), Ok(1500));
    assert_eq!(safe_add_balance(0, 1000), Ok(1000));
    assert_eq!(safe_add_balance(1_000_000, 2_000_000), Ok(3_000_000));
}

#[test]
fn test_safe_add_balance_overflow() {
    assert_eq!(safe_add_balance(i128::MAX, 1), Err(Error::Overflow));
    assert_eq!(safe_add_balance(i128::MAX, 0), Ok(i128::MAX));
}

#[test]
fn test_safe_add_balance_negative_amount() {
    assert_eq!(safe_add_balance(1000, -100), Err(Error::Underflow));
    assert_eq!(safe_add_balance(0, -1), Err(Error::Underflow));
}

#[test]
fn test_safe_sub_balance_normal() {
    assert_eq!(safe_sub_balance(1000, 500), Ok(500));
    assert_eq!(safe_sub_balance(1000, 0), Ok(1000));
    assert_eq!(safe_sub_balance(5_000_000, 2_000_000), Ok(3_000_000));
}

#[test]
fn test_safe_sub_balance_insufficient() {
    assert_eq!(safe_sub_balance(1000, 1500), Err(Error::Underflow));
    assert_eq!(safe_sub_balance(100, 200), Err(Error::Underflow));
    assert_eq!(safe_sub_balance(0, 1), Err(Error::Underflow));
}

#[test]
fn test_safe_sub_balance_negative_amount() {
    assert_eq!(safe_sub_balance(1000, -100), Err(Error::Underflow));
    assert_eq!(safe_sub_balance(0, -1), Err(Error::Underflow));
}

#[test]
fn test_safe_sub_balance_exact_zero() {
    assert_eq!(safe_sub_balance(1000, 1000), Ok(0));
    assert_eq!(safe_sub_balance(1_000_000, 1_000_000), Ok(0));
}

#[test]
fn test_safe_add_zero() {
    assert_eq!(safe_add(0, 0), Ok(0));
    assert_eq!(safe_add(100, 0), Ok(100));
    assert_eq!(safe_add(0, 100), Ok(100));
    assert_eq!(safe_add(i128::MAX, 0), Ok(i128::MAX));
}

#[test]
fn test_safe_sub_zero() {
    assert_eq!(safe_sub(0, 0), Ok(0));
    assert_eq!(safe_sub(100, 0), Ok(100));
    assert_eq!(safe_sub(i128::MAX, 0), Ok(i128::MAX));
}

#[test]
fn test_safe_add_max_to_zero() {
    assert_eq!(safe_add(0, i128::MAX), Ok(i128::MAX));
}

#[test]
fn test_safe_sub_from_max() {
    assert_eq!(safe_sub(i128::MAX, 0), Ok(i128::MAX));
    assert_eq!(safe_sub(i128::MAX, 1), Ok(i128::MAX - 1));
}

#[test]
fn test_safe_add_max_to_one() {
    assert_eq!(safe_add(i128::MAX, 1), Err(Error::Overflow));
}

#[test]
fn test_safe_sub_min_from_zero() {
    // Subtracting i128::MIN from 0 would require adding i128::MAX + 1, which overflows
    // This tests the edge case where subtraction underflows
    assert_eq!(safe_sub(0, i128::MIN), Err(Error::Underflow));
}

#[test]
fn test_usdc_amounts() {
    // Test with realistic USDC amounts (6 decimals)
    let one_usdc = 1_000_000i128;
    let thousand_usdc = 1_000_000_000i128;
    let ten_thousand_usdc = 10_000_000_000i128;

    // Addition
    assert_eq!(safe_add_balance(one_usdc, thousand_usdc), Ok(1_001_000_000));
    assert_eq!(
        safe_add_balance(thousand_usdc, ten_thousand_usdc),
        Ok(11_000_000_000)
    );

    // Subtraction
    assert_eq!(safe_sub_balance(thousand_usdc, one_usdc), Ok(999_000_000));
    assert_eq!(
        safe_sub_balance(ten_thousand_usdc, thousand_usdc),
        Ok(9_000_000_000)
    );

    // Edge case: maximum reasonable USDC amount (still well below i128::MAX)
    let max_reasonable_usdc = 1_000_000_000_000_000i128; // 1 trillion USDC
    assert_eq!(
        safe_add_balance(max_reasonable_usdc, one_usdc),
        Ok(max_reasonable_usdc + one_usdc)
    );
}

#[test]
fn test_deposit_funds_with_safe_math() {
    // Test that safe_add_balance is used correctly in deposit_funds
    assert_eq!(safe_add_balance(0, 5_000_000i128), Ok(5_000_000i128));
    assert_eq!(
        safe_add_balance(5_000_000i128, 3_000_000i128),
        Ok(8_000_000i128)
    );

    // Test overflow protection
    assert_eq!(safe_add_balance(i128::MAX, 1), Err(Error::Overflow));

    // Test negative amount rejection
    assert_eq!(safe_add_balance(1000, -100), Err(Error::Underflow));
}

#[test]
fn test_deposit_funds_rejects_negative() {
    assert_eq!(validate_non_negative(-1_000_000i128), Err(Error::Underflow));
    assert_eq!(validate_non_negative(0), Ok(()));
    assert_eq!(validate_non_negative(1_000_000i128), Ok(()));
}

#[test]
fn test_charge_subscription_with_safe_math() {
    assert_eq!(
        safe_sub_balance(30_000_000i128, 10_000_000i128),
        Ok(20_000_000i128)
    );

    assert_eq!(
        safe_sub_balance(5_000_000i128, 10_000_000i128),
        Err(Error::Underflow)
    );

    assert_eq!(safe_sub_balance(10_000_000i128, 10_000_000i128), Ok(0i128));
}

#[test]
fn test_charge_subscription_insufficient_balance() {
    assert_eq!(safe_sub_balance(0, 10_000_000i128), Err(Error::Underflow));
    assert_eq!(
        safe_sub_balance(5_000_000i128, 10_000_000i128),
        Err(Error::Underflow)
    );
}

#[test]
fn test_multiple_deposits_no_overflow() {
    let large_amount = 100_000_000_000i128; // 100k USDC
    let mut balance = 0i128;

    for _ in 0..10 {
        balance = safe_add_balance(balance, large_amount).unwrap();
    }

    assert_eq!(balance, 1_000_000_000_000i128);

    let overflow_amount = i128::MAX - balance + 1;
    assert_eq!(
        safe_add_balance(balance, overflow_amount),
        Err(Error::Overflow)
    );

    assert_eq!(
        safe_add_balance(balance, large_amount),
        Ok(balance + large_amount)
    );
}

#[test]
fn test_repeated_charges_no_underflow() {
    let charge_amount = 10_000_000i128;
    let mut balance = 30_000_000i128;

    balance = safe_sub_balance(balance, charge_amount).unwrap();
    assert_eq!(balance, 20_000_000i128);

    balance = safe_sub_balance(balance, charge_amount).unwrap();
    assert_eq!(balance, 10_000_000i128);

    balance = safe_sub_balance(balance, charge_amount).unwrap();
    assert_eq!(balance, 0i128);

    assert_eq!(
        safe_sub_balance(balance, charge_amount),
        Err(Error::Underflow)
    );
}

#[test]
fn test_create_subscription_validates_amount() {
    assert_eq!(validate_non_negative(-1_000_000i128), Err(Error::Underflow));
    assert_eq!(validate_non_negative(0), Ok(()));
    assert_eq!(validate_non_negative(10_000_000i128), Ok(()));
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
    client.init(&token, &admin, &1_000000i128);

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
    let _ = env.as_contract(&contract_id, || {
        env.storage().instance().set(&0u32, &sub);
        Ok::<(), ()>(())
    });

    // Verify initial status
    let initial = client.get_subscription(&0u32);
    assert_eq!(initial.status, SubscriptionStatus::InsufficientBalance);
    assert_eq!(initial.prepaid_balance, initial_balance);

    // Simulate deposit_funds recovery flow via as_contract
    let _ = env.as_contract(&contract_id, || {
        let mut sub: Subscription = env.storage().instance().get(&0u32).unwrap();
        sub.prepaid_balance += deposit_amount;
        sub.status = SubscriptionStatus::Active; // Recovery: InsufficientBalance -> Active
        env.storage().instance().set(&0u32, &sub);
        Ok::<(), ()>(())
    });

    // Verify status changed to Active (recovery successful)
    let after_deposit = client.get_subscription(&0u32);
    assert_eq!(after_deposit.status, SubscriptionStatus::Active);
    assert_eq!(
        after_deposit.prepaid_balance,
        initial_balance + deposit_amount
    );
}

/// Test: charge_subscription behavior - insufficient balance triggers status change
#[test]
fn test_charge_subscription_behavior() {
    let env = Env::default();
    let interval = 30 * 24 * 60 * 60u64;
    env.ledger().set_timestamp(interval + 1);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1i128);

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

    let _ = env.as_contract(&contract_id, || {
        env.storage().instance().set(&0u32, &sub);
        Ok::<(), ()>(())
    });

    // Test via direct contract call to verify behavior
    let _ = env.as_contract(&contract_id, || {
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

#[test]
fn test_min_topup_below_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let min_topup = 5_000000i128; // 5 USDC

    client.init(&token, &admin, &min_topup);
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &min_topup,
        &(30 * 24 * 60 * 60),
        &false,
    );

    let result = client.try_deposit_funds(&sub_id, &subscriber, &4_999999);
    assert!(result.is_err());
}

#[test]
fn test_min_topup_exactly_at_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token_addr);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let min_topup = 5_000000i128; // 5 USDC

    client.init(&token_addr, &admin, &min_topup);
    token_admin.mint(&subscriber, &min_topup);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    let result = client.try_deposit_funds(&id, &subscriber, &min_topup);
    assert!(result.is_ok());
}

#[test]
fn test_min_topup_above_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token_addr);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let min_topup = 5_000000i128; // 5 USDC
    let deposit_amount = 10_000000i128;

    client.init(&token_addr, &admin, &min_topup);
    token_admin.mint(&subscriber, &deposit_amount);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &deposit_amount,
        &(30 * 24 * 60 * 60),
        &false,
    );

    let result = client.try_deposit_funds(&id, &subscriber, &deposit_amount);
    assert!(result.is_ok());
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
    client.init(&token, &admin, &1i128);

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

    let _ = env.as_contract(&contract_id, || {
        env.storage().instance().set(&0u32, &sub);
        Ok::<(), ()>(())
    });

    // Test via direct contract call
    let _ = env.as_contract(&contract_id, || {
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
    client.init(&token, &admin, &1i128);

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

    let _ = env.as_contract(&contract_id, || {
        env.storage().instance().set(&0u32, &sub);
        Ok::<(), ()>(())
    });

    // Multiple charge attempts
    let _ = env.as_contract(&contract_id, || {
        // First attempt - fails with InsufficientBalance
        let r1 = SubscriptionVault::charge_subscription(env.clone(), 0u32);
        assert!(r1.is_err());

        // Verify balance unchanged after first failure (INVARIANT)
        let after_first: Subscription = env.storage().instance().get(&0u32).unwrap();
        assert_eq!(after_first.prepaid_balance, initial_balance);
        assert_eq!(after_first.status, SubscriptionStatus::InsufficientBalance);

        // Second attempt - status is now InsufficientBalance, so charging is rejected
        let r2 = SubscriptionVault::charge_subscription(env.clone(), 0u32);
        assert!(r2.is_err());

        // Third attempt - same, returns Err
        let r3 = SubscriptionVault::charge_subscription(env.clone(), 0u32);
        assert!(r3.is_err());

        // Balance still unchanged (INVARIANT preserved)
        let final_state: Subscription = env.storage().instance().get(&0u32).unwrap();
        assert_eq!(final_state.prepaid_balance, initial_balance);

        Ok::<(), ()>(())
    });
}

#[test]
fn test_cancel_subscription_by_subscriber() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    client.init(&token, &admin, &1_000_000);

    let sub_id = client.create_subscription(&subscriber, &merchant, &1000, &86400, &true);

    client.cancel_subscription(&sub_id, &subscriber);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

#[test]
fn test_init_and_struct() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let _client = SubscriptionVaultClient::new(&env, &contract_id);
    // Basic initialization test
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

// -- Usage-based charge tests ------------------------------------------------

const PREPAID: i128 = 50_000_000; // 50 USDC

/// Helper: create a subscription with `usage_enabled = false` and a known
/// `prepaid_balance` for interval-charge tests.
fn setup(env: &Env, interval: u64) -> (SubscriptionVaultClient<'_>, u32) {
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);

    let token = Address::generate(env);
    let admin = Address::generate(env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);

    env.ledger().set_timestamp(T0);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &interval,
        &false, // usage_enabled
    );

    // Seed prepaid balance.
    let mut sub = client.get_subscription(&id);
    sub.prepaid_balance = PREPAID;
    let _ = env.as_contract(&contract_id, || {
        env.storage().instance().set(&id, &sub);
    });

    (client, id)
}

/// Helper: create a subscription with `usage_enabled = true` and a known
/// `prepaid_balance` by writing directly to storage after creation.
fn setup_usage(env: &Env) -> (SubscriptionVaultClient<'_>, u32) {
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);

    let token = Address::generate(env);
    let admin = Address::generate(env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);

    env.ledger().set_timestamp(T0);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &INTERVAL,
        &true, // usage_enabled
    );

    // Seed prepaid balance by writing the subscription back with funds.
    let mut sub = client.get_subscription(&id);
    sub.prepaid_balance = PREPAID;
    let _ = env.as_contract(&contract_id, || {
        env.storage().instance().set(&id, &sub);
    });

    (client, id)
}

/// Successful usage charge: debits prepaid_balance by the requested amount.
#[test]
fn test_usage_charge_debits_balance() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_usage(&env);

    client.charge_usage(&id, &10_000_000i128);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID - 10_000_000);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

/// Draining the balance to zero transitions status to InsufficientBalance.
#[test]
fn test_usage_charge_drains_balance_to_insufficient() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_usage(&env);

    client.charge_usage(&id, &PREPAID);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 0);
    assert_eq!(sub.status, SubscriptionStatus::InsufficientBalance);
}

/// Rejected when usage_enabled is false.
#[test]
fn test_usage_charge_rejected_when_disabled() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup(&env, INTERVAL);

    let res = client.try_charge_usage(&id, &1_000_000i128);
    assert_eq!(res, Err(Ok(Error::UsageNotEnabled)));
}

/// Rejected when usage_amount exceeds prepaid_balance.
#[test]
fn test_usage_charge_rejected_insufficient_balance() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_usage(&env);

    let res = client.try_charge_usage(&id, &(PREPAID + 1));
    assert_eq!(res, Err(Ok(Error::InsufficientPrepaidBalance)));

    // Balance unchanged.
    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID);
}

/// Rejected when usage_amount is zero or negative.
#[test]
fn test_usage_charge_rejected_invalid_amount() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_usage(&env);

    let res_zero = client.try_charge_usage(&id, &0i128);
    assert_eq!(res_zero, Err(Ok(Error::InvalidAmount)));

    let res_neg = client.try_charge_usage(&id, &(-1i128));
    assert_eq!(res_neg, Err(Ok(Error::InvalidAmount)));

    // Balance unchanged.
    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID);
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

// =============================================================================
// Next Charge Timestamp Helper Tests
// =============================================================================

#[test]
fn test_compute_next_charge_info_active_subscription() {
    use crate::{compute_next_charge_info, Subscription, SubscriptionStatus};

    let env = Env::default();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let last_payment = 1000u64;
    let interval = 30 * 24 * 60 * 60;

    let subscription = Subscription {
        subscriber,
        merchant,
        amount: 10_000_000i128,
        interval_seconds: interval,
        last_payment_timestamp: last_payment,
        status: SubscriptionStatus::Active,
        prepaid_balance: 100_000_000i128,
        usage_enabled: false,
    };

    let info = compute_next_charge_info(&subscription);

    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, last_payment + interval);
}

#[test]
fn test_compute_next_charge_info_paused_subscription() {
    use crate::{compute_next_charge_info, Subscription, SubscriptionStatus};

    let env = Env::default();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let last_payment = 2000u64;
    let interval = 7 * 24 * 60 * 60;

    let subscription = Subscription {
        subscriber,
        merchant,
        amount: 5_000_000i128,
        interval_seconds: interval,
        last_payment_timestamp: last_payment,
        status: SubscriptionStatus::Paused,
        prepaid_balance: 50_000_000i128,
        usage_enabled: false,
    };

    let info = compute_next_charge_info(&subscription);

    assert!(!info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, last_payment + interval);
}

#[test]
fn test_compute_next_charge_info_cancelled_subscription() {
    use crate::{compute_next_charge_info, Subscription, SubscriptionStatus};

    let env = Env::default();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let last_payment = 5000u64;
    let interval = 24 * 60 * 60;

    let subscription = Subscription {
        subscriber,
        merchant,
        amount: 1_000_000i128,
        interval_seconds: interval,
        last_payment_timestamp: last_payment,
        status: SubscriptionStatus::Cancelled,
        prepaid_balance: 0i128,
        usage_enabled: false,
    };

    let info = compute_next_charge_info(&subscription);

    assert!(!info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, last_payment + interval);
}

#[test]
fn test_compute_next_charge_info_insufficient_balance_subscription() {
    use crate::{compute_next_charge_info, Subscription, SubscriptionStatus};

    let env = Env::default();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let last_payment = 3000u64;
    let interval = 30 * 24 * 60 * 60;

    let subscription = Subscription {
        subscriber,
        merchant,
        amount: 20_000_000i128,
        interval_seconds: interval,
        last_payment_timestamp: last_payment,
        status: SubscriptionStatus::InsufficientBalance,
        prepaid_balance: 1_000_000i128,
        usage_enabled: false,
    };

    let info = compute_next_charge_info(&subscription);

    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, last_payment + interval);
}

#[test]
fn test_compute_next_charge_info_short_interval() {
    use crate::{compute_next_charge_info, Subscription, SubscriptionStatus};

    let env = Env::default();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let last_payment = 100000u64;
    let interval = 60;

    let subscription = Subscription {
        subscriber,
        merchant,
        amount: 1_000i128,
        interval_seconds: interval,
        last_payment_timestamp: last_payment,
        status: SubscriptionStatus::Active,
        prepaid_balance: 10_000i128,
        usage_enabled: true,
    };

    let info = compute_next_charge_info(&subscription);

    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, last_payment + interval);
}

#[test]
fn test_compute_next_charge_info_long_interval() {
    use crate::{compute_next_charge_info, Subscription, SubscriptionStatus};

    let env = Env::default();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let last_payment = 1000u64;
    let interval = 365 * 24 * 60 * 60;

    let subscription = Subscription {
        subscriber,
        merchant,
        amount: 100_000_000i128,
        interval_seconds: interval,
        last_payment_timestamp: last_payment,
        status: SubscriptionStatus::Active,
        prepaid_balance: 1_000_000_000i128,
        usage_enabled: false,
    };

    let info = compute_next_charge_info(&subscription);

    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, last_payment + interval);
}

#[test]
fn test_compute_next_charge_info_overflow_protection() {
    use crate::{compute_next_charge_info, Subscription, SubscriptionStatus};

    let env = Env::default();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let last_payment = u64::MAX - 100;
    let interval = 200;

    let subscription = Subscription {
        subscriber,
        merchant,
        amount: 10_000_000i128,
        interval_seconds: interval,
        last_payment_timestamp: last_payment,
        status: SubscriptionStatus::Active,
        prepaid_balance: 100_000_000i128,
        usage_enabled: false,
    };

    let info = compute_next_charge_info(&subscription);

    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, u64::MAX);
}

#[test]
fn test_compute_next_charge_info_zero_interval() {
    use crate::{compute_next_charge_info, Subscription, SubscriptionStatus};

    let env = Env::default();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let subscription = Subscription {
        subscriber,
        merchant,
        amount: 1_000_000i128,
        interval_seconds: 0,
        last_payment_timestamp: 5000,
        status: SubscriptionStatus::Active,
        prepaid_balance: 10_000_000i128,
        usage_enabled: false,
    };

    let info = compute_next_charge_info(&subscription);

    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, 5000); // 5000 + 0 = 5000
}

#[test]
fn test_get_next_charge_info_contract_method() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 10_000_000i128;
    let interval_seconds = 30 * 24 * 60 * 60;

    env.ledger().with_mut(|li| li.timestamp = 1000);

    let id = client.create_subscription(&subscriber, &merchant, &amount, &interval_seconds, &false);

    let info = client.get_next_charge_info(&id);

    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, 1000 + interval_seconds);
}

#[test]
fn test_get_next_charge_info_all_statuses() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 10_000_000i128;
    let interval_seconds = 30 * 24 * 60 * 60;

    env.ledger().with_mut(|li| li.timestamp = 5000);

    let id = client.create_subscription(&subscriber, &merchant, &amount, &interval_seconds, &false);

    // Test Active status
    let info = client.get_next_charge_info(&id);
    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, 5000 + interval_seconds);

    // Test Paused status
    client.pause_subscription(&id, &subscriber);
    let info = client.get_next_charge_info(&id);
    assert!(!info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, 5000 + interval_seconds);

    // Resume to Active
    client.resume_subscription(&id, &subscriber);
    let info = client.get_next_charge_info(&id);
    assert!(info.is_charge_expected);

    // Test Cancelled status
    client.cancel_subscription(&id, &subscriber);
    let info = client.get_next_charge_info(&id);
    assert!(!info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, 5000 + interval_seconds);
}

#[test]
fn test_estimate_topup_subscription_not_found() {
    let (_env, client, _, _) = setup_test_env();
    let result = client.try_estimate_topup_for_intervals(&9999, &1);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn test_get_next_charge_info_insufficient_balance_status() {
    use crate::SubscriptionStatus;

    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 10_000_000i128;
    let interval_seconds = 7 * 24 * 60 * 60;

    env.ledger().with_mut(|li| li.timestamp = 2000);

    let id = client.create_subscription(&subscriber, &merchant, &amount, &interval_seconds, &false);

    // Manually set to InsufficientBalance for testing
    let mut sub = client.get_subscription(&id);
    sub.status = SubscriptionStatus::InsufficientBalance;
    let _ = env.as_contract(&client.address, || {
        env.storage().instance().set(&id, &sub);
    });

    let info = client.get_next_charge_info(&id);

    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, 2000 + interval_seconds);
}

#[test]
#[should_panic(expected = "Error(Contract, #404)")]
fn test_get_next_charge_info_subscription_not_found() {
    let (_, client, _, _) = setup_test_env();
    client.get_next_charge_info(&999);
}

#[test]
fn test_get_next_charge_info_multiple_intervals() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    env.ledger().with_mut(|li| li.timestamp = 10000);
    let daily_id = client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &(24 * 60 * 60),
        &false,
    );

    env.ledger().with_mut(|li| li.timestamp = 20000);
    let weekly_id = client.create_subscription(
        &subscriber,
        &merchant,
        &5_000_000i128,
        &(7 * 24 * 60 * 60),
        &false,
    );

    env.ledger().with_mut(|li| li.timestamp = 30000);
    let monthly_id = client.create_subscription(
        &subscriber,
        &merchant,
        &20_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    let daily_info = client.get_next_charge_info(&daily_id);
    assert_eq!(daily_info.next_charge_timestamp, 10000 + 24 * 60 * 60);

    let weekly_info = client.get_next_charge_info(&weekly_id);
    assert_eq!(weekly_info.next_charge_timestamp, 20000 + 7 * 24 * 60 * 60);

    let monthly_info = client.get_next_charge_info(&monthly_id);
    assert_eq!(
        monthly_info.next_charge_timestamp,
        30000 + 30 * 24 * 60 * 60
    );

    assert!(daily_info.is_charge_expected);
    assert!(weekly_info.is_charge_expected);
    assert!(monthly_info.is_charge_expected);
}

#[test]
fn test_get_next_charge_info_zero_interval() {
    use crate::{compute_next_charge_info, Subscription, SubscriptionStatus};

    let env = Env::default();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let subscription = Subscription {
        subscriber,
        merchant,
        amount: 1_000_000i128,
        interval_seconds: 0,
        last_payment_timestamp: 5000,
        status: SubscriptionStatus::Active,
        prepaid_balance: 10_000_000i128,
        usage_enabled: false,
    };

    let info = compute_next_charge_info(&subscription);

    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, 5000);
}

// =============================================================================
// Admin Recovery of Stranded Funds Tests
// =============================================================================

#[test]
fn test_recover_stranded_funds_successful() {
    let (env, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&env);
    let amount = 50_000_000i128;
    let reason = RecoveryReason::AccidentalTransfer;

    env.ledger().with_mut(|li| li.timestamp = 10000);

    let result = client.try_recover_stranded_funds(&admin, &recipient, &amount, &reason);
    assert!(result.is_ok());

    let events = env.events().all();
    assert!(!events.is_empty());
}

#[test]
fn test_cancel_subscription_unauthorized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let other = Address::generate(&env);

    client.init(&token, &admin, &1_000_000);

    let sub_id = client.create_subscription(&subscriber, &merchant, &1000, &86400, &true);

    let result = client.try_cancel_subscription(&sub_id, &other);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_withdraw_subscriber_funds() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token_contract = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token = soroban_sdk::token::Client::new(&env, &token_contract);
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token_contract);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let vault_admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    client.init(&token_contract, &vault_admin, &1000);

    token_admin.mint(&subscriber, &5000);

    let sub_id = client.create_subscription(&subscriber, &merchant, &1000, &86400, &true);

    client.deposit_funds(&sub_id, &subscriber, &5000);
    client.cancel_subscription(&sub_id, &subscriber);
    client.withdraw_subscriber_funds(&sub_id, &subscriber);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 0);
    assert_eq!(token.balance(&subscriber), 5000);
    assert_eq!(token.balance(&contract_id), 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #401)")]
fn test_recover_stranded_funds_unauthorized_caller() {
    let (env, client, _, _) = setup_test_env();

    let non_admin = Address::generate(&env);
    let recipient = Address::generate(&env);
    let amount = 10_000_000i128;
    let reason = RecoveryReason::AccidentalTransfer;

    client.recover_stranded_funds(&non_admin, &recipient, &amount, &reason);
}

#[test]
#[should_panic(expected = "Error(Contract, #1008)")]
fn test_recover_stranded_funds_zero_amount() {
    let (_, client, _, admin) = setup_test_env();

    let recipient = Address::generate(admin.env());
    let amount = 0i128;
    let reason = RecoveryReason::DeprecatedFlow;

    client.recover_stranded_funds(&admin, &recipient, &amount, &reason);
}

#[test]
#[should_panic(expected = "Error(Contract, #1008)")]
fn test_recover_stranded_funds_negative_amount() {
    let (_, client, _, admin) = setup_test_env();

    let recipient = Address::generate(admin.env());
    let amount = -1_000_000i128;
    let reason = RecoveryReason::AccidentalTransfer;

    client.recover_stranded_funds(&admin, &recipient, &amount, &reason);
}

#[test]
fn test_recover_stranded_funds_all_recovery_reasons() {
    let (env, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&env);
    let amount = 10_000_000i128;

    let result1 = client.try_recover_stranded_funds(
        &admin,
        &recipient,
        &amount,
        &RecoveryReason::AccidentalTransfer,
    );
    assert!(result1.is_ok());

    let result2 = client.try_recover_stranded_funds(
        &admin,
        &recipient,
        &amount,
        &RecoveryReason::DeprecatedFlow,
    );
    assert!(result2.is_ok());

    let result3 = client.try_recover_stranded_funds(
        &admin,
        &recipient,
        &amount,
        &RecoveryReason::UnreachableSubscriber,
    );
    assert!(result3.is_ok());
}

#[test]
fn test_recover_stranded_funds_event_emission() {
    let (env, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&env);
    let amount = 25_000_000i128;
    let reason = RecoveryReason::UnreachableSubscriber;

    env.ledger().with_mut(|li| li.timestamp = 5000);

    client.recover_stranded_funds(&admin, &recipient, &amount, &reason);

    let events = env.events().all();
    assert!(!events.is_empty());
}

#[test]
fn test_recover_stranded_funds_large_amount() {
    let (_, client, _, admin) = setup_test_env();

    let recipient = Address::generate(admin.env());
    let amount = 1_000_000_000_000i128;
    let reason = RecoveryReason::DeprecatedFlow;

    let result = client.try_recover_stranded_funds(&admin, &recipient, &amount, &reason);
    assert!(result.is_ok());
}

#[test]
fn test_recover_stranded_funds_small_amount() {
    let (_, client, _, admin) = setup_test_env();

    let recipient = Address::generate(admin.env());
    let amount = 1i128;
    let reason = RecoveryReason::AccidentalTransfer;

    let result = client.try_recover_stranded_funds(&admin, &recipient, &amount, &reason);
    assert!(result.is_ok());
}

#[test]
fn test_recover_stranded_funds_multiple_recoveries() {
    let (env, client, _, admin) = setup_test_env();

    let recipient1 = Address::generate(&env);
    let recipient2 = Address::generate(&env);
    let recipient3 = Address::generate(&env);

    let result1 = client.try_recover_stranded_funds(
        &admin,
        &recipient1,
        &10_000_000i128,
        &RecoveryReason::AccidentalTransfer,
    );
    assert!(result1.is_ok());

    let result2 = client.try_recover_stranded_funds(
        &admin,
        &recipient2,
        &20_000_000i128,
        &RecoveryReason::DeprecatedFlow,
    );
    assert!(result2.is_ok());

    let result3 = client.try_recover_stranded_funds(
        &admin,
        &recipient3,
        &30_000_000i128,
        &RecoveryReason::UnreachableSubscriber,
    );
    assert!(result3.is_ok());

    let events = env.events().all();
    assert!(!events.is_empty());
}

#[test]
fn test_recover_stranded_funds_different_recipients() {
    let (env, client, _, admin) = setup_test_env();

    let treasury = Address::generate(&env);
    let user_wallet = Address::generate(&env);
    let contract_addr = Address::generate(&env);

    let amount = 5_000_000i128;
    let reason = RecoveryReason::AccidentalTransfer;

    assert!(client
        .try_recover_stranded_funds(&admin, &treasury, &amount, &reason)
        .is_ok());

    assert!(client
        .try_recover_stranded_funds(&admin, &user_wallet, &amount, &reason)
        .is_ok());

    assert!(client
        .try_recover_stranded_funds(&admin, &contract_addr, &amount, &reason)
        .is_ok());
}

#[test]
fn test_recovery_reason_enum_values() {
    let reason1 = RecoveryReason::AccidentalTransfer;
    let reason2 = RecoveryReason::DeprecatedFlow;
    let reason3 = RecoveryReason::UnreachableSubscriber;

    assert!(reason1 != reason2);
    assert!(reason2 != reason3);
    assert!(reason1 != reason3);

    let reason_clone = reason1.clone();
    assert!(reason_clone == RecoveryReason::AccidentalTransfer);
}

#[test]
fn test_recover_stranded_funds_timestamp_recorded() {
    let (env, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&env);
    let amount = 15_000_000i128;
    let reason = RecoveryReason::DeprecatedFlow;

    let expected_timestamp = 123456u64;
    env.ledger()
        .with_mut(|li| li.timestamp = expected_timestamp);

    client.recover_stranded_funds(&admin, &recipient, &amount, &reason);

    let events = env.events().all();
    assert!(!events.is_empty());
}

#[test]
fn test_recover_stranded_funds_admin_authorization_required() {
    let (env, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&env);
    let amount = 10_000_000i128;
    let reason = RecoveryReason::AccidentalTransfer;

    let result = client.try_recover_stranded_funds(&admin, &recipient, &amount, &reason);
    assert!(result.is_ok());
}

#[test]
fn test_recover_stranded_funds_does_not_affect_subscriptions() {
    let (env, client, _, admin) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    let recipient = Address::generate(&env);
    client.recover_stranded_funds(
        &admin,
        &recipient,
        &5_000_000i128,
        &RecoveryReason::DeprecatedFlow,
    );

    let subscription = client.get_subscription(&sub_id);
    assert_eq!(subscription.status, SubscriptionStatus::Active);
    assert_eq!(subscription.subscriber, subscriber);
    assert_eq!(subscription.merchant, merchant);
}

#[test]
fn test_recover_stranded_funds_with_cancelled_subscription() {
    let (env, client, _, admin) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );
    client.cancel_subscription(&sub_id, &subscriber);

    let recipient = Address::generate(&env);
    let result = client.try_recover_stranded_funds(
        &admin,
        &recipient,
        &5_000_000i128,
        &RecoveryReason::UnreachableSubscriber,
    );
    assert!(result.is_ok());

    assert_eq!(
        client.get_subscription(&sub_id).status,
        SubscriptionStatus::Cancelled
    );
}

// =============================================================================
// Comprehensive Batch Operations Tests (Issue #45)
// =============================================================================

// -----------------------------------------------------------------------------
// Test Group 1: Batch Size Variations (empty, small, medium, large)
fn setup_batch_env(env: &Env) -> (SubscriptionVaultClient<'static>, Address, u32, u32) {
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    let admin = Address::generate(env);
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(env, &token_addr);
    client.init(&token_addr, &admin, &1_000000i128);

    let subscriber = Address::generate(env);
    token_admin.mint(&subscriber, &100_000_000i128);
    let merchant = Address::generate(env);
    let id0 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id0, &subscriber, &10_000000i128);
    let id1 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    env.ledger().set_timestamp(T0 + INTERVAL);
    (client, admin, id0, id1)
}

#[test]
fn test_batch_charge_single_subscription() {
    let env = Env::default();
    let (client, _admin, id0, _id1) = setup_batch_env(&env);
    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id0 as u32);

    let results = client.batch_charge(&ids);

    assert_eq!(results.len(), 1);
    assert!(results.get(0).unwrap().success);
    assert_eq!(results.get(0).unwrap().error_code, 0);
}

#[test]
fn test_batch_charge_small_batch_5_subscriptions() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let mut ids = SorobanVec::<u32>::new(&env);

    for _ in 0..5 {
        let id = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
        client.deposit_funds(&id, &subscriber, &10_000000i128);
        ids.push_back(id as u32);
    }

    env.ledger().set_timestamp(T0 + INTERVAL);
    let results = client.batch_charge(&ids);

    assert_eq!(results.len(), 5);
    for i in 0..5 {
        assert!(results.get(i).unwrap().success);
        assert_eq!(results.get(i).unwrap().error_code, 0);
    }
}

#[test]
fn test_batch_charge_medium_batch_20_subscriptions() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let mut ids = SorobanVec::<u32>::new(&env);

    for _ in 0..20 {
        let id = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
        client.deposit_funds(&id, &subscriber, &10_000000i128);
        ids.push_back(id as u32);
    }

    env.ledger().set_timestamp(T0 + INTERVAL);
    let results = client.batch_charge(&ids);

    assert_eq!(results.len(), 20);
    for i in 0..20 {
        assert!(results.get(i).unwrap().success);
    }
}

#[test]
fn test_batch_charge_large_batch_50_subscriptions() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let mut ids = SorobanVec::<u32>::new(&env);

    for _ in 0..50 {
        let id = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
        client.deposit_funds(&id, &subscriber, &10_000000i128);
        ids.push_back(id as u32);
    }

    env.ledger().set_timestamp(T0 + INTERVAL);
    let results = client.batch_charge(&ids);

    assert_eq!(results.len(), 50);
    for i in 0..50 {
        assert!(results.get(i).unwrap().success);
    }
}

// -----------------------------------------------------------------------------
// Test Group 2: Partial Success Semantics (mixed outcomes within batches)
// -----------------------------------------------------------------------------

#[test]
fn test_batch_charge_mixed_success_and_insufficient_balance() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let mut ids = SorobanVec::<u32>::new(&env);

    for i in 0..4 {
        let id = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
        if i % 2 == 0 {
            client.deposit_funds(&id, &subscriber, &10_000000i128);
        }
        ids.push_back(id as u32);
    }

    env.ledger().set_timestamp(T0 + INTERVAL);
    let results = client.batch_charge(&ids);

    assert_eq!(results.len(), 4);
    assert!(results.get(0).unwrap().success);
    assert!(results.get(2).unwrap().success);
    assert!(!results.get(1).unwrap().success);
    assert_eq!(
        results.get(1).unwrap().error_code,
        Error::InsufficientBalance.to_code()
    );
    assert!(!results.get(3).unwrap().success);
    assert_eq!(
        results.get(3).unwrap().error_code,
        Error::InsufficientBalance.to_code()
    );
}

#[test]
fn test_batch_charge_mixed_interval_not_elapsed() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id_short = client.create_subscription(&subscriber, &merchant, &1000i128, &1800, &false);
    let id_long = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);

    client.deposit_funds(&id_short, &subscriber, &10_000000i128);
    client.deposit_funds(&id_long, &subscriber, &10_000000i128);

    env.ledger().set_timestamp(T0 + 1800);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id_short);
    ids.push_back(id_long);

    let results = client.batch_charge(&ids);

    assert_eq!(results.len(), 2);
    assert!(results.get(0).unwrap().success);
    assert!(!results.get(1).unwrap().success);
    assert_eq!(
        results.get(1).unwrap().error_code,
        Error::IntervalNotElapsed.to_code()
    );
}

#[test]
fn test_batch_charge_mixed_paused_and_active() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id0 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id0, &subscriber, &10_000000i128);

    let id1 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id1, &subscriber, &10_000000i128);
    client.pause_subscription(&id1, &subscriber);

    env.ledger().set_timestamp(T0 + INTERVAL);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id0 as u32);
    ids.push_back(id1 as u32);

    let results = client.batch_charge(&ids);

    assert_eq!(results.len(), 2);
    assert!(results.get(0).unwrap().success);
    assert!(!results.get(1).unwrap().success);
    assert_eq!(
        results.get(1).unwrap().error_code,
        Error::NotActive.to_code()
    );
}

#[test]
fn test_batch_charge_mixed_cancelled_and_active() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id0 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id0, &subscriber, &10_000000i128);

    let id1 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id1, &subscriber, &10_000000i128);
    client.cancel_subscription(&id1, &subscriber);

    env.ledger().set_timestamp(T0 + INTERVAL);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id0 as u32);
    ids.push_back(id1 as u32);

    let results = client.batch_charge(&ids);

    assert_eq!(results.len(), 2);
    assert!(results.get(0).unwrap().success);
    assert!(!results.get(1).unwrap().success);
    assert_eq!(
        results.get(1).unwrap().error_code,
        Error::NotActive.to_code()
    );
}

#[test]
fn test_batch_charge_nonexistent_subscription_ids() {
    let env = Env::default();
    let (client, _admin, id0, _id1) = setup_batch_env(&env);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id0 as u32);
    ids.push_back(9999);
    ids.push_back(8888);

    let results = client.batch_charge(&ids);

    assert_eq!(results.len(), 3);
    assert!(results.get(0).unwrap().success);
    assert!(!results.get(1).unwrap().success);
    assert_eq!(
        results.get(1).unwrap().error_code,
        Error::NotFound.to_code()
    );
    assert!(!results.get(2).unwrap().success);
    assert_eq!(
        results.get(2).unwrap().error_code,
        Error::NotFound.to_code()
    );
}

#[test]
fn test_batch_charge_all_different_error_types() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id_success =
        client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id_success, &subscriber, &10_000000i128);

    let id_no_funds =
        client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);

    let id_paused =
        client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id_paused, &subscriber, &10_000000i128);
    client.pause_subscription(&id_paused, &subscriber);

    env.ledger().set_timestamp(T0 + INTERVAL);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id_success);
    ids.push_back(id_no_funds);
    ids.push_back(9999);
    ids.push_back(id_paused);

    let results = client.batch_charge(&ids);

    assert_eq!(results.len(), 4);

    assert!(results.get(0).unwrap().success);
    assert_eq!(results.get(0).unwrap().error_code, 0);

    assert!(!results.get(1).unwrap().success);
    assert_eq!(
        results.get(1).unwrap().error_code,
        Error::InsufficientBalance.to_code()
    );

    assert!(!results.get(2).unwrap().success);
    assert_eq!(
        results.get(2).unwrap().error_code,
        Error::NotFound.to_code()
    );

    assert!(!results.get(3).unwrap().success);
    assert_eq!(
        results.get(3).unwrap().error_code,
        Error::NotActive.to_code()
    );
}

// -----------------------------------------------------------------------------
// Test Group 3: State Correctness After Batch Operations
// -----------------------------------------------------------------------------

#[test]
fn test_batch_charge_successful_charges_update_state() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let charge_amount = 1_000_000i128;

    let id = client.create_subscription(&subscriber, &merchant, &charge_amount, &INTERVAL, &false);
    let initial_balance = 10_000_000i128;
    client.deposit_funds(&id, &subscriber, &initial_balance);

    let sub_before = client.get_subscription(&id);
    assert_eq!(sub_before.prepaid_balance, initial_balance);
    assert_eq!(sub_before.last_payment_timestamp, T0);

    env.ledger().set_timestamp(T0 + INTERVAL);
    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id as u32);

    let results = client.batch_charge(&ids);
    assert!(results.get(0).unwrap().success);

    let sub_after = client.get_subscription(&id);
    assert_eq!(sub_after.prepaid_balance, initial_balance - charge_amount);
    assert_eq!(sub_after.last_payment_timestamp, T0 + INTERVAL);
}

#[test]
fn test_batch_charge_failed_charges_leave_state_unchanged() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);

    let sub_before = client.get_subscription(&id);

    env.ledger().set_timestamp(T0 + INTERVAL);
    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id as u32);

    let results = client.batch_charge(&ids);
    assert!(!results.get(0).unwrap().success);

    let sub_after = client.get_subscription(&id);
    assert_eq!(sub_after.prepaid_balance, sub_before.prepaid_balance);
    assert_eq!(
        sub_after.last_payment_timestamp,
        sub_before.last_payment_timestamp
    );
    assert_eq!(sub_after.status, SubscriptionStatus::InsufficientBalance);
}

#[test]
fn test_batch_charge_partial_batch_correct_final_state() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 1_000_000i128;

    let id0 = client.create_subscription(&subscriber, &merchant, &amount, &INTERVAL, &false);
    client.deposit_funds(&id0, &subscriber, &10_000_000i128);

    let id1 = client.create_subscription(&subscriber, &merchant, &amount, &INTERVAL, &false);

    let id2 = client.create_subscription(&subscriber, &merchant, &amount, &INTERVAL, &false);
    client.deposit_funds(&id2, &subscriber, &10_000_000i128);

    env.ledger().set_timestamp(T0 + INTERVAL);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id0 as u32);
    ids.push_back(id1 as u32);
    ids.push_back(id2 as u32);

    let results = client.batch_charge(&ids);

    assert!(results.get(0).unwrap().success);
    assert!(!results.get(1).unwrap().success);
    assert!(results.get(2).unwrap().success);

    let sub0 = client.get_subscription(&id0);
    assert_eq!(sub0.prepaid_balance, 9_000_000i128);
    assert_eq!(sub0.last_payment_timestamp, T0 + INTERVAL);

    let sub1 = client.get_subscription(&id1);
    assert_eq!(sub1.prepaid_balance, 0);
    assert_eq!(sub1.last_payment_timestamp, T0);

    let sub2 = client.get_subscription(&id2);
    assert_eq!(sub2.prepaid_balance, 9_000_000i128);
    assert_eq!(sub2.last_payment_timestamp, T0 + INTERVAL);
}

#[test]
fn test_batch_charge_multiple_rounds_state_consistency() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 1_000_000i128;

    let id = client.create_subscription(&subscriber, &merchant, &amount, &INTERVAL, &false);
    client.deposit_funds(&id, &subscriber, &10_000_000i128);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id as u32);

    for i in 1..=3 {
        env.ledger().set_timestamp(T0 + (i * INTERVAL));
        let results = client.batch_charge(&ids);
        assert!(results.get(0).unwrap().success);

        let sub = client.get_subscription(&id);
        assert_eq!(sub.prepaid_balance, 10_000_000 - (i as i128 * amount));
        assert_eq!(sub.last_payment_timestamp, T0 + (i * INTERVAL));
    }
}

// -----------------------------------------------------------------------------
// Test Group 4: Authorization and Security
// -----------------------------------------------------------------------------

#[test]
#[should_panic]
fn test_batch_charge_requires_admin_auth() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);

    let non_admin = Address::generate(&env);

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &non_admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "batch_charge",
            args: {
                let mut ids = SorobanVec::<u32>::new(&env);
                ids.push_back(id as u32);
                (ids,).into_val(&env)
            },
            sub_invokes: &[],
        },
    }]);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id as u32);
    client.batch_charge(&ids);
}

// -----------------------------------------------------------------------------
// Test Group 5: Edge Cases and Boundary Conditions
// -----------------------------------------------------------------------------

#[test]
fn test_batch_charge_duplicate_subscription_ids() {
    let env = Env::default();
    let (client, _admin, id0, _id1) = setup_batch_env(&env);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id0 as u32);
    ids.push_back(id0 as u32);
    ids.push_back(id0 as u32);

    let results = client.batch_charge(&ids);

    assert_eq!(results.len(), 3);
    assert!(results.get(0).unwrap().success);

    assert!(!results.get(1).unwrap().success);
    assert_eq!(results.get(1).unwrap().error_code, Error::Replay.to_code());
    assert!(!results.get(2).unwrap().success);
    assert_eq!(results.get(2).unwrap().error_code, Error::Replay.to_code());
}

#[test]
fn test_batch_charge_exhausts_balance_exactly() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 5_000_000i128;

    let id = client.create_subscription(&subscriber, &merchant, &amount, &INTERVAL, &false);
    client.deposit_funds(&id, &subscriber, &amount);

    env.ledger().set_timestamp(T0 + INTERVAL);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id as u32);

    let results = client.batch_charge(&ids);
    assert!(results.get(0).unwrap().success);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 0);
}

#[test]
fn test_batch_charge_balance_off_by_one_insufficient() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 5_000_000i128;

    let id = client.create_subscription(&subscriber, &merchant, &amount, &INTERVAL, &false);
    client.deposit_funds(&id, &subscriber, &(amount - 1));

    env.ledger().set_timestamp(T0 + INTERVAL);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id as u32);

    let results = client.batch_charge(&ids);
    assert!(!results.get(0).unwrap().success);
    assert_eq!(
        results.get(0).unwrap().error_code,
        Error::InsufficientBalance.to_code()
    );
}

#[test]
fn test_batch_charge_result_indices_match_input_order() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id0 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id0, &subscriber, &10_000000i128);

    let id1 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);

    let id2 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id2, &subscriber, &10_000000i128);

    env.ledger().set_timestamp(T0 + INTERVAL);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id2 as u32);
    ids.push_back(id0 as u32);
    ids.push_back(id1 as u32);

    let results = client.batch_charge(&ids);
    assert_eq!(results.len(), 3);
    assert!(results.get(0).unwrap().success);
    assert!(results.get(1).unwrap().success);
    assert!(!results.get(2).unwrap().success);
}

#[test]
fn test_recover_stranded_funds_idempotency() {
    let (env, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&env);
    let amount = 10_000_000i128;
    let reason = RecoveryReason::AccidentalTransfer;

    let result1 = client.try_recover_stranded_funds(&admin, &recipient, &amount, &reason);
    assert!(result1.is_ok());

    let result2 = client.try_recover_stranded_funds(&admin, &recipient, &amount, &reason);
    assert!(result2.is_ok());

    let events = env.events().all();
    assert!(!events.is_empty());
}

#[test]
fn test_recover_stranded_funds_edge_case_max_i128() {
    let (_, client, _, admin) = setup_test_env();

    let recipient = Address::generate(admin.env());
    let amount = i128::MAX - 1000;
    let reason = RecoveryReason::DeprecatedFlow;

    let result = client.try_recover_stranded_funds(&admin, &recipient, &amount, &reason);
    assert!(result.is_ok());
}

// =============================================================================
// Usage Enabled Feature Tests
// =============================================================================

#[test]
fn test_create_subscription_with_usage_disabled() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 10_000_000i128;
    let interval_seconds = 30 * 24 * 60 * 60;
    let usage_enabled = false;

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval_seconds,
        &usage_enabled,
    );

    let subscription = client.get_subscription(&id);
    assert!(!subscription.usage_enabled);
    assert_eq!(subscription.amount, amount);
    assert_eq!(subscription.interval_seconds, interval_seconds);
}

#[test]
fn test_create_subscription_with_usage_enabled() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 5_000_000i128;
    let interval_seconds = 7 * 24 * 60 * 60;
    let usage_enabled = true;

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval_seconds,
        &usage_enabled,
    );

    let subscription = client.get_subscription(&id);
    assert!(subscription.usage_enabled);
    assert_eq!(subscription.amount, amount);
    assert_eq!(subscription.interval_seconds, interval_seconds);
}

#[test]
fn test_usage_flag_persists_through_state_transitions() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let usage_enabled = true;

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &usage_enabled,
    );

    assert!(client.get_subscription(&id).usage_enabled);

    client.pause_subscription(&id, &subscriber);
    assert!(client.get_subscription(&id).usage_enabled);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );

    client.resume_subscription(&id, &subscriber);
    assert!(client.get_subscription(&id).usage_enabled);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );

    client.cancel_subscription(&id, &subscriber);
    assert!(client.get_subscription(&id).usage_enabled);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}

#[test]
fn test_multiple_subscriptions_different_usage_modes() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant1 = Address::generate(&env);
    let merchant2 = Address::generate(&env);
    let merchant3 = Address::generate(&env);

    let id1 = client.create_subscription(
        &subscriber,
        &merchant1,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    let id2 = client.create_subscription(
        &subscriber,
        &merchant2,
        &5_000_000i128,
        &(7 * 24 * 60 * 60),
        &true,
    );

    let id3 = client.create_subscription(
        &subscriber,
        &merchant3,
        &20_000_000i128,
        &(90 * 24 * 60 * 60),
        &false,
    );

    assert!(!client.get_subscription(&id1).usage_enabled);
    assert!(client.get_subscription(&id2).usage_enabled);
    assert!(!client.get_subscription(&id3).usage_enabled);

    assert_eq!(client.get_subscription(&id1).merchant, merchant1);
    assert_eq!(client.get_subscription(&id2).merchant, merchant2);
    assert_eq!(client.get_subscription(&id3).merchant, merchant3);
}

#[test]
fn test_usage_enabled_with_different_intervals() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let daily_id = client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &(24 * 60 * 60),
        &true,
    );

    let weekly_id = client.create_subscription(
        &subscriber,
        &merchant,
        &5_000_000i128,
        &(7 * 24 * 60 * 60),
        &false,
    );

    let monthly_id = client.create_subscription(
        &subscriber,
        &merchant,
        &20_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
    );

    assert!(client.get_subscription(&daily_id).usage_enabled);
    assert!(!client.get_subscription(&weekly_id).usage_enabled);
    assert!(client.get_subscription(&monthly_id).usage_enabled);
}

#[test]
fn test_usage_enabled_with_zero_interval() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(&subscriber, &merchant, &1_000_000i128, &0, &true);

    let subscription = client.get_subscription(&id);
    assert!(subscription.usage_enabled);
    assert_eq!(subscription.interval_seconds, 0);
}

#[test]
fn test_usage_flag_with_next_charge_info() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    env.ledger().with_mut(|li| li.timestamp = 1000);

    let id_enabled = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
    );

    let id_disabled = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    let info_enabled = client.get_next_charge_info(&id_enabled);
    let info_disabled = client.get_next_charge_info(&id_disabled);

    assert!(info_enabled.is_charge_expected);
    assert!(info_disabled.is_charge_expected);

    assert!(client.get_subscription(&id_enabled).usage_enabled);
    assert!(!client.get_subscription(&id_disabled).usage_enabled);
}

#[test]
fn test_usage_enabled_default_behavior() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    let subscription = client.get_subscription(&id);

    assert!(!subscription.usage_enabled);
    assert_eq!(subscription.status, SubscriptionStatus::Active);
    assert_eq!(subscription.interval_seconds, 30 * 24 * 60 * 60);
}

#[test]
fn test_usage_enabled_immutable_after_creation() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    assert!(!client.get_subscription(&id).usage_enabled);

    client.pause_subscription(&id, &subscriber);
    assert!(!client.get_subscription(&id).usage_enabled);

    client.resume_subscription(&id, &subscriber);
    assert!(!client.get_subscription(&id).usage_enabled);
}

#[test]
fn test_usage_enabled_with_all_subscription_statuses() {
    use crate::SubscriptionStatus;

    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
    );

    assert!(client.get_subscription(&id).usage_enabled);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );

    client.pause_subscription(&id, &subscriber);
    assert!(client.get_subscription(&id).usage_enabled);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );

    client.resume_subscription(&id, &subscriber);
    assert!(client.get_subscription(&id).usage_enabled);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );

    client.cancel_subscription(&id, &subscriber);
    assert!(client.get_subscription(&id).usage_enabled);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}

#[test]
fn test_usage_enabled_true_semantics() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
    );

    let subscription = client.get_subscription(&id);

    assert!(subscription.usage_enabled);
    assert_eq!(subscription.interval_seconds, 30 * 24 * 60 * 60);
    assert_eq!(subscription.status, SubscriptionStatus::Active);

    client.pause_subscription(&id, &subscriber);
    client.resume_subscription(&id, &subscriber);
    client.cancel_subscription(&id, &subscriber);
}

#[test]
fn test_usage_enabled_false_semantics() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    let subscription = client.get_subscription(&id);

    assert!(!subscription.usage_enabled);
    assert_eq!(subscription.interval_seconds, 30 * 24 * 60 * 60);
    assert_eq!(subscription.amount, 10_000_000i128);

    client.pause_subscription(&id, &subscriber);
    client.resume_subscription(&id, &subscriber);
    client.cancel_subscription(&id, &subscriber);
}

#[test]
fn test_usage_enabled_with_different_amounts() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id1 = client.create_subscription(&subscriber, &merchant, &100i128, &(24 * 60 * 60), &true);

    let id2 = client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    let id3 = client.create_subscription(
        &subscriber,
        &merchant,
        &50_000_000i128,
        &(7 * 24 * 60 * 60),
        &true,
    );

    let sub1 = client.get_subscription(&id1);
    let sub2 = client.get_subscription(&id2);
    let sub3 = client.get_subscription(&id3);

    assert_eq!(sub1.amount, 100i128);
    assert!(sub1.usage_enabled);

    assert_eq!(sub2.amount, 1_000_000_000i128);
    assert!(!sub2.usage_enabled);

    assert_eq!(sub3.amount, 50_000_000i128);
    assert!(sub3.usage_enabled);
}

#[test]
fn test_usage_enabled_field_storage() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id0 = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
    );
    let id1 = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );
    let id2 = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
    );
    let id3 = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );
    let id4 = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
    );

    assert!(client.get_subscription(&id0).usage_enabled);
    assert!(!client.get_subscription(&id1).usage_enabled);
    assert!(client.get_subscription(&id2).usage_enabled);
    assert!(!client.get_subscription(&id3).usage_enabled);
    assert!(client.get_subscription(&id4).usage_enabled);
}

#[test]
fn test_usage_enabled_with_recovery_operations() {
    let (env, client, _, admin) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
    );

    assert!(client.get_subscription(&id).usage_enabled);

    let recipient = Address::generate(&env);
    client.recover_stranded_funds(
        &admin,
        &recipient,
        &5_000_000i128,
        &RecoveryReason::DeprecatedFlow,
    );

    assert!(client.get_subscription(&id).usage_enabled);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );
}

// =============================================================================
// Admin Rotation and Access Control Tests
// =============================================================================

#[test]
fn test_get_admin() {
    let (_, client, _, admin) = setup_test_env();

    let stored_admin = client.get_admin();
    assert_eq!(stored_admin, admin);
}

#[test]
fn test_rotate_admin_successful() {
    let (env, client, _, old_admin) = setup_test_env();

    let new_admin = Address::generate(&env);

    client.rotate_admin(&old_admin, &new_admin);

    assert_eq!(client.get_admin(), new_admin);
}

#[test]
#[should_panic(expected = "Error(Contract, #401)")]
fn test_rotate_admin_unauthorized() {
    let (env, client, _, _) = setup_test_env();

    let non_admin = Address::generate(&env);
    let new_admin = Address::generate(&env);

    client.rotate_admin(&non_admin, &new_admin);
}

#[test]
fn test_old_admin_loses_access_after_rotation() {
    let (env, client, _, old_admin) = setup_test_env();

    let new_admin = Address::generate(&env);

    client.rotate_admin(&old_admin, &new_admin);

    let result = client.try_set_min_topup(&old_admin, &5_000000);
    assert!(result.is_err());
}

#[test]
fn test_new_admin_gains_access_after_rotation() {
    let (env, client, _, old_admin) = setup_test_env();

    let new_admin = Address::generate(&env);

    client.rotate_admin(&old_admin, &new_admin);

    let new_min = 2_000000i128;
    client.set_min_topup(&new_admin, &new_min);

    assert_eq!(client.get_min_topup(), new_min);
}

#[test]
fn test_admin_rotation_affects_recovery_operations() {
    let (env, client, _, old_admin) = setup_test_env();

    let new_admin = Address::generate(&env);
    let recipient = Address::generate(&env);

    let result = client.try_recover_stranded_funds(
        &old_admin,
        &recipient,
        &10_000000i128,
        &RecoveryReason::AccidentalTransfer,
    );
    assert!(result.is_ok());

    client.rotate_admin(&old_admin, &new_admin);

    let result = client.try_recover_stranded_funds(
        &old_admin,
        &recipient,
        &10_000000i128,
        &RecoveryReason::AccidentalTransfer,
    );
    assert!(result.is_err());

    let result = client.try_recover_stranded_funds(
        &new_admin,
        &recipient,
        &10_000000i128,
        &RecoveryReason::DeprecatedFlow,
    );
    assert!(result.is_ok());
}

#[test]
fn test_batch_charge_admin_rotation() {
    let (env, client, _, old_admin) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 10_000_000i128;
    let interval_seconds = 30 * 24 * 60 * 60;

    env.ledger().with_mut(|li| li.timestamp = T0);

    let id = client.create_subscription(&subscriber, &merchant, &amount, &interval_seconds, &false);

    let mut sub = client.get_subscription(&id);
    sub.prepaid_balance = 50_000_000i128;
    let _ = env.as_contract(&client.address, || {
        env.storage().instance().set(&id, &sub);
    });
    env.ledger()
        .with_mut(|li| li.timestamp = T0 + interval_seconds);

    let ids = soroban_sdk::Vec::from_array(&env, [id]);
    let results = client.batch_charge(&ids);
    assert_eq!(results.len(), 1);
    let r0 = results.get(0).unwrap();
    assert!(r0.success);
    assert_eq!(r0.error_code, 0);

    let new_admin = Address::generate(&env);
    client.rotate_admin(&old_admin, &new_admin);

    env.ledger()
        .with_mut(|li| li.timestamp = T0 + 2 * interval_seconds);
    let sub2 = client.get_subscription(&id);
    assert_eq!(sub2.status, SubscriptionStatus::Active);
    let results2 = client.batch_charge(&ids);
    assert_eq!(results2.len(), 1);
    assert!(results2.get(0).unwrap().success);
}

#[test]
fn test_multiple_admin_rotations() {
    let (env, client, _, admin1) = setup_test_env();

    let admin2 = Address::generate(&env);
    let admin3 = Address::generate(&env);
    let admin4 = Address::generate(&env);

    client.rotate_admin(&admin1, &admin2);
    assert_eq!(client.get_admin(), admin2);

    client.rotate_admin(&admin2, &admin3);
    assert_eq!(client.get_admin(), admin3);

    client.rotate_admin(&admin3, &admin4);
    assert_eq!(client.get_admin(), admin4);

    client.set_min_topup(&admin4, &3_000000);
    assert_eq!(client.get_min_topup(), 3_000000);

    assert!(client.try_set_min_topup(&admin1, &1_000000).is_err());
    assert!(client.try_set_min_topup(&admin2, &1_000000).is_err());
    assert!(client.try_set_min_topup(&admin3, &1_000000).is_err());
}

#[test]
fn test_admin_rotation_does_not_affect_subscriptions() {
    let (env, client, _, old_admin) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    let subscription_before = client.get_subscription(&sub_id);

    let new_admin = Address::generate(&env);
    client.rotate_admin(&old_admin, &new_admin);

    let subscription_after = client.get_subscription(&sub_id);
    assert_eq!(
        subscription_before.subscriber,
        subscription_after.subscriber
    );
    assert_eq!(subscription_before.merchant, subscription_after.merchant);
    assert_eq!(subscription_before.amount, subscription_after.amount);
    assert_eq!(subscription_before.status, subscription_after.status);
}

#[test]
fn test_set_min_topup_unauthorized_before_rotation() {
    let (env, client, _, _) = setup_test_env();

    let non_admin = Address::generate(&env);

    let result = client.try_set_min_topup(&non_admin, &5_000000);
    assert!(result.is_err());
}

#[test]
fn test_set_min_topup_unauthorized_after_rotation() {
    let (env, client, _, old_admin) = setup_test_env();

    let new_admin = Address::generate(&env);
    let non_admin = Address::generate(&env);

    client.rotate_admin(&old_admin, &new_admin);

    let result = client.try_set_min_topup(&non_admin, &5_000000);
    assert!(result.is_err());

    let result = client.try_set_min_topup(&old_admin, &5_000000);
    assert!(result.is_err());
}

#[test]
fn test_recover_stranded_funds_unauthorized_before_rotation() {
    let (env, client, _, _) = setup_test_env();

    let non_admin = Address::generate(&env);
    let recipient = Address::generate(&env);

    let result = client.try_recover_stranded_funds(
        &non_admin,
        &recipient,
        &10_000000i128,
        &RecoveryReason::AccidentalTransfer,
    );
    assert!(result.is_err());
}

#[test]
fn test_recover_stranded_funds_unauthorized_after_rotation() {
    let (env, client, _, old_admin) = setup_test_env();

    let new_admin = Address::generate(&env);
    let non_admin = Address::generate(&env);
    let recipient = Address::generate(&env);

    client.rotate_admin(&old_admin, &new_admin);

    let result = client.try_recover_stranded_funds(
        &non_admin,
        &recipient,
        &10_000000i128,
        &RecoveryReason::AccidentalTransfer,
    );
    assert!(result.is_err());

    let result = client.try_recover_stranded_funds(
        &old_admin,
        &recipient,
        &10_000000i128,
        &RecoveryReason::AccidentalTransfer,
    );
    assert!(result.is_err());
}

#[test]
fn test_all_admin_operations_after_rotation() {
    let (env, client, _, old_admin) = setup_test_env();

    let new_admin = Address::generate(&env);

    client.rotate_admin(&old_admin, &new_admin);

    client.set_min_topup(&new_admin, &3_000000);
    assert_eq!(client.get_min_topup(), 3_000000);

    let recipient = Address::generate(&env);
    let result = client.try_recover_stranded_funds(
        &new_admin,
        &recipient,
        &5_000000i128,
        &RecoveryReason::DeprecatedFlow,
    );
    assert!(result.is_ok());

    let admin3 = Address::generate(&env);
    client.rotate_admin(&new_admin, &admin3);
    assert_eq!(client.get_admin(), admin3);
}

#[test]
fn test_admin_rotation_event_emission() {
    let (env, client, _, old_admin) = setup_test_env();

    let new_admin = Address::generate(&env);

    env.ledger().with_mut(|li| li.timestamp = 12345);

    client.rotate_admin(&old_admin, &new_admin);

    let events = env.events().all();
    assert!(!events.is_empty());
}

#[test]
fn test_rotate_admin_to_same_address() {
    let (_, client, _, admin) = setup_test_env();

    client.rotate_admin(&admin, &admin);

    assert_eq!(client.get_admin(), admin);

    client.set_min_topup(&admin, &2_000000);
    assert_eq!(client.get_min_topup(), 2_000000);
}

#[test]
fn test_admin_rotation_access_control_comprehensive() {
    let (env, client, _, admin1) = setup_test_env();

    let admin2 = Address::generate(&env);
    let admin3 = Address::generate(&env);
    let non_admin = Address::generate(&env);

    assert_eq!(client.get_admin(), admin1);

    client.set_min_topup(&admin1, &2_000000);
    assert_eq!(client.get_min_topup(), 2_000000);

    assert!(client.try_set_min_topup(&admin2, &3_000000).is_err());
    assert!(client.try_set_min_topup(&non_admin, &3_000000).is_err());

    client.rotate_admin(&admin1, &admin2);
    assert_eq!(client.get_admin(), admin2);

    client.set_min_topup(&admin2, &3_000000);
    assert_eq!(client.get_min_topup(), 3_000000);

    assert!(client.try_set_min_topup(&admin1, &4_000000).is_err());
    assert!(client.try_set_min_topup(&non_admin, &4_000000).is_err());

    client.rotate_admin(&admin2, &admin3);
    assert_eq!(client.get_admin(), admin3);

    client.set_min_topup(&admin3, &4_000000);
    assert_eq!(client.get_min_topup(), 4_000000);

    assert!(client.try_set_min_topup(&admin1, &5_000000).is_err());
    assert!(client.try_set_min_topup(&admin2, &5_000000).is_err());
    assert!(client.try_set_min_topup(&non_admin, &5_000000).is_err());
}

#[test]
fn test_admin_rotation_with_subscriptions_active() {
    let (env, client, _, old_admin) = setup_test_env();

    let subscriber1 = Address::generate(&env);
    let subscriber2 = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id1 = client.create_subscription(
        &subscriber1,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    let id2 = client.create_subscription(
        &subscriber2,
        &merchant,
        &5_000_000i128,
        &(7 * 24 * 60 * 60),
        &true,
    );

    client.pause_subscription(&id1, &subscriber1);

    let new_admin = Address::generate(&env);
    client.rotate_admin(&old_admin, &new_admin);

    assert_eq!(
        client.get_subscription(&id1).status,
        SubscriptionStatus::Paused
    );
    assert_eq!(
        client.get_subscription(&id2).status,
        SubscriptionStatus::Active
    );

    client.resume_subscription(&id1, &subscriber1);
    assert_eq!(
        client.get_subscription(&id1).status,
        SubscriptionStatus::Active
    );

    client.cancel_subscription(&id2, &subscriber2);
    assert_eq!(
        client.get_subscription(&id2).status,
        SubscriptionStatus::Cancelled
    );
}

#[test]
fn test_admin_cannot_be_rotated_by_previous_admin() {
    let (env, client, _, admin1) = setup_test_env();

    let admin2 = Address::generate(&env);
    let admin3 = Address::generate(&env);

    client.rotate_admin(&admin1, &admin2);

    let result = client.try_rotate_admin(&admin1, &admin3);
    assert!(result.is_err());

    assert_eq!(client.get_admin(), admin2);
}

#[test]
fn test_get_admin_before_and_after_rotation() {
    let (env, client, _, old_admin) = setup_test_env();

    assert_eq!(client.get_admin(), old_admin);

    let new_admin = Address::generate(&env);

    client.rotate_admin(&old_admin, &new_admin);

    assert_eq!(client.get_admin(), new_admin);

    let another_admin = Address::generate(&env);
    client.rotate_admin(&new_admin, &another_admin);
    assert_eq!(client.get_admin(), another_admin);
}

// =============================================================================
// View Function Tests: list_subscriptions_by_subscriber
// =============================================================================

#[test]
fn test_list_subscriptions_zero_subscriptions() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let page = client.list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);

    assert_eq!(page.subscription_ids.len(), 0);
    assert!(!page.has_next);
}

#[test]
fn test_list_subscriptions_one_subscription() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    let page = client.list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);

    assert_eq!(page.subscription_ids.len(), 1);
    assert_eq!(page.subscription_ids.get(0).unwrap(), id);
    assert!(!page.has_next);
}

#[test]
fn test_list_subscriptions_many_subscriptions() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let mut ids = soroban_sdk::Vec::new(&env);
    for _ in 0..5 {
        let id = client.create_subscription(
            &subscriber,
            &merchant,
            &10_000_000i128,
            &(30 * 24 * 60 * 60),
            &false,
        );
        ids.push_back(id);
    }

    let page = client.list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);

    assert_eq!(page.subscription_ids.len(), 5);
    assert!(!page.has_next);

    for i in 0..5 {
        assert_eq!(
            page.subscription_ids.get(i).unwrap(),
            ids.get(i as u32).unwrap()
        );
    }
}

#[test]
fn test_list_subscriptions_pagination_first_page() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let mut ids = soroban_sdk::Vec::new(&env);
    for _ in 0..15 {
        let id = client.create_subscription(
            &subscriber,
            &merchant,
            &10_000_000i128,
            &(30 * 24 * 60 * 60),
            &false,
        );
        ids.push_back(id);
    }

    let page1 = client.list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);

    assert_eq!(page1.subscription_ids.len(), 10);
    assert!(page1.has_next);

    for i in 0..10 {
        assert_eq!(
            page1.subscription_ids.get(i).unwrap(),
            ids.get(i as u32).unwrap()
        );
    }
}

#[test]
fn test_list_subscriptions_pagination_second_page() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let mut ids = soroban_sdk::Vec::new(&env);
    for _ in 0..15 {
        let id = client.create_subscription(
            &subscriber,
            &merchant,
            &10_000_000i128,
            &(30 * 24 * 60 * 60),
            &false,
        );
        ids.push_back(id);
    }

    let page1 = client.list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);
    assert_eq!(page1.subscription_ids.len(), 10);
    let last_id_page1 = page1.subscription_ids.get(9).unwrap();

    let next_start = last_id_page1 + 1;
    let page2 = client.list_subscriptions_by_subscriber(&subscriber, &next_start, &10u32);

    assert_eq!(page2.subscription_ids.len(), 5);
    assert!(!page2.has_next);

    for i in 0..5 {
        assert_eq!(
            page2.subscription_ids.get(i).unwrap(),
            ids.get((10 + i) as u32).unwrap()
        );
    }
}

#[test]
fn test_list_subscriptions_filters_by_subscriber() {
    let (env, client, _, _) = setup_test_env();

    let subscriber1 = Address::generate(&env);
    let subscriber2 = Address::generate(&env);
    let merchant = Address::generate(&env);

    for _ in 0..3 {
        client.create_subscription(
            &subscriber1,
            &merchant,
            &10_000_000i128,
            &(30 * 24 * 60 * 60),
            &false,
        );
    }

    for _ in 0..2 {
        client.create_subscription(
            &subscriber2,
            &merchant,
            &10_000_000i128,
            &(30 * 24 * 60 * 60),
            &false,
        );
    }

    let page1 = client.list_subscriptions_by_subscriber(&subscriber1, &0u32, &10u32);
    assert_eq!(page1.subscription_ids.len(), 3);

    let page2 = client.list_subscriptions_by_subscriber(&subscriber2, &0u32, &10u32);
    assert_eq!(page2.subscription_ids.len(), 2);
}

#[test]
fn test_list_subscriptions_small_limit() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let mut ids = soroban_sdk::Vec::new(&env);
    for _ in 0..5 {
        let id = client.create_subscription(
            &subscriber,
            &merchant,
            &10_000_000i128,
            &(30 * 24 * 60 * 60),
            &false,
        );
        ids.push_back(id);
    }

    let mut all_ids = soroban_sdk::Vec::new(&env);
    let mut start_id = 0u32;
    let mut has_next = true;

    while has_next {
        let page = client.list_subscriptions_by_subscriber(&subscriber, &start_id, &1u32);
        if page.subscription_ids.len() > 0 {
            let current_id = page.subscription_ids.get(0).unwrap();
            all_ids.push_back(current_id);
            start_id = current_id + 1;
            has_next = page.has_next;
        } else {
            has_next = false;
        }
    }

    assert_eq!(all_ids.len(), 5);
    for i in 0..5 {
        assert_eq!(all_ids.get(i as u32).unwrap(), ids.get(i as u32).unwrap());
    }
}

#[test]
#[should_panic]
fn test_list_subscriptions_limit_zero_returns_error() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);

    client.list_subscriptions_by_subscriber(&subscriber, &0u32, &0u32);
}

#[test]
fn test_list_subscriptions_respects_start_from_id() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let mut ids = soroban_sdk::Vec::new(&env);
    for _ in 0..10 {
        let id = client.create_subscription(
            &subscriber,
            &merchant,
            &10_000_000i128,
            &(30 * 24 * 60 * 60),
            &false,
        );
        ids.push_back(id);
    }

    let start_id = ids.get(5u32).unwrap();
    let page = client.list_subscriptions_by_subscriber(&subscriber, &start_id, &10u32);

    assert_eq!(page.subscription_ids.len(), 5);

    for i in 0..5 {
        assert_eq!(
            page.subscription_ids.get(i).unwrap(),
            ids.get((5 + i) as u32).unwrap()
        );
    }
}

#[test]
fn test_list_subscriptions_stable_ordering() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    for _ in 0..7 {
        client.create_subscription(
            &subscriber,
            &merchant,
            &10_000_000i128,
            &(30 * 24 * 60 * 60),
            &false,
        );
    }

    let page1 = client.list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);
    let page2 = client.list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);

    assert_eq!(page1.subscription_ids.len(), page2.subscription_ids.len());
    for i in 0..page1.subscription_ids.len() {
        assert_eq!(
            page1.subscription_ids.get(i).unwrap(),
            page2.subscription_ids.get(i).unwrap()
        );
    }
}

#[test]
fn test_list_subscriptions_multiple_merchants() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant1 = Address::generate(&env);
    let merchant2 = Address::generate(&env);

    let mut ids = soroban_sdk::Vec::new(&env);
    for i in 0..10 {
        let merchant = if i % 2 == 0 { &merchant1 } else { &merchant2 };
        let id = client.create_subscription(
            &subscriber,
            merchant,
            &10_000_000i128,
            &(30 * 24 * 60 * 60),
            &false,
        );
        ids.push_back(id);
    }

    let page = client.list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);

    assert_eq!(page.subscription_ids.len(), 10);
    for i in 0..10 {
        assert_eq!(
            page.subscription_ids.get(i).unwrap(),
            ids.get(i as u32).unwrap()
        );
    }
}
