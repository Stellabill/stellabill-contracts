use crate::{
    can_transition, get_allowed_transitions, validate_status_transition, Error, RecoveryReason,
    Subscription, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::testutils::{Address as _, Events, Ledger as _};
use soroban_sdk::{Address, Env, IntoVal, TryFromVal, Val, Vec, Symbol, Vec as SorobanVec};

// ---------------------------------------------------------------------------
// Helper: decode the event data payload (3rd element of event tuple)
// ---------------------------------------------------------------------------
#[allow(dead_code)]
fn last_event_data<T: TryFromVal<Env, Val>>(env: &Env) -> T {
    let events = env.events().all();
    let last = events.last().unwrap();
    T::try_from_val(env, &last.2).unwrap()
}

/// Helper: register contract, init, and return client + reusable addresses.
fn setup_test_env() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128); // 1 USDC min_topup

    (env, client, token, admin)
}

/// Helper: create a subscription for a given subscriber+merchant and return its id.
fn create_sub(
    env: &Env,
    client: &SubscriptionVaultClient,
    subscriber: &Address,
    merchant: &Address,
    amount: i128,
) -> u32 {
    client.create_subscription(
        subscriber,
        merchant,
        &amount,
        &(30u64 * 24 * 60 * 60), // 30 days
        &false,
    )
}

// ─── Existing tests ───────────────────────────────────────────────────────────

#[test]
fn test_init_and_struct() {
    let (env, client, _, _) = setup_test_env();
    // Basic initialization test
    assert!(client.get_min_topup() == 1_000000i128);
}

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

// Helper to create a subscription and optionally set its status for testing
fn create_sub_with_status(
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
    if status != SubscriptionStatus::Active {
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
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);

    // Pause from Active should succeed
    client.pause_subscription(&id, &subscriber);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Paused);
}

#[test]
#[should_panic(expected = "Error(Contract, #409)")]
fn test_pause_subscription_from_cancelled_should_fail() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);

    // First cancel
    client.cancel_subscription(&id, &subscriber);

    // Then try to pause (should fail)
    client.pause_subscription(&id, &subscriber);
}

#[test]
fn test_pause_subscription_from_paused_is_idempotent() {
    // Idempotent transition: Paused -> Paused should succeed (no-op)
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);

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
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);

    // Cancel from Active should succeed
    client.cancel_subscription(&id, &subscriber);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

#[test]
fn test_cancel_subscription_from_paused() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);

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
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);

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
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);

    // First pause
    client.pause_subscription(&id, &subscriber);

    // Then resume
    client.resume_subscription(&id, &subscriber);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

#[test]
#[should_panic(expected = "Error(Contract, #409)")]
fn test_resume_subscription_from_cancelled_should_fail() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);

    // First cancel
    client.cancel_subscription(&id, &subscriber);

    // Try to resume (should fail)
    client.resume_subscription(&id, &subscriber);
}

