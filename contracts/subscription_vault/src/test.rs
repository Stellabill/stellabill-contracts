use crate::{
    can_transition, get_allowed_transitions, validate_status_transition, Error, Subscription,
    SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::testutils::{Address as _, Events, Ledger as _};
use soroban_sdk::{Address, Env, IntoVal, TryFromVal, Val, Vec};

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
fn setup_env() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
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
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);
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

fn setup_test_env() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
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
}

#[test]
fn test_init_with_min_topup() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
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
            create_test_subscription(&env, &client, SubscriptionStatus::Active);

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

// ─── Merchant view helper tests ───────────────────────────────────────────────
// ─── Merchant view helper tests ───────────────────────────────────────────────

#[test]
fn test_merchant_with_no_subscriptions() {
    let (env, client, _, _) = setup_env();
    let merchant = Address::generate(&env);

    let subs = client.get_subscriptions_by_merchant(&merchant, &0, &10);
    assert_eq!(subs.len(), 0);

    let count = client.get_merchant_subscription_count(&merchant);
    assert_eq!(count, 0);
}

#[test]
fn test_merchant_with_one_subscription() {
    let (env, client, _, _) = setup_env();
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
    let (env, client, _, _) = setup_env();
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
    let (env, client, _, _) = setup_env();
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
    let (env, client, _, _) = setup_env();
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
    let (env, client, _, _) = setup_env();
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
    let (env, client, _, _) = setup_env();
    let merchant = Address::generate(&env);

    let subscriber = Address::generate(&env);
    create_sub(&env, &client, &subscriber, &merchant, 1_000_000);

    // Start way past the end
    let page = client.get_subscriptions_by_merchant(&merchant, &100, &10);
    assert_eq!(page.len(), 0);
}

#[test]
fn test_multiple_merchants_isolated() {
    let (env, client, _, _) = setup_env();
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
    let (env, client, _, _) = setup_env();
    let merchant = Address::generate(&env);

    assert_eq!(client.get_merchant_subscription_count(&merchant), 0);

    for _ in 0..4 {
        let subscriber = Address::generate(&env);
        create_sub(&env, &client, &subscriber, &merchant, 5_000_000);
    }

    assert_eq!(client.get_merchant_subscription_count(&merchant), 4);
}

// -- Billing interval enforcement tests --------------------------------------

const T0: u64 = 1000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days in seconds

/// Setup env with contract, ledger at T0, and one subscription with given interval_seconds.
/// The subscription has enough prepaid balance for multiple charges (10 USDC).
fn setup(env: &Env, interval_seconds: u64) -> (SubscriptionVaultClient<'static>, u32) {
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    let token = Address::generate(env);
    let admin = Address::generate(env);
    client.init(&token, &admin, &1_000000i128);
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    let id =
        client.create_subscription(&subscriber, &merchant, &1000i128, &interval_seconds, &false);
    client.deposit_funds(&id, &subscriber, &10_000000i128); // 10 USDC so charge can succeed
    (client, id)
}

/// Just-before: charge 1 second before the interval elapses.
/// Must reject with IntervalNotElapsed and leave storage untouched.
#[test]
fn test_charge_rejected_before_interval() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup(&env, INTERVAL);

    // 1 second too early.
    env.ledger().set_timestamp(T0 + INTERVAL - 1);

    let res = client.try_charge_subscription(&id, &None);
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
    let (client, id) = setup(&env, INTERVAL);

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id, &None);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.last_payment_timestamp, T0 + INTERVAL);
}

/// After interval: charge well past the interval boundary.
/// Must succeed and set last_payment_timestamp to the current ledger time.
#[test]
fn test_charge_succeeds_after_interval() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup(&env, INTERVAL);

    let charge_time = T0 + 2 * INTERVAL;
    env.ledger().set_timestamp(charge_time);
    client.charge_subscription(&id, &None);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.last_payment_timestamp, charge_time);
}

