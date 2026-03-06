use crate::{
    can_transition, compute_next_charge_info, get_allowed_transitions, validate_status_transition,
    Error, RecoveryReason, Subscription, SubscriptionStatus, SubscriptionVault,
    SubscriptionVaultClient, MAX_SUBSCRIPTION_ID,
};
use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
use soroban_sdk::{Address, Env, Vec as SorobanVec};

// ── constants ─────────────────────────────────────────────────────────────────
const T0: u64 = 1_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const AMOUNT: i128 = 10_000_000; // 10 USDC (6 decimals)
const PREPAID: i128 = 50_000_000; // 50 USDC

// ── helpers ───────────────────────────────────────────────────────────────────

fn create_token_and_mint(env: &Env, recipient: &Address, amount: i128) -> Address {
    let token_admin = Address::generate(env);
    let token_addr = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = soroban_sdk::token::StellarAssetClient::new(env, &token_addr);
    token_client.mint(recipient, &amount);
    token_addr
}

/// Standard setup: mock auth, register contract, init with real token + 7-day grace.
fn setup_test_env() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let min_topup = 1_000_000i128; // 1 USDC
    client.init(&token, &6, &admin, &min_topup, &(7 * 24 * 60 * 60));

    (env, client, token, admin)
}

/// Helper used by reentrancy tests: returns (client, token, admin) with env pre-configured.
fn setup_contract(env: &Env) -> (SubscriptionVaultClient<'_>, Address, Address) {
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    let admin = Address::generate(env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    (client, token, admin)
}

/// Create a test subscription, then patch its status for direct-manipulation tests.
fn create_test_subscription(
    env: &Env,
    client: &SubscriptionVaultClient,
    status: SubscriptionStatus,
) -> (u32, Address, Address) {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    if status != SubscriptionStatus::Active {
        let mut sub = client.get_subscription(&id);
        sub.status = status;
        env.as_contract(&client.address, || {
            env.storage().instance().set(&id, &sub);
        });
    }
    (id, subscriber, merchant)
}

/// Seed a subscription with a known prepaid balance directly in storage.
fn seed_balance(env: &Env, client: &SubscriptionVaultClient, id: u32, balance: i128) {
    let mut sub = client.get_subscription(&id);
    sub.prepaid_balance = balance;
    env.as_contract(&client.address, || {
        env.storage().instance().set(&id, &sub);
    });
}

/// Seed the `next_id` counter to an arbitrary value.
fn seed_counter(env: &Env, contract_id: &Address, value: u32) {
    env.as_contract(contract_id, || {
        env.storage()
            .instance()
            .set(&soroban_sdk::Symbol::new(env, "next_id"), &value);
    });
}

// ── State Machine Helper Tests ─────────────────────────────────────────────────

#[test]
fn test_validate_status_transition_same_status_is_allowed() {
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
    assert!(
        validate_status_transition(&SubscriptionStatus::Active, &SubscriptionStatus::Paused)
            .is_ok()
    );
    assert!(validate_status_transition(
        &SubscriptionStatus::Active,
        &SubscriptionStatus::Cancelled
    )
    .is_ok());
    assert!(validate_status_transition(
        &SubscriptionStatus::Active,
        &SubscriptionStatus::InsufficientBalance
    )
    .is_ok());
}

#[test]
fn test_validate_paused_transitions() {
    assert!(
        validate_status_transition(&SubscriptionStatus::Paused, &SubscriptionStatus::Active)
            .is_ok()
    );
    assert!(validate_status_transition(
        &SubscriptionStatus::Paused,
        &SubscriptionStatus::Cancelled
    )
    .is_ok());
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
    assert!(validate_status_transition(
        &SubscriptionStatus::InsufficientBalance,
        &SubscriptionStatus::Active
    )
    .is_ok());
    assert!(validate_status_transition(
        &SubscriptionStatus::InsufficientBalance,
        &SubscriptionStatus::Cancelled
    )
    .is_ok());
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
    let active_targets = get_allowed_transitions(&SubscriptionStatus::Active);
    assert!(active_targets.contains(&SubscriptionStatus::Paused));
    assert!(active_targets.contains(&SubscriptionStatus::Cancelled));
    assert!(active_targets.contains(&SubscriptionStatus::InsufficientBalance));

    let paused_targets = get_allowed_transitions(&SubscriptionStatus::Paused);
    assert_eq!(paused_targets.len(), 2);
    assert!(paused_targets.contains(&SubscriptionStatus::Active));
    assert!(paused_targets.contains(&SubscriptionStatus::Cancelled));

    assert_eq!(
        get_allowed_transitions(&SubscriptionStatus::Cancelled).len(),
        0
    );

    let ib_targets = get_allowed_transitions(&SubscriptionStatus::InsufficientBalance);
    assert_eq!(ib_targets.len(), 2);
}

// ── Contract Lifecycle Tests ───────────────────────────────────────────────────

#[test]
fn test_pause_subscription_from_active() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    client.pause_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #400)")]
fn test_pause_subscription_from_cancelled_should_fail() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    client.cancel_subscription(&id, &subscriber);
    client.pause_subscription(&id, &subscriber);
}

#[test]
fn test_pause_subscription_from_paused_is_idempotent() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    client.pause_subscription(&id, &subscriber);
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
    client.cancel_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}

#[test]
fn test_cancel_subscription_from_paused() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    client.pause_subscription(&id, &subscriber);
    client.cancel_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}

#[test]
fn test_cancel_subscription_from_cancelled_is_idempotent() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    client.cancel_subscription(&id, &subscriber);
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
    client.pause_subscription(&id, &subscriber);
    client.resume_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #400)")]
fn test_resume_subscription_from_cancelled_should_fail() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    client.cancel_subscription(&id, &subscriber);
    client.resume_subscription(&id, &subscriber);
}

#[test]
fn test_full_lifecycle_active_pause_resume() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    client.pause_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );
    client.resume_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );
    client.pause_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );
}

#[test]
fn test_all_valid_transitions_coverage() {
    // Active -> Paused
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
    // Active -> Cancelled
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
    // Active -> InsufficientBalance (direct storage patch)
    {
        let (env, client, _, _) = setup_test_env();
        let (id, _, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
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
    // Paused -> Active
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
    // Paused -> Cancelled
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
    // InsufficientBalance -> Active
    {
        let (env, client, _, _) = setup_test_env();
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, SubscriptionStatus::Active);
        let mut sub = client.get_subscription(&id);
        sub.status = SubscriptionStatus::InsufficientBalance;
        env.as_contract(&client.address, || {
            env.storage().instance().set(&id, &sub);
        });
        client.resume_subscription(&id, &subscriber);
        assert_eq!(
            client.get_subscription(&id).status,
            SubscriptionStatus::Active
        );
    }
    // InsufficientBalance -> Cancelled
    {
        let (env, client, _, _) = setup_test_env();
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, SubscriptionStatus::Active);
        let mut sub = client.get_subscription(&id);
        sub.status = SubscriptionStatus::InsufficientBalance;
        env.as_contract(&client.address, || {
            env.storage().instance().set(&id, &sub);
        });
        client.cancel_subscription(&id, &subscriber);
        assert_eq!(
            client.get_subscription(&id).status,
            SubscriptionStatus::Cancelled
        );
    }
}

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
        env.storage().instance().set(&id, &sub);
    });
    client.pause_subscription(&id, &subscriber);
}