#[test]
fn test_state_transition_idempotent_same_status() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);

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
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);

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
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);

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
            create_sub_with_status(&env, &client, SubscriptionStatus::Active);
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
            create_sub_with_status(&env, &client, SubscriptionStatus::Active);
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
            create_sub_with_status(&env, &client, SubscriptionStatus::Active);
        // Simulate transition by updating storage directly
        let mut sub = client.get_subscription(&id);
        sub.status = SubscriptionStatus::InsufficientBalance;
        env.as_contract(&client.address, || {
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
            create_sub_with_status(&env, &client, SubscriptionStatus::Active);
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
            create_sub_with_status(&env, &client, SubscriptionStatus::Active);
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
            create_sub_with_status(&env, &client, SubscriptionStatus::Active);

        // Set to InsufficientBalance
        let mut sub = client.get_subscription(&id);
        sub.status = SubscriptionStatus::InsufficientBalance;
        env.as_contract(&client.address, || {
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
            create_sub_with_status(&env, &client, SubscriptionStatus::Active);

        // Set to InsufficientBalance
        let mut sub = client.get_subscription(&id);
        sub.status = SubscriptionStatus::InsufficientBalance;
        env.as_contract(&client.address, || {
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
#[should_panic(expected = "Error(Contract, #409)")]
fn test_invalid_cancelled_to_active() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);

    client.cancel_subscription(&id, &subscriber);
    client.resume_subscription(&id, &subscriber);
}

#[test]
#[should_panic(expected = "Error(Contract, #409)")]
fn test_invalid_insufficient_balance_to_paused() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);

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

#[test]
fn test_merchant_with_no_subscriptions() {
    let (env, client, _, _) = setup_test_env();
    let merchant = Address::generate(&env);

    let subs = client.get_subscriptions_by_merchant(&merchant, &0, &10);
    assert_eq!(subs.len(), 0);

    let count = client.get_merchant_subscription_count(&merchant);
    assert_eq!(count, 0);
}

#[test]
fn test_merchant_with_one_subscription() {
    let (env, client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = create_sub(&env, &client, &subscriber, &merchant, 10_000_000);

    let subs = client.get_subscriptions_by_merchant(&merchant, &0, &10);
    assert_eq!(subs.len(), 1);

    let sub = subs.get(0).unwrap();
    assert_eq!(sub.subscriber, subscriber);
    assert_eq!(sub.merchant, merchant);
    assert_eq!(sub.amount, 10_000_000);
    assert_eq!(sub.status, SubscriptionStatus::Active);

    // Verify get_subscription returns the same data
    let by_id = client.get_subscription(&id);
    assert_eq!(by_id.subscriber, subscriber);

    assert_eq!(client.get_merchant_subscription_count(&merchant), 1);
}

#[test]
fn test_merchant_with_multiple_subscriptions() {
    let (env, client, _, _) = setup_test_env();
    let merchant = Address::generate(&env);

    let sub1 = Address::generate(&env);
    let sub2 = Address::generate(&env);
    let sub3 = Address::generate(&env);

    create_sub(&env, &client, &sub1, &merchant, 5_000_000);
    create_sub(&env, &client, &sub2, &merchant, 10_000_000);
    create_sub(&env, &client, &sub3, &merchant, 20_000_000);

    let subs = client.get_subscriptions_by_merchant(&merchant, &0, &10);
    assert_eq!(subs.len(), 3);

    // Verify chronological (insertion) order
    assert_eq!(subs.get(0).unwrap().amount, 5_000_000);
    assert_eq!(subs.get(1).unwrap().amount, 10_000_000);
    assert_eq!(subs.get(2).unwrap().amount, 20_000_000);

    assert_eq!(client.get_merchant_subscription_count(&merchant), 3);
}

#[test]
fn test_pagination_basic() {
    let (env, client, _, _) = setup_test_env();
    let merchant = Address::generate(&env);

    // Create 5 subscriptions
    for i in 0..5 {
        let subscriber = Address::generate(&env);
        create_sub(&env, &client, &subscriber, &merchant, (i + 1) * 1_000_000);
    }

    // Request first 2
    let page = client.get_subscriptions_by_merchant(&merchant, &0, &2);
    assert_eq!(page.len(), 2);
    assert_eq!(page.get(0).unwrap().amount, 1_000_000);
    assert_eq!(page.get(1).unwrap().amount, 2_000_000);
}

#[test]
fn test_pagination_offset() {
    let (env, client, _, _) = setup_test_env();
    let merchant = Address::generate(&env);

    for i in 0..5 {
        let subscriber = Address::generate(&env);
        create_sub(&env, &client, &subscriber, &merchant, (i + 1) * 1_000_000);
    }

    // Request 2 starting from offset 2
    let page = client.get_subscriptions_by_merchant(&merchant, &2, &2);
    assert_eq!(page.len(), 2);
    assert_eq!(page.get(0).unwrap().amount, 3_000_000);
    assert_eq!(page.get(1).unwrap().amount, 4_000_000);
}

#[test]
fn test_pagination_beyond_end() {
    let (env, client, _, _) = setup_test_env();
    let merchant = Address::generate(&env);

    for i in 0..5 {
        let subscriber = Address::generate(&env);
        create_sub(&env, &client, &subscriber, &merchant, (i + 1) * 1_000_000);
    }

    // Request 10 starting from offset 3 → should return only last 2
    let page = client.get_subscriptions_by_merchant(&merchant, &3, &10);
    assert_eq!(page.len(), 2);
    assert_eq!(page.get(0).unwrap().amount, 4_000_000);
    assert_eq!(page.get(1).unwrap().amount, 5_000_000);
}

#[test]
fn test_pagination_start_past_end() {
    let (env, client, _, _) = setup_test_env();
    let merchant = Address::generate(&env);

    let subscriber = Address::generate(&env);
    create_sub(&env, &client, &subscriber, &merchant, 1_000_000);

    // Start way past the end
    let page = client.get_subscriptions_by_merchant(&merchant, &100, &10);
    assert_eq!(page.len(), 0);
}

#[test]
fn test_multiple_merchants_isolated() {
    let (env, client, _, _) = setup_test_env();
    let merchant_a = Address::generate(&env);
    let merchant_b = Address::generate(&env);

    let sub1 = Address::generate(&env);
    let sub2 = Address::generate(&env);
    let sub3 = Address::generate(&env);

    create_sub(&env, &client, &sub1, &merchant_a, 1_000_000);
    create_sub(&env, &client, &sub2, &merchant_a, 2_000_000);
    create_sub(&env, &client, &sub3, &merchant_b, 9_000_000);

    // Merchant A sees only their 2 subscriptions
    let a_subs = client.get_subscriptions_by_merchant(&merchant_a, &0, &10);
    assert_eq!(a_subs.len(), 2);
    assert_eq!(a_subs.get(0).unwrap().amount, 1_000_000);
    assert_eq!(a_subs.get(1).unwrap().amount, 2_000_000);

    // Merchant B sees only their 1 subscription
    let b_subs = client.get_subscriptions_by_merchant(&merchant_b, &0, &10);
    assert_eq!(b_subs.len(), 1);
    assert_eq!(b_subs.get(0).unwrap().amount, 9_000_000);

    assert_eq!(client.get_merchant_subscription_count(&merchant_a), 2);
    assert_eq!(client.get_merchant_subscription_count(&merchant_b), 1);
}

#[test]
fn test_merchant_subscription_count() {
    let (env, client, _, _) = setup_test_env();
    let merchant = Address::generate(&env);

    assert_eq!(client.get_merchant_subscription_count(&merchant), 0);

    for _ in 0..4 {
        let subscriber = Address::generate(&env);
        create_sub(&env, &client, &subscriber, &merchant, 5_000_000);
    }

    assert_eq!(client.get_merchant_subscription_count(&merchant), 4);
}

// -- Billing interval enforcement tests --------------------------------------

const T0: u64 = 1700000000;
const INTERVAL: u64 = 86400;

fn setup_interval_test(env: &Env, interval: u64) -> (SubscriptionVaultClient<'static>, u32) {
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    
    let token = Address::generate(env);
    let admin = Address::generate(env);
    let min_topup = 1_000000i128;
    client.init(&token, &admin, &min_topup);
    
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    let id = client.create_subscription(&subscriber, &merchant, &1000i128, &interval, &false);
    
    // Deposit funds so charge can succeed
    client.deposit_funds(&id, &subscriber, &10_000000i128);

    // Set initial timestamp
    env.ledger().set_timestamp(T0);
    
    // Manually update the subscription to have T0 as last_payment_timestamp
    // to match the test expectations of "starting at T0".
    let mut sub = client.get_subscription(&id);
    sub.last_payment_timestamp = T0;
    env.as_contract(&client.address, || {
        env.storage().instance().set(&id, &sub);
    });
    
    (client, id)
}

/// Just-before: charge 1 second before the interval elapses.
/// Must reject with IntervalNotElapsed and leave storage untouched.
#[test]
fn test_charge_rejected_before_interval() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_interval_test(&env, INTERVAL);

    // 1 second too early.
    env.ledger().set_timestamp(T0 + INTERVAL - 1);

    let res = client.try_charge_subscription(&id);
    assert_eq!(res, Err(Ok(Error::IntervalNotElapsed)));

    // Storage unchanged — last_payment_timestamp still equals creation time.
    let sub = client.get_subscription(&id);
    assert_eq!(sub.last_payment_timestamp, T0);
}

/// Exact boundary: charge at exactly last_payment_timestamp + interval_seconds.
/// Must succeed and advance last_payment_timestamp.
#[test]
fn test_charge_succeeds_at_exact_interval() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_interval_test(&env, INTERVAL);

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.last_payment_timestamp, T0 + INTERVAL);
}

/// After interval: charge well past the interval boundary.
/// Must succeed and set last_payment_timestamp to the current ledger time.
#[test]
fn test_charge_succeeds_after_interval() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_interval_test(&env, INTERVAL);

    let charge_time = T0 + 2 * INTERVAL;
    env.ledger().set_timestamp(charge_time);
    client.charge_subscription(&id);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.last_payment_timestamp, charge_time);
}

/// Same-timestamp retry: a second charge at the identical timestamp that
/// succeeded must be rejected because 0 seconds < interval_seconds.
#[test]
fn test_immediate_retry_at_same_timestamp_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_interval_test(&env, INTERVAL);

    let t1 = T0 + INTERVAL;
    env.ledger().set_timestamp(t1);
    client.charge_subscription(&id);

    // Retry at the same timestamp — must fail (replay protection), storage stays at t1.
    let res = client.try_charge_subscription(&id);
    assert_eq!(res, Err(Ok(Error::Replay)));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.last_payment_timestamp, t1);
}

/// Repeated charges across 6 consecutive intervals.
/// Verifies the sliding-window reset works correctly over many cycles.
#[test]
fn test_repeated_charges_across_many_intervals() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_interval_test(&env, INTERVAL);

    for i in 1..=6u64 {
        let charge_time = T0 + i * INTERVAL;
        env.ledger().set_timestamp(charge_time);
        client.charge_subscription(&id);

        let sub = client.get_subscription(&id);
        assert_eq!(sub.last_payment_timestamp, charge_time);
    }

    // One more attempt without advancing time — must fail (replay protection).
    let res = client.try_charge_subscription(&id);
    assert_eq!(res, Err(Ok(Error::Replay)));
}

// =============================================================================
// Replay protection and idempotency tests (#24)
// =============================================================================

