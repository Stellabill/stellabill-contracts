use crate::{
    can_transition, get_allowed_transitions, validate_status_transition, DataKey, Error,
    RecoveryReason, Subscription, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
use soroban_sdk::{Address, Env, IntoVal, Vec as SorobanVec};

/// Baseline creation timestamp used by test helpers.
const T0: u64 = 1_000;
/// Default billing interval for tests (30 days in seconds).
const INTERVAL: u64 = 30 * 24 * 60 * 60;
/// Large mint amount used to fund batch tests.
const BATCH_MINT: i128 = 1_000_000_000;

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
        &None,
    );

    // Manually set status if not Active (bypassing state machine for test setup)
    // Note: In production, this would go through proper transitions
    if status != SubscriptionStatus::Active {
        let mut sub = client.get_subscription(&id);
        sub.status = status;
        env.as_contract(&client.address, || {
            env.storage().instance().set(&DataKey::Sub(id), &sub);
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

        let mut sub = client.get_subscription(&id);
        sub.status = SubscriptionStatus::InsufficientBalance;
        env.as_contract(&client.address, || {
            env.storage().instance().set(&DataKey::Sub(id), &sub);
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

        let mut sub = client.get_subscription(&id);
        sub.status = SubscriptionStatus::InsufficientBalance;
        env.as_contract(&client.address, || {
            env.storage().instance().set(&DataKey::Sub(id), &sub);
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

        let mut sub = client.get_subscription(&id);
        sub.status = SubscriptionStatus::InsufficientBalance;
        env.as_contract(&client.address, || {
            env.storage().instance().set(&DataKey::Sub(id), &sub);
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

    let mut sub = client.get_subscription(&id);
    sub.status = SubscriptionStatus::InsufficientBalance;
    env.as_contract(&client.address, || {
        env.storage().instance().set(&DataKey::Sub(id), &sub);
    });

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
    let id = client.create_subscription(&subscriber, &merchant, &10_000000i128, &(86400), &true);

    client.cancel_subscription(&id, &merchant);

    let result = client.try_deposit_funds(&id, &subscriber, &4_999999);
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
        &None,
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
        &None,
    );

    let result = client.try_deposit_funds(&id, &subscriber, &deposit_amount);
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
        &None,
    );

    let mut sub = client.get_subscription(&id);
    sub.prepaid_balance = PREPAID;
    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::Sub(id), &sub);
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
        &None,
    );

    let mut sub = client.get_subscription(&id);
    sub.prepaid_balance = PREPAID;
    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::Sub(id), &sub);
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
    // Use the regular setup helper which creates usage_enabled = false.
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
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval_seconds,
        &false,
        &None,
    );

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
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval_seconds,
        &false,
        &None,
    );

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
    let interval_seconds = 7 * 24 * 60 * 60; // 7 days

    env.ledger().with_mut(|li| li.timestamp = 2000);

    // Create subscription
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval_seconds,
        &false,
        &None,
    );

    let mut sub = client.get_subscription(&id);
    sub.status = SubscriptionStatus::InsufficientBalance;
    env.as_contract(&client.address, || {
        env.storage().instance().set(&DataKey::Sub(id), &sub);
    });

    // Get next charge info
    let info = client.get_next_charge_info(&id);

    // InsufficientBalance: charge IS expected (will retry after funding)
    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, 2000 + interval_seconds);
}

#[test]
#[should_panic(expected = "Error(Contract, #404)")]
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
        &None,
    );

    // Weekly subscription
    env.ledger().with_mut(|li| li.timestamp = 20000);
    let weekly_id = client.create_subscription(
        &subscriber,
        &merchant,
        &5_000_000i128,
        &(7 * 24 * 60 * 60), // 7 days
        &false,
        &None,
    );

    // Monthly subscription
    env.ledger().with_mut(|li| li.timestamp = 30000);
    let monthly_id = client.create_subscription(
        &subscriber,
        &merchant,
        &20_000_000i128,
        &(30 * 24 * 60 * 60), // 30 days
        &false,
        &None,
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

    // Setup mock token
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

    // Mint some to the subscriber
    token_admin.mint(&subscriber, &5000);

    let sub_id = client.create_subscription(&subscriber, &merchant, &1000, &86400, &true);

    // Deposit funds to increase prepaid balance
    client.deposit_funds(&sub_id, &subscriber, &5000);

    // Cancel subscription
    client.cancel_subscription(&sub_id, &subscriber);

    // Withdraw funds
    client.withdraw_subscriber_funds(&sub_id, &subscriber);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 0);
    assert_eq!(token.balance(&subscriber), 5000); // 5000 minted - 5000 deposited + 5000 withdrawn
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

    // Should fail: caller is not admin
    client.recover_stranded_funds(&non_admin, &recipient, &amount, &reason);
}

#[test]
#[should_panic(expected = "Error(Contract, #406)")]
fn test_recover_stranded_funds_zero_amount() {
    let (_, client, _, admin) = setup_test_env();

    let recipient = Address::generate(admin.env());
    let amount = 0i128; // Invalid: zero amount
    let reason = RecoveryReason::DeprecatedFlow;

    // Should fail: amount must be positive
    client.recover_stranded_funds(&admin, &recipient, &amount, &reason);
}