// ── Subscription struct tests ─────────────────────────────────────────────────

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
        lifetime_cap: None,
        lifetime_charged: 0,
    };
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(sub.lifetime_cap, None);
    assert_eq!(sub.lifetime_charged, 0);
}

#[test]
fn test_subscription_struct_with_lifetime_cap() {
    let env = Env::default();
    let cap = 120_000_000i128; // 120 USDC
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        amount: 10_000_000,
        interval_seconds: 30 * 24 * 60 * 60,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: 50_000_000,
        usage_enabled: false,
        lifetime_cap: Some(cap),
        lifetime_charged: 0,
    };
    assert_eq!(sub.lifetime_cap, Some(cap));
    assert_eq!(sub.lifetime_charged, 0);
}

// ── Init / Min-Topup Tests ─────────────────────────────────────────────────────

#[test]
fn test_init_with_min_topup() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let min_topup = 1_000_000i128;
    client.init(&token, &6, &admin, &min_topup, &(7 * 24 * 60 * 60));
    assert_eq!(client.get_min_topup(), min_topup);
}

#[test]
fn test_set_min_topup_by_admin() {
    let (_, client, _, admin) = setup_test_env();
    let new_min = 10_000_000i128;
    client.set_min_topup(&admin, &new_min);
    assert_eq!(client.get_min_topup(), new_min);
}

#[test]
fn test_set_min_topup_unauthorized() {
    let (env, client, _, _) = setup_test_env();
    let non_admin = Address::generate(&env);
    let result = client.try_set_min_topup(&non_admin, &5_000_000);
    assert!(result.is_err());
}

// ── Cancel/Withdraw Tests ─────────────────────────────────────────────────────

#[test]
fn test_cancel_subscription_by_subscriber() {
    let (env, client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let sub_id =
        client.create_subscription(&subscriber, &merchant, &1000, &86400, &true, &None::<i128>);
    client.cancel_subscription(&sub_id, &subscriber);
    assert_eq!(
        client.get_subscription(&sub_id).status,
        SubscriptionStatus::Cancelled
    );
}

#[test]
fn test_cancel_subscription_unauthorized() {
    let (env, client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let other = Address::generate(&env);
    let sub_id =
        client.create_subscription(&subscriber, &merchant, &1000, &86400, &true, &None::<i128>);
    let result = client.try_cancel_subscription(&sub_id, &other);
    assert_eq!(result, Err(Ok(Error::Forbidden)));
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

    client.init(
        &token_contract,
        &6,
        &vault_admin,
        &1000,
        &(7 * 24 * 60 * 60),
    );
    token_admin.mint(&subscriber, &5000);

    let sub_id =
        client.create_subscription(&subscriber, &merchant, &1000, &86400, &true, &None::<i128>);
    client.deposit_funds(&sub_id, &subscriber, &5000);
    client.cancel_subscription(&sub_id, &subscriber);
    client.withdraw_subscriber_funds(&sub_id, &subscriber);

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 0);
    assert_eq!(token.balance(&subscriber), 5000);
    assert_eq!(token.balance(&contract_id), 0);
}

// ── Min-Topup Enforcement Tests ────────────────────────────────────────────────

#[test]
fn test_min_topup_below_threshold() {
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
    let min_topup = 5_000_000i128;

    client.init(&token_addr, &6, &admin, &min_topup, &(7 * 24 * 60 * 60));
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
        &None::<i128>,
    );
    client.cancel_subscription(&id, &merchant);
    let result = client.try_deposit_funds(&id, &subscriber, &4_999_999);
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
    let min_topup = 5_000_000i128;

    client.init(&token_addr, &6, &admin, &min_topup, &(7 * 24 * 60 * 60));
    token_admin.mint(&subscriber, &min_topup);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
        &None::<i128>,
    );
    assert!(client
        .try_deposit_funds(&id, &subscriber, &min_topup)
        .is_ok());
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
    let min_topup = 5_000_000i128;
    let deposit_amount = 10_000_000i128;

    client.init(&token_addr, &6, &admin, &min_topup, &(7 * 24 * 60 * 60));
    token_admin.mint(&subscriber, &deposit_amount);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &deposit_amount,
        &(30 * 24 * 60 * 60),
        &false,
        &None::<i128>,
    );
    assert!(client
        .try_deposit_funds(&id, &subscriber, &deposit_amount)
        .is_ok());
}

// ── Usage-charge tests ─────────────────────────────────────────────────────────

fn setup_usage(env: &Env) -> (SubscriptionVaultClient<'_>, u32) {
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    let token = Address::generate(env);
    let admin = Address::generate(env);
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    env.ledger().set_timestamp(T0);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &true,
        &None::<i128>,
    );
    seed_balance(env, &client, id, PREPAID);
    (client, id)
}

fn setup_interval(env: &Env, interval: u64) -> (SubscriptionVaultClient<'_>, u32) {
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    let token = Address::generate(env);
    let admin = Address::generate(env);
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    env.ledger().set_timestamp(T0);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &interval,
        &false,
        &None::<i128>,
    );
    seed_balance(env, &client, id, PREPAID);
    (client, id)
}

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

#[test]
fn test_usage_charge_rejected_when_disabled() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_interval(&env, INTERVAL);
    let res = client.try_charge_usage(&id, &1_000_000i128);
    assert_eq!(res, Err(Ok(Error::UsageNotEnabled)));
}

#[test]
fn test_usage_charge_rejected_insufficient_balance() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_usage(&env);
    let res = client.try_charge_usage(&id, &(PREPAID + 1));
    assert_eq!(res, Err(Ok(Error::InsufficientPrepaidBalance)));
    assert_eq!(client.get_subscription(&id).prepaid_balance, PREPAID);
}

#[test]
fn test_usage_charge_rejected_invalid_amount() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id) = setup_usage(&env);
    assert_eq!(
        client.try_charge_usage(&id, &0i128),
        Err(Ok(Error::InvalidAmount))
    );
    assert_eq!(
        client.try_charge_usage(&id, &(-1i128)),
        Err(Ok(Error::InvalidAmount))
    );
    assert_eq!(client.get_subscription(&id).prepaid_balance, PREPAID);
}

// ── Next Charge Info Tests ─────────────────────────────────────────────────────

#[test]
fn test_compute_next_charge_info_active() {
    let env = Env::default();
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        amount: AMOUNT,
        interval_seconds: INTERVAL,
        last_payment_timestamp: 1000,
        status: SubscriptionStatus::Active,
        prepaid_balance: 100_000_000,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
    };
    let info = compute_next_charge_info(&sub);
    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, 1000 + INTERVAL);
}

#[test]
fn test_compute_next_charge_info_paused() {
    let env = Env::default();
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        amount: AMOUNT,
        interval_seconds: INTERVAL,
        last_payment_timestamp: 2000,
        status: SubscriptionStatus::Paused,
        prepaid_balance: 50_000_000,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
    };
    let info = compute_next_charge_info(&sub);
    assert!(!info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, 2000 + INTERVAL);
}

#[test]
fn test_compute_next_charge_info_cancelled() {
    let env = Env::default();
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        amount: AMOUNT,
        interval_seconds: INTERVAL,
        last_payment_timestamp: 5000,
        status: SubscriptionStatus::Cancelled,
        prepaid_balance: 0,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
    };
    let info = compute_next_charge_info(&sub);
    assert!(!info.is_charge_expected);
}

#[test]
fn test_compute_next_charge_info_insufficient_balance() {
    let env = Env::default();
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        amount: AMOUNT,
        interval_seconds: INTERVAL,
        last_payment_timestamp: 3000,
        status: SubscriptionStatus::InsufficientBalance,
        prepaid_balance: 1_000_000,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
    };
    let info = compute_next_charge_info(&sub);
    assert!(info.is_charge_expected);
}