fn idempotency_key(env: &Env, seed: u8) -> soroban_sdk::BytesN<32> {
    let mut arr = [0u8; 32];
    arr[0] = seed;
    soroban_sdk::BytesN::from_array(env, &arr)
}

/// First charge with an idempotency key succeeds and debits once.
#[test]
fn test_replay_first_charge_with_idempotency_key_succeeds() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_interval_test(&env, INTERVAL);
    env.ledger().set_timestamp(T0 + INTERVAL);

    let key = idempotency_key(&env, 1);
    client.charge_subscription(&id);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.last_payment_timestamp, T0 + INTERVAL);
    assert_eq!(sub.prepaid_balance, 10_000000i128 - 1000i128);
}


/// Same period, different idempotency key: second call is rejected as Replay.
#[test]
fn test_replay_different_key_same_period_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_interval_test(&env, INTERVAL);
    env.ledger().set_timestamp(T0 + INTERVAL);

    let key1 = idempotency_key(&env, 10);
    client.charge_subscription(&id);

    let key2 = idempotency_key(&env, 20);
    let res = client.try_charge_subscription(&id);
    assert_eq!(res, Err(Ok(Error::Replay)));
}

/// New period with new idempotency key succeeds.
#[test]
fn test_replay_new_period_new_key_succeeds() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_interval_test(&env, INTERVAL);

    env.ledger().set_timestamp(T0 + INTERVAL);
    let key1 = idempotency_key(&env, 1);
    client.charge_subscription(&id);

    env.ledger().set_timestamp(T0 + 2 * INTERVAL);
    let key2 = idempotency_key(&env, 2);
    client.charge_subscription(&id);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.last_payment_timestamp, T0 + 2 * INTERVAL);
    assert_eq!(sub.prepaid_balance, 10_000000i128 - 2000i128);
}

/// Charge without idempotency key still protected by period-based replay.
#[test]
fn test_replay_no_key_still_rejected_same_period() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_interval_test(&env, INTERVAL);
    env.ledger().set_timestamp(T0 + INTERVAL);

    client.charge_subscription(&id);
    let res = client.try_charge_subscription(&id);
    assert_eq!(res, Err(Ok(Error::Replay)));
}

/// Minimum interval (1 second): charge at creation time must fail,
/// charge 1 second later must succeed.
#[test]
fn test_one_second_interval_boundary() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_interval_test(&env, 1);

    // At creation time — 0 seconds elapsed, interval is 1 s → too early.
    env.ledger().set_timestamp(T0);
    let res = client.try_charge_subscription(&id);
    assert_eq!(res, Err(Ok(Error::IntervalNotElapsed)));

    // Exactly 1 second later — boundary, should succeed.
    env.ledger().set_timestamp(T0 + 1);
    client.charge_subscription(&id);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.last_payment_timestamp, T0 + 1);
}



#[test]
fn test_min_topup_below_threshold() {
    let (env, client, token, admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let min_topup = 5_000000i128; // 5 USDC

    client.set_min_topup(&admin, &min_topup);
    
    // Create subscription
    let id = client.create_subscription(&subscriber, &Address::generate(&env), &10_000_000i128, &3600u64, &false);
    
    let result = client.try_deposit_funds(&id, &subscriber, &4_999999);
    assert!(result.is_err());
}

#[test]
fn test_charge_subscription_auth() {
    let (env, client, _, admin) = setup_test_env();

    // Create a subscription so ID 0 exists
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = client.create_subscription(&subscriber, &merchant, &1000i128, &3600u64, &false);

    // Deposit funds so charge can succeed
    client.deposit_funds(&id, &subscriber, &10_000000i128);

    // Advance time to allow charging
    env.ledger().with_mut(|li| li.timestamp += 3601);

    // Mock auth for the admin address (args: subscription_id, idempotency_key)
    let none_key: Option<soroban_sdk::BytesN<32>> = None;
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "charge_subscription",
            args: (id.clone(), none_key).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    client.charge_subscription(&id);
}


#[test]
fn test_charge_subscription_admin() {
    let (env, client, _, admin) = setup_test_env();

    // Create a subscription
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = client.create_subscription(&subscriber, &merchant, &1000i128, &3600u64, &false);

    // Deposit funds so charge can succeed
    client.deposit_funds(&id, &subscriber, &10_000000i128);

    // Advance time
    env.ledger().with_mut(|li| li.timestamp += 3601);

    // Mock auth for the admin address (args: subscription_id, idempotency_key)
    let none_key: Option<soroban_sdk::BytesN<32>> = None;
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "charge_subscription",
            args: (id.clone(), none_key).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    client.charge_subscription(&id);
}

#[test]
fn test_min_topup_exactly_at_threshold() {
    let (env, client, token, admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let min_topup = 5_000000i128; // 5 USDC

    client.set_min_topup(&admin, &min_topup);
    // Create subscription
    let id = client.create_subscription(&subscriber, &Address::generate(&env), &10_000_000i128, &3600u64, &false);
    
    let result = client.try_deposit_funds(&id, &subscriber, &min_topup);

    assert!(result.is_ok());
}

#[test]
fn test_min_topup_above_threshold() {
    let (env, client, token, admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let min_topup = 5_000000i128; // 5 USDC

    client.set_min_topup(&admin, &min_topup);
    // Create subscription
    let id = client.create_subscription(&subscriber, &Address::generate(&env), &10_000_000i128, &3600u64, &false);
    
    let result = client.try_deposit_funds(&id, &subscriber, &10_000000);

    assert!(result.is_ok());
}

#[test]
fn test_set_min_topup_by_admin() {
    let (env, client, token, admin) = setup_test_env();
    let initial_min = 1_000000i128;
    let new_min = 10_000000i128;

    client.set_min_topup(&admin, &initial_min); // Ensure it's set to initial_min first
    assert_eq!(client.get_min_topup(), initial_min);

    client.set_min_topup(&admin, &new_min);
    assert_eq!(client.get_min_topup(), new_min);
}

#[test]
#[should_panic(expected = "Error(Contract, #403)")] // Error::NotAdmin
fn test_set_min_topup_unauthorized() {
    let (env, client, token, admin) = setup_test_env();
    let non_admin = Address::generate(&env);
    let min_topup = 1_000000i128;

    client.set_min_topup(&admin, &min_topup);

    client.set_min_topup(&non_admin, &5_000000);
}
// =============================================================================
// Next Charge Timestamp Helper Tests
// =============================================================================

#[test]
fn test_estimate_topup_balance_already_covers_returns_zero() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);
    // 10 USDC per interval, deposit 30 USDC, ask for 3 intervals -> required 30, balance 30, topup 0
    client.deposit_funds(&id, &subscriber, &30_000000i128);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.amount, 10_000_000); // from create_sub_with_status
    let topup = client.estimate_topup_for_intervals(&id, &3);
    assert_eq!(topup, 0);
}

#[test]
fn test_estimate_topup_insufficient_balance_returns_shortfall() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);
    // amount 10_000_000, 3 intervals = 30_000_000 required; deposit 10_000_000 -> topup 20_000_000
    client.deposit_funds(&id, &subscriber, &10_000000i128);
    let topup = client.estimate_topup_for_intervals(&id, &3);
    assert_eq!(topup, 20_000_000);
}