#[test]
#[should_panic(expected = "Error(Contract, #406)")]
fn test_recover_stranded_funds_negative_amount() {
    let (_, client, _, admin) = setup_test_env();

    let recipient = Address::generate(admin.env());
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
    assert!(!events.is_empty());

    // The event should contain recovery information
    // Note: Event details verification depends on SDK version
}

#[test]
fn test_recover_stranded_funds_large_amount() {
    let (_, client, _, admin) = setup_test_env();

    let recipient = Address::generate(admin.env());
    let amount = 1_000_000_000_000i128; // 1 million USDC (with 6 decimals)
    let reason = RecoveryReason::DeprecatedFlow;

    // Should handle large amounts
    let result = client.try_recover_stranded_funds(&admin, &recipient, &amount, &reason);
    assert!(result.is_ok());
}

#[test]
fn test_recover_stranded_funds_small_amount() {
    let (_, client, _, admin) = setup_test_env();

    let recipient = Address::generate(admin.env());
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
    assert!(!events.is_empty());
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
    assert!(!events.is_empty());
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
        &None,
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
        &None,
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
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(env, &token);
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    client.init(&token, &admin, &1_000000i128);
    token_admin.mint(&subscriber, &100_000000i128);
    let id0 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id0, &subscriber, &10_000000i128);
    let id1 =
        client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false, &None);
    env.ledger().set_timestamp(T0 + INTERVAL);
    (client, admin, id0, id1)
}

fn mint_for_subscriber(env: &Env, token_addr: &Address, subscriber: &Address, amount: i128) {
    let token_admin = soroban_sdk::token::StellarAssetClient::new(env, token_addr);
    token_admin.mint(subscriber, &amount);
}

fn setup_batch_token_for_test(
    env: &Env,
    client: &SubscriptionVaultClient,
    admin: &Address,
    subscriber: &Address,
) -> Address {
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token_addr, &7, admin, &1_000000i128, &43200);
    mint_for_subscriber(env, &token_addr, subscriber, BATCH_MINT);
    token_addr
}

// -----------------------------------------------------------------------------

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
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);
    let mut ids = SorobanVec::<u32>::new(&env);

    // Create 5 subscriptions with sufficient balance
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
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);
    let mut ids = SorobanVec::<u32>::new(&env);

    // Create 20 subscriptions
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
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);
    let mut ids = SorobanVec::<u32>::new(&env);

    // Create 50 subscriptions to test scalability
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
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);
    let mut ids = SorobanVec::<u32>::new(&env);

    // Create alternating pattern: funded, unfunded, funded, unfunded
    for i in 0..4 {
        let id =
            client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false, &None);
        if i % 2 == 0 {
            client.deposit_funds(&id, &subscriber, &10_000000i128);
        }
        // Odd indices have no funds
        ids.push_back(id as u32);
    }

    env.ledger().set_timestamp(T0 + INTERVAL);
    let results = client.batch_charge(&ids);

    assert_eq!(results.len(), 4);
    // Even indices should succeed
    assert!(results.get(0).unwrap().success);
    assert!(results.get(2).unwrap().success);
    // Odd indices should fail with InsufficientBalance
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
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);

    // Create subscriptions with different intervals
    let id_short =
        client.create_subscription(&subscriber, &merchant, &1000i128, &1800, &false, &None); // 30 min
    let id_long =
        client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false, &None); // 30 days

    client.deposit_funds(&id_short, &subscriber, &10_000000i128);
    client.deposit_funds(&id_long, &subscriber, &10_000000i128);

    // Advance time only enough for short interval
    env.ledger().set_timestamp(T0 + 1800);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id_short);
    ids.push_back(id_long);

    let results = client.batch_charge(&ids);

    assert_eq!(results.len(), 2);
    assert!(results.get(0).unwrap().success); // Short interval elapsed
    assert!(!results.get(1).unwrap().success); // Long interval not elapsed
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
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);

    let id0 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id0, &subscriber, &10_000000i128);

    let id1 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id1, &subscriber, &10_000000i128);
    client.pause_subscription(&id1, &subscriber); // Pause this one

    env.ledger().set_timestamp(T0 + INTERVAL);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id0 as u32);
    ids.push_back(id1 as u32);

    let results = client.batch_charge(&ids);

    assert_eq!(results.len(), 2);
    assert!(results.get(0).unwrap().success); // Active subscription charges
    assert!(!results.get(1).unwrap().success); // Paused subscription fails
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
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);

    let id0 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id0, &subscriber, &10_000000i128);

    let id1 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id1, &subscriber, &10_000000i128);
    client.cancel_subscription(&id1, &subscriber); // Cancel this one

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
    ids.push_back(id0 as u32); // Valid
    ids.push_back(9999); // Nonexistent
    ids.push_back(8888); // Nonexistent

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
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);

    // Sub 0: Success case
    let id_success =
        client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id_success, &subscriber, &10_000000i128);

    // Sub 1: Insufficient balance
    let id_no_funds =
        client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false, &None);

    // Sub 2: Paused
    let id_paused =
        client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id_paused, &subscriber, &10_000000i128);
    client.pause_subscription(&id_paused, &subscriber);

    // Advance time for eligible subscriptions
    env.ledger().set_timestamp(T0 + INTERVAL);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id_success);
    ids.push_back(id_no_funds);
    ids.push_back(9999); // NotFound
    ids.push_back(id_paused);

    let results = client.batch_charge(&ids);

    assert_eq!(results.len(), 4);

    // Verify each specific error
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
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);
    let charge_amount = 1_000_000i128; // 1 USDC

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &charge_amount,
        &INTERVAL,
        &false,
        &None,
    );
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
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);

    let id =
        client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false, &None);
    // No deposit - will fail with InsufficientBalance

    let sub_before = client.get_subscription(&id);

    env.ledger().set_timestamp(T0 + INTERVAL);
    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id as u32);

    let results = client.batch_charge(&ids);
    assert!(!results.get(0).unwrap().success);

    let sub_after = client.get_subscription(&id);
    // State should be unchanged
    assert_eq!(sub_after.prepaid_balance, sub_before.prepaid_balance);
    assert_eq!(
        sub_after.last_payment_timestamp,
        sub_before.last_payment_timestamp
    );
    // Status changes to InsufficientBalance when charge fails due to insufficient funds
    assert_eq!(sub_after.status, SubscriptionStatus::InsufficientBalance);
}

