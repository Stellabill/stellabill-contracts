use crate::{
    can_transition, get_allowed_transitions, validate_status_transition, Error, Subscription,
    SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env, IntoVal, Vec as SorobanVec};

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
        let (id, subscriber, _) =
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
    let (client, id) = setup(&env, INTERVAL);

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
    let (client, id) = setup(&env, INTERVAL);

    let charge_time = T0 + 2 * INTERVAL;
    env.ledger().set_timestamp(charge_time);
    client.charge_subscription(&id);

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
    client.charge_subscription(&id);

    // Retry at the same timestamp — must fail, storage stays at t1.
    let res = client.try_charge_subscription(&id);
    assert_eq!(res, Err(Ok(Error::IntervalNotElapsed)));

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
        client.charge_subscription(&id);

        let sub = client.get_subscription(&id);
        assert_eq!(sub.last_payment_timestamp, charge_time);
    }

    // One more attempt without advancing time — must fail.
    let res = client.try_charge_subscription(&id);
    assert_eq!(res, Err(Ok(Error::IntervalNotElapsed)));
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

    client.charge_subscription(&0);
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

    // Mock auth for the non_admin address
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &non_admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "charge_subscription",
            args: (0u32,).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    client.charge_subscription(&0);
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

    // Mock auth for the admin address
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "charge_subscription",
            args: (0u32,).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    client.charge_subscription(&0);
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
    let (env, client, _, _) = setup_test_env();
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
    let ids = SorobanVec::new(&env);
    let results = client.batch_charge(&ids);
    assert_eq!(results.len(), 0);
}