// -- Edge cases: boundary timestamps & repeated calls ------------------------
//
// Assumptions about ledger time monotonicity:
//   Soroban ledger timestamps are set by validators and are expected to be
//   non-decreasing across ledger closes (~5-6 s on mainnet). The contract
//   does NOT assume strict monotonicity — it only requires
//   `now >= last_payment_timestamp + interval_seconds`. If a validator were
//   to produce a timestamp equal to the previous ledger's (same second), the
//   charge would simply be rejected as the interval cannot have elapsed in
//   zero additional seconds. The contract never relies on `now > previous_now`.

/// Same-timestamp retry: a second charge at the identical timestamp that
/// succeeded must be rejected because 0 seconds < interval_seconds.
#[test]
fn test_immediate_retry_at_same_timestamp_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup(&env, INTERVAL);

    let t1 = T0 + INTERVAL;
    env.ledger().set_timestamp(t1);
    client.charge_subscription(&id, &None);

    // Retry at the same timestamp — must fail (replay protection), storage stays at t1.
    let res = client.try_charge_subscription(&id, &None);
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
    let (client, id) = setup(&env, INTERVAL);

    for i in 1..=6u64 {
        let charge_time = T0 + i * INTERVAL;
        env.ledger().set_timestamp(charge_time);
        client.charge_subscription(&id, &None);

        let sub = client.get_subscription(&id);
        assert_eq!(sub.last_payment_timestamp, charge_time);
    }

    // One more attempt without advancing time — must fail (replay protection).
    let res = client.try_charge_subscription(&id, &None);
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
    let (client, id) = setup(&env, INTERVAL);
    env.ledger().set_timestamp(T0 + INTERVAL);

    let key = idempotency_key(&env, 1);
    client.charge_subscription(&id, &Some(key.clone()));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.last_payment_timestamp, T0 + INTERVAL);
    assert_eq!(sub.prepaid_balance, 10_000000i128 - 1000i128);
}

/// Repeating the same call with the same idempotency key returns Ok without double-debit.
#[test]
fn test_replay_same_idempotency_key_returns_ok_no_double_debit() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup(&env, INTERVAL);
    env.ledger().set_timestamp(T0 + INTERVAL);

    let key = idempotency_key(&env, 2);
    client.charge_subscription(&id, &Some(key.clone()));
    let balance_after_first = client.get_subscription(&id).prepaid_balance;

    client.charge_subscription(&id, &Some(key));
    let balance_after_second = client.get_subscription(&id).prepaid_balance;

    assert_eq!(balance_after_first, balance_after_second);
}

/// Same period, different idempotency key: second call is rejected as Replay.
#[test]
fn test_replay_different_key_same_period_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup(&env, INTERVAL);
    env.ledger().set_timestamp(T0 + INTERVAL);

    let key1 = idempotency_key(&env, 10);
    client.charge_subscription(&id, &Some(key1));

    let key2 = idempotency_key(&env, 20);
    let res = client.try_charge_subscription(&id, &Some(key2));
    assert_eq!(res, Err(Ok(Error::Replay)));
}

/// New period with new idempotency key succeeds.
#[test]
fn test_replay_new_period_new_key_succeeds() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup(&env, INTERVAL);

    env.ledger().set_timestamp(T0 + INTERVAL);
    let key1 = idempotency_key(&env, 1);
    client.charge_subscription(&id, &Some(key1));

    env.ledger().set_timestamp(T0 + 2 * INTERVAL);
    let key2 = idempotency_key(&env, 2);
    client.charge_subscription(&id, &Some(key2));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.last_payment_timestamp, T0 + 2 * INTERVAL);
    assert_eq!(sub.prepaid_balance, 10_000000i128 - 2000i128);
}

/// Charge without idempotency key still protected by period-based replay.
#[test]
fn test_replay_no_key_still_rejected_same_period() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup(&env, INTERVAL);
    env.ledger().set_timestamp(T0 + INTERVAL);

    client.charge_subscription(&id, &None);
    let res = client.try_charge_subscription(&id, &None);
    assert_eq!(res, Err(Ok(Error::Replay)));
}