#[test]
fn test_batch_charge_partial_batch_correct_final_state() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);
    let amount = 1_000_000i128;

    let id0 = client.create_subscription(&subscriber, &merchant, &amount, &INTERVAL, &false);
    client.deposit_funds(&id0, &subscriber, &10_000_000i128);

    let id1 = client.create_subscription(&subscriber, &merchant, &amount, &INTERVAL, &false, &None);
    // id1 has no funds - will fail

    let id2 = client.create_subscription(&subscriber, &merchant, &amount, &INTERVAL, &false);
    client.deposit_funds(&id2, &subscriber, &10_000_000i128);

    env.ledger().set_timestamp(T0 + INTERVAL);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id0 as u32);
    ids.push_back(id1 as u32);
    ids.push_back(id2 as u32);

    let results = client.batch_charge(&ids);

    // Verify results
    assert!(results.get(0).unwrap().success);
    assert!(!results.get(1).unwrap().success);
    assert!(results.get(2).unwrap().success);

    // Verify final states
    let sub0 = client.get_subscription(&id0);
    assert_eq!(sub0.prepaid_balance, 9_000_000i128); // Charged
    assert_eq!(sub0.last_payment_timestamp, T0 + INTERVAL);

    let sub1 = client.get_subscription(&id1);
    assert_eq!(sub1.prepaid_balance, 0); // Unchanged (failed)
    assert_eq!(sub1.last_payment_timestamp, T0); // Unchanged

    let sub2 = client.get_subscription(&id2);
    assert_eq!(sub2.prepaid_balance, 9_000_000i128); // Charged
    assert_eq!(sub2.last_payment_timestamp, T0 + INTERVAL);
}

#[test]
fn test_batch_charge_multiple_rounds_state_consistency() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);
    let amount = 1_000_000i128;

    let id = client.create_subscription(&subscriber, &merchant, &amount, &INTERVAL, &false);
    client.deposit_funds(&id, &subscriber, &10_000_000i128);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id as u32);

    // Charge 3 times over 3 intervals
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
#[should_panic] // Auth failure causes panic in Soroban tests
fn test_batch_charge_requires_admin_auth() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);
    let id = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);

    let non_admin = Address::generate(&env);

    // Mock auth for non-admin (should fail)
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
    ids.push_back(id0 as u32); // Duplicate
    ids.push_back(id0 as u32); // Duplicate

    let results = client.batch_charge(&ids);

    // First should succeed
    assert_eq!(results.len(), 3);
    assert!(results.get(0).unwrap().success);

    // Duplicates should fail because interval hasn't elapsed again
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
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);
    let amount = 5_000_000i128;

    let id = client.create_subscription(&subscriber, &merchant, &amount, &INTERVAL, &false);
    client.deposit_funds(&id, &subscriber, &amount); // Exact amount for one charge

    env.ledger().set_timestamp(T0 + INTERVAL);

    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id as u32);

    let results = client.batch_charge(&ids);
    assert!(results.get(0).unwrap().success);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 0); // Exactly exhausted
}

#[test]
fn test_batch_charge_balance_off_by_one_insufficient() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);
    let amount = 5_000_000i128;

    let id = client.create_subscription(&subscriber, &merchant, &amount, &INTERVAL, &false);
    client.deposit_funds(&id, &subscriber, &(amount - 1)); // One stroops short

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
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&subscriber, &1_000_000_000i128);

    let id0 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id0, &subscriber, &10_000000i128);

    let id1 =
        client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false, &None);
    // No funds for id1

    let id2 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    client.deposit_funds(&id2, &subscriber, &10_000000i128);

    env.ledger().set_timestamp(T0 + INTERVAL);

    // Test specific order: id2, id0, id1
    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id2 as u32);
    ids.push_back(id0 as u32);
    ids.push_back(id1 as u32);

    let results = client.batch_charge(&ids);
    assert_eq!(results.len(), 3);
    assert!(results.get(0).unwrap().success); // id2
    assert!(results.get(1).unwrap().success); // id0
    assert!(!results.get(2).unwrap().success); // id1
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
    assert!(!events.is_empty());
}