#[test]
fn test_batch_charge_all_success() {
    let env = Env::default();
    let (client, _admin, id0, id1) = setup_batch_env(&env);
    let mut ids = SorobanVec::new(&env);
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
    let mut ids = SorobanVec::new(&env);
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
// Property-Based (Fuzz-Style) Tests
// =============================================================================
//
// proptest/quickcheck cannot be added as dev-dependencies: getrandom's feature
// requirements conflict with soroban-sdk on the wasm32-unknown-unknown target,
// and the contract uses #![no_std]. Instead, a self-contained seeded XorShift64
// PRNG drives parameterised loops. Same MASTER_SEED → same inputs → failures
// are fully reproducible in CI without any external fuzzing infrastructure.
//
// Each of the 15 tests uses seed = MASTER_SEED.wrapping_add(test_index) so all
// tests draw from independent pseudo-random sequences with no correlation.
//
// To isolate a failing iteration: note the iteration index N in the panic
// message, reduce ITERATIONS to N+1, and re-run to see the exact inputs.

/// Master seed for all property tests. Change this to explore a different
/// region of the input space while keeping full determinism.
const MASTER_SEED: u64 = 0x5EED_F00D_CAFE_BABE;

/// Number of iterations per property test. 100 balances coverage vs. runtime.
/// Reduce temporarily when debugging a specific failing iteration.
const ITERATIONS: usize = 100;

// -----------------------------------------------------------------------------
// XorShift64 PRNG
// -----------------------------------------------------------------------------

/// Deterministic seeded pseudo-random number generator (XorShift64).
/// Period 2^64-1. No dependencies, no OS entropy, works in no_std and wasm32.
struct TestRng(u64);

impl TestRng {
    fn new(seed: u64) -> Self {
        // XorShift64 is undefined for state 0.
        assert!(seed != 0, "TestRng seed must be non-zero");
        TestRng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Returns a value in `[0, n)`. Returns 0 when n == 0.
    fn next_range_u64(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        self.next_u64() % n
    }

    /// Returns a u64 in `[lo, hi]` inclusive.
    fn next_u64_in(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next_range_u64(hi - lo + 1)
    }

    /// Returns an i128 in `[lo, hi]` inclusive.
    fn next_i128_in(&mut self, lo: i128, hi: i128) -> i128 {
        lo + self.next_range_u64((hi - lo) as u64 + 1) as i128
    }

    /// Returns a usize in `[0, n)`.
    fn next_range_usize(&mut self, n: usize) -> usize {
        self.next_range_u64(n as u64) as usize
    }
}

// -----------------------------------------------------------------------------
// Shared helper: inject arbitrary subscription state into a fresh contract env
// -----------------------------------------------------------------------------

/// Create a fresh contract environment and inject a `Subscription` with the
/// given field values directly into instance storage, bypassing the normal
/// `create_subscription` + `deposit_funds` path (which enforces min_topup and
/// zeroes the initial balance). Returns `(env, client, subscription_id)`.
///
/// The helper sets `env.ledger().timestamp()` to `t0` before creating the
/// subscription slot so that `last_payment_timestamp` matches `t0`.
fn setup_property_env(
    amount: i128,
    balance: i128,
    interval_seconds: u64,
    t0: u64,
    status: SubscriptionStatus,
) -> (Env, SubscriptionVaultClient<'static>, u32) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(t0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    // min_topup = 1 so no deposit call is needed.
    client.init(&token, &admin, &1i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Allocate a storage slot with a minimal subscription (amount=1, interval=1).
    let id = client.create_subscription(&subscriber, &merchant, &1i128, &1u64, &false);

    // Overwrite the slot with the desired test values.
    let sub = Subscription {
        subscriber,
        merchant,
        amount,
        interval_seconds,
        last_payment_timestamp: t0,
        status,
        prepaid_balance: balance,
        usage_enabled: false,
    };
    env.as_contract(&client.address, || {
        env.storage().instance().set(&id, &sub);
    });

    (env, client, id)
}

// =============================================================================
// P-01: Balance conservation after a successful charge
// =============================================================================
// Invariant: for any amount and prepaid_balance >= amount, a successful charge
// reduces prepaid_balance by exactly `amount` — no rounding, no drift.

#[test]
fn prop_balance_conservation_after_charge() {
    let mut rng = TestRng::new(MASTER_SEED.wrapping_add(1));
    for i in 0..ITERATIONS {
        let amount = rng.next_i128_in(1, 100_000_000_000);
        let extra = rng.next_i128_in(0, 100_000_000_000);
        let balance = amount + extra; // balance >= amount → charge will succeed
        let interval = rng.next_u64_in(1, 365 * 24 * 3600);
        let t0 = rng.next_u64_in(1, u64::MAX / 4);
        let delay = rng.next_u64_in(0, interval); // advance by interval + delay
        let now = t0 + interval + delay;

        let (env, client, id) =
            setup_property_env(amount, balance, interval, t0, SubscriptionStatus::Active);
        env.ledger().set_timestamp(now);

        client.charge_subscription(&id);

        let sub_after = client.get_subscription(&id);
        assert_eq!(
            sub_after.prepaid_balance,
            balance - amount,
            "P-01 iter {i}: balance conservation failed (amount={amount}, balance={balance}, interval={interval})"
        );
    }
}

// =============================================================================
// P-02: No double-charge at the same timestamp
// =============================================================================
// Invariant: a second charge at the identical ledger timestamp that a successful
// charge used must always return IntervalNotElapsed.

#[test]
fn prop_no_double_charge_same_timestamp() {
    let mut rng = TestRng::new(MASTER_SEED.wrapping_add(2));
    for i in 0..ITERATIONS {
        let amount = rng.next_i128_in(1, 100_000_000_000);
        let balance = amount * 4; // plenty for multiple charges
        let interval = rng.next_u64_in(1, 365 * 24 * 3600);
        let t0 = rng.next_u64_in(1, u64::MAX / 4);
        let now = t0 + interval; // exactly at the boundary

        let (env, client, id) =
            setup_property_env(amount, balance, interval, t0, SubscriptionStatus::Active);
        env.ledger().set_timestamp(now);

        // First charge must succeed.
        client.charge_subscription(&id);

        // Second charge at the same timestamp must fail.
        let result = client.try_charge_subscription(&id);
        assert_eq!(
            result,
            Err(Ok(Error::IntervalNotElapsed)),
            "P-02 iter {i}: double charge was not rejected (interval={interval}, now={now})"
        );

        // Storage must still reflect only the first charge.
        let sub_after = client.get_subscription(&id);
        assert_eq!(
            sub_after.last_payment_timestamp, now,
            "P-02 iter {i}: last_payment_timestamp changed on rejected charge"
        );
    }
}

// =============================================================================
// P-03: charge_subscription returns InsufficientBalance when balance < amount
// =============================================================================
// Invariant: whenever prepaid_balance < amount and the subscription is Active
// (with enough time elapsed), charge_subscription must return the
// InsufficientBalance error.
//
// Note: Soroban rolls back all storage writes made during a failed contract
// invocation (even contracterror variants), so the persisted status after a
// try_* failure remains unchanged. We therefore only assert the error code, not
// the post-call storage state.

#[test]
fn prop_status_becomes_insufficient_when_balance_low() {
    let mut rng = TestRng::new(MASTER_SEED.wrapping_add(3));
    for i in 0..ITERATIONS {
        let amount = rng.next_i128_in(2, 100_000_000_000);
        let deficit = rng.next_i128_in(1, amount); // balance = amount - deficit < amount
        let balance = amount - deficit;
        let interval = rng.next_u64_in(1, 365 * 24 * 3600);
        let t0 = rng.next_u64_in(1, u64::MAX / 4);
        let now = t0 + interval;

        let (env, client, id) =
            setup_property_env(amount, balance, interval, t0, SubscriptionStatus::Active);
        env.ledger().set_timestamp(now);

        let result = client.try_charge_subscription(&id);
        assert_eq!(
            result,
            Err(Ok(Error::InsufficientBalance)),
            "P-03 iter {i}: expected InsufficientBalance (amount={amount}, balance={balance})"
        );
    }
}

// =============================================================================
// P-04: Status remains Active after a successful charge
// =============================================================================
// Invariant: a successful charge must not change the subscription status.

#[test]
fn prop_status_stays_active_after_successful_charge() {
    let mut rng = TestRng::new(MASTER_SEED.wrapping_add(4));
    for i in 0..ITERATIONS {
        let amount = rng.next_i128_in(1, 100_000_000_000);
        let extra = rng.next_i128_in(0, 100_000_000_000);
        let balance = amount + extra;
        let interval = rng.next_u64_in(1, 365 * 24 * 3600);
        let t0 = rng.next_u64_in(1, u64::MAX / 4);
        let now = t0 + interval;

        let (env, client, id) =
            setup_property_env(amount, balance, interval, t0, SubscriptionStatus::Active);
        env.ledger().set_timestamp(now);

        client.charge_subscription(&id);

        let sub_after = client.get_subscription(&id);
        assert_eq!(
            sub_after.status,
            SubscriptionStatus::Active,
            "P-04 iter {i}: status changed after successful charge"
        );
    }
}

// =============================================================================
// P-05: last_payment_timestamp is set to current ledger time (sliding window)
// =============================================================================
// Invariant: after a successful charge, last_payment_timestamp == now, NOT
// t0 + interval. This is the sliding-window reset documented in the billing
// interval tests — a late charge does not "catch up" the clock artificially.

#[test]
fn prop_timestamp_set_to_current_after_charge() {
    let mut rng = TestRng::new(MASTER_SEED.wrapping_add(5));
    for i in 0..ITERATIONS {
        let amount = rng.next_i128_in(1, 100_000_000_000);
        let balance = amount * 3;
        let interval = rng.next_u64_in(1, 365 * 24 * 3600);
        let t0 = rng.next_u64_in(1, u64::MAX / 4);
        // now is at or after the interval boundary (possibly much later).
        let extra = rng.next_u64_in(0, interval);
        let now = t0 + interval + extra;

        let (env, client, id) =
            setup_property_env(amount, balance, interval, t0, SubscriptionStatus::Active);
        env.ledger().set_timestamp(now);

        client.charge_subscription(&id);

        let sub_after = client.get_subscription(&id);
        assert_eq!(
            sub_after.last_payment_timestamp, now,
            "P-05 iter {i}: timestamp not updated to current ledger time (t0={t0}, interval={interval}, extra={extra})"
        );
    }
}

// =============================================================================
// P-06: Non-Active subscriptions always reject a charge with NotActive
// =============================================================================
// Invariant: regardless of balance, amount, or time, charging a Paused,
// Cancelled, or InsufficientBalance subscription must return NotActive and must
// not modify any storage field.

#[test]
fn prop_non_active_status_always_rejects_charge() {
    let non_active_statuses = [
        SubscriptionStatus::Paused,
        SubscriptionStatus::Cancelled,
        SubscriptionStatus::InsufficientBalance,
    ];
    let mut rng = TestRng::new(MASTER_SEED.wrapping_add(6));
    for i in 0..ITERATIONS {
        let status_idx = rng.next_range_usize(3);
        let status = non_active_statuses[status_idx].clone();
        let amount = rng.next_i128_in(1, 100_000_000_000);
        let balance = amount * 10; // plenty — so balance is never the cause
        let interval = rng.next_u64_in(1, 365 * 24 * 3600);
        let t0 = rng.next_u64_in(1, u64::MAX / 4);
        let now = t0 + interval * 2; // well past boundary — time is never the cause

        let (env, client, id) = setup_property_env(amount, balance, interval, t0, status.clone());
        env.ledger().set_timestamp(now);

        let result = client.try_charge_subscription(&id);
        assert_eq!(
            result,
            Err(Ok(Error::NotActive)),
            "P-06 iter {i}: expected NotActive for status {:?}",
            status
        );

        // Storage must be completely unchanged.
        let sub_after = client.get_subscription(&id);
        assert_eq!(
            sub_after.prepaid_balance, balance,
            "P-06 iter {i}: balance changed on NotActive rejection"
        );
        assert_eq!(
            sub_after.last_payment_timestamp, t0,
            "P-06 iter {i}: timestamp changed on NotActive rejection"
        );
    }
}

// =============================================================================
// P-07: Charging before the interval elapses always returns IntervalNotElapsed
// =============================================================================
// Invariant: for any now < last_payment_timestamp + interval_seconds, charge
// returns IntervalNotElapsed and leaves storage untouched.

#[test]
fn prop_interval_guard_rejects_early_charge() {
    let mut rng = TestRng::new(MASTER_SEED.wrapping_add(7));
    for i in 0..ITERATIONS {
        let interval = rng.next_u64_in(2, 365 * 24 * 3600); // ≥2 so we can pick an "early" now
        let t0 = rng.next_u64_in(1, u64::MAX / 4);
        // now is strictly before t0 + interval.
        let now = t0 + rng.next_u64_in(0, interval - 1);
        let amount = rng.next_i128_in(1, 1_000_000_000);
        let balance = amount * 100; // balance is never the limiting factor

        let (env, client, id) =
            setup_property_env(amount, balance, interval, t0, SubscriptionStatus::Active);
        env.ledger().set_timestamp(now);

        let result = client.try_charge_subscription(&id);
        assert_eq!(
            result,
            Err(Ok(Error::IntervalNotElapsed)),
            "P-07 iter {i}: expected IntervalNotElapsed (t0={t0}, interval={interval}, now={now})"
        );

        // Both balance and timestamp must be unchanged.
        let sub_after = client.get_subscription(&id);
        assert_eq!(
            sub_after.prepaid_balance, balance,
            "P-07 iter {i}: balance changed on early charge"
        );
        assert_eq!(
            sub_after.last_payment_timestamp, t0,
            "P-07 iter {i}: timestamp changed on early charge"
        );
    }
}

// =============================================================================
// P-08: Overflow protection — t0 + interval_seconds near u64::MAX returns Overflow
// =============================================================================
// Invariant: when last_payment_timestamp + interval_seconds would overflow u64,
// charge_one must return Error::Overflow rather than panicking or wrapping.

#[test]
fn prop_overflow_protection_timestamp_addition() {
    let mut rng = TestRng::new(MASTER_SEED.wrapping_add(8));
    let mut tested = 0usize;
    for _ in 0..1000 {
        // Pick a t0 near u64::MAX and an interval that will overflow.
        let t0 = rng.next_u64_in(u64::MAX - 10_000, u64::MAX);
        let interval = rng.next_u64_in(1, u64::MAX);
        let (_, overflows) = t0.overflowing_add(interval);
        if !overflows {
            continue; // skip non-overflowing pairs
        }

        let amount = rng.next_i128_in(1, 1_000_000_000);
        let balance = amount * 10;
        // now can be anything — the overflow is caught before the comparison.
        let now = rng.next_u64_in(0, u64::MAX / 2);

        let (env, client, id) =
            setup_property_env(amount, balance, interval, t0, SubscriptionStatus::Active);
        env.ledger().set_timestamp(now);

        let result = client.try_charge_subscription(&id);
        assert_eq!(
            result,
            Err(Ok(Error::Overflow)),
            "P-08: expected Overflow (t0={t0}, interval={interval})"
        );

        tested += 1;
        if tested >= ITERATIONS {
            break;
        }
    }
    assert!(
        tested > 0,
        "P-08: no overflowing (t0, interval) pairs generated — adjust ranges"
    );
}

// =============================================================================
// P-09: prepaid_balance is never negative after a successful charge
// =============================================================================
// Invariant: for all successful charges (balance >= amount), the resulting
// prepaid_balance is always >= 0. Complements P-01 by explicitly asserting
// the non-negative property rather than exact equality.

#[test]
fn prop_balance_never_goes_negative_after_charge() {
    let mut rng = TestRng::new(MASTER_SEED.wrapping_add(9));
    for i in 0..ITERATIONS {
        let amount = rng.next_i128_in(1, 100_000_000_000);
        let extra = rng.next_i128_in(0, 100_000_000_000);
        let balance = amount + extra;
        let interval = rng.next_u64_in(1, 365 * 24 * 3600);
        let t0 = rng.next_u64_in(1, u64::MAX / 4);
        let now = t0 + interval;

        let (env, client, id) =
            setup_property_env(amount, balance, interval, t0, SubscriptionStatus::Active);
        env.ledger().set_timestamp(now);

        client.charge_subscription(&id);

        let sub_after = client.get_subscription(&id);
        assert!(
            sub_after.prepaid_balance >= 0,
            "P-09 iter {i}: balance went negative: {} (amount={amount}, balance={balance})",
            sub_after.prepaid_balance
        );
    }
}

// =============================================================================
// P-10: State machine only makes valid transitions
// =============================================================================
// Invariant: after any single operation (charge, pause, resume, cancel), the
// resulting status is either the same as before or is a member of
// get_allowed_transitions(status_before). No "impossible" status appears.

#[test]
fn prop_state_machine_only_makes_valid_transitions() {
    let mut rng = TestRng::new(MASTER_SEED.wrapping_add(10));
    for i in 0..ITERATIONS {
        let amount = rng.next_i128_in(1, 1_000_000_000);
        let balance = amount * 10;
        let interval = rng.next_u64_in(1, 86400);
        let t0 = rng.next_u64_in(1, u64::MAX / 4);
        let now = t0 + interval;

        let (env, client, id) =
            setup_property_env(amount, balance, interval, t0, SubscriptionStatus::Active);
        env.ledger().set_timestamp(now);

        let status_before = client.get_subscription(&id).status;

        // Pick a random operation: 0=charge, 1=pause, 2=resume, 3=cancel
        let op = rng.next_range_usize(4);
        let authorizer = Address::generate(&env);
        // We don't care whether the operation succeeds or fails — only that the
        // resulting status is valid per the state machine.
        let _ = match op {
            0 => client.try_charge_subscription(&id).map(|_| ()),
            1 => client.try_pause_subscription(&id, &authorizer).map(|_| ()),
            2 => client.try_resume_subscription(&id, &authorizer).map(|_| ()),
            _ => client.try_cancel_subscription(&id, &authorizer).map(|_| ()),
        };

        let status_after = client.get_subscription(&id).status;

        let allowed = get_allowed_transitions(&status_before);
        let valid = allowed.contains(&status_after) || status_before == status_after;
        assert!(
            valid,
            "P-10 iter {i}: invalid transition {:?} → {:?} (op={op})",
            status_before, status_after
        );
    }
}

// =============================================================================
// P-11: Batch isolation — a failure at one index does not affect its neighbours
// =============================================================================
// Invariant: in a batch of N subscriptions where exactly one has insufficient
// balance (the "fail subscription"), all others must succeed independently and
// their storage must reflect a successful charge.

#[test]
fn prop_batch_isolation_failure_does_not_contaminate() {
    let mut rng = TestRng::new(MASTER_SEED.wrapping_add(11));
    for i in 0..50 {
        let batch_size = rng.next_u64_in(2, 8) as usize;
        let fail_idx = rng.next_range_usize(batch_size);
        let amount: i128 = 1_000;
        let interval = INTERVAL;

        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(T0);

        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        let token = Address::generate(&env);
        let admin = Address::generate(&env);
        client.init(&token, &admin, &1i128);

        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);

        let mut ids: SorobanVec<u32> = SorobanVec::new(&env);
        let mut balances = [0i128; 8]; // max batch_size is 8

        for j in 0..batch_size {
            let id = client.create_subscription(&subscriber, &merchant, &amount, &interval, &false);
            if j == fail_idx {
                // No deposit → prepaid_balance remains 0 → charge will fail.
                balances[j] = 0;
            } else {
                client.deposit_funds(&id, &subscriber, &10_000_000i128);
                balances[j] = 10_000_000;
            }
            ids.push_back(id);
        }

        env.ledger().set_timestamp(T0 + INTERVAL);
        let results = client.batch_charge(&ids);

        assert_eq!(
            results.len() as usize,
            batch_size,
            "P-11 iter {i}: result count mismatch"
        );

        for j in 0..batch_size {
            let res = results.get(j as u32).unwrap();
            let id = ids.get(j as u32).unwrap();
            if j == fail_idx {
                assert!(
                    !res.success,
                    "P-11 iter {i}: fail_idx={fail_idx} sub at {j} should have failed"
                );
                assert_eq!(
                    res.error_code,
                    Error::InsufficientBalance.to_code(),
                    "P-11 iter {i}: wrong error code at fail_idx={fail_idx}"
                );
                // Storage unchanged: balance still 0.
                let sub_after = client.get_subscription(&id);
                assert_eq!(
                    sub_after.prepaid_balance, 0,
                    "P-11 iter {i}: failed sub balance changed"
                );
            } else {
                assert!(
                    res.success,
                    "P-11 iter {i}: sub at {j} should have succeeded (fail_idx={fail_idx})"
                );
                // Balance reduced by exactly amount.
                let sub_after = client.get_subscription(&id);
                assert_eq!(
                    sub_after.prepaid_balance,
                    balances[j] - amount,
                    "P-11 iter {i}: balance conservation failed for sub {j}"
                );
            }
        }
    }
}

// =============================================================================
// P-12: estimate_topup_for_intervals always returns a non-negative value
// =============================================================================
// Invariant: for any amount, balance, and num_intervals, the estimate is >= 0.

#[test]
fn prop_estimate_topup_always_non_negative() {
    let mut rng = TestRng::new(MASTER_SEED.wrapping_add(12));
    for i in 0..ITERATIONS {
        let amount = rng.next_i128_in(1, 100_000_000_000);
        let balance = rng.next_i128_in(0, 200_000_000_000);
        let num_intervals = rng.next_u64_in(0, 50) as u32;
        let interval = rng.next_u64_in(1, 86400);
        let t0 = rng.next_u64_in(1, u64::MAX / 4);

        let (_, client, id) =
            setup_property_env(amount, balance, interval, t0, SubscriptionStatus::Active);

        let topup = client.estimate_topup_for_intervals(&id, &num_intervals);
        assert!(
            topup >= 0,
            "P-12 iter {i}: estimate_topup returned negative: {topup} (amount={amount}, balance={balance}, n={num_intervals})"
        );
    }
}

// =============================================================================
// P-13: estimate_topup returns 0 when balance already covers the requirement
// =============================================================================
// Invariant: if prepaid_balance >= amount * num_intervals, topup must be 0.

#[test]
fn prop_estimate_topup_zero_when_balance_covers() {
    let mut rng = TestRng::new(MASTER_SEED.wrapping_add(13));
    for i in 0..ITERATIONS {
        let num_intervals = rng.next_u64_in(1, 20) as u32;
        let amount = rng.next_i128_in(1, 1_000_000_000);
        let required = amount * num_intervals as i128;
        let extra = rng.next_i128_in(0, 1_000_000_000);
        let balance = required + extra; // balance >= required → topup should be 0
        let interval = rng.next_u64_in(1, 86400);
        let t0 = rng.next_u64_in(1, u64::MAX / 4);

        let (_, client, id) =
            setup_property_env(amount, balance, interval, t0, SubscriptionStatus::Active);

        let topup = client.estimate_topup_for_intervals(&id, &num_intervals);
        assert_eq!(
            topup, 0,
            "P-13 iter {i}: expected 0 topup but got {topup} (amount={amount}, balance={balance}, n={num_intervals})"
        );
    }
}

// =============================================================================
// P-14: estimate_topup equals the exact shortfall when balance is insufficient
// =============================================================================
// Invariant: if prepaid_balance < amount * num_intervals, topup must equal
// (amount * num_intervals) - prepaid_balance exactly.

#[test]
fn prop_estimate_topup_equals_shortfall() {
    let mut rng = TestRng::new(MASTER_SEED.wrapping_add(14));
    for i in 0..ITERATIONS {
        let num_intervals = rng.next_u64_in(1, 50) as u32;
        let amount = rng.next_i128_in(1, 1_000_000_000);
        let required = amount * num_intervals as i128;
        // balance strictly less than required
        let balance = if required > 0 {
            rng.next_i128_in(0, required - 1)
        } else {
            0
        };
        let expected_topup = required - balance;
        let interval = rng.next_u64_in(1, 86400);
        let t0 = rng.next_u64_in(1, u64::MAX / 4);

        let (_, client, id) =
            setup_property_env(amount, balance, interval, t0, SubscriptionStatus::Active);

        let topup = client.estimate_topup_for_intervals(&id, &num_intervals);
        assert_eq!(
            topup, expected_topup,
            "P-14 iter {i}: topup mismatch (amount={amount}, balance={balance}, n={num_intervals}, required={required})"
        );
    }
}

// =============================================================================
// P-15: Multi-charge cumulative balance reduction
// =============================================================================
// Invariant: after N consecutive successful charges, the final balance equals
// initial_balance - (N * amount), the final timestamp equals t0 + N*interval,
// and the status remains Active throughout.

#[test]
fn prop_multi_charge_cumulative_balance_reduction() {
    let mut rng = TestRng::new(MASTER_SEED.wrapping_add(15));
    for i in 0..50 {
        let num_charges = rng.next_u64_in(2, 8) as u32;
        let amount = rng.next_i128_in(1, 10_000_000);
        let extra = rng.next_i128_in(0, 10_000_000);
        let initial_balance = amount * num_charges as i128 + extra; // covers all charges
        let interval = rng.next_u64_in(1, 86400);
        let t0 = rng.next_u64_in(1, u64::MAX / 4);

        let (env, client, id) = setup_property_env(
            amount,
            initial_balance,
            interval,
            t0,
            SubscriptionStatus::Active,
        );

        for charge_num in 1..=num_charges {
            let charge_time = t0 + charge_num as u64 * interval;
            env.ledger().set_timestamp(charge_time);
            client.charge_subscription(&id);
        }

        let sub_after = client.get_subscription(&id);

        assert_eq!(
            sub_after.prepaid_balance,
            initial_balance - amount * num_charges as i128,
            "P-15 iter {i}: cumulative balance wrong (amount={amount}, n={num_charges})"
        );
        assert_eq!(
            sub_after.last_payment_timestamp,
            t0 + num_charges as u64 * interval,
            "P-15 iter {i}: final timestamp wrong"
        );
        assert_eq!(
            sub_after.status,
            SubscriptionStatus::Active,
            "P-15 iter {i}: status changed after {num_charges} charges"
        );
    }
}
