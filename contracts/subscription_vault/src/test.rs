use crate::{
    can_transition, get_allowed_transitions, validate_status_transition, Error, Subscription,
    SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
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

/// Register a Stellar Asset Contract token, mint `amount` to `recipient`.
/// Returns the token contract address to pass to `SubscriptionVault::init`.
fn create_token_and_mint(env: &Env, recipient: &Address, amount: i128) -> Address {
    let admin = Address::generate(env);
    let token = env.register_stellar_asset_contract_v2(admin.clone());
    let token_admin_client = TokenAdminClient::new(env, &token.address());
    token_admin_client.mint(recipient, &amount);
    token.address()
}

fn setup_test_env() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    // A real Stellar Asset Contract is required so that token::Client::transfer
    // has a WASM-backed contract to invoke during deposit_funds.
    let placeholder = Address::generate(&env);
    let token = create_token_and_mint(&env, &placeholder, 0);
    let admin = Address::generate(&env);
    let min_topup = 1_000000i128; // 1 USDC
    client.init(&token, &admin, &min_topup);

    (env, client, token, admin)
}

fn create_test_subscription(
    env: &Env,
    client: &SubscriptionVaultClient,
    token: &Address,
    status: SubscriptionStatus,
) -> (u32, Address, Address) {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    let amount = 10_000_000i128; // 10 USDC
    let interval_seconds = 30 * 24 * 60 * 60; // 30 days
    let usage_enabled = false;

    // Mint enough tokens to the subscriber for deposits used in this test.
    let token_admin_client = TokenAdminClient::new(env, token);
    token_admin_client.mint(&subscriber, &1_000_000_000i128);

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
    let (env, client, token, _) = setup_test_env();
    let (id, subscriber, _) =
        create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

    // Pause from Active should succeed
    client.pause_subscription(&id, &subscriber);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Paused);
}

#[test]
#[should_panic(expected = "Error(Contract, #400)")]
fn test_pause_subscription_from_cancelled_should_fail() {
    let (env, client, token, _) = setup_test_env();
    let (id, subscriber, _) =
        create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

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
    let (env, client, token, _) = setup_test_env();
    let (id, subscriber, _) =
        create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

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
    let (env, client, token, _) = setup_test_env();
    let (id, subscriber, _) =
        create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

    // Cancel from Active should succeed
    client.cancel_subscription(&id, &subscriber);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

#[test]
fn test_cancel_subscription_from_paused() {
    let (env, client, token, _) = setup_test_env();
    let (id, subscriber, _) =
        create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

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
    let (env, client, token, _) = setup_test_env();
    let (id, subscriber, _) =
        create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

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
    let (env, client, token, _) = setup_test_env();
    let (id, subscriber, _) =
        create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

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
    let (env, client, token, _) = setup_test_env();
    let (id, subscriber, _) =
        create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

    // First cancel
    client.cancel_subscription(&id, &subscriber);

    // Try to resume (should fail)
    client.resume_subscription(&id, &subscriber);
}

#[test]
fn test_state_transition_idempotent_same_status() {
    let (env, client, token, _) = setup_test_env();
    let (id, subscriber, _) =
        create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

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
    let (env, client, token, _) = setup_test_env();
    let (id, subscriber, _) =
        create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

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
    let (env, client, token, _) = setup_test_env();
    let (id, subscriber, _) =
        create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

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
        let (env, client, token, _) = setup_test_env();
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);
        client.pause_subscription(&id, &subscriber);
        assert_eq!(
            client.get_subscription(&id).status,
            SubscriptionStatus::Paused
        );
    }

    // 2. Active -> Cancelled
    {
        let (env, client, token, _) = setup_test_env();
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);
        client.cancel_subscription(&id, &subscriber);
        assert_eq!(
            client.get_subscription(&id).status,
            SubscriptionStatus::Cancelled
        );
    }

    // 3. Active -> InsufficientBalance (simulated via direct storage manipulation)
    {
        let (env, client, token, _) = setup_test_env();
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

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
        let (env, client, token, _) = setup_test_env();
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);
        client.pause_subscription(&id, &subscriber);
        client.resume_subscription(&id, &subscriber);
        assert_eq!(
            client.get_subscription(&id).status,
            SubscriptionStatus::Active
        );
    }

    // 5. Paused -> Cancelled
    {
        let (env, client, token, _) = setup_test_env();
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);
        client.pause_subscription(&id, &subscriber);
        client.cancel_subscription(&id, &subscriber);
        assert_eq!(
            client.get_subscription(&id).status,
            SubscriptionStatus::Cancelled
        );
    }

    // 6. InsufficientBalance -> Active
    {
        let (env, client, token, _) = setup_test_env();
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

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
        let (env, client, token, _) = setup_test_env();
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

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
    let (env, client, token, _) = setup_test_env();
    let (id, subscriber, _) =
        create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

    client.cancel_subscription(&id, &subscriber);
    client.resume_subscription(&id, &subscriber);
}