#[test]
fn test_recover_stranded_funds_edge_case_max_i128() {
    let (_, client, _, admin) = setup_test_env();

    let recipient = Address::generate(admin.env());
    // Test near max i128 value
    let amount = i128::MAX - 1000;
    let reason = RecoveryReason::DeprecatedFlow;

    // Should handle large values
    let result = client.try_recover_stranded_funds(&admin, &recipient, &amount, &reason);
    assert!(result.is_ok());
}

// =============================================================================
// Migration Export Hooks Tests
// =============================================================================

#[test]
fn test_export_contract_snapshot_admin_only() {
    let (env, client, token, admin) = setup_test_env();

    let snapshot = client.export_contract_snapshot(&admin);
    assert_eq!(snapshot.admin, admin);
    assert_eq!(snapshot.token, token);
    assert_eq!(snapshot.min_topup, 1_000000i128);

    let non_admin = Address::generate(&env);
    let result = client.try_export_contract_snapshot(&non_admin);
    assert!(result.is_err());
}

#[test]
fn test_export_subscription_summary_fields() {
    let (env, client, _, admin) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let amount = 12_000_000i128;
    let interval_seconds = 14 * 24 * 60 * 60;

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval_seconds,
        &false,
        &None,
    );

    let summary = client.export_subscription_summary(&admin, &id);
    assert_eq!(summary.subscription_id, id);
    assert_eq!(summary.subscriber, subscriber);
    assert_eq!(summary.merchant, merchant);
    assert_eq!(summary.amount, amount);
    assert_eq!(summary.interval_seconds, interval_seconds);
    assert_eq!(summary.status, SubscriptionStatus::Active);
}

#[test]
fn test_export_subscription_summaries_pagination() {
    let (env, client, _, admin) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id1 = client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &(24 * 60 * 60),
        &false,
        &None,
    );
    let id2 = client.create_subscription(
        &subscriber,
        &merchant,
        &2_000_000i128,
        &(7 * 24 * 60 * 60),
        &false,
        &None,
    );
    let id3 = client.create_subscription(
        &subscriber,
        &merchant,
        &3_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
        &None,
    );

    let page1 = client.export_subscription_summaries(&admin, &id1, &2);
    assert_eq!(page1.len(), 2);
    assert_eq!(page1.get(0).unwrap().subscription_id, id1);
    assert_eq!(page1.get(1).unwrap().subscription_id, id2);

    let page2 = client.export_subscription_summaries(&admin, &id3, &2);
    assert_eq!(page2.len(), 1);
    assert_eq!(page2.get(0).unwrap().subscription_id, id3);
}

#[test]
fn test_export_subscription_summaries_limit_enforced() {
    let (env, client, _, admin) = setup_test_env();

    let result = client.try_export_subscription_summaries(&admin, &0, &101);
    assert!(result.is_err());

    let non_admin = Address::generate(&env);
    let result = client.try_export_subscription_summaries(&non_admin, &0, &1);
    assert!(result.is_err());
}