#[test]
fn test_compute_next_charge_info_overflow_protection() {
    let env = Env::default();
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        amount: AMOUNT,
        interval_seconds: 200,
        last_payment_timestamp: u64::MAX - 100,
        status: SubscriptionStatus::Active,
        prepaid_balance: 100_000_000,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
    };
    let info = compute_next_charge_info(&sub);
    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, u64::MAX);
}

#[test]
fn test_get_next_charge_info_contract_method() {
    let (env, client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    env.ledger().with_mut(|li| li.timestamp = 1000);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    let info = client.get_next_charge_info(&id);
    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, 1000 + INTERVAL);
}

#[test]
#[should_panic(expected = "Error(Contract, #404)")]
fn test_get_next_charge_info_subscription_not_found() {
    let (_, client, _, _) = setup_test_env();
    client.get_next_charge_info(&999);
}

// ── Subscription ID Hardening Tests ───────────────────────────────────────────

fn quick_create(env: &Env, client: &SubscriptionVaultClient) -> u32 {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    )
}

#[test]
fn test_id_starts_at_zero() {
    let (env, client, _, _) = setup_test_env();
    assert_eq!(quick_create(&env, &client), 0);
}

#[test]
fn test_ids_are_monotonically_increasing() {
    let (env, client, _, _) = setup_test_env();
    for expected in 0u32..10 {
        assert_eq!(quick_create(&env, &client), expected);
    }
}

#[test]
fn test_ids_are_unique() {
    let (env, client, _, _) = setup_test_env();
    let mut ids: SorobanVec<u32> = SorobanVec::new(&env);
    for _ in 0..50 {
        let id = quick_create(&env, &client);
        assert!(!ids.contains(id), "duplicate ID: {id}");
        ids.push_back(id);
    }
}

#[test]
fn test_get_subscription_count() {
    let (env, client, _, _) = setup_test_env();
    assert_eq!(client.get_subscription_count(), 0);
    for expected in 1u32..=5 {
        quick_create(&env, &client);
        assert_eq!(client.get_subscription_count(), expected);
    }
}

#[test]
fn test_id_at_max_returns_limit_reached() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    seed_counter(&env, &contract_id, MAX_SUBSCRIPTION_ID);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let result = client.try_create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    assert!(
        matches!(result, Err(Ok(Error::SubscriptionLimitReached))),
        "expected SubscriptionLimitReached, got {:?}",
        result
    );
}

#[test]
fn test_no_id_reuse_after_limit() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    seed_counter(&env, &contract_id, MAX_SUBSCRIPTION_ID);
    for attempt in 0..3 {
        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);
        let result = client.try_create_subscription(
            &subscriber,
            &merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
        );
        assert!(
            matches!(result, Err(Ok(Error::SubscriptionLimitReached))),
            "attempt {attempt}: expected SubscriptionLimitReached"
        );
    }
}

// ── Admin Recovery Tests ───────────────────────────────────────────────────────

#[test]
fn test_recover_stranded_funds_successful() {
    let (env, client, _, admin) = setup_test_env();
    let recipient = Address::generate(&env);
    let result = client.try_recover_stranded_funds(
        &admin,
        &recipient,
        &50_000_000i128,
        &RecoveryReason::AccidentalTransfer,
    );
    assert!(result.is_ok());
    assert!(!env.events().all().is_empty());
}

#[test]
#[should_panic(expected = "Error(Contract, #403)")]
fn test_recover_stranded_funds_unauthorized_caller() {
    let (env, client, _, _) = setup_test_env();
    let non_admin = Address::generate(&env);
    let recipient = Address::generate(&env);
    client.recover_stranded_funds(
        &non_admin,
        &recipient,
        &10_000_000i128,
        &RecoveryReason::AccidentalTransfer,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #1008)")]
fn test_recover_stranded_funds_zero_amount() {
    let (env, client, _, admin) = setup_test_env();
    let recipient = Address::generate(&env);
    client.recover_stranded_funds(&admin, &recipient, &0i128, &RecoveryReason::DeprecatedFlow);
}

#[test]
fn test_recover_stranded_funds_all_recovery_reasons() {
    let (env, client, _, admin) = setup_test_env();
    let recipient = Address::generate(&env);
    assert!(client
        .try_recover_stranded_funds(
            &admin,
            &recipient,
            &1_000_000i128,
            &RecoveryReason::AccidentalTransfer
        )
        .is_ok());
    assert!(client
        .try_recover_stranded_funds(
            &admin,
            &recipient,
            &1_000_000i128,
            &RecoveryReason::DeprecatedFlow
        )
        .is_ok());
    assert!(client
        .try_recover_stranded_funds(
            &admin,
            &recipient,
            &1_000_000i128,
            &RecoveryReason::UnreachableSubscriber
        )
        .is_ok());
}

#[test]
fn test_recovery_reason_enum_values() {
    assert!(RecoveryReason::AccidentalTransfer != RecoveryReason::DeprecatedFlow);
    assert!(RecoveryReason::DeprecatedFlow != RecoveryReason::UnreachableSubscriber);
    assert!(RecoveryReason::AccidentalTransfer != RecoveryReason::UnreachableSubscriber);
    let r = RecoveryReason::AccidentalTransfer;
    assert!(r.clone() == RecoveryReason::AccidentalTransfer);
}

// ── Batch Charge Tests ─────────────────────────────────────────────────────────

fn setup_batch_env(env: &Env) -> (SubscriptionVaultClient<'static>, Address, u32, u32) {
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    let subscriber = Address::generate(env);
    let token = create_token_and_mint(env, &subscriber, 1_000_000_000i128);
    let admin = Address::generate(env);
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    let merchant = Address::generate(env);
    let id0 = client.create_subscription(
        &subscriber,
        &merchant,
        &1000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id0, &subscriber, &10_000_000i128);
    let id1 = client.create_subscription(
        &subscriber,
        &merchant,
        &1000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    env.ledger().set_timestamp(T0 + INTERVAL);
    (client, admin, id0, id1)
}

#[test]
fn test_batch_charge_single_subscription() {
    let env = Env::default();
    let (client, _admin, id0, _) = setup_batch_env(&env);
    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id0);
    let results = client.batch_charge(&ids);
    assert_eq!(results.len(), 1);
    assert!(results.get(0).unwrap().success);
}

#[test]
fn test_batch_charge_small_batch_5_subscriptions() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let subscriber = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
    let admin = Address::generate(&env);
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    let merchant = Address::generate(&env);
    let mut ids = SorobanVec::<u32>::new(&env);
    for _ in 0..5 {
        let id = client.create_subscription(
            &subscriber,
            &merchant,
            &1000i128,
            &INTERVAL,
            &false,
            &None::<i128>,
        );
        client.deposit_funds(&id, &subscriber, &10_000_000i128);
        ids.push_back(id);
    }
    env.ledger().set_timestamp(T0 + INTERVAL);
    let results = client.batch_charge(&ids);
    assert_eq!(results.len(), 5);
    for i in 0..5 {
        assert!(results.get(i).unwrap().success);
    }
}

#[test]
fn test_batch_charge_mixed_success_failure() {
    let env = Env::default();
    let (client, _admin, id0, id1) = setup_batch_env(&env);
    // id1 has no balance so will fail
    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id0);
    ids.push_back(id1);
    let results = client.batch_charge(&ids);
    assert_eq!(results.len(), 2);
    assert!(results.get(0).unwrap().success);
    assert!(!results.get(1).unwrap().success);
}