#[test]
#[should_panic(expected = "Error(Contract, #400)")]
fn test_invalid_insufficient_balance_to_paused() {
    let (env, client, token, _) = setup_test_env();
    let (id, subscriber, _) =
        create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);

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
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    // Register real token and pre-fund subscriber so deposit_funds transfer succeeds.
    let token = create_token_and_mint(env, &subscriber, 100_000_000i128);
    let admin = Address::generate(env);
    client.init(&token, &admin, &1_000000i128);
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

    let subscriber = Address::generate(&env);
    let admin = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
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

    let admin = Address::generate(&env);
    let min_topup = 1_000000i128;

    // Test authorized call
    env.mock_all_auths();

    // Create a subscription so ID 0 exists
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
    client.init(&token, &admin, &min_topup);
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
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let min_topup = 1_000000i128;

    // Create a subscription so ID 0 exists.
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
    client.init(&token, &admin, &min_topup);
    client.create_subscription(&subscriber, &merchant, &1000i128, &3600u64, &false);
    client.deposit_funds(&0, &subscriber, &10_000000i128);
    env.ledger().set_timestamp(3600); // interval elapsed so charge is allowed

    // Mock auth for the admin address only (charge_subscription checks admin auth).
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

    let subscriber = Address::generate(&env);
    let admin = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
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

    let subscriber = Address::generate(&env);
    let admin = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
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
    let (env, client, token, _) = setup_test_env();
    let (id, _, _) = create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);
    let topup = client.estimate_topup_for_intervals(&id, &0);
    assert_eq!(topup, 0);
}

#[test]
fn test_estimate_topup_balance_already_covers_returns_zero() {
    let (env, client, token, _) = setup_test_env();
    let (id, subscriber, _) =
        create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);
    // 10 USDC per interval, deposit 30 USDC, ask for 3 intervals -> required 30, balance 30, topup 0
    client.deposit_funds(&id, &subscriber, &30_000000i128);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.amount, 10_000_000); // from create_test_subscription
    let topup = client.estimate_topup_for_intervals(&id, &3);
    assert_eq!(topup, 0);
}

#[test]
fn test_estimate_topup_insufficient_balance_returns_shortfall() {
    let (env, client, token, _) = setup_test_env();
    let (id, subscriber, _) =
        create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);
    // amount 10_000_000, 3 intervals = 30_000_000 required; deposit 10_000_000 -> topup 20_000_000
    client.deposit_funds(&id, &subscriber, &10_000000i128);
    let topup = client.estimate_topup_for_intervals(&id, &3);
    assert_eq!(topup, 20_000_000);
}

#[test]
fn test_estimate_topup_no_balance_returns_full_required() {
    let (env, client, token, _) = setup_test_env();
    let (id, _, _) = create_test_subscription(&env, &client, &token, SubscriptionStatus::Active);
    // prepaid_balance 0, 5 intervals * 10_000_000 = 50_000_000
    let topup = client.estimate_topup_for_intervals(&id, &5);
    assert_eq!(topup, 50_000_000);
}

#[test]
fn test_estimate_topup_subscription_not_found() {
    let (env, client, token, _) = setup_test_env();
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
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    // Register real token and pre-fund subscriber for two deposit_funds calls.
    let token = create_token_and_mint(env, &subscriber, 100_000_000i128);
    let admin = Address::generate(env);
    client.init(&token, &admin, &1_000000i128);
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
    let subscriber = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 100_000_000i128);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);
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
// deposit_funds tests
// =============================================================================

/// Repeated top-ups: three sequential deposits must accumulate correctly.
#[test]
fn test_deposit_repeated_topups() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let subscriber = Address::generate(&env);
    let admin = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
    let merchant = Address::generate(&env);

    client.init(&token, &admin, &1_000_000i128);
    let id = client.create_subscription(&subscriber, &merchant, &1_000_000i128, &86_400u64, &false);

    let deposit = 5_000_000i128; // 0.5 USDC each
    client.deposit_funds(&id, &subscriber, &deposit);
    client.deposit_funds(&id, &subscriber, &deposit);
    client.deposit_funds(&id, &subscriber, &deposit);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, deposit * 3);
}

/// Invalid subscription ID: deposit on a non-existent ID must return NotFound.
#[test]
fn test_deposit_invalid_id() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let subscriber = Address::generate(&env);
    let admin = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);

    client.init(&token, &admin, &1_000_000i128);

    let result = client.try_deposit_funds(&9_999u32, &subscriber, &5_000_000i128);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