#[test]
fn test_export_subscription_does_not_mutate_state() {
    let (env, client, _, admin) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &5_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
        &None,
    );

    let before = client.get_subscription(&id);
    let _summary = client.export_subscription_summary(&admin, &id);
    let after = client.get_subscription(&id);

    assert_eq!(before.subscriber, after.subscriber);
    assert_eq!(before.merchant, after.merchant);
    assert_eq!(before.amount, after.amount);
    assert_eq!(before.interval_seconds, after.interval_seconds);
    assert_eq!(before.status, after.status);
    assert_eq!(before.prepaid_balance, after.prepaid_balance);
    assert_eq!(before.usage_enabled, after.usage_enabled);
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
        &None,
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
        &None,
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
        &None,
    );

    // Verify initial state
    assert!(client.get_subscription(&id).usage_enabled);

    // Pause subscription
    client.pause_subscription(&id, &subscriber);
    assert!(client.get_subscription(&id).usage_enabled);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );

    // Resume subscription
    client.resume_subscription(&id, &subscriber);
    assert!(client.get_subscription(&id).usage_enabled);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );

    // Cancel subscription
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

    // Create subscription with usage disabled
    let id1 = client.create_subscription(
        &subscriber,
        &merchant1,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
        &None,
    );

    // Create subscription with usage enabled
    let id2 = client.create_subscription(
        &subscriber,
        &merchant2,
        &5_000_000i128,
        &(7 * 24 * 60 * 60),
        &true,
        &None,
    );

    // Create another with usage disabled
    let id3 = client.create_subscription(
        &subscriber,
        &merchant3,
        &20_000_000i128,
        &(90 * 24 * 60 * 60),
        &false,
        &None,
    );

    // Verify each subscription has correct usage_enabled value
    assert!(!client.get_subscription(&id1).usage_enabled);
    assert!(client.get_subscription(&id2).usage_enabled);
    assert!(!client.get_subscription(&id3).usage_enabled);

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
        &None,
    );

    // Weekly subscription with usage disabled
    let weekly_id = client.create_subscription(
        &subscriber,
        &merchant,
        &5_000_000i128,
        &(7 * 24 * 60 * 60), // 7 days
        &false,
        &None,
    );

    // Monthly subscription with usage enabled
    let monthly_id = client.create_subscription(
        &subscriber,
        &merchant,
        &20_000_000i128,
        &(30 * 24 * 60 * 60), // 30 days
        &true,
        &None,
    );

    // Verify usage_enabled is independent of interval
    assert!(client.get_subscription(&daily_id).usage_enabled);
    assert!(!client.get_subscription(&weekly_id).usage_enabled);
    assert!(client.get_subscription(&monthly_id).usage_enabled);
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
        &None,
    );

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

    // Create subscription with usage enabled
    let id_enabled = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
        &None,
    );

    // Create subscription with usage disabled
    let id_disabled = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
        &None,
    );

    // Both should compute next charge info regardless of usage_enabled
    let info_enabled = client.get_next_charge_info(&id_enabled);
    let info_disabled = client.get_next_charge_info(&id_disabled);

    assert!(info_enabled.is_charge_expected);
    assert!(info_disabled.is_charge_expected);

    // Verify subscriptions still have correct usage_enabled values
    assert!(client.get_subscription(&id_enabled).usage_enabled);
    assert!(!client.get_subscription(&id_disabled).usage_enabled);
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
        &None,
    );

    let subscription = client.get_subscription(&id);

    // Should work fine with interval-based billing
    assert!(!subscription.usage_enabled);
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
        &None,
    );

    assert!(!client.get_subscription(&id).usage_enabled);

    // Perform various operations
    client.pause_subscription(&id, &subscriber);
    assert!(!client.get_subscription(&id).usage_enabled);

    client.resume_subscription(&id, &subscriber);
    assert!(!client.get_subscription(&id).usage_enabled);

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
        &None,
    );

    // Test Active status
    assert!(client.get_subscription(&id).usage_enabled);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );

    // Test Paused status
    client.pause_subscription(&id, &subscriber);
    assert!(client.get_subscription(&id).usage_enabled);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );

    // Test Active again (resumed)
    client.resume_subscription(&id, &subscriber);
    assert!(client.get_subscription(&id).usage_enabled);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );

    // Test Cancelled status
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

    // When usage_enabled is true, this indicates the subscription supports
    // usage-based billing in addition to or instead of interval-based billing
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
        &None,
    );

    let subscription = client.get_subscription(&id);

    // The subscription is created successfully
    assert!(subscription.usage_enabled);

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
        &None,
    );

    let subscription = client.get_subscription(&id);

    // The subscription is created successfully
    assert!(!subscription.usage_enabled);

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
    let id1 = client.create_subscription(
        &subscriber,
        &merchant,
        &100i128,
        &(24 * 60 * 60),
        &true,
        &None,
    );

    // Large amount with usage disabled
    let id2 = client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
        &None,
    );

    // Medium amount with usage enabled
    let id3 = client.create_subscription(
        &subscriber,
        &merchant,
        &50_000_000i128,
        &(7 * 24 * 60 * 60),
        &true,
        &None,
    );

    // Verify amounts and usage_enabled are independent
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

    // Create multiple subscriptions with alternating usage_enabled values
    let id0 = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
        &None,
    );

    let id1 = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
        &None,
    );

    let id2 = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
        &None,
    );

    let id3 = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
        &None,
    );

    let id4 = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
        &None,
    );

    // Verify each subscription has the correct usage_enabled value
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

    // Create subscription with usage enabled
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &true,
        &None,
    );

    assert!(client.get_subscription(&id).usage_enabled);

    // Admin recovery should not affect subscription's usage_enabled flag
    let recipient = Address::generate(&env);
    client.recover_stranded_funds(
        &admin,
        &recipient,
        &5_000_000i128,
        &RecoveryReason::DeprecatedFlow,
    );

    // Subscription should still exist with same usage_enabled value
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
    env.as_contract(&client.address, || {
        env.storage().instance().set(&DataKey::Sub(id), &sub);
    });
    env.ledger()
        .with_mut(|li| li.timestamp = T0 + interval_seconds);

    // Old admin can batch_charge before rotation
    let ids = soroban_sdk::Vec::from_array(&env, [id]);
    let results = client.batch_charge(&ids);
    assert_eq!(results.len(), 1);
    let r0 = results.get(0).unwrap();
    assert!(r0.success);
    assert_eq!(r0.error_code, 0);

    // Rotate admin
    let new_admin = Address::generate(&env);
    client.rotate_admin(&old_admin, &new_admin);

    // New admin can batch_charge after rotation (stored admin = new_admin)
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
        &None,
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
    assert!(!events.is_empty());
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
        &None,
    );

    let id2 = client.create_subscription(
        &subscriber2,
        &merchant,
        &5_000_000i128,
        &(7 * 24 * 60 * 60),
        &true,
        &None,
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

// =============================================================================
// View Function Tests: list_subscriptions_by_subscriber
// =============================================================================

#[test]
fn test_list_subscriptions_zero_subscriptions() {
    // Test querying a subscriber with no subscriptions
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let page = client.list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);

    assert_eq!(page.subscription_ids.len(), 0);
    assert!(!page.has_next);
}