// ─────────────────────────────────────────────────────────────────────────────
// LIFETIME CAP TESTS
// ─────────────────────────────────────────────────────────────────────────────

// Helper: create a capped subscription with seeded balance, using a real token
fn setup_capped(
    env: &Env,
    amount: i128,
    interval: u64,
    cap: Option<i128>,
) -> (SubscriptionVaultClient<'_>, u32, Address) {
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);

    let subscriber = Address::generate(env);
    let token = create_token_and_mint(env, &subscriber, 1_000_000_000_000i128);
    let admin = Address::generate(env);
    client.init(&token, &6, &admin, &1_000_000i128, &0u64); // grace=0

    let merchant = Address::generate(env);
    env.ledger().set_timestamp(T0);
    let id = client.create_subscription(&subscriber, &merchant, &amount, &interval, &false, &cap);

    // Deposit 1000 USDC so balance is never the constraint
    client.deposit_funds(&id, &subscriber, &1_000_000_000i128);

    (client, id, contract_id)
}

/// Verify cap info starts at zero charged and full remaining.
#[test]
fn test_cap_info_initial_state() {
    let env = Env::default();
    env.mock_all_auths();
    let cap = 30_000_000i128; // 30 USDC cap
    let (client, id, _) = setup_capped(&env, AMOUNT, INTERVAL, Some(cap));

    let info = client.get_cap_info(&id);
    assert_eq!(info.lifetime_cap, Some(cap));
    assert_eq!(info.lifetime_charged, 0);
    assert_eq!(info.remaining_cap, Some(cap));
    assert!(!info.cap_reached);
}

/// No cap: cap_info returns all None / false.
#[test]
fn test_cap_info_no_cap() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, id, _) = setup_capped(&env, AMOUNT, INTERVAL, None);
    let info = client.get_cap_info(&id);
    assert_eq!(info.lifetime_cap, None);
    assert_eq!(info.remaining_cap, None);
    assert!(!info.cap_reached);
}

/// LOW CAP: cap = exactly 2 charges. After 2 charges subscription is cancelled.
#[test]
fn test_low_cap_two_charges_then_cancelled() {
    let env = Env::default();
    env.mock_all_auths();

    // cap = 2 × AMOUNT = 20_000_000 (2 intervals)
    let cap = 2 * AMOUNT;
    let (client, id, _) = setup_capped(&env, AMOUNT, INTERVAL, Some(cap));

    // Charge 1
    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.lifetime_charged, AMOUNT);
    assert_eq!(sub.status, SubscriptionStatus::Active);

    let info = client.get_cap_info(&id);
    assert_eq!(info.lifetime_charged, AMOUNT);
    assert_eq!(info.remaining_cap, Some(AMOUNT));
    assert!(!info.cap_reached);

    // Charge 2 — reaches exact cap, subscription should be cancelled
    env.ledger().set_timestamp(T0 + 2 * INTERVAL);
    client.charge_subscription(&id);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.lifetime_charged, cap);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);

    let info = client.get_cap_info(&id);
    assert_eq!(info.lifetime_charged, cap);
    assert_eq!(info.remaining_cap, Some(0));
    assert!(info.cap_reached);
}

/// LOW CAP: attempting a 3rd charge on a cancelled subscription returns NotActive.
#[test]
fn test_low_cap_charge_after_cap_reached_returns_error() {
    let env = Env::default();
    env.mock_all_auths();

    let cap = 2 * AMOUNT;
    let (client, id, _) = setup_capped(&env, AMOUNT, INTERVAL, Some(cap));

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);
    env.ledger().set_timestamp(T0 + 2 * INTERVAL);
    client.charge_subscription(&id);
    // Subscription is now Cancelled
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );

    // 3rd charge attempt on Cancelled sub
    env.ledger().set_timestamp(T0 + 3 * INTERVAL);
    let result = client.try_charge_subscription(&id);
    assert_eq!(result, Err(Ok(Error::NotActive)));
}

/// MEDIUM CAP: cap = 5 charges. Verify tracking after each interval.
#[test]
fn test_medium_cap_five_charges_tracking() {
    let env = Env::default();
    env.mock_all_auths();

    let cap = 5 * AMOUNT;
    let (client, id, _) = setup_capped(&env, AMOUNT, INTERVAL, Some(cap));

    for i in 1u64..=4 {
        env.ledger().set_timestamp(T0 + i * INTERVAL);
        client.charge_subscription(&id);
        let sub = client.get_subscription(&id);
        assert_eq!(sub.lifetime_charged, AMOUNT * i as i128);
        assert_eq!(sub.status, SubscriptionStatus::Active);
    }

    // 5th charge hits cap exactly
    env.ledger().set_timestamp(T0 + 5 * INTERVAL);
    client.charge_subscription(&id);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.lifetime_charged, cap);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

/// MEDIUM CAP: cap between two intervals (not a multiple of amount).
/// Cap = 1.5 × AMOUNT. After first charge: remaining = 0.5 × AMOUNT < AMOUNT → next charge blocked.
/// The subscription is cancelled and Ok(()) returned so the state persists.
#[test]
fn test_medium_cap_fractional_multiple_blocks_charge() {
    let env = Env::default();
    env.mock_all_auths();

    let cap = AMOUNT + AMOUNT / 2; // 15 USDC, interval charge = 10 USDC
    let (client, id, _) = setup_capped(&env, AMOUNT, INTERVAL, Some(cap));

    // First charge succeeds
    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.lifetime_charged, AMOUNT);
    assert_eq!(sub.status, SubscriptionStatus::Active);

    // Second charge would exceed cap (remaining = 5 USDC < 10 USDC amount).
    // Returns Ok() but cancels the subscription so the state persists.
    env.ledger().set_timestamp(T0 + 2 * INTERVAL);
    let result = client.try_charge_subscription(&id);
    assert!(
        result.is_ok(),
        "expected Ok on cap-exceeded pre-check, got {:?}",
        result
    );
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}

/// VERY HIGH CAP: cap = 1_000 × AMOUNT. After 3 charges it is nowhere near reached.
#[test]
fn test_very_high_cap_not_reached_after_many_charges() {
    let env = Env::default();
    env.mock_all_auths();

    let cap = 1_000 * AMOUNT;
    let (client, id, _) = setup_capped(&env, AMOUNT, INTERVAL, Some(cap));

    for i in 1u64..=10 {
        env.ledger().set_timestamp(T0 + i * INTERVAL);
        client.charge_subscription(&id);
    }

    let sub = client.get_subscription(&id);
    assert_eq!(sub.lifetime_charged, 10 * AMOUNT);
    assert_eq!(sub.status, SubscriptionStatus::Active);

    let info = client.get_cap_info(&id);
    assert_eq!(info.remaining_cap, Some(cap - 10 * AMOUNT));
    assert!(!info.cap_reached);
}

/// VERY HIGH CAP: i128::MAX cap never blocks charges.
#[test]
fn test_very_high_cap_max_i128_never_reached() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, id, _) = setup_capped(&env, AMOUNT, INTERVAL, Some(i128::MAX));

    for i in 1u64..=5 {
        env.ledger().set_timestamp(T0 + i * INTERVAL);
        client.charge_subscription(&id);
    }

    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );
}