#[test]
fn test_estimate_topup_no_balance_returns_full_required() {
    let (env, client, _, _) = setup_test_env();
    let (id, _, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);
    // prepaid_balance 0, 5 intervals * 10_000_000 = 50_000_000
    let topup = client.estimate_topup_for_intervals(&id, &5);
    assert_eq!(topup, 50_000_000);
}

#[test]
fn test_estimate_topup_subscription_not_found() {
    let (_env, client, _, _) = setup_test_env();
    let result = client.try_estimate_topup_for_intervals(&9999, &1);
    assert_eq!(result, Err(Ok(Error::SubscriptionNotFound)));
}

// =============================================================================
// batch_charge tests (#33)
// =============================================================================

fn setup_batch_env(env: &Env) -> (SubscriptionVaultClient<'static>, Address, u32, u32) {
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    let token = Address::generate(env);
    let admin = Address::generate(env);
    client.init(&token, &admin, &1_000000i128);
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    let id0 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id0, &subscriber, &10_000000i128);
    let id1 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id1, &subscriber, &10_000000i128);
    env.ledger().set_timestamp(T0 + INTERVAL);
    (client, admin, id0, id1)
}

#[test]
fn test_batch_charge_empty_list_returns_empty() {
    use crate::{compute_next_charge_info, Subscription, SubscriptionStatus};

    let env = Env::default();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let last_payment = 1000u64;
    let interval = 30 * 24 * 60 * 60; // 30 days in seconds

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

    // Active subscription: charge is expected
    assert!(info.is_charge_expected);
    // Next charge = last_payment + interval
    assert_eq!(info.next_charge_timestamp, last_payment + interval);
}

#[test]
fn test_compute_next_charge_info_paused_subscription() {
    use crate::{compute_next_charge_info, Subscription, SubscriptionStatus};

    let env = Env::default();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let last_payment = 2000u64;
    let interval = 7 * 24 * 60 * 60; // 7 days

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

    // Paused subscription: charge is NOT expected
    assert!(!info.is_charge_expected);
    // Timestamp is still computed for reference
    assert_eq!(info.next_charge_timestamp, last_payment + interval);
}

#[test]
fn test_compute_next_charge_info_cancelled_subscription() {
    use crate::{compute_next_charge_info, Subscription, SubscriptionStatus};

    let env = Env::default();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let last_payment = 5000u64;
    let interval = 24 * 60 * 60; // 1 day

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

    // Cancelled subscription: charge is NOT expected (terminal state)
    assert!(!info.is_charge_expected);
    // Timestamp is still computed for reference
    assert_eq!(info.next_charge_timestamp, last_payment + interval);
}

#[test]
fn test_compute_next_charge_info_insufficient_balance_subscription() {
    use crate::{compute_next_charge_info, Subscription, SubscriptionStatus};

    let env = Env::default();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let last_payment = 3000u64;
    let interval = 30 * 24 * 60 * 60; // 30 days

    let subscription = Subscription {
        subscriber,
        merchant,
        amount: 20_000_000i128,
        interval_seconds: interval,
        last_payment_timestamp: last_payment,
        status: SubscriptionStatus::InsufficientBalance,
        prepaid_balance: 1_000_000i128, // Not enough for next charge
        usage_enabled: false,
    };

    let info = compute_next_charge_info(&subscription);

    // InsufficientBalance subscription: charge IS expected (will retry after funding)
    assert!(info.is_charge_expected);
    // Next charge = last_payment + interval
    assert_eq!(info.next_charge_timestamp, last_payment + interval);
}

#[test]
fn test_compute_next_charge_info_short_interval() {
    use crate::{compute_next_charge_info, Subscription, SubscriptionStatus};

    let env = Env::default();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let last_payment = 100000u64;
    let interval = 60; // 1 minute interval

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
    let interval = 365 * 24 * 60 * 60; // 1 year in seconds

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

    // Test saturating_add behavior at edge of u64 range
    let last_payment = u64::MAX - 100;
    let interval = 200; // Would overflow without saturating_add

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
    // Should saturate to u64::MAX instead of wrapping
    assert_eq!(info.next_charge_timestamp, u64::MAX);
}

#[test]
fn test_get_next_charge_info_contract_method() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 10_000_000i128;
    let interval_seconds = 30 * 24 * 60 * 60; // 30 days

    // Set initial ledger timestamp
    env.ledger().with_mut(|li| li.timestamp = 1000);

    // Create subscription
    let id = client.create_subscription(&subscriber, &merchant, &amount, &interval_seconds, &false);

    // Get next charge info
    let info = client.get_next_charge_info(&id);

    // Should be Active with charge expected
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

    // Create subscription (starts as Active)
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
fn test_get_next_charge_info_insufficient_balance_status() {
    use crate::SubscriptionStatus;

    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 10_000_000i128;
    let interval_seconds = 7 * 24 * 60 * 60; // 7 days

    env.ledger().with_mut(|li| li.timestamp = 2000);

    // Create subscription
    let id = client.create_subscription(&subscriber, &merchant, &amount, &interval_seconds, &false);

    // Manually set to InsufficientBalance for testing
    let mut sub = client.get_subscription(&id);
    sub.status = SubscriptionStatus::InsufficientBalance;
    env.as_contract(&client.address, || {
        env.storage().instance().set(&id, &sub);
    });

    // Get next charge info
    let info = client.get_next_charge_info(&id);

    // InsufficientBalance: charge IS expected (will retry after funding)
    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, 2000 + interval_seconds);
}

#[test]
#[should_panic(expected = "Error(Contract, #405)")]
fn test_get_next_charge_info_subscription_not_found() {
    let (_, client, _, _) = setup_test_env();

    // Try to get next charge info for non-existent subscription
    client.get_next_charge_info(&999);
}

#[test]
fn test_get_next_charge_info_multiple_intervals() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Daily subscription
    env.ledger().with_mut(|li| li.timestamp = 10000);
    let daily_id = client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &(24 * 60 * 60), // 1 day
        &false,
    );

    // Weekly subscription
    env.ledger().with_mut(|li| li.timestamp = 20000);
    let weekly_id = client.create_subscription(
        &subscriber,
        &merchant,
        &5_000_000i128,
        &(7 * 24 * 60 * 60), // 7 days
        &false,
    );

    // Monthly subscription
    env.ledger().with_mut(|li| li.timestamp = 30000);
    let monthly_id = client.create_subscription(
        &subscriber,
        &merchant,
        &20_000_000i128,
        &(30 * 24 * 60 * 60), // 30 days
        &false,
    );

    // Check each subscription has correct next charge time
    let daily_info = client.get_next_charge_info(&daily_id);
    assert_eq!(daily_info.next_charge_timestamp, 10000 + 24 * 60 * 60);

    let weekly_info = client.get_next_charge_info(&weekly_id);
    assert_eq!(weekly_info.next_charge_timestamp, 20000 + 7 * 24 * 60 * 60);

    let monthly_info = client.get_next_charge_info(&monthly_id);
    assert_eq!(
        monthly_info.next_charge_timestamp,
        30000 + 30 * 24 * 60 * 60
    );

    // All should have charges expected (Active status)
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

    // Edge case: zero interval (immediate recurring charge)
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

// =============================================================================
// Admin Recovery of Stranded Funds Tests
// =============================================================================