#[test]
fn test_list_subscriptions_one_subscription() {
    // Test querying a subscriber with exactly one subscription
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
    // Test querying a subscriber with multiple subscriptions
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
            &None,
        );
        ids.push_back(id);
    }

    let page = client.list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);

    assert_eq!(page.subscription_ids.len(), 5);
    assert!(!page.has_next);

    // Verify subscriptions are returned in order by ID
    for i in 0..5 {
        assert_eq!(
            page.subscription_ids.get(i).unwrap(),
            ids.get(i as u32).unwrap()
        );
    }
}

#[test]
fn test_list_subscriptions_pagination_first_page() {
    // Test first page of pagination
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
            &None,
        );
        ids.push_back(id);
    }

    let page1 = client.list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);

    assert_eq!(page1.subscription_ids.len(), 10);
    assert!(page1.has_next);

    // Verify first page contains the first 10 subscriptions
    for i in 0..10 {
        assert_eq!(
            page1.subscription_ids.get(i).unwrap(),
            ids.get(i as u32).unwrap()
        );
    }
}

#[test]
fn test_list_subscriptions_pagination_second_page() {
    // Test second page of pagination
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
            &None,
        );
        ids.push_back(id);
    }

    // Get first page
    let page1 = client.list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);
    assert_eq!(page1.subscription_ids.len(), 10);
    let last_id_page1 = page1.subscription_ids.get(9).unwrap();

    // Get second page using start_from_id = last_id + 1
    let next_start = last_id_page1 + 1;
    let page2 = client.list_subscriptions_by_subscriber(&subscriber, &next_start, &10u32);

    assert_eq!(page2.subscription_ids.len(), 5);
    assert!(!page2.has_next);

    // Verify second page contains the remaining 5 subscriptions
    for i in 0..5 {
        assert_eq!(
            page2.subscription_ids.get(i).unwrap(),
            ids.get((10 + i) as u32).unwrap()
        );
    }
}

#[test]
fn test_list_subscriptions_filters_by_subscriber() {
    // Test that only subscriptions for the specific subscriber are returned
    let (env, client, _, _) = setup_test_env();

    let subscriber1 = Address::generate(&env);
    let subscriber2 = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Create 3 subscriptions for subscriber1
    for _ in 0..3 {
        client.create_subscription(
            &subscriber1,
            &merchant,
            &10_000_000i128,
            &(30 * 24 * 60 * 60),
            &false,
            &None,
        );
    }

    // Create 2 subscriptions for subscriber2
    for _ in 0..2 {
        client.create_subscription(
            &subscriber2,
            &merchant,
            &10_000_000i128,
            &(30 * 24 * 60 * 60),
            &false,
            &None,
        );
    }

    // Query subscriber1
    let page1 = client.list_subscriptions_by_subscriber(&subscriber1, &0u32, &10u32);
    assert_eq!(page1.subscription_ids.len(), 3);

    // Query subscriber2
    let page2 = client.list_subscriptions_by_subscriber(&subscriber2, &0u32, &10u32);
    assert_eq!(page2.subscription_ids.len(), 2);
}

#[test]
fn test_list_subscriptions_small_limit() {
    // Test pagination with very small limit (limit=1)
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
            &None,
        );
        ids.push_back(id);
    }

    // Get all pages with limit=1
    let mut all_ids = soroban_sdk::Vec::new(&env);
    let mut start_id = 0u32;
    let mut has_next = true;

    while has_next {
        let page = client.list_subscriptions_by_subscriber(&subscriber, &start_id, &1u32);
        if page.subscription_ids.len() > 0 {
            let current_id = page.subscription_ids.get(0).unwrap();
            all_ids.push_back(current_id);
            // Advance start cursor past the current ID
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
    // Test that limit=0 returns an error
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);

    client.list_subscriptions_by_subscriber(&subscriber, &0u32, &0u32);
}

#[test]
fn test_list_subscriptions_respects_start_from_id() {
    // Test that start_from_id correctly includes only subscriptions from that ID onward
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
            &None,
        );
        ids.push_back(id);
    }

    // Get subscriptions starting from the 6th one (index 5, IDs 5-9)
    let start_id = ids.get(5u32).unwrap();
    let page = client.list_subscriptions_by_subscriber(&subscriber, &start_id, &10u32);

    // Should contain subscriptions 5-9 (5 subscriptions, inclusive)
    assert_eq!(page.subscription_ids.len(), 5);

    // Verify these are subscriptions at indices 5-9
    for i in 0..5 {
        assert_eq!(
            page.subscription_ids.get(i).unwrap(),
            ids.get((5 + i) as u32).unwrap()
        );
    }
}