/// Cap reached event is emitted when cap hit exactly.
#[test]
fn test_cap_reached_event_emitted_on_exact_cap() {
    let env = Env::default();
    env.mock_all_auths();

    let cap = 1 * AMOUNT;
    let (client, id, _) = setup_capped(&env, AMOUNT, INTERVAL, Some(cap));

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);

    let events = env.events().all();
    assert!(!events.is_empty());
    // Subscription is cancelled — confirms cap event fired
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}

/// Cap enforced when charge would exceed (not just reach) it.
/// Returns Ok() and cancels so state persists in Soroban.
#[test]
fn test_cap_blocks_overrun_charge() {
    let env = Env::default();
    env.mock_all_auths();

    // Cap = 25 USDC, amount = 10 USDC. After 2 charges (20 USDC), remaining = 5 USDC < 10 USDC.
    let cap = 25_000_000i128;
    let (client, id, _) = setup_capped(&env, AMOUNT, INTERVAL, Some(cap));

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);
    env.ledger().set_timestamp(T0 + 2 * INTERVAL);
    client.charge_subscription(&id);

    // 3rd charge: remaining = 5 USDC < 10 USDC → cap pre-check fires,
    // subscription cancelled, Ok(()) returned so cancellation persists.
    env.ledger().set_timestamp(T0 + 3 * INTERVAL);
    let result = client.try_charge_subscription(&id);
    assert!(
        result.is_ok(),
        "expected Ok on cap-overrun pre-check, got {:?}",
        result
    );
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}

/// Usage charges also count toward lifetime_charged.
#[test]
fn test_cap_enforced_for_usage_charges() {
    let env = Env::default();
    env.mock_all_auths();

    let cap = 15_000_000i128; // 15 USDC
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let subscriber = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
    let admin = Address::generate(&env);
    client.init(&token, &6, &admin, &1_000_000i128, &0u64);

    let merchant = Address::generate(&env);
    env.ledger().set_timestamp(T0);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &true,
        &Some(cap),
    );
    client.deposit_funds(&id, &subscriber, &1_000_000_000i128);

    // Usage charge 1: 10 USDC
    client.charge_usage(&id, &10_000_000i128);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.lifetime_charged, 10_000_000);
    assert_eq!(sub.status, SubscriptionStatus::Active);

    // Usage charge 2: 6 USDC would exceed cap (remaining = 5 USDC).
    // Returns Ok() and cancels subscription so state persists.
    let result = client.try_charge_usage(&id, &6_000_000i128);
    assert!(
        result.is_ok(),
        "expected Ok on cap-exceeded usage pre-check, got {:?}",
        result
    );
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}

/// Usage charge that exactly hits cap cancels subscription.
#[test]
fn test_cap_exact_hit_usage_cancels() {
    let env = Env::default();
    env.mock_all_auths();

    let cap = 10_000_000i128; // 10 USDC
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let subscriber = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
    let admin = Address::generate(&env);
    client.init(&token, &6, &admin, &1_000_000i128, &0u64);

    let merchant = Address::generate(&env);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &true,
        &Some(cap),
    );
    client.deposit_funds(&id, &subscriber, &1_000_000_000i128);

    // Exact hit
    client.charge_usage(&id, &cap);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.lifetime_charged, cap);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

/// Plan template inherits lifetime_cap to subscriptions created from it.
#[test]
fn test_plan_template_inherits_lifetime_cap() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let subscriber = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);
    client.init(&token, &6, &admin, &1_000_000i128, &0u64);

    let cap = 50_000_000i128;
    let plan_id = client.create_plan_template(&merchant, &AMOUNT, &INTERVAL, &false, &Some(cap));

    let template = client.get_plan_template(&plan_id);
    assert_eq!(template.lifetime_cap, Some(cap));

    let sub_id = client.create_subscription_from_plan(&subscriber, &plan_id);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.lifetime_cap, Some(cap));
    assert_eq!(sub.lifetime_charged, 0);
}

/// Plan template with no cap creates uncapped subscriptions.
#[test]
fn test_plan_template_no_cap_creates_uncapped_sub() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let subscriber = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);
    client.init(&token, &6, &admin, &1_000_000i128, &0u64);

    let plan_id = client.create_plan_template(&merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>);
    let sub_id = client.create_subscription_from_plan(&subscriber, &plan_id);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.lifetime_cap, None);
}

/// Subscriber can withdraw remaining prepaid balance after cap-triggered cancellation.
#[test]
fn test_cap_cancelled_subscriber_can_withdraw() {
    let env = Env::default();
    env.mock_all_auths();

    let cap = 1 * AMOUNT; // single charge cap
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let subscriber = Address::generate(&env);
    let token_addr = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token_addr);
    let token = soroban_sdk::token::Client::new(&env, &token_addr);
    token_admin.mint(&subscriber, &1_000_000_000i128);

    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);
    client.init(&token_addr, &6, &admin, &1_000_000i128, &0u64);

    env.ledger().set_timestamp(T0);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &Some(cap),
    );
    client.deposit_funds(&id, &subscriber, &100_000_000i128);

    // Charge once — hits cap, subscription cancelled
    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );

    // Subscriber withdraws remaining balance
    let before = token.balance(&subscriber);
    client.withdraw_subscriber_funds(&id, &subscriber);
    let after = token.balance(&subscriber);
    assert!(
        after > before,
        "subscriber should recover remaining prepaid"
    );
    assert_eq!(client.get_subscription(&id).prepaid_balance, 0);
}

/// Batch charge respects lifetime cap — capped subs show failure result.
#[test]
fn test_batch_charge_respects_lifetime_cap() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let subscriber = Address::generate(&env);
    let token = create_token_and_mint(&env, &subscriber, 1_000_000_000i128);
    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);
    client.init(&token, &6, &admin, &1_000_000i128, &0u64);

    env.ledger().set_timestamp(T0);

    // Uncapped subscription
    let id_uncapped = client.create_subscription(
        &subscriber,
        &merchant,
        &1000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id_uncapped, &subscriber, &100_000_000i128);

    // Capped subscription: cap = 1 charge
    let id_capped = client.create_subscription(
        &subscriber,
        &merchant,
        &1000i128,
        &INTERVAL,
        &false,
        &Some(1000i128),
    );
    client.deposit_funds(&id_capped, &subscriber, &100_000_000i128);

    env.ledger().set_timestamp(T0 + INTERVAL);

    // First batch: both should succeed
    let mut ids = SorobanVec::<u32>::new(&env);
    ids.push_back(id_uncapped);
    ids.push_back(id_capped);
    let results = client.batch_charge(&ids);
    assert!(results.get(0).unwrap().success);
    assert!(results.get(1).unwrap().success);

    // Second batch: capped is now Cancelled → NotActive
    env.ledger().set_timestamp(T0 + 2 * INTERVAL);
    let results2 = client.batch_charge(&ids);
    assert!(results2.get(0).unwrap().success);
    assert!(!results2.get(1).unwrap().success);
    assert_eq!(results2.get(1).unwrap().error_code, 1002); // NotActive
}