/// Minimum interval (1 second): charge at creation time must fail,
/// charge 1 second later must succeed.
#[test]
fn test_one_second_interval_boundary() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup(&env, 1);

    // At creation time — 0 seconds elapsed, interval is 1 s → too early.
    env.ledger().set_timestamp(T0);
    let res = client.try_charge_subscription(&id, &None);
    assert_eq!(res, Err(Ok(Error::IntervalNotElapsed)));

    // Exactly 1 second later — boundary, should succeed.
    env.ledger().set_timestamp(T0 + 1);
    client.charge_subscription(&id, &None);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.last_payment_timestamp, T0 + 1);
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
    let min_topup = 5_000000i128; // 5 USDC

    client.init(&token, &admin, &min_topup);

    let result = client.try_deposit_funds(&0, &subscriber, &4_999999);
    assert!(result.is_err());
}

#[test]
fn test_charge_subscription_auth() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let min_topup = 1_000000i128;
    client.init(&token, &admin, &min_topup);

    // Test authorized call
    env.mock_all_auths();

    // Create a subscription so ID 0 exists
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    client.create_subscription(&subscriber, &merchant, &1000i128, &3600u64, &false);
    client.deposit_funds(&0, &subscriber, &10_000000i128);
    env.ledger().set_timestamp(3600); // interval elapsed so charge is allowed

    client.charge_subscription(&0, &None);
}

#[test]
#[should_panic] // Soroban panic on require_auth failure
fn test_charge_subscription_unauthorized() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let min_topup = 1_000000i128;
    client.init(&token, &admin, &min_topup);

    // Create a subscription so ID 0 exists (using mock_all_auths for setup)
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    env.mock_all_auths();
    client.create_subscription(&subscriber, &merchant, &1000i128, &3600u64, &false);

    let non_admin = Address::generate(&env);

    // Mock auth for the non_admin address (args: subscription_id, idempotency_key)
    let none_key: Option<soroban_sdk::BytesN<32>> = None;
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &non_admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "charge_subscription",
            args: (0u32, none_key).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    client.charge_subscription(&0, &None);
}

#[test]
fn test_charge_subscription_admin() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let min_topup = 1_000000i128;
    client.init(&token, &admin, &min_topup);

    // Create a subscription so ID 0 exists (using mock_all_auths for setup)
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    env.mock_all_auths();
    client.create_subscription(&subscriber, &merchant, &1000i128, &3600u64, &false);
    client.deposit_funds(&0, &subscriber, &10_000000i128);
    env.ledger().set_timestamp(3600); // interval elapsed so charge is allowed

    // Mock auth for the admin address (args: subscription_id, idempotency_key)
    let none_key: Option<soroban_sdk::BytesN<32>> = None;
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "charge_subscription",
            args: (0u32, none_key).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    client.charge_subscription(&0, &None);
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
    let merchant = Address::generate(&env);
    let min_topup = 5_000000i128; // 5 USDC

    client.init(&token, &admin, &min_topup);
    client.create_subscription(&subscriber, &merchant, &1000i128, &86400u64, &false);

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
    let merchant = Address::generate(&env);
    let min_topup = 5_000000i128; // 5 USDC

    client.init(&token, &admin, &min_topup);
    client.create_subscription(&subscriber, &merchant, &1000i128, &86400u64, &false);

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

// =============================================================================
// estimate_topup_for_intervals tests (#28)
// =============================================================================