#[test]
fn test_list_subscriptions_stable_ordering() {
    // Test that subscriptions are always returned in the same order (by ID, ascending)
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
            &None,
        );
    }

    // Query multiple times and verify consistent ordering
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
    // Test pagination with subscriptions to multiple merchants
    let (env, client, _, _) = setup_test_env();

    let subscriber = Address::generate(&env);
    let merchant1 = Address::generate(&env);
    let merchant2 = Address::generate(&env);

    let mut ids = soroban_sdk::Vec::new(&env);
    // Create subscriptions to different merchants
    for i in 0..10 {
        let merchant = if i % 2 == 0 { &merchant1 } else { &merchant2 };
        let id = client.create_subscription(
            &subscriber,
            merchant,
            &10_000_000i128,
            &(30 * 24 * 60 * 60),
            &false,
            &None,
        );
        ids.push_back(id);
    }

    let page = client.list_subscriptions_by_subscriber(&subscriber, &0u32, &10u32);

    assert_eq!(page.subscription_ids.len(), 10);
    // All subscriptions should be from this subscriber regardless of merchant
    for i in 0..10 {
        assert_eq!(
            page.subscription_ids.get(i).unwrap(),
            ids.get(i as u32).unwrap()
        );
    }
}

// =============================================================================
// Storage Versioning and Upgrade Tests
// =============================================================================

#[test]
fn test_init_sets_schema_version() {
    let (_, client, _, _) = setup_test_env();
    assert_eq!(client.get_storage_version(), crate::STORAGE_VERSION);
}

#[test]
fn test_schema_version_is_one() {
    assert_eq!(crate::STORAGE_VERSION, 1u32);
}

#[test]
fn test_get_storage_version_before_init_returns_zero() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    assert_eq!(client.get_storage_version(), 0);
}

#[test]
fn test_schema_version_survives_operations() {
    let (env, client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    client.create_subscription(&subscriber, &merchant, &10_000_000i128, &INTERVAL, &false);
    assert_eq!(client.get_storage_version(), crate::STORAGE_VERSION);

    client.set_min_topup(&client.get_admin(), &2_000_000i128);
    assert_eq!(client.get_storage_version(), crate::STORAGE_VERSION);
}

#[test]
fn test_datakey_sub_does_not_collide_with_config_keys() {
    let (env, client, _, admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(&subscriber, &merchant, &10_000_000i128, &INTERVAL, &false);
    assert_eq!(id, 0);

    // Config keys must remain intact after subscription creation
    assert_eq!(client.get_admin(), admin);
    assert_eq!(client.get_min_topup(), 1_000_000i128);
    assert_eq!(client.get_storage_version(), 1);

    // Subscription must be readable
    let sub = client.get_subscription(&id);
    assert_eq!(sub.subscriber, subscriber);
}

#[test]
fn test_many_subscriptions_no_key_collision() {
    let (env, client, _, admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    for i in 0u32..20 {
        let id = client.create_subscription(
            &subscriber,
            &merchant,
            &(1_000_000i128 + i as i128),
            &INTERVAL,
            &false,
        );
        assert_eq!(id, i);
    }

    // All subscriptions readable
    for i in 0u32..20 {
        let sub = client.get_subscription(&i);
        assert_eq!(sub.amount, 1_000_000i128 + i as i128);
    }

    // Config still intact
    assert_eq!(client.get_admin(), admin);
    assert_eq!(client.get_storage_version(), 1);
}

#[test]
fn test_next_id_overflow_protection() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    // Force next_id to u32::MAX
    env.as_contract(&client.address, || {
        env.storage().instance().set(&DataKey::NextId, &u32::MAX);
    });

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let result =
        client.try_create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    assert!(result.is_err());
}

#[test]
fn test_admin_migrate_v0_to_v1_simulation() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);

    // Simulate v0 state: write config and subscriptions using old key layout
    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::MinTopup, &1_000_000i128);
        // v0 stored next_id under a Symbol key
        let old_next_id_key = soroban_sdk::Symbol::new(&env, "next_id");
        env.storage().instance().set(&old_next_id_key, &3u32);
        // v0 stored subscriptions under bare u32 keys
        let sub0 = Subscription {
            subscriber: Address::generate(&env),
            merchant: Address::generate(&env),
            amount: 5_000_000i128,
            interval_seconds: INTERVAL,
            last_payment_timestamp: T0,
            status: SubscriptionStatus::Active,
            prepaid_balance: 50_000_000i128,
            usage_enabled: false,
        };
        let sub1 = Subscription {
            subscriber: Address::generate(&env),
            merchant: Address::generate(&env),
            amount: 10_000_000i128,
            interval_seconds: INTERVAL,
            last_payment_timestamp: T0,
            status: SubscriptionStatus::Paused,
            prepaid_balance: 0i128,
            usage_enabled: true,
        };
        let sub2 = Subscription {
            subscriber: Address::generate(&env),
            merchant: Address::generate(&env),
            amount: 1_000_000i128,
            interval_seconds: 86400,
            last_payment_timestamp: T0,
            status: SubscriptionStatus::Cancelled,
            prepaid_balance: 0i128,
            usage_enabled: false,
        };
        env.storage().instance().set(&0u32, &sub0);
        env.storage().instance().set(&1u32, &sub1);
        env.storage().instance().set(&2u32, &sub2);
    });

    // No schema version set (v0 default)
    assert_eq!(client.get_storage_version(), 0);

    // Run migration
    client.admin_migrate(&admin, &0u32);

    // Version now set to 1
    assert_eq!(client.get_storage_version(), crate::STORAGE_VERSION);

    // All subscriptions readable through the new DataKey::Sub keys
    let s0 = client.get_subscription(&0u32);
    assert_eq!(s0.amount, 5_000_000i128);
    assert_eq!(s0.status, SubscriptionStatus::Active);
    assert_eq!(s0.prepaid_balance, 50_000_000i128);

    let s1 = client.get_subscription(&1u32);
    assert_eq!(s1.amount, 10_000_000i128);
    assert_eq!(s1.status, SubscriptionStatus::Paused);
    assert!(s1.usage_enabled);

    let s2 = client.get_subscription(&2u32);
    assert_eq!(s2.amount, 1_000_000i128);
    assert_eq!(s2.status, SubscriptionStatus::Cancelled);
}