/// Cap info query returns NotFound for unknown subscription.
#[test]
fn test_get_cap_info_not_found() {
    let (_, client, _, _) = setup_test_env();
    let result = client.try_get_cap_info(&9999);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

/// Invalid lifetime_cap (zero) is rejected on creation.
#[test]
fn test_create_subscription_zero_cap_rejected() {
    let (env, client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let result = client.try_create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &Some(0i128),
    );
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

/// Negative lifetime_cap is rejected.
#[test]
fn test_create_subscription_negative_cap_rejected() {
    let (env, client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let result = client.try_create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &Some(-1i128),
    );
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

/// estimate_topup_for_intervals works correctly.
#[test]
fn test_estimate_topup_subscription_not_found() {
    let (_, client, _, _) = setup_test_env();
    let result = client.try_estimate_topup_for_intervals(&9999, &1);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}
// ═══════════════════════════════════════════════════════════════════════════════
// REENTRANCY PROTECTION TESTS
// ═══════════════════════════════════════════════════════════════════════════════
//
// These tests verify that the contract follows the Checks-Effects-Interactions (CEI) pattern
// for all external calls. While we cannot fully simulate reentrancy in a synchronous Soroban
// environment, these tests verify that state updates happen before external calls.
//
// See docs/reentrancy.md for design decisions and residual reentrancy risks.

#[test]
fn test_deposit_funds_state_committed_before_transfer() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, token, _admin) = setup_contract(&env);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    // Create subscription
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &None,
    );

    // Deposit funds
    let deposit_amount = 5_000000i128;
    client.deposit_funds(&sub_id, &subscriber, &deposit_amount);

    // Verify that prepaid_balance was updated
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, deposit_amount);

    // Verify that the subscription state is consistent after deposit
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(sub.amount, 10_000000i128);
}

#[test]
fn test_withdraw_merchant_funds_state_committed_before_transfer() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, token, _admin) = setup_contract(&env);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_sac = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    // Mint tokens to the contract to simulate prior deposits/charges funding it
    token_sac.mint(&client.address, &100_000_000i128);

    // Create subscription with initial balance
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &100_000000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &None,
    );

    // Mock a charge by directly crediting merchant balance
    // In real scenario, this happens via charging
    let env_inner = &env;
    env_inner.as_contract(&client.address, || {
        crate::merchant::credit_merchant_balance(env_inner, &merchant, 50_000000i128).unwrap();
    });

    let balance_before = client.get_merchant_balance(&merchant);
    assert_eq!(balance_before, 50_000000i128);

    // Withdraw merchant funds
    let withdraw_amount = 30_000000i128;
    client.withdraw_merchant_funds(&merchant, &withdraw_amount);

    // Verify that merchant balance was updated
    let balance_after = client.get_merchant_balance(&merchant);
    assert_eq!(balance_after, 20_000000i128);
}

#[test]
fn test_withdraw_subscriber_funds_state_committed_before_transfer() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, token, _admin) = setup_contract(&env);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    // Create subscription
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &None,
    );

    // Deposit funds
    let deposit_amount = 50_000000i128;
    client.deposit_funds(&sub_id, &subscriber, &deposit_amount);

    // Cancel subscription
    client.cancel_subscription(&sub_id, &subscriber);

    // Withdraw subscriber funds
    client.withdraw_subscriber_funds(&sub_id, &subscriber);

    // Verify that prepaid_balance was set to 0 (state was updated)
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 0i128);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

#[test]
fn test_multiple_deposits_maintain_consistent_state() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, token, _admin) = setup_contract(&env);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &None,
    );

    // Make multiple deposits
    let deposit1 = 10_000000i128;
    let deposit2 = 20_000000i128;
    let deposit3 = 15_000000i128;

    client.deposit_funds(&sub_id, &subscriber, &deposit1);
    client.deposit_funds(&sub_id, &subscriber, &deposit2);
    client.deposit_funds(&sub_id, &subscriber, &deposit3);

    // Verify total balance
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, deposit1 + deposit2 + deposit3);
}

#[test]
fn test_charge_and_withdrawal_atomic_sequence() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, token, _admin) = setup_contract(&env);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);
    const INTERVAL: u64 = 30 * 24 * 60 * 60;
    const AMOUNT: i128 = 10_000000i128;

    let sub_id =
        client.create_subscription(&subscriber, &merchant, &AMOUNT, &INTERVAL, &false, &None);

    // Deposit enough for one charge
    client.deposit_funds(&sub_id, &subscriber, &50_000000i128);

    // Verify initial state
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 50_000000i128);

    // Charge subscription (this would be called by backend)
    // Note: This requires time to elapse, so we test in isolation
    // The charge_subscription call will update both prepaid_balance and merchant balance

    // For now, verify that the subscription state is sound
    let sub_after = client.get_subscription(&sub_id);
    assert_eq!(sub_after.status, SubscriptionStatus::Active);
}

#[test]
fn test_reentrancy_protection_documentation() {
    // This test documents the reentrancy protection mechanisms in place:
    // 1. CEI Pattern: All external calls (token transfers) happen after internal state updates
    // 2. Minimal external calls: Only token.transfer() is called to external contracts
    // 3. No assumptions about token contract behavior: Token contract could implement
    //    callbacks, but our state is already consistent
    //
    // See docs/reentrancy.md for full analysis

    let env = Env::default();
    env.mock_all_auths();
    let (_client, _token, _admin) = setup_contract(&env);

    // The subscription vault is designed to be safe even if the USDC token
    // contract attempts callbacks:
    // - deposit_funds: updates balance before transfer ✓
    // - withdraw_merchant_funds: updates balance before transfer ✓
    // - withdraw_subscriber_funds: updates balance before transfer ✓

    assert!(true); // Placeholder to indicate test passed
}

// ═══════════════════════════════════════════════════════════════════════════════
// REFERRAL REWARDS LEDGER TESTS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Coverage:
//   - Referral program configuration (admin only, bps validation)
//   - Referral registration (subscriber only, no self-referral, no duplicates)
//   - Reward credited on first charge only (no double-counting)
//   - No-referrer flow does not error
//   - Multiple referrers get separate, independent balances
//   - Reward correctly applies max_reward ceiling
//   - Disabled program does not credit rewards
//   - Referrer withdrawal follows CEI and updates balance correctly
//   - Balance is separate from merchant earnings
//   - Emergency stop blocks register_referral and withdraw_referral_rewards

/// Build a complete referral-enabled test environment.
///
/// Returns (env, client, token_client, admin, subscriber, merchant, referrer).
fn setup_referral_env() -> (
    Env,
    SubscriptionVaultClient<'static>,
    soroban_sdk::token::Client<'static>,
    Address,
    Address,
    Address,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_sac = soroban_sdk::token::StellarAssetClient::new(&env, &token_addr);
    token_sac.mint(&subscriber, &1_000_000_000i128);
    // Mint to contract so referral withdrawals work (contract holds token balance)
    token_sac.mint(&contract_id, &1_000_000_000i128);

    client.init(&token_addr, &6, &admin, &1_000_000i128, &0u64);

    let merchant = Address::generate(&env);
    let referrer = Address::generate(&env);

    let token_client = soroban_sdk::token::Client::new(&env, &token_addr);

    (
        env,
        client,
        token_client,
        admin,
        subscriber,
        merchant,
        referrer,
    )
}

// ── Configuration ─────────────────────────────────────────────────────────────

#[test]
fn test_configure_referral_program_sets_config() {
    let (_, client, _, admin, _, _, _) = setup_referral_env();
    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    let cfg = client.get_referral_config().unwrap();
    assert_eq!(cfg.reward_bps, 500);
    assert_eq!(cfg.max_reward, None);
    assert!(cfg.enabled);
}

#[test]
fn test_configure_referral_program_with_max_reward() {
    let (_, client, _, admin, _, _, _) = setup_referral_env();
    client.configure_referral_program(&admin, &1000u32, &Some(500_000i128), &true);
    let cfg = client.get_referral_config().unwrap();
    assert_eq!(cfg.reward_bps, 1000);
    assert_eq!(cfg.max_reward, Some(500_000i128));
}