#[test]
fn test_estimate_topup_zero_intervals_returns_zero() {
    let (env, client, _, _) = setup_test_env();
    let (id, _, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    let topup = client.estimate_topup_for_intervals(&id, &0);
    assert_eq!(topup, 0);
}

#[test]
fn test_estimate_topup_balance_already_covers_returns_zero() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    // 10 USDC per interval, deposit 30 USDC, ask for 3 intervals -> required 30, balance 30, topup 0
    client.deposit_funds(&id, &subscriber, &30_000000i128);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.amount, 10_000_000); // from create_test_subscription
    let topup = client.estimate_topup_for_intervals(&id, &3);
    assert_eq!(topup, 0);
}

#[test]
fn test_estimate_topup_insufficient_balance_returns_shortfall() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    // amount 10_000_000, 3 intervals = 30_000_000 required; deposit 10_000_000 -> topup 20_000_000
    client.deposit_funds(&id, &subscriber, &10_000000i128);
    let topup = client.estimate_topup_for_intervals(&id, &3);
    assert_eq!(topup, 20_000_000);
}

#[test]
fn test_estimate_topup_no_balance_returns_full_required() {
    let (env, client, _, _) = setup_test_env();
    let (id, _, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    // prepaid_balance 0, 5 intervals * 10_000_000 = 50_000_000
    let topup = client.estimate_topup_for_intervals(&id, &5);
    assert_eq!(topup, 50_000_000);
}

#[test]
fn test_estimate_topup_subscription_not_found() {
    let (_env, client, _, _) = setup_test_env();
    let result = client.try_estimate_topup_for_intervals(&9999, &1);
    assert_eq!(result, Err(Ok(Error::NotFound)));
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
    let env = Env::default();
    let (client, _admin, _, _) = setup_batch_env(&env);
    let ids = Vec::new(&env);
    let results = client.batch_charge(&ids);
    assert_eq!(results.len(), 0);
}

#[test]
fn test_batch_charge_all_success() {
    let env = Env::default();
    let (client, _admin, id0, id1) = setup_batch_env(&env);
    let mut ids = Vec::new(&env);
    ids.push_back(id0);
    ids.push_back(id1);
    let results = client.batch_charge(&ids);
    assert_eq!(results.len(), 2);
    assert!(results.get(0).unwrap().success);
    assert!(results.get(1).unwrap().success);
}

#[test]
fn test_batch_charge_partial_failure() {
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
    // id1 has no deposit -> charge will fail with InsufficientBalance
    env.ledger().set_timestamp(T0 + INTERVAL);
    let mut ids = Vec::new(&env);
    ids.push_back(id0);
    ids.push_back(id1);
    let results = client.batch_charge(&ids);
    assert_eq!(results.len(), 2);
    assert!(results.get(0).unwrap().success);
    assert!(!results.get(1).unwrap().success);
    assert_eq!(
        results.get(1).unwrap().error_code,
        Error::InsufficientBalance.to_code()
    );
}

// =============================================================================
// Merchant-initiated one-off charge tests (#30)
// =============================================================================

#[test]
fn test_oneoff_charge_valid_debits_balance() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, merchant) =
        create_test_subscription(&env, &client, SubscriptionStatus::Active);
    client.deposit_funds(&id, &subscriber, &20_000000i128);
    let before = client.get_subscription(&id).prepaid_balance;

    client.charge_one_off(&id, &merchant, &5_000000i128);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, before - 5_000000i128);
}

#[test]
fn test_oneoff_charge_exceeds_balance_fails() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, merchant) =
        create_test_subscription(&env, &client, SubscriptionStatus::Active);
    client.deposit_funds(&id, &subscriber, &3_000000i128);

    let res = client.try_charge_one_off(&id, &merchant, &5_000000i128);
    assert_eq!(res, Err(Ok(Error::InsufficientBalance)));
}