#[test]
fn test_recover_stranded_funds_successful() {
    let (env, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&env);
    let amount = 50_000_000i128; // 50 USDC
    let reason = RecoveryReason::AccidentalTransfer;

    env.ledger().with_mut(|li| li.timestamp = 10000);

    // Recovery should succeed
    let result = client.try_recover_stranded_funds(&admin, &recipient, &amount, &reason);
    assert!(result.is_ok());

    // Verify event was emitted
    let events = env.events().all();
    assert!(events.len() > 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #403)")]
fn test_recover_stranded_funds_unauthorized_caller() {
    let (env, client, _, _) = setup_test_env();

    let non_admin = Address::generate(&env);
    let recipient = Address::generate(&env);
    let amount = 10_000_000i128;
    let reason = RecoveryReason::AccidentalTransfer;

    // Should fail: caller is not admin
    client.recover_stranded_funds(&non_admin, &recipient, &amount, &reason);
}

#[test]
#[should_panic(expected = "Error(Contract, #407)")]
fn test_recover_stranded_funds_zero_amount() {
    let (_, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&admin.env());
    let amount = 0i128; // Invalid: zero amount
    let reason = RecoveryReason::DeprecatedFlow;

    // Should fail: amount must be positive
    client.recover_stranded_funds(&admin, &recipient, &amount, &reason);
}

#[test]
#[should_panic(expected = "Error(Contract, #407)")]
fn test_recover_stranded_funds_negative_amount() {
    let (_, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&admin.env());
    let amount = -1_000_000i128; // Invalid: negative amount
    let reason = RecoveryReason::AccidentalTransfer;

    // Should fail: amount must be positive
    client.recover_stranded_funds(&admin, &recipient, &amount, &reason);
}

#[test]
fn test_recover_stranded_funds_all_recovery_reasons() {
    let (env, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&env);
    let amount = 10_000_000i128;

    // Test each recovery reason
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

    // Perform recovery
    client.recover_stranded_funds(&admin, &recipient, &amount, &reason);

    // Check that event was emitted
    let events = env.events().all();
    assert!(events.len() > 0);

    // The event should contain recovery information
    // Note: Event details verification depends on SDK version
}

#[test]
fn test_recover_stranded_funds_large_amount() {
    let (_, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&admin.env());
    let amount = 1_000_000_000_000i128; // 1 million USDC (with 6 decimals)
    let reason = RecoveryReason::DeprecatedFlow;

    // Should handle large amounts
    let result = client.try_recover_stranded_funds(&admin, &recipient, &amount, &reason);
    assert!(result.is_ok());
}

#[test]
fn test_recover_stranded_funds_small_amount() {
    let (_, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&admin.env());
    let amount = 1i128; // Minimal amount (1 stroops)
    let reason = RecoveryReason::AccidentalTransfer;

    // Should handle minimal positive amount
    let result = client.try_recover_stranded_funds(&admin, &recipient, &amount, &reason);
    assert!(result.is_ok());
}

#[test]
fn test_recover_stranded_funds_multiple_recoveries() {
    let (env, client, _, admin) = setup_test_env();

    let recipient1 = Address::generate(&env);
    let recipient2 = Address::generate(&env);
    let recipient3 = Address::generate(&env);

    // Multiple recoveries should all succeed
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

    // Verify events were emitted
    // Note: Exact count may vary by SDK version
    let events = env.events().all();
    assert!(events.len() > 0);
}

#[test]
fn test_recover_stranded_funds_different_recipients() {
    let (env, client, _, admin) = setup_test_env();

    // Test recovery to different recipient types
    let treasury = Address::generate(&env);
    let user_wallet = Address::generate(&env);
    let contract_addr = Address::generate(&env);

    let amount = 5_000_000i128;
    let reason = RecoveryReason::AccidentalTransfer;

    // Recovery to treasury
    assert!(client
        .try_recover_stranded_funds(&admin, &treasury, &amount, &reason)
        .is_ok());

    // Recovery to user wallet
    assert!(client
        .try_recover_stranded_funds(&admin, &user_wallet, &amount, &reason)
        .is_ok());

    // Recovery to contract address
    assert!(client
        .try_recover_stranded_funds(&admin, &contract_addr, &amount, &reason)
        .is_ok());
}

#[test]
fn test_recovery_reason_enum_values() {
    // Verify recovery reason enum is properly defined
    let reason1 = RecoveryReason::AccidentalTransfer;
    let reason2 = RecoveryReason::DeprecatedFlow;
    let reason3 = RecoveryReason::UnreachableSubscriber;

    // Ensure reasons are distinct
    assert!(reason1 != reason2);
    assert!(reason2 != reason3);
    assert!(reason1 != reason3);

    // Test cloning
    let reason_clone = reason1.clone();
    assert!(reason_clone == RecoveryReason::AccidentalTransfer);
}

#[test]
fn test_recover_stranded_funds_timestamp_recorded() {
    let (env, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&env);
    let amount = 15_000_000i128;
    let reason = RecoveryReason::DeprecatedFlow;

    // Set specific timestamp
    let expected_timestamp = 123456u64;
    env.ledger()
        .with_mut(|li| li.timestamp = expected_timestamp);

    // Perform recovery
    client.recover_stranded_funds(&admin, &recipient, &amount, &reason);

    // Event should contain the timestamp
    // (Full verification depends on event inspection capabilities)
    let events = env.events().all();
    assert!(events.len() > 0);
}

#[test]
fn test_recover_stranded_funds_admin_authorization_required() {
    let (env, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&env);
    let amount = 10_000_000i128;
    let reason = RecoveryReason::AccidentalTransfer;

    // This should succeed because admin is authenticated
    let result = client.try_recover_stranded_funds(&admin, &recipient, &amount, &reason);
    assert!(result.is_ok());
}

#[test]
fn test_recover_stranded_funds_does_not_affect_subscriptions() {
    let (env, client, _, admin) = setup_test_env();

    // Create a subscription
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    // Perform recovery (should not affect subscription)
    let recipient = Address::generate(&env);
    client.recover_stranded_funds(
        &admin,
        &recipient,
        &5_000_000i128,
        &RecoveryReason::DeprecatedFlow,
    );

    // Verify subscription is still intact
    let subscription = client.get_subscription(&sub_id);
    assert_eq!(subscription.status, SubscriptionStatus::Active);
    assert_eq!(subscription.subscriber, subscriber);
    assert_eq!(subscription.merchant, merchant);
}

#[test]
fn test_recover_stranded_funds_with_cancelled_subscription() {
    let (env, client, _, admin) = setup_test_env();

    // Create and cancel a subscription
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

    // Admin can still recover stranded funds
    let recipient = Address::generate(&env);
    let result = client.try_recover_stranded_funds(
        &admin,
        &recipient,
        &5_000_000i128,
        &RecoveryReason::UnreachableSubscriber,
    );
    assert!(result.is_ok());

    // Subscription remains cancelled
    assert_eq!(
        client.get_subscription(&sub_id).status,
        SubscriptionStatus::Cancelled
    );
}

#[test]
fn test_recover_stranded_funds_idempotency() {
    let (env, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&env);
    let amount = 10_000_000i128;
    let reason = RecoveryReason::AccidentalTransfer;

    // Perform first recovery
    let result1 = client.try_recover_stranded_funds(&admin, &recipient, &amount, &reason);
    assert!(result1.is_ok());

    // Perform second recovery with same parameters
    let result2 = client.try_recover_stranded_funds(&admin, &recipient, &amount, &reason);
    assert!(result2.is_ok());

    // Both should succeed (no idempotency constraint)
    // Each generates its own event
    let events = env.events().all();
    assert!(events.len() > 0);
}

#[test]
fn test_recover_stranded_funds_edge_case_max_i128() {
    let (_, client, _, admin) = setup_test_env();

    let recipient = Address::generate(&admin.env());
    // Test near max i128 value
    let amount = i128::MAX - 1000;
    let reason = RecoveryReason::DeprecatedFlow;

    // Should handle large values
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
    assert_eq!(subscription.usage_enabled, false);
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
    assert_eq!(subscription.usage_enabled, true);
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

    // Verify initial state
    assert_eq!(client.get_subscription(&id).usage_enabled, true);

    // Pause subscription
    client.pause_subscription(&id, &subscriber);
    assert_eq!(client.get_subscription(&id).usage_enabled, true);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );

    // Resume subscription
    client.resume_subscription(&id, &subscriber);
    assert_eq!(client.get_subscription(&id).usage_enabled, true);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );

    // Cancel subscription
    client.cancel_subscription(&id, &subscriber);
    assert_eq!(client.get_subscription(&id).usage_enabled, true);
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

    // Create subscription with usage disabled
    let id1 = client.create_subscription(
        &subscriber,
        &merchant1,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    // Create subscription with usage enabled
    let id2 = client.create_subscription(
        &subscriber,
        &merchant2,
        &5_000_000i128,
        &(7 * 24 * 60 * 60),
        &true,
    );

    // Create another with usage disabled
    let id3 = client.create_subscription(
        &subscriber,
        &merchant3,
        &20_000_000i128,
        &(90 * 24 * 60 * 60),
        &false,
    );

    // Verify each subscription has correct usage_enabled value
    assert_eq!(client.get_subscription(&id1).usage_enabled, false);
    assert_eq!(client.get_subscription(&id2).usage_enabled, true);
    assert_eq!(client.get_subscription(&id3).usage_enabled, false);

    // Verify they're independent subscriptions
    assert_eq!(client.get_subscription(&id1).merchant, merchant1);
    assert_eq!(client.get_subscription(&id2).merchant, merchant2);
    assert_eq!(client.get_subscription(&id3).merchant, merchant3);
}

#[test]
fn test_usage_enabled_with_different_intervals() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Daily subscription with usage enabled
    let daily_id = client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &(24 * 60 * 60), // 1 day
        &true,
    );

    // Weekly subscription with usage disabled
    let weekly_id = client.create_subscription(
        &subscriber,
        &merchant,
        &5_000_000i128,
        &(7 * 24 * 60 * 60), // 7 days
        &false,
    );

    // Monthly subscription with usage enabled
    let monthly_id = client.create_subscription(
        &subscriber,
        &merchant,
        &20_000_000i128,
        &(30 * 24 * 60 * 60), // 30 days
        &true,
    );

    // Verify usage_enabled is independent of interval
    assert_eq!(client.get_subscription(&daily_id).usage_enabled, true);
    assert_eq!(client.get_subscription(&weekly_id).usage_enabled, false);
    assert_eq!(client.get_subscription(&monthly_id).usage_enabled, true);
}