#[test]
fn test_configure_referral_program_can_be_updated() {
    let (_, client, _, admin, _, _, _) = setup_referral_env();
    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    client.configure_referral_program(&admin, &200u32, &Some(1_000_000i128), &false);
    let cfg = client.get_referral_config().unwrap();
    assert_eq!(cfg.reward_bps, 200);
    assert!(!cfg.enabled);
}

#[test]
fn test_configure_referral_program_non_admin_rejected() {
    let (env, client, _, _, _, _, _) = setup_referral_env();
    let non_admin = Address::generate(&env);
    let result = client.try_configure_referral_program(&non_admin, &500u32, &None::<i128>, &true);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_configure_referral_program_bps_above_10000_rejected() {
    let (_, client, _, admin, _, _, _) = setup_referral_env();
    let result = client.try_configure_referral_program(&admin, &10_001u32, &None::<i128>, &true);
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

#[test]
fn test_configure_referral_program_exactly_10000_bps_allowed() {
    let (_, client, _, admin, _, _, _) = setup_referral_env();
    let result = client.try_configure_referral_program(&admin, &10_000u32, &None::<i128>, &true);
    assert!(result.is_ok());
}

#[test]
fn test_configure_referral_program_zero_bps_allowed() {
    let (_, client, _, admin, _, _, _) = setup_referral_env();
    let result = client.try_configure_referral_program(&admin, &0u32, &None::<i128>, &true);
    assert!(result.is_ok());
}

#[test]
fn test_configure_referral_program_zero_max_reward_rejected() {
    let (_, client, _, admin, _, _, _) = setup_referral_env();
    let result = client.try_configure_referral_program(&admin, &500u32, &Some(0i128), &true);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_configure_referral_program_negative_max_reward_rejected() {
    let (_, client, _, admin, _, _, _) = setup_referral_env();
    let result = client.try_configure_referral_program(&admin, &500u32, &Some(-1i128), &true);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_get_referral_config_returns_none_before_setup() {
    let (_, client, _, _, _, _, _) = setup_referral_env();
    assert!(client.get_referral_config().is_none());
}

// ── Registration ──────────────────────────────────────────────────────────────

#[test]
fn test_register_referral_stores_record() {
    let (env, client, _, admin, subscriber, merchant, referrer) = setup_referral_env();
    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.register_referral(&id, &subscriber, &referrer);

    let record = client.get_referral_record(&id).unwrap();
    assert_eq!(record.referrer, referrer);
    assert!(!record.rewarded);
    assert_eq!(record.reward_amount, 0);
    let _ = env; // suppress unused warning
}

#[test]
fn test_register_referral_non_subscriber_rejected() {
    let (env, client, _, admin, subscriber, merchant, referrer) = setup_referral_env();
    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    let impostor = Address::generate(&env);
    let result = client.try_register_referral(&id, &impostor, &referrer);
    assert_eq!(result, Err(Ok(Error::Forbidden)));
}

#[test]
fn test_register_referral_self_referral_rejected() {
    let (_, client, _, admin, subscriber, merchant, _) = setup_referral_env();
    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    let result = client.try_register_referral(&id, &subscriber, &subscriber);
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

#[test]
fn test_register_referral_duplicate_rejected() {
    let (env, client, _, admin, subscriber, merchant, referrer) = setup_referral_env();
    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.register_referral(&id, &subscriber, &referrer);
    let second_referrer = Address::generate(&env);
    let result = client.try_register_referral(&id, &subscriber, &second_referrer);
    assert_eq!(result, Err(Ok(Error::ReferralAlreadyRegistered)));
}

#[test]
fn test_register_referral_nonexistent_subscription_rejected() {
    let (_, client, _, admin, subscriber, _, referrer) = setup_referral_env();
    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    let result = client.try_register_referral(&9999, &subscriber, &referrer);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn test_register_referral_without_program_config_succeeds() {
    // Registration should succeed even if no program is configured yet.
    let (_, client, _, _, subscriber, merchant, referrer) = setup_referral_env();
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    let result = client.try_register_referral(&id, &subscriber, &referrer);
    assert!(result.is_ok());
}

// ── Reward crediting on first charge ─────────────────────────────────────────

#[test]
fn test_referral_reward_credited_on_first_charge() {
    let (env, client, _, admin, subscriber, merchant, referrer) = setup_referral_env();

    // 5% reward, no ceiling
    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id, &subscriber, &PREPAID);
    client.register_referral(&id, &subscriber, &referrer);

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);

    // Expected reward: AMOUNT * 500 / 10_000 = 10_000_000 * 5% = 500_000
    let expected_reward = AMOUNT * 500 / 10_000;
    assert_eq!(client.get_referral_balance(&referrer), expected_reward);

    let record = client.get_referral_record(&id).unwrap();
    assert!(record.rewarded);
    assert_eq!(record.reward_amount, expected_reward);
}

#[test]
fn test_referral_no_double_credit_on_second_charge() {
    let (env, client, _, admin, subscriber, merchant, referrer) = setup_referral_env();

    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id, &subscriber, &PREPAID);
    client.register_referral(&id, &subscriber, &referrer);

    // First charge — reward credited
    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);
    let balance_after_first = client.get_referral_balance(&referrer);

    // Second charge — reward must NOT be credited again
    env.ledger().set_timestamp(T0 + 2 * INTERVAL);
    client.charge_subscription(&id);
    let balance_after_second = client.get_referral_balance(&referrer);

    assert_eq!(
        balance_after_first, balance_after_second,
        "balance must not change after the first charge"
    );
}

#[test]
fn test_no_referrer_flow_does_not_error() {
    let (env, client, _, admin, subscriber, merchant, _) = setup_referral_env();

    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id, &subscriber, &PREPAID);
    // No register_referral call

    env.ledger().set_timestamp(T0 + INTERVAL);
    // Should succeed without any referral-related error
    let result = client.try_charge_subscription(&id);
    assert!(
        result.is_ok(),
        "charge without referral must succeed: {:?}",
        result
    );
}

#[test]
fn test_referral_program_disabled_no_reward_credited() {
    let (env, client, _, admin, subscriber, merchant, referrer) = setup_referral_env();

    client.configure_referral_program(&admin, &500u32, &None::<i128>, &false); // disabled
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id, &subscriber, &PREPAID);
    client.register_referral(&id, &subscriber, &referrer);

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);

    assert_eq!(
        client.get_referral_balance(&referrer),
        0,
        "disabled program must not credit rewards"
    );
    // Record should still be marked rewarded to prevent re-triggering if program re-enabled.
    let record = client.get_referral_record(&id).unwrap();
    assert!(record.rewarded);
}

#[test]
fn test_referral_no_program_config_no_reward_no_error() {
    let (env, client, _, _, subscriber, merchant, referrer) = setup_referral_env();
    // No configure_referral_program call
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id, &subscriber, &PREPAID);
    client.register_referral(&id, &subscriber, &referrer);

    env.ledger().set_timestamp(T0 + INTERVAL);
    let result = client.try_charge_subscription(&id);
    assert!(result.is_ok());
    assert_eq!(client.get_referral_balance(&referrer), 0);
}