#[test]
fn test_oneoff_charge_wrong_merchant_unauthorized() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _merchant) =
        create_test_subscription(&env, &client, SubscriptionStatus::Active);
    client.deposit_funds(&id, &subscriber, &10_000000i128);
    let other_merchant = Address::generate(&env);

    let res = client.try_charge_one_off(&id, &other_merchant, &1_000000i128);
    assert_eq!(res, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_oneoff_charge_cancelled_subscription_fails() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, merchant) =
        create_test_subscription(&env, &client, SubscriptionStatus::Active);
    client.deposit_funds(&id, &subscriber, &10_000000i128);
    client.cancel_subscription(&id, &subscriber);

    let res = client.try_charge_one_off(&id, &merchant, &1_000000i128);
    assert_eq!(res, Err(Ok(Error::NotActive)));
}

#[test]
fn test_oneoff_charge_paused_subscription_succeeds() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, merchant) =
        create_test_subscription(&env, &client, SubscriptionStatus::Active);
    client.deposit_funds(&id, &subscriber, &20_000000i128);
    client.pause_subscription(&id, &subscriber);

    client.charge_one_off(&id, &merchant, &2_000000i128);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 20_000000i128 - 2_000000i128);
}

#[test]
fn test_oneoff_charge_zero_amount_fails() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, merchant) =
        create_test_subscription(&env, &client, SubscriptionStatus::Active);
    client.deposit_funds(&id, &subscriber, &10_000000i128);

    let res = client.try_charge_one_off(&id, &merchant, &0i128);
    assert_eq!(res, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_oneoff_and_recurring_charge_coexist() {
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
    client.deposit_funds(&id, &subscriber, &15_000000i128);

    client.charge_one_off(&id, &merchant, &3_000000i128);
    assert_eq!(client.get_subscription(&id).prepaid_balance, 12_000000i128);

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id, &None);
    assert_eq!(
        client.get_subscription(&id).prepaid_balance,
        12_000000i128 - 1000i128
    );
}

// =============================================================================
// Multi-merchant and multi-subscriber scenario tests (#40)
// =============================================================================

/// Setup: 2 merchants, 3 subscribers, 5 subscriptions (mixed pairs). All active with deposits.
fn setup_multi_actor(
    env: &Env,
) -> (
    SubscriptionVaultClient<'static>,
    Address,
    [Address; 3],
    [Address; 2],
    Vec<u32>,
) {
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    let token = Address::generate(env);
    let admin = Address::generate(env);
    client.init(&token, &admin, &1_000000i128);

    let merchants = [Address::generate(env), Address::generate(env)];
    let subscribers = [
        Address::generate(env),
        Address::generate(env),
        Address::generate(env),
    ];
    let amount = 1000i128;
    let interval = INTERVAL;
    let mut ids = Vec::new(env);

    // Sub0 -> M0, Sub0 -> M1, Sub1 -> M0, Sub1 -> M1, Sub2 -> M0
    let pairs = [(0usize, 0usize), (0, 1), (1, 0), (1, 1), (2, 0)];
    for (si, mi) in pairs {
        let id = client.create_subscription(
            &subscribers[si],
            &merchants[mi],
            &amount,
            &interval,
            &false,
        );
        client.deposit_funds(&id, &subscribers[si], &20_000000i128);
        ids.push_back(id);
    }
    (client, admin, subscribers, merchants, ids)
}

#[test]
fn test_multi_actor_balances_and_statuses_after_setup() {
    let env = Env::default();
    let (client, _admin, subscribers, merchants, ids) = setup_multi_actor(&env);

    assert_eq!(ids.len(), 5);
    for (i, id) in ids.iter().enumerate() {
        let sub = client.get_subscription(&id);
        assert_eq!(sub.status, SubscriptionStatus::Active);
        assert_eq!(sub.prepaid_balance, 20_000000i128);
        assert_eq!(sub.amount, 1000i128);
        if i < 2 {
            assert_eq!(sub.subscriber, subscribers[0]);
        } else if i < 4 {
            assert_eq!(sub.subscriber, subscribers[1]);
        } else {
            assert_eq!(sub.subscriber, subscribers[2]);
        }
        assert!(sub.merchant == merchants[0] || sub.merchant == merchants[1]);
    }
}