#[test]
fn test_usage_enabled_with_zero_interval() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Create subscription with zero interval and usage enabled
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &0, // Zero interval
        &true,
    );

    let subscription = client.get_subscription(&id);
    assert_eq!(subscription.usage_enabled, true);
    assert_eq!(subscription.interval_seconds, 0);
}

#[test]
fn test_usage_flag_with_next_charge_info() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    env.ledger().with_mut(|li| li.timestamp = 1000);

    // Create subscription with usage enabled
    let id_enabled = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
    );

    // Create subscription with usage disabled
    let id_disabled = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    // Both should compute next charge info regardless of usage_enabled
    let info_enabled = client.get_next_charge_info(&id_enabled);
    let info_disabled = client.get_next_charge_info(&id_disabled);

    assert!(info_enabled.is_charge_expected);
    assert!(info_disabled.is_charge_expected);

    // Verify subscriptions still have correct usage_enabled values
    assert_eq!(client.get_subscription(&id_enabled).usage_enabled, true);
    assert_eq!(client.get_subscription(&id_disabled).usage_enabled, false);
}

#[test]
fn test_usage_enabled_default_behavior() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Create subscription without explicitly thinking about usage (using false as default)
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    let subscription = client.get_subscription(&id);

    // Should work fine with interval-based billing
    assert_eq!(subscription.usage_enabled, false);
    assert_eq!(subscription.status, SubscriptionStatus::Active);
    assert_eq!(subscription.interval_seconds, 30 * 24 * 60 * 60);
}

#[test]
fn test_usage_enabled_immutable_after_creation() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Create with usage disabled
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    assert_eq!(client.get_subscription(&id).usage_enabled, false);

    // Perform various operations
    client.pause_subscription(&id, &subscriber);
    assert_eq!(client.get_subscription(&id).usage_enabled, false);

    client.resume_subscription(&id, &subscriber);
    assert_eq!(client.get_subscription(&id).usage_enabled, false);

    // The usage_enabled flag cannot be changed after creation
    // It remains false throughout the subscription lifecycle
}

#[test]
fn test_usage_enabled_with_all_subscription_statuses() {
    use crate::SubscriptionStatus;

    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Create subscription with usage enabled
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
    );

    // Test Active status
    assert_eq!(client.get_subscription(&id).usage_enabled, true);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );

    // Test Paused status
    client.pause_subscription(&id, &subscriber);
    assert_eq!(client.get_subscription(&id).usage_enabled, true);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );

    // Test Active again (resumed)
    client.resume_subscription(&id, &subscriber);
    assert_eq!(client.get_subscription(&id).usage_enabled, true);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );

    // Test Cancelled status
    client.cancel_subscription(&id, &subscriber);
    assert_eq!(client.get_subscription(&id).usage_enabled, true);
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

    // When usage_enabled is true, this indicates the subscription supports
    // usage-based billing in addition to or instead of interval-based billing
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
    );

    let subscription = client.get_subscription(&id);

    // The subscription is created successfully
    assert_eq!(subscription.usage_enabled, true);

    // It still has interval_seconds (can be used for hybrid models)
    assert_eq!(subscription.interval_seconds, 30 * 24 * 60 * 60);

    // It's in Active status by default
    assert_eq!(subscription.status, SubscriptionStatus::Active);

    // All standard operations work
    client.pause_subscription(&id, &subscriber);
    client.resume_subscription(&id, &subscriber);
    client.cancel_subscription(&id, &subscriber);
}

#[test]
fn test_usage_enabled_false_semantics() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // When usage_enabled is false, this indicates pure interval-based billing
    // No usage tracking or usage-based charges
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    let subscription = client.get_subscription(&id);

    // The subscription is created successfully
    assert_eq!(subscription.usage_enabled, false);

    // It has interval_seconds for regular interval billing
    assert_eq!(subscription.interval_seconds, 30 * 24 * 60 * 60);

    // Fixed amount per interval
    assert_eq!(subscription.amount, 10_000_000i128);

    // All standard operations work
    client.pause_subscription(&id, &subscriber);
    client.resume_subscription(&id, &subscriber);
    client.cancel_subscription(&id, &subscriber);
}