/// Overflow guard: starting near i128::MAX and depositing 2 must trigger Overflow.
#[test]
fn test_deposit_overflow() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let subscriber = Address::generate(&env);
    let admin = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
    let merchant = Address::generate(&env);

    // min_topup must not exceed the deposit amounts we will use.
    client.init(&token, &admin, &1i128);

    let id = client.create_subscription(&subscriber, &merchant, &1i128, &86_400u64, &false);

    // Directly write a near-max balance so a subsequent deposit of 2 overflows i128.
    let near_max: i128 = i128::MAX - 1;
    env.as_contract(&contract_id, || {
        let mut sub: crate::Subscription = env.storage().instance().get(&id).unwrap();
        sub.prepaid_balance = near_max;
        env.storage().instance().set(&id, &sub);
    });

    // Depositing 2 would require i128::MAX + 1 — must return Overflow.
    let result = client.try_deposit_funds(&id, &subscriber, &2i128);
    assert_eq!(result, Err(Ok(Error::Overflow)));
}

/// Cancelled guard: a deposit on a Cancelled subscription must be rejected.
#[test]
fn test_deposit_cancelled_subscription() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let subscriber = Address::generate(&env);
    let admin = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
    let merchant = Address::generate(&env);

    client.init(&token, &admin, &1_000_000i128);
    let id = client.create_subscription(&subscriber, &merchant, &1_000_000i128, &86_400u64, &false);
    client.cancel_subscription(&id, &subscriber);

    let result = client.try_deposit_funds(&id, &subscriber, &5_000_000i128);
    assert_eq!(result, Err(Ok(Error::InvalidStatusTransition)));
}

/// Auto-reactivation: a deposit while InsufficientBalance must atomically set status to Active.
#[test]
fn test_deposit_reactivates_insufficient_balance() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let subscriber = Address::generate(&env);
    let admin = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
    let merchant = Address::generate(&env);

    client.init(&token, &admin, &1_000_000i128);
    let id = client.create_subscription(&subscriber, &merchant, &1_000_000i128, &86_400u64, &false);

    // Force the subscription into InsufficientBalance (simulating a failed charge).
    env.as_contract(&contract_id, || {
        let mut sub: crate::Subscription = env.storage().instance().get(&id).unwrap();
        sub.status = SubscriptionStatus::InsufficientBalance;
        env.storage().instance().set(&id, &sub);
    });

    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::InsufficientBalance
    );

    // A successful deposit must auto-transition the subscription back to Active.
    client.deposit_funds(&id, &subscriber, &5_000_000i128);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(sub.prepaid_balance, 5_000_000i128);
}

/// Authorization enforcement: deposit_funds must panic when the subscriber
/// has not granted authorization, proving require_auth() is enforced before
/// any USDC transfer or state mutation takes place.
#[test]
#[should_panic]
fn test_deposit_requires_subscriber_auth() {
    let env = Env::default();
    // Do NOT call env.mock_all_auths() so that require_auth() is enforced.
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let subscriber = Address::generate(&env);
    let admin = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
    let merchant = Address::generate(&env);

    // Use mock_all_auths only for contract setup, then clear it.
    env.mock_all_auths();
    client.init(&token, &admin, &1_000_000i128);
    let id = client.create_subscription(&subscriber, &merchant, &1_000_000i128, &86_400u64, &false);

    // Clear mocked auth so the next call is evaluated without any granted authorization.
    // An unrecognized caller must not be able to deposit into another subscriber's account.
    env.mock_auths(&[]);

    // This must panic because subscriber has not signed the invocation.
    client.deposit_funds(&id, &subscriber, &5_000_000i128);
}

/// Zero amount: depositing 0 must be rejected regardless of min_topup setting.
#[test]
fn test_deposit_zero_amount() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let subscriber = Address::generate(&env);
    let admin = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
    let merchant = Address::generate(&env);
    client.init(&token, &admin, &1_000_000i128);
    let id = client.create_subscription(&subscriber, &merchant, &1_000_000i128, &86_400u64, &false);

    let result = client.try_deposit_funds(&id, &subscriber, &0i128);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

/// Negative amount: depositing a negative value must be rejected. i128 is signed and
/// a naive caller could pass -1; the guard must catch this before token interaction.
#[test]
fn test_deposit_negative_amount() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let subscriber = Address::generate(&env);
    let admin = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
    let merchant = Address::generate(&env);
    client.init(&token, &admin, &1_000_000i128);
    let id = client.create_subscription(&subscriber, &merchant, &1_000_000i128, &86_400u64, &false);

    let result = client.try_deposit_funds(&id, &subscriber, &-1i128);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

/// Init rejects zero min_topup: a zero floor would allow deposits of any size.
#[test]
fn test_init_rejects_zero_min_topup() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = Address::generate(&env);

    let result = client.try_init(&token, &admin, &0i128);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}