#[test]
fn test_multi_actor_batch_charge_all_then_verify() {
    let env = Env::default();
    let (client, _admin, _subscribers, _merchants, ids) = setup_multi_actor(&env);
    env.ledger().set_timestamp(T0 + INTERVAL);

    let results = client.batch_charge(&ids);
    assert_eq!(results.len(), 5);
    for i in 0..5 {
        assert!(results.get(i).unwrap().success);
    }

    for id in ids.iter() {
        let sub = client.get_subscription(&id);
        assert_eq!(sub.prepaid_balance, 20_000000i128 - 1000i128);
        assert_eq!(sub.last_payment_timestamp, T0 + INTERVAL);
    }
}

#[test]
fn test_multi_actor_oneoff_and_recurring_mixed() {
    let env = Env::default();
    let (client, _admin, _subscribers, merchants, ids) = setup_multi_actor(&env);
    env.ledger().set_timestamp(T0 + INTERVAL);

    let id0 = ids.get(0).unwrap();
    let id1 = ids.get(1).unwrap();
    let id2 = ids.get(2).unwrap();
    client.charge_one_off(&id0, &merchants[0], &5_000000i128);
    client.charge_one_off(&id2, &merchants[0], &3_000000i128);

    client.batch_charge(&ids);

    let sub0 = client.get_subscription(&id0);
    assert_eq!(
        sub0.prepaid_balance,
        20_000000i128 - 5_000000i128 - 1000i128
    );
    let sub2 = client.get_subscription(&id2);
    assert_eq!(
        sub2.prepaid_balance,
        20_000000i128 - 3_000000i128 - 1000i128
    );
    let sub1 = client.get_subscription(&id1);
    assert_eq!(sub1.prepaid_balance, 20_000000i128 - 1000i128);
}

#[test]
fn test_multi_actor_pause_and_resume_subset() {
    let env = Env::default();
    let (client, _admin, subscribers, _merchants, ids) = setup_multi_actor(&env);

    let id0 = ids.get(0).unwrap();
    let id1 = ids.get(1).unwrap();
    let id3 = ids.get(3).unwrap();
    client.pause_subscription(&id0, &subscribers[0]);
    client.pause_subscription(&id3, &subscribers[1]);

    assert_eq!(
        client.get_subscription(&id0).status,
        SubscriptionStatus::Paused
    );
    assert_eq!(
        client.get_subscription(&id3).status,
        SubscriptionStatus::Paused
    );
    assert_eq!(
        client.get_subscription(&id1).status,
        SubscriptionStatus::Active
    );

    client.resume_subscription(&id0, &subscribers[0]);
    assert_eq!(
        client.get_subscription(&id0).status,
        SubscriptionStatus::Active
    );
}

#[test]
fn test_multi_actor_cancel_one_subscription_others_unchanged() {
    let env = Env::default();
    let (client, _admin, subscribers, _merchants, ids) = setup_multi_actor(&env);

    let id_cancel = ids.get(2).unwrap();
    client.cancel_subscription(&id_cancel, &subscribers[1]);

    assert_eq!(
        client.get_subscription(&id_cancel).status,
        SubscriptionStatus::Cancelled
    );
    for (i, id) in ids.iter().enumerate() {
        if i != 2 {
            assert_eq!(
                client.get_subscription(&id).status,
                SubscriptionStatus::Active
            );
        }
    }
}

#[test]
fn test_multi_actor_view_helpers_consistent() {
    let env = Env::default();
    let (client, _admin, _subscribers, _merchants, ids) = setup_multi_actor(&env);

    for id in ids.iter() {
        let sub = client.get_subscription(&id);
        let topup_0 = client.estimate_topup_for_intervals(&id, &0);
        assert_eq!(topup_0, 0);
        let topup_2 = client.estimate_topup_for_intervals(&id, &2);
        let expected = (2 * 1000i128 - sub.prepaid_balance).max(0);
        assert_eq!(topup_2, expected);
    }
}