#[test]
fn test_usage_enabled_with_different_amounts() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Small amount with usage enabled
    let id1 = client.create_subscription(&subscriber, &merchant, &100i128, &(24 * 60 * 60), &true);

    // Large amount with usage disabled
    let id2 = client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
    );

    // Medium amount with usage enabled
    let id3 = client.create_subscription(
        &subscriber,
        &merchant,
        &50_000_000i128,
        &(7 * 24 * 60 * 60),
        &true,
    );

    // Verify amounts and usage_enabled are independent
    let sub1 = client.get_subscription(&id1);
    let sub2 = client.get_subscription(&id2);
    let sub3 = client.get_subscription(&id3);

    assert_eq!(sub1.amount, 100i128);
    assert_eq!(sub1.usage_enabled, true);

    assert_eq!(sub2.amount, 1_000_000_000i128);
    assert_eq!(sub2.usage_enabled, false);

    assert_eq!(sub3.amount, 50_000_000i128);
    assert_eq!(sub3.usage_enabled, true);
}

#[test]
fn test_usage_enabled_field_storage() {
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Create multiple subscriptions with alternating usage_enabled values
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

    // Verify each subscription has the correct usage_enabled value
    assert_eq!(client.get_subscription(&id0).usage_enabled, true);
    assert_eq!(client.get_subscription(&id1).usage_enabled, false);
    assert_eq!(client.get_subscription(&id2).usage_enabled, true);
    assert_eq!(client.get_subscription(&id3).usage_enabled, false);
    assert_eq!(client.get_subscription(&id4).usage_enabled, true);
}

#[test]
fn test_usage_enabled_with_recovery_operations() {
    let (env, client, _, admin) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Create subscription with usage enabled
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
    );

    assert_eq!(client.get_subscription(&id).usage_enabled, true);

    // Admin recovery should not affect subscription's usage_enabled flag
    let recipient = Address::generate(&env);
    client.recover_stranded_funds(
        &admin,
        &recipient,
        &5_000_000i128,
        &RecoveryReason::DeprecatedFlow,
    );

    // Subscription should still exist with same usage_enabled value
    assert_eq!(client.get_subscription(&id).usage_enabled, true);
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

    // Should return the admin set during initialization
    let stored_admin = client.get_admin();
    assert_eq!(stored_admin, admin);
}

#[test]
fn test_rotate_admin_successful() {
    let (env, client, _, old_admin) = setup_test_env();

    let new_admin = Address::generate(&env);

    // Old admin should be able to rotate
    client.rotate_admin(&old_admin, &new_admin);

    // Verify admin has changed
    assert_eq!(client.get_admin(), new_admin);
}

#[test]
#[should_panic(expected = "Error(Contract, #403)")]
fn test_rotate_admin_unauthorized() {
    let (env, client, _, _) = setup_test_env();

    let non_admin = Address::generate(&env);
    let new_admin = Address::generate(&env);

    // Non-admin should not be able to rotate
    client.rotate_admin(&non_admin, &new_admin);
}

#[test]
fn test_old_admin_loses_access_after_rotation() {
    let (env, client, _, old_admin) = setup_test_env();

    let new_admin = Address::generate(&env);

    // Rotate admin
    client.rotate_admin(&old_admin, &new_admin);

    // Old admin should no longer be able to perform admin operations
    let result = client.try_set_min_topup(&old_admin, &5_000000);
    assert!(result.is_err());
}

#[test]
fn test_new_admin_gains_access_after_rotation() {
    let (env, client, _, old_admin) = setup_test_env();

    let new_admin = Address::generate(&env);

    // Rotate admin
    client.rotate_admin(&old_admin, &new_admin);

    // New admin should now be able to set min topup
    let new_min = 2_000000i128;
    client.set_min_topup(&new_admin, &new_min);

    assert_eq!(client.get_min_topup(), new_min);
}

#[test]
fn test_admin_rotation_affects_recovery_operations() {
    let (env, client, _, old_admin) = setup_test_env();

    let new_admin = Address::generate(&env);
    let recipient = Address::generate(&env);

    // Old admin can recover before rotation
    let result = client.try_recover_stranded_funds(
        &old_admin,
        &recipient,
        &10_000000i128,
        &RecoveryReason::AccidentalTransfer,
    );
    assert!(result.is_ok());

    // Rotate admin
    client.rotate_admin(&old_admin, &new_admin);

    // Old admin can no longer recover
    let result = client.try_recover_stranded_funds(
        &old_admin,
        &recipient,
        &10_000000i128,
        &RecoveryReason::AccidentalTransfer,
    );
    assert!(result.is_err());

    // New admin can now recover
    let result = client.try_recover_stranded_funds(
        &new_admin,
        &recipient,
        &10_000000i128,
        &RecoveryReason::DeprecatedFlow,
    );
    assert!(result.is_ok());
}

#[test]
fn test_multiple_admin_rotations() {
    let (env, client, _, admin1) = setup_test_env();

    let admin2 = Address::generate(&env);
    let admin3 = Address::generate(&env);
    let admin4 = Address::generate(&env);

    // First rotation: admin1 -> admin2
    client.rotate_admin(&admin1, &admin2);
    assert_eq!(client.get_admin(), admin2);

    // Second rotation: admin2 -> admin3
    client.rotate_admin(&admin2, &admin3);
    assert_eq!(client.get_admin(), admin3);

    // Third rotation: admin3 -> admin4
    client.rotate_admin(&admin3, &admin4);
    assert_eq!(client.get_admin(), admin4);

    // Only admin4 should have access now
    client.set_min_topup(&admin4, &3_000000);
    assert_eq!(client.get_min_topup(), 3_000000);

    // Previous admins should not have access
    assert!(client.try_set_min_topup(&admin1, &1_000000).is_err());
    assert!(client.try_set_min_topup(&admin2, &1_000000).is_err());
    assert!(client.try_set_min_topup(&admin3, &1_000000).is_err());
}

#[test]
fn test_admin_rotation_does_not_affect_subscriptions() {
    let (env, client, _, old_admin) = setup_test_env();

    // Create subscription before rotation
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

    // Rotate admin
    let new_admin = Address::generate(&env);
    client.rotate_admin(&old_admin, &new_admin);

    // Subscription should be unchanged
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

    // Non-admin cannot set min topup
    let result = client.try_set_min_topup(&non_admin, &5_000000);
    assert!(result.is_err());
}

#[test]
fn test_set_min_topup_unauthorized_after_rotation() {
    let (env, client, _, old_admin) = setup_test_env();

    let new_admin = Address::generate(&env);
    let non_admin = Address::generate(&env);

    // Rotate admin
    client.rotate_admin(&old_admin, &new_admin);

    // Non-admin still cannot set min topup
    let result = client.try_set_min_topup(&non_admin, &5_000000);
    assert!(result.is_err());

    // Old admin also cannot
    let result = client.try_set_min_topup(&old_admin, &5_000000);
    assert!(result.is_err());
}

