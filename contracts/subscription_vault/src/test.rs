use crate::{

    Error, Subscription, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
    MAX_SUBSCRIPTION_ID,
};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, Symbol};

// ── helpers ──────────────────────────────────────────────────────────────────

fn setup_contract(env: &Env) -> (SubscriptionVaultClient, Address, Address) {
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    let token = Address::generate(env);
    let admin = Address::generate(env);
    client.init(&token, &admin, &1_000000i128); // 1 USDC min_topup
    (client, token, admin)
}

fn make_subscription(
    env: &Env,
    client: &SubscriptionVaultClient,
    expiration: Option<u64>,
) -> u32 {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    client.create_subscription(
        &subscriber,
        &merchant,
        &10_000000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &expiration,
    )
}

/// Seed the internal `next_id` counter to an arbitrary value via instance storage.
/// This lets us simulate near-overflow conditions without creating millions of real subscriptions.
fn seed_counter(env: &Env, contract_id: &Address, value: u32) {
    env.as_contract(contract_id, || {
        env.storage()
            .instance()
            .set(&Symbol::new(env, "next_id"), &value);
    });
}

// ── existing tests (updated for new expiration field & _next_id signature) ───

    can_transition, get_allowed_transitions, validate_status_transition, Error,
    Subscription, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
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


    let min_topup = 1_000000i128;

    client.init(&token, &admin);
    

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
        expiration: None,
    };
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(sub.expiration, None);
}

#[test]
fn test_subscription_struct_with_expiration() {
    let env = Env::default();
    let exp_ts: u64 = 1_800_000_000;
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        amount: 10_000_0000,
        interval_seconds: 30 * 24 * 60 * 60,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: 50_000_0000,
        usage_enabled: false,
        expiration: Some(exp_ts),
    };
    assert_eq!(sub.expiration, Some(exp_ts));
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

    let min_topup = 5_000000i128;

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

    let min_topup = 5_000000i128;

    client.init(&token, &admin, &min_topup);

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

    let min_topup = 5_000000i128;

    client.init(&token, &admin, &min_topup);

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


// ── expiration tests ──────────────────────────────────────────────────────────

#[test]
fn test_create_subscription_no_expiration() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let id = make_subscription(&env, &client, None);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.expiration, None);
}

#[test]
fn test_create_subscription_with_expiration() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let exp_ts: u64 = 90 * 24 * 60 * 60;
    let id = make_subscription(&env, &client, Some(exp_ts));
    let sub = client.get_subscription(&id);
    assert_eq!(sub.expiration, Some(exp_ts));
}

#[test]
fn test_charge_expired_subscription() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let exp_ts: u64 = 1_000;
    let id = make_subscription(&env, &client, Some(exp_ts));
    env.ledger().set_timestamp(exp_ts + 1);
    let result = client.try_charge_subscription(&id);
    assert!(
        matches!(result, Err(Ok(Error::SubscriptionExpired))),
        "expected SubscriptionExpired, got {:?}",
        result
    );
}

#[test]
fn test_charge_at_exact_expiration_boundary() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let exp_ts: u64 = 5_000;
    let id = make_subscription(&env, &client, Some(exp_ts));
    env.ledger().set_timestamp(exp_ts);
    let result = client.try_charge_subscription(&id);
    assert!(
        matches!(result, Err(Ok(Error::SubscriptionExpired))),
        "expected SubscriptionExpired at boundary, got {:?}",
        result
    );
}

#[test]
fn test_charge_one_second_before_expiration() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let exp_ts: u64 = 5_000;
    let id = make_subscription(&env, &client, Some(exp_ts));
    env.ledger().set_timestamp(exp_ts - 1);
    let result = client.try_charge_subscription(&id);
    assert!(result.is_ok(), "expected Ok before expiration, got {:?}", result);
}

#[test]
fn test_charge_no_expiration_always_allowed() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let id = make_subscription(&env, &client, None);
    env.ledger().set_timestamp(u64::MAX / 2);
    let result = client.try_charge_subscription(&id);
    assert!(result.is_ok(), "expected Ok for open-ended subscription, got {:?}", result);
}

#[test]
fn test_charge_nonexistent_subscription() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let result = client.try_charge_subscription(&999);
    assert!(
        matches!(result, Err(Ok(Error::NotFound))),
        "expected NotFound, got {:?}",
        result
    );
}