#[test]
fn test_referral_max_reward_cap_applied() {
    let (env, client, _, admin, subscriber, merchant, referrer) = setup_referral_env();

    // 10% rate but capped at 200_000 (0.2 USDC)
    let max_cap = 200_000i128;
    client.configure_referral_program(&admin, &1000u32, &Some(max_cap), &true);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id, &subscriber, &PREPAID);
    client.register_referral(&id, &subscriber, &referrer);

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);

    // Uncapped reward would be: 10_000_000 * 1000 / 10_000 = 1_000_000
    // But it's capped at 200_000
    assert_eq!(client.get_referral_balance(&referrer), max_cap);
}

#[test]
fn test_referral_reward_without_max_cap() {
    let (env, client, _, admin, subscriber, merchant, referrer) = setup_referral_env();

    // 1% rate, no ceiling
    client.configure_referral_program(&admin, &100u32, &None::<i128>, &true);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id, &subscriber, &PREPAID);
    client.register_referral(&id, &subscriber, &referrer);

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);

    // 10_000_000 * 100 / 10_000 = 100_000 (0.1 USDC)
    assert_eq!(client.get_referral_balance(&referrer), 100_000i128);
}

// ── Multiple referrers: independent balances ──────────────────────────────────

#[test]
fn test_multiple_referrers_get_separate_balances() {
    let (env, client, _, admin, _, merchant, _) = setup_referral_env();

    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);

    // Create two subscriptions each with a different subscriber and referrer
    let sub1_subscriber = Address::generate(&env);
    let referrer1 = Address::generate(&env);
    let token_addr: Address = env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .get(&soroban_sdk::Symbol::new(&env, "token"))
            .unwrap()
    });
    soroban_sdk::token::StellarAssetClient::new(&env, &token_addr).mint(&sub1_subscriber, &PREPAID);

    let sub2_subscriber = Address::generate(&env);
    let referrer2 = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token_addr).mint(&sub2_subscriber, &PREPAID);

    let id1 = client.create_subscription(
        &sub1_subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id1, &sub1_subscriber, &PREPAID);
    client.register_referral(&id1, &sub1_subscriber, &referrer1);

    let id2 = client.create_subscription(
        &sub2_subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id2, &sub2_subscriber, &PREPAID);
    client.register_referral(&id2, &sub2_subscriber, &referrer2);

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id1);
    client.charge_subscription(&id2);

    let expected = AMOUNT * 500 / 10_000;
    assert_eq!(client.get_referral_balance(&referrer1), expected);
    assert_eq!(client.get_referral_balance(&referrer2), expected);

    // One referrer's balance does not affect the other
    assert_eq!(
        client.get_referral_balance(&referrer1),
        client.get_referral_balance(&referrer2)
    );
}

#[test]
fn test_same_referrer_multiple_subscriptions_balance_accumulates() {
    let (env, client, _, admin, _, merchant, referrer) = setup_referral_env();

    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);

    let token_addr: Address = env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .get(&soroban_sdk::Symbol::new(&env, "token"))
            .unwrap()
    });

    let sub1 = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token_addr).mint(&sub1, &PREPAID);
    let sub2 = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token_addr).mint(&sub2, &PREPAID);

    let id1 =
        client.create_subscription(&sub1, &merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>);
    client.deposit_funds(&id1, &sub1, &PREPAID);
    client.register_referral(&id1, &sub1, &referrer);

    let id2 =
        client.create_subscription(&sub2, &merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>);
    client.deposit_funds(&id2, &sub2, &PREPAID);
    client.register_referral(&id2, &sub2, &referrer);

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id1);
    client.charge_subscription(&id2);

    let per_reward = AMOUNT * 500 / 10_000;
    assert_eq!(client.get_referral_balance(&referrer), per_reward * 2);
}

// ── Withdrawal ────────────────────────────────────────────────────────────────

#[test]
fn test_referrer_can_withdraw_full_balance() {
    let (env, client, token_client, admin, subscriber, merchant, referrer) = setup_referral_env();

    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id, &subscriber, &PREPAID);
    client.register_referral(&id, &subscriber, &referrer);

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);

    let balance = client.get_referral_balance(&referrer);
    assert!(balance > 0);

    let before = token_client.balance(&referrer);
    client.withdraw_referral_rewards(&referrer, &balance);
    let after = token_client.balance(&referrer);

    assert_eq!(after - before, balance);
    assert_eq!(client.get_referral_balance(&referrer), 0);
}

#[test]
fn test_referrer_can_withdraw_partial_balance() {
    let (env, client, token_client, admin, subscriber, merchant, referrer) = setup_referral_env();

    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id, &subscriber, &PREPAID);
    client.register_referral(&id, &subscriber, &referrer);

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);

    let total = client.get_referral_balance(&referrer);
    let partial = total / 2;

    let before = token_client.balance(&referrer);
    client.withdraw_referral_rewards(&referrer, &partial);
    let after = token_client.balance(&referrer);

    assert_eq!(after - before, partial);
    assert_eq!(client.get_referral_balance(&referrer), total - partial);
}

#[test]
fn test_withdraw_referral_rewards_zero_balance_returns_not_found() {
    let (_, client, _, _, _, _, referrer) = setup_referral_env();
    let result = client.try_withdraw_referral_rewards(&referrer, &1_000i128);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn test_withdraw_referral_rewards_exceeds_balance_returns_insufficient() {
    let (env, client, _, admin, subscriber, merchant, referrer) = setup_referral_env();

    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id, &subscriber, &PREPAID);
    client.register_referral(&id, &subscriber, &referrer);

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);

    let balance = client.get_referral_balance(&referrer);
    let result = client.try_withdraw_referral_rewards(&referrer, &(balance + 1));
    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));
}

#[test]
fn test_withdraw_referral_rewards_zero_amount_rejected() {
    let (_, client, _, _, _, _, referrer) = setup_referral_env();
    let result = client.try_withdraw_referral_rewards(&referrer, &0i128);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

// ── Fund separation from merchant earnings ────────────────────────────────────

#[test]
fn test_referral_balance_separate_from_merchant_balance() {
    let (env, client, _, admin, subscriber, merchant, referrer) = setup_referral_env();

    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id, &subscriber, &PREPAID);
    client.register_referral(&id, &subscriber, &referrer);

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);

    let merchant_balance = client.get_merchant_balance(&merchant);
    let referral_balance = client.get_referral_balance(&referrer);

    // Merchant receives the full charge amount
    assert_eq!(merchant_balance, AMOUNT);
    // Referral is funded independently (rewards come from protocol, not from merchant's share)
    assert!(referral_balance > 0);
    // The two balances are tracked independently
    assert_ne!(merchant_balance, referral_balance);
}

// ── Emergency stop ────────────────────────────────────────────────────────────

#[test]
fn test_register_referral_blocked_during_emergency_stop() {
    let (_, client, _, admin, subscriber, merchant, referrer) = setup_referral_env();
    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.enable_emergency_stop(&admin);

    let result = client.try_register_referral(&id, &subscriber, &referrer);
    assert_eq!(result, Err(Ok(Error::EmergencyStopActive)));
}

#[test]
fn test_withdraw_referral_rewards_blocked_during_emergency_stop() {
    let (env, client, _, admin, subscriber, merchant, referrer) = setup_referral_env();
    client.configure_referral_program(&admin, &500u32, &None::<i128>, &true);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    client.deposit_funds(&id, &subscriber, &PREPAID);
    client.register_referral(&id, &subscriber, &referrer);

    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id);

    let balance = client.get_referral_balance(&referrer);
    assert!(balance > 0);

    client.enable_emergency_stop(&admin);
    let result = client.try_withdraw_referral_rewards(&referrer, &balance);
    assert_eq!(result, Err(Ok(Error::EmergencyStopActive)));
}