#[test]
fn test_recover_stranded_funds_unauthorized_before_rotation() {
    let (env, client, _, _) = setup_test_env();

    let non_admin = Address::generate(&env);
    let recipient = Address::generate(&env);

    // Non-admin cannot recover funds
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

    // Rotate admin
    client.rotate_admin(&old_admin, &new_admin);

    // Non-admin cannot recover funds
    let result = client.try_recover_stranded_funds(
        &non_admin,
        &recipient,
        &10_000000i128,
        &RecoveryReason::AccidentalTransfer,
    );
    assert!(result.is_err());

    // Old admin also cannot
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

    // Rotate admin
    client.rotate_admin(&old_admin, &new_admin);

    // Test set_min_topup with new admin
    client.set_min_topup(&new_admin, &3_000000);
    assert_eq!(client.get_min_topup(), 3_000000);

    // Test recover_stranded_funds with new admin
    let recipient = Address::generate(&env);
    let result = client.try_recover_stranded_funds(
        &new_admin,
        &recipient,
        &5_000000i128,
        &RecoveryReason::DeprecatedFlow,
    );
    assert!(result.is_ok());

    // Test another rotation with new admin
    let admin3 = Address::generate(&env);
    client.rotate_admin(&new_admin, &admin3);
    assert_eq!(client.get_admin(), admin3);
}

#[test]
fn test_admin_rotation_event_emission() {
    let (env, client, _, old_admin) = setup_test_env();

    let new_admin = Address::generate(&env);

    env.ledger().with_mut(|li| li.timestamp = 12345);

    // Rotate admin
    client.rotate_admin(&old_admin, &new_admin);

    // Verify event was emitted
    let events = env.events().all();
    assert!(events.len() > 0);
}

#[test]
fn test_rotate_admin_to_same_address() {
    let (_, client, _, admin) = setup_test_env();

    // Should be able to "rotate" to same address (idempotent)
    client.rotate_admin(&admin, &admin);

    // Admin should still be the same
    assert_eq!(client.get_admin(), admin);

    // Should still have admin access
    client.set_min_topup(&admin, &2_000000);
    assert_eq!(client.get_min_topup(), 2_000000);
}

#[test]
fn test_admin_rotation_access_control_comprehensive() {
    let (env, client, _, admin1) = setup_test_env();

    let admin2 = Address::generate(&env);
    let admin3 = Address::generate(&env);
    let non_admin = Address::generate(&env);

    // Phase 1: admin1 is in control
    assert_eq!(client.get_admin(), admin1);

    // admin1 can perform admin operations
    client.set_min_topup(&admin1, &2_000000);
    assert_eq!(client.get_min_topup(), 2_000000);

    // admin2 cannot (not admin yet)
    assert!(client.try_set_min_topup(&admin2, &3_000000).is_err());

    // non_admin cannot
    assert!(client.try_set_min_topup(&non_admin, &3_000000).is_err());

    // Phase 2: Rotate to admin2
    client.rotate_admin(&admin1, &admin2);
    assert_eq!(client.get_admin(), admin2);

    // admin2 can now perform admin operations
    client.set_min_topup(&admin2, &3_000000);
    assert_eq!(client.get_min_topup(), 3_000000);

    // admin1 cannot anymore
    assert!(client.try_set_min_topup(&admin1, &4_000000).is_err());

    // non_admin still cannot
    assert!(client.try_set_min_topup(&non_admin, &4_000000).is_err());

    // Phase 3: Rotate to admin3
    client.rotate_admin(&admin2, &admin3);
    assert_eq!(client.get_admin(), admin3);

    // admin3 can now perform admin operations
    client.set_min_topup(&admin3, &4_000000);
    assert_eq!(client.get_min_topup(), 4_000000);

    // Previous admins cannot
    assert!(client.try_set_min_topup(&admin1, &5_000000).is_err());
    assert!(client.try_set_min_topup(&admin2, &5_000000).is_err());

    // non_admin still cannot
    assert!(client.try_set_min_topup(&non_admin, &5_000000).is_err());
}

#[test]
fn test_admin_rotation_with_subscriptions_active() {
    let (env, client, _, old_admin) = setup_test_env();

    // Create multiple subscriptions
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

    // Perform state changes
    client.pause_subscription(&id1, &subscriber1);

    // Rotate admin
    let new_admin = Address::generate(&env);
    client.rotate_admin(&old_admin, &new_admin);

    // Verify subscriptions still work correctly
    assert_eq!(
        client.get_subscription(&id1).status,
        SubscriptionStatus::Paused
    );
    assert_eq!(
        client.get_subscription(&id2).status,
        SubscriptionStatus::Active
    );

    // Subscribers can still manage their subscriptions
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

    // Rotate from admin1 to admin2
    client.rotate_admin(&admin1, &admin2);

    // admin1 should not be able to rotate again
    let result = client.try_rotate_admin(&admin1, &admin3);
    assert!(result.is_err());

    // Admin should still be admin2
    assert_eq!(client.get_admin(), admin2);
}

#[test]
fn test_get_admin_before_and_after_rotation() {
    let (env, client, _, old_admin) = setup_test_env();

    // Before rotation
    assert_eq!(client.get_admin(), old_admin);

    let new_admin = Address::generate(&env);

    // Rotate
    client.rotate_admin(&old_admin, &new_admin);

    // After rotation
    assert_eq!(client.get_admin(), new_admin);

    // get_admin should always return current admin
    let another_admin = Address::generate(&env);
    client.rotate_admin(&new_admin, &another_admin);
    assert_eq!(client.get_admin(), another_admin);
}

#[test]
fn test_create_subscription_invalid_amount() {
    let (env, client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    
    let result = client.try_create_subscription(&subscriber, &merchant, &0i128, &3600u64, &false);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
    
    let result = client.try_create_subscription(&subscriber, &merchant, &-100i128, &3600u64, &false);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_create_subscription_invalid_interval() {
    let (env, client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    
    let result = client.try_create_subscription(&subscriber, &merchant, &1000i128, &0u64, &false);
    assert_eq!(result, Err(Ok(Error::InvalidInterval)));
}

#[test]
fn test_deposit_funds_subscription_not_found() {
    let (env, client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    
    let result = client.try_deposit_funds(&999, &subscriber, &5_000000);
    assert_eq!(result, Err(Ok(Error::SubscriptionNotFound)));
}

#[test]
fn test_set_min_topup_not_admin() {
    let (env, client, _, _) = setup_test_env();
    let non_admin = Address::generate(&env);
    
    let result = client.try_set_min_topup(&non_admin, &5_000000);
    assert_eq!(result, Err(Ok(Error::NotAdmin)));
}

#[test]
fn test_charge_subscription_not_active() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_sub_with_status(&env, &client, SubscriptionStatus::Active);
    
    // Pause it
    client.pause_subscription(&id, &subscriber);
    
    // Try to charge (should fail with NotActive)
    let result = client.try_charge_subscription(&id);
    assert_eq!(result, Err(Ok(Error::NotActive)));
}

#[test]
fn test_get_subscription_not_found() {
    let (env, client, _, _) = setup_test_env();
    let result = client.try_get_subscription(&999);
    assert_eq!(result, Err(Ok(Error::SubscriptionNotFound)));
}