#[test]
fn test_long_running_no_expiration() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let id = make_subscription(&env, &client, None);
    let one_month: u64 = 30 * 24 * 60 * 60;
    for month in 1u64..=60 {
        env.ledger().set_timestamp(month * one_month);
        let result = client.try_charge_subscription(&id);
        assert!(result.is_ok(), "month {} failed: {:?}", month, result);
    }
}

// ── ID hardening tests ────────────────────────────────────────────────────────

/// The very first subscription always receives ID 0.
#[test]
fn test_id_starts_at_zero() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let id = make_subscription(&env, &client, None);
    assert_eq!(id, 0, "first subscription must have ID 0");
}

/// Consecutive subscriptions receive strictly increasing IDs (0, 1, 2, …).
#[test]
fn test_ids_are_monotonically_increasing() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    for expected in 0u32..10 {
        let id = make_subscription(&env, &client, None);
        assert_eq!(id, expected, "expected monotone ID {expected}, got {id}");
    }
}

/// 100 consecutive subscriptions produce 100 pairwise-distinct IDs.
#[test]
fn test_ids_are_unique() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    let mut ids: soroban_sdk::Vec<u32> = soroban_sdk::Vec::new(&env);
    for _ in 0..100 {
        let id = make_subscription(&env, &client, None);
        // Verify the new ID is not already in our collected set.
        assert!(
            !ids.contains(id),
            "duplicate ID detected: {id}"
        );
        ids.push_back(id);
    }
    assert_eq!(ids.len(), 100);
}

/// `get_subscription_count` reflects the total number ever created.
#[test]
fn test_get_subscription_count() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup_contract(&env);
    assert_eq!(client.get_subscription_count(), 0, "count must be 0 before any subscription");
    for expected_count in 1u32..=5 {
        make_subscription(&env, &client, None);
        assert_eq!(
            client.get_subscription_count(),
            expected_count,
            "count mismatch after {expected_count} subscription(s)"
        );
    }
}

/// Allocation at counter = MAX_SUBSCRIPTION_ID - 1 succeeds and returns that value.
#[test]
fn test_id_at_max_minus_one_succeeds() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    // Seed counter to one below the ceiling.
    let high_id = MAX_SUBSCRIPTION_ID - 1;
    seed_counter(&env, &contract_id, high_id);

    let id = make_subscription(&env, &client, None);
    assert_eq!(
        id, high_id,
        "expected ID {high_id} at counter MAX-1, got {id}"
    );
    // Counter should now be at MAX_SUBSCRIPTION_ID.
    assert_eq!(client.get_subscription_count(), MAX_SUBSCRIPTION_ID);
}

/// When the counter is already at MAX_SUBSCRIPTION_ID, allocation returns SubscriptionLimitReached.
#[test]
fn test_id_at_max_returns_limit_reached() {
    let env = Env::default();
    env.mock_all_auths();

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


    // Seed counter directly to the ceiling.
    seed_counter(&env, &contract_id, MAX_SUBSCRIPTION_ID);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let result = client.try_create_subscription(
        &subscriber,
        &merchant,
        &10_000000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &None,
    );
    assert!(
        matches!(result, Err(Ok(Error::SubscriptionLimitReached))),
        "expected SubscriptionLimitReached, got {:?}",
        result
    );
}

/// Repeated calls after the limit is reached all return SubscriptionLimitReached (no wrap).
#[test]
fn test_no_id_reuse_after_limit() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    seed_counter(&env, &contract_id, MAX_SUBSCRIPTION_ID);

    for attempt in 0..5 {
        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);
        let result = client.try_create_subscription(
            &subscriber,
            &merchant,
            &10_000000i128,
            &(30 * 24 * 60 * 60u64),
            &false,
            &None,
        );
        assert!(
            matches!(result, Err(Ok(Error::SubscriptionLimitReached))),
            "attempt {attempt}: expected SubscriptionLimitReached, got {:?}",
            result
        );
        // Counter must remain at MAX — no wrap to 0.
        assert_eq!(
            client.get_subscription_count(),
            MAX_SUBSCRIPTION_ID,
            "counter must not change after limit is reached"
        );
    }
}

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