#[test]
fn test_admin_migrate_is_idempotent() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::MinTopup, &1_000_000i128);
        let old_key = soroban_sdk::Symbol::new(&env, "next_id");
        env.storage().instance().set(&old_key, &1u32);
        let sub = Subscription {
            subscriber: Address::generate(&env),
            merchant: Address::generate(&env),
            amount: 5_000_000i128,
            interval_seconds: INTERVAL,
            last_payment_timestamp: T0,
            status: SubscriptionStatus::Active,
            prepaid_balance: 10_000_000i128,
            usage_enabled: false,
        };
        env.storage().instance().set(&0u32, &sub);
    });

    client.admin_migrate(&admin, &0u32);
    assert_eq!(client.get_storage_version(), 1);

    // Running again should not corrupt anything
    client.admin_migrate(&admin, &0u32);
    assert_eq!(client.get_storage_version(), 1);

    let s = client.get_subscription(&0u32);
    assert_eq!(s.amount, 5_000_000i128);
    assert_eq!(s.prepaid_balance, 10_000_000i128);
}

#[test]
#[should_panic(expected = "Error(Contract, #401)")]
fn test_admin_migrate_unauthorized() {
    let (env, client, _, _) = setup_test_env();
    let rando = Address::generate(&env);
    client.admin_migrate(&rando, &0u32);
}

#[test]
fn test_subscription_fields_stable_after_lifecycle() {
    let (env, client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription(&subscriber, &merchant, &10_000_000i128, &INTERVAL, &true);
    let created = client.get_subscription(&id);

    // Pause, resume, cancel — field integrity must hold through state machine transitions
    client.pause_subscription(&id, &subscriber);
    let paused = client.get_subscription(&id);
    assert_eq!(paused.subscriber, created.subscriber);
    assert_eq!(paused.merchant, created.merchant);
    assert_eq!(paused.amount, created.amount);
    assert_eq!(paused.interval_seconds, created.interval_seconds);
    assert!(paused.usage_enabled);
    assert_eq!(paused.status, SubscriptionStatus::Paused);

    client.resume_subscription(&id, &subscriber);
    let resumed = client.get_subscription(&id);
    assert_eq!(resumed.status, SubscriptionStatus::Active);
    assert_eq!(resumed.amount, created.amount);

    client.cancel_subscription(&id, &subscriber);
    let cancelled = client.get_subscription(&id);
    assert_eq!(cancelled.status, SubscriptionStatus::Cancelled);
    assert_eq!(cancelled.subscriber, created.subscriber);
    assert_eq!(cancelled.merchant, created.merchant);
    assert!(cancelled.usage_enabled);
}

#[test]
fn test_charged_period_key_isolation() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    env.ledger().set_timestamp(T0);
    let id0 = client.create_subscription(&subscriber, &merchant, &1000i128, &INTERVAL, &false);
    let id1 = client.create_subscription(&subscriber, &merchant, &2000i128, &INTERVAL, &false);

    // Seed balances
    env.as_contract(&contract_id, || {
        let mut s0 = env
            .storage()
            .instance()
            .get::<DataKey, Subscription>(&DataKey::Sub(id0))
            .unwrap();
        s0.prepaid_balance = 100_000_000i128;
        env.storage().instance().set(&DataKey::Sub(id0), &s0);

        let mut s1 = env
            .storage()
            .instance()
            .get::<DataKey, Subscription>(&DataKey::Sub(id1))
            .unwrap();
        s1.prepaid_balance = 100_000_000i128;
        env.storage().instance().set(&DataKey::Sub(id1), &s1);
    });

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id0);

    // id0 charged successfully — replay should fail
    let r0 = client.try_charge_subscription(&id0);
    assert!(r0.is_err());

    // id1 should still be chargeable (independent period tracking)
    client.charge_subscription(&id1);
    let s1 = client.get_subscription(&id1);
    assert_eq!(s1.prepaid_balance, 100_000_000i128 - 2000i128);
}

#[test]
fn test_version_persists_across_admin_rotation() {
    let (env, client, _, admin) = setup_test_env();
    assert_eq!(client.get_storage_version(), 1);

    let new_admin = Address::generate(&env);
    client.rotate_admin(&admin, &new_admin);
    assert_eq!(client.get_storage_version(), 1);

    // New admin can still read version
    assert_eq!(client.get_admin(), new_admin);
    assert_eq!(client.get_storage_version(), 1);
}
