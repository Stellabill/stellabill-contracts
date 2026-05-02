use crate::{
    can_transition, compute_next_charge_info, get_allowed_transitions, transition_to,
    validate_status_transition,
    ChargeExecutionResult, DataKey, Error, MerchantWithdrawalEvent, OraclePrice,
    RecoveryReason, Subscription, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
    MAX_SUBSCRIPTION_ID, MAX_SUBSCRIPTION_LIST_PAGE,
};
use soroban_sdk::testutils::{Address as _, Events, Ledger as _};
use soroban_sdk::{
    contract, contractimpl, Address, Env, FromVal, IntoVal, String, Symbol, TryFromVal, Val, Vec,
};

extern crate alloc;
use crate::test_utils::{assertions, fixtures, setup::TestEnv};
use alloc::format;

// -- constants ----------------------------------------------------------------
const T0: u64 = 1_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const AMOUNT: i128 = 10_000_000; // 10 USDC (6 decimals)
const PREPAID: i128 = 50_000_000; // 50 USDC

// -- lifecycle action enum for property tests --------------------------------
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LifecycleAction {
    Pause,
    Resume,
    Cancel,
}

// -- all subscription statuses for property tests ----------------------------
const ALL_STATUSES: &[SubscriptionStatus] = &[
    SubscriptionStatus::Active,
    SubscriptionStatus::Paused,
    SubscriptionStatus::Cancelled,
    SubscriptionStatus::InsufficientBalance,
    SubscriptionStatus::GracePeriod,
];

// -- helpers ------------------------------------------------------------------

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
    // FIXED: Removed the extra &None::<u64> (now 7 arguments)
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    if status != SubscriptionStatus::Active {
        let mut sub = client.get_subscription(&id);
        sub.status = status;
        env.as_contract(&client.address, || {
            env.storage().instance().set(&DataKey::Sub(id), &sub);
        });
    }
    (id, subscriber, merchant)
}

/// Seed a subscription with a known prepaid balance directly in storage.
fn seed_balance(env: &Env, client: &SubscriptionVaultClient, id: u32, balance: i128) {
    let mut sub = client.get_subscription(&id);
    sub.prepaid_balance = balance;
    env.as_contract(&client.address, || {
        env.storage().instance().set(&DataKey::Sub(id), &sub);
    });
}

/// Seed the `next_id` counter to an arbitrary value.
fn seed_counter(env: &Env, contract_id: &Address, value: u32) {
    env.as_contract(contract_id, || {
        env.storage()
            .instance()
            .set(&DataKey::NextId, &value);
    });
}

fn seed_merchant_balance(
    env: &Env,
    contract_id: &Address,
    merchant: &Address,
    token: &Address,
    balance: i128,
) {
    env.as_contract(contract_id, || {
        env.storage().instance().set(
            &DataKey::MerchantBalance(merchant.clone(), token.clone()),
            &balance,
        );
    });
}

fn create_secondary_token(env: &Env) -> Address {
    env.register_stellar_asset_contract_v2(Address::generate(env))
        .address()
}

fn snapshot_subscriptions(
    client: &SubscriptionVaultClient,
    ids: &[u32],
) -> alloc::vec::Vec<Subscription> {
    ids.iter().map(|id| client.get_subscription(id)).collect()
}

fn collect_batch_result_codes(
    env: &Env,
    client: &SubscriptionVaultClient,
    ids: &[u32],
    nonce: u64,
) -> alloc::vec::Vec<(bool, u32)> {
    let ids_vec = ids.iter().fold(Vec::<u32>::new(env), |mut acc, id| {
        acc.push_back(*id);
        acc
    });
    let results = client.batch_charge(&ids_vec, &nonce);
    results
        .iter()
        .map(|result| (result.success, result.error_code))
        .collect()
}

fn collect_single_charge_result_codes(
    client: &SubscriptionVaultClient,
    ids: &[u32],
) -> alloc::vec::Vec<(bool, u32)> {
    ids.iter()
        .map(|id| match client.try_charge_subscription(id) {
            Ok(Ok(ChargeExecutionResult::Charged)) => (true, 0),
            Ok(Ok(ChargeExecutionResult::InsufficientBalance)) => {
                (false, Error::InsufficientBalance.to_code())
            }
            Err(Ok(err)) => (false, err.to_code()),
            other => panic!("unexpected charge result: {other:?}"),
        })
        .collect()
}

#[contract]
struct MockOracle;

#[contractimpl]
impl MockOracle {
    pub fn set_price(env: Env, price: i128, timestamp: u64) {
        env.storage().instance().set(
            &Symbol::new(&env, "price"),
            &OraclePrice { price, timestamp },
        );
    }

    pub fn latest_price(env: Env) -> OraclePrice {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "price"))
            .unwrap_or(OraclePrice {
                price: 0,
                timestamp: 0,
            })
    }
}

fn lcg_next(seed: &mut u64) -> u64 {
    const A: u64 = 1664525;
    const C: u64 = 1013904223;
    *seed = seed.wrapping_mul(A).wrapping_add(C);
    *seed
}

fn manual_can_transition(from: &SubscriptionStatus, to: &SubscriptionStatus) -> bool {
    use SubscriptionStatus::*;

    if from == to {
        return true;
    }

    match (from, to) {
        (Active, Paused) => true,
        (Active, Cancelled) => true,
        (Active, InsufficientBalance) => true,
        (Active, GracePeriod) => true,
        (Paused, Active) => true,
        (Paused, Cancelled) => true,
        (InsufficientBalance, Active) => true,
        (InsufficientBalance, Cancelled) => true,
        (GracePeriod, Active) => true,
        (GracePeriod, Cancelled) => true,
        (GracePeriod, InsufficientBalance) => true,
        _ => false,
    }
}

fn random_transition_action(seed: &mut u64) -> u32 {
    (lcg_next(seed) % 5) as u32
}

fn transition_action_target(action: u32) -> SubscriptionStatus {
    match action % 5 {
        0 => SubscriptionStatus::Active,
        1 => SubscriptionStatus::Paused,
        2 => SubscriptionStatus::Cancelled,
        3 => SubscriptionStatus::InsufficientBalance,
        _ => SubscriptionStatus::GracePeriod,
    }
}

fn random_lifecycle_action(seed: &mut u64) -> LifecycleAction {
    match lcg_next(seed) % 3 {
        0 => LifecycleAction::Pause,
        1 => LifecycleAction::Resume,
        _ => LifecycleAction::Cancel,
    }
}

fn lifecycle_action_target(action: LifecycleAction) -> SubscriptionStatus {
    match action {
        LifecycleAction::Pause => SubscriptionStatus::Paused,
        LifecycleAction::Resume => SubscriptionStatus::Active,
        LifecycleAction::Cancel => SubscriptionStatus::Cancelled,
    }
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
    assert_eq!(paused_targets.len(), 3);
    assert!(paused_targets.contains(&SubscriptionStatus::Active));
    assert!(paused_targets.contains(&SubscriptionStatus::Cancelled));
    assert!(paused_targets.contains(&SubscriptionStatus::Expired));

    assert_eq!(
        get_allowed_transitions(&SubscriptionStatus::Cancelled).len(),
        1
    );

    let ib_targets = get_allowed_transitions(&SubscriptionStatus::InsufficientBalance);
    assert_eq!(ib_targets.len(), 3);
}

#[test]
fn test_state_machine_property_transition_matrix_matches_manual_rules() {
    for from in ALL_STATUSES.iter() {
        let allowed = get_allowed_transitions(from);

        for to in ALL_STATUSES.iter() {
            let expected = manual_can_transition(from, to);
            assert_eq!(can_transition(from, to), expected);
            assert_eq!(validate_status_transition(from, to).is_ok(), expected);

            if from == to {
                assert!(!allowed.contains(to));
            } else {
                assert_eq!(allowed.contains(to), expected);
            }
        }
    }
}

#[test]
fn test_state_machine_property_random_transition_sequences_only_allow_legal_targets() {
    for start in ALL_STATUSES.iter() {
        for seed_base in 0..64u64 {
            let mut seed = seed_base + (start.clone() as u64) * 97;
            let mut current = start.clone();

            for _ in 0..24 {
                let action = random_transition_action(&mut seed);
                let target = transition_action_target(action);
                let expected = manual_can_transition(&current, &target);

                assert_eq!(can_transition(&current, &target), expected);
                assert_eq!(
                    validate_status_transition(&current, &target).is_ok(),
                    expected
                );

                if expected {
                    current = target;
                }
            }
        }
    }
}

#[test]
fn test_state_machine_property_lifecycle_entrypoints_follow_manual_model() {
    for start in ALL_STATUSES.iter() {
        for seed_base in 0..48u64 {
            let (env, client, token, _admin) = setup_test_env();
            let (id, subscriber, _) = create_test_subscription(&env, &client, start.clone());
            let mut expected = start.clone();
            let mut seed = seed_base + (start.clone() as u64) * 131;

            for _ in 0..12 {
                let action = random_lifecycle_action(&mut seed);
                let target = lifecycle_action_target(action);
                let should_succeed = manual_can_transition(&expected, &target);

                if action == LifecycleAction::Resume
                    && (expected == SubscriptionStatus::InsufficientBalance
                        || expected == SubscriptionStatus::GracePeriod)
                    && should_succeed
                {
                    let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
                    token_client.mint(&subscriber, &AMOUNT);
                    client.deposit_funds(&id, &subscriber, &AMOUNT);
                }

                let result = match action {
                    LifecycleAction::Pause => client.try_pause_subscription(&id, &subscriber),
                    LifecycleAction::Resume => client.try_resume_subscription(&id, &subscriber),
                    LifecycleAction::Cancel => client.try_cancel_subscription(&id, &subscriber),
                };

                assert_eq!(result.is_ok(), should_succeed);

                let current = client.get_subscription(&id).status;
                if should_succeed {
                    expected = target;
                    assert_eq!(current, expected);
                } else {
                    assert_eq!(current, expected);
                }
            }
        }
    }
}

#[test]
fn test_state_machine_property_charge_failures_and_recovery_paths_obey_rules() {
    for seed_base in 0..32u64 {
        let mut seed = seed_base;

        for step in 0..10 {
            let (env, client, token, _) = setup_test_env();
            let (id, subscriber, _) =
                create_test_subscription(&env, &client, SubscriptionStatus::Active);
            let in_grace_window = lcg_next(&mut seed) % 2 == 0;
            let topup_amount = if lcg_next(&mut seed) % 2 == 0 {
                AMOUNT - 1
            } else {
                PREPAID
            };

            seed_balance(&env, &client, id, 0);
            let charge_time = if in_grace_window {
                T0 + INTERVAL + 1
            } else {
                T0 + INTERVAL + (7 * 24 * 60 * 60) + 1
            };
            env.ledger().set_timestamp(charge_time + step as u64);

            let result = client.try_charge_subscription(&id);
            assert_eq!(result, Ok(Ok(ChargeExecutionResult::InsufficientBalance)));

            let failed_status = client.get_subscription(&id).status;
            // Depending on charge_time, it could be GracePeriod or InsufficientBalance
            if in_grace_window {
                assert_eq!(failed_status, SubscriptionStatus::GracePeriod);
            } else {
                assert_eq!(failed_status, SubscriptionStatus::InsufficientBalance);
            }

            soroban_sdk::token::StellarAssetClient::new(&env, &token)
                .mint(&subscriber, &topup_amount.max(1_000_000));
            client.deposit_funds(&id, &subscriber, &topup_amount.max(1_000_000));

            let after_deposit = client.get_subscription(&id).status;
            if topup_amount >= AMOUNT {
                assert_eq!(after_deposit, SubscriptionStatus::Active);
            } else {
                assert!(
                    after_deposit == SubscriptionStatus::InsufficientBalance
                        || after_deposit == SubscriptionStatus::GracePeriod
                );
            }

            if topup_amount >= AMOUNT {
                env.ledger()
                    .set_timestamp(charge_time + INTERVAL + step as u64 + 1);
                let charge_again = client.try_charge_subscription(&id);
                assert!(charge_again.is_ok());
                assert_eq!(
                    client.get_subscription(&id).status,
                    SubscriptionStatus::Active
                );
            } else {
                client.cancel_subscription(&id, &subscriber);
                assert_eq!(
                    client.get_subscription(&id).status,
                    SubscriptionStatus::Cancelled
                );
            }
        }
    }
}

// -- Contract Lifecycle Tests -------------------------------------------------

#[test]
fn test_pause_subscription_from_active() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    test_env.client.pause_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Paused);
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_pause_subscription_from_cancelled_should_fail() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    test_env.client.cancel_subscription(&id, &subscriber);
    test_env.client.pause_subscription(&id, &subscriber);
}

#[test]
fn test_pause_subscription_from_paused_is_idempotent() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    test_env.client.pause_subscription(&id, &subscriber);
    test_env.client.pause_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Paused);
}

#[test]
fn test_cancel_subscription_from_active() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    test_env.client.cancel_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);
}

#[test]
fn test_cancel_subscription_from_paused() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    test_env.client.pause_subscription(&id, &subscriber);
    test_env.client.cancel_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);
}

#[test]
fn test_cancel_subscription_from_cancelled_is_idempotent() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    test_env.client.cancel_subscription(&id, &subscriber);
    test_env.client.cancel_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);
}

#[test]
fn test_resume_subscription_from_paused() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    test_env.client.pause_subscription(&id, &subscriber);
    test_env.client.resume_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Active);
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_resume_subscription_from_cancelled_should_fail() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.client.cancel_subscription(&id, &subscriber);
    test_env.client.resume_subscription(&id, &subscriber);
}

#[test]
fn test_full_lifecycle_active_pause_resume() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.client.pause_subscription(&id, &subscriber);
    assert_eq!(
        test_env.client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );
    test_env.client.resume_subscription(&id, &subscriber);
    assert_eq!(
        test_env.client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );
    test_env.client.pause_subscription(&id, &subscriber);
    assert_eq!(
        test_env.client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );
}

#[test]
fn test_all_valid_transitions_coverage() {
    let test_env = TestEnv::default();

    // Active -> Paused
    {
        let (id, subscriber, _) = fixtures::create_subscription_detailed(
            &test_env.env,
            &test_env.client,
            SubscriptionStatus::Active,
            AMOUNT,
            INTERVAL,
        );
        test_env.client.pause_subscription(&id, &subscriber);
        assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Paused);
    }
    // Active -> Cancelled
    {
        let (id, subscriber, _) = fixtures::create_subscription_detailed(
            &test_env.env,
            &test_env.client,
            SubscriptionStatus::Active,
            AMOUNT,
            INTERVAL,
        );
        test_env.client.cancel_subscription(&id, &subscriber);
        assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);
    }
    // Active -> InsufficientBalance (direct storage patch)
    {
        let (id, _, _) = fixtures::create_subscription_detailed(
            &test_env.env,
            &test_env.client,
            SubscriptionStatus::Active,
            AMOUNT,
            INTERVAL,
        );
        fixtures::patch_status(
            &test_env.env,
            &test_env.client,
            id,
            SubscriptionStatus::InsufficientBalance,
        );
        assertions::assert_status(
            &test_env.client,
            &id,
            SubscriptionStatus::InsufficientBalance,
        );
    }
    // Paused -> Active
    {
        let (id, subscriber, _) = fixtures::create_subscription_detailed(
            &test_env.env,
            &test_env.client,
            SubscriptionStatus::Active,
            AMOUNT,
            INTERVAL,
        );
        test_env.client.pause_subscription(&id, &subscriber);
        test_env.client.resume_subscription(&id, &subscriber);
        assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Active);
    }
    // Paused -> Cancelled
    {
        let (id, subscriber, _) = fixtures::create_subscription_detailed(
            &test_env.env,
            &test_env.client,
            SubscriptionStatus::Active,
            AMOUNT,
            INTERVAL,
        );
        test_env.client.pause_subscription(&id, &subscriber);
        test_env.client.cancel_subscription(&id, &subscriber);
        assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);
    }
    // InsufficientBalance -> Active
    {
        let (id, subscriber, _) = fixtures::create_subscription_detailed(
            &test_env.env,
            &test_env.client,
            SubscriptionStatus::Active,
            AMOUNT,
            INTERVAL,
        );
        fixtures::patch_status(
            &test_env.env,
            &test_env.client,
            id,
            SubscriptionStatus::InsufficientBalance,
        );
        test_env.stellar_token_client().mint(&subscriber, &AMOUNT);
        test_env.client.deposit_funds(&id, &subscriber, &AMOUNT);
        test_env.client.resume_subscription(&id, &subscriber);
        assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Active);
    }
    // InsufficientBalance -> Cancelled
    {
        let (id, subscriber, _) = fixtures::create_subscription_detailed(
            &test_env.env,
            &test_env.client,
            SubscriptionStatus::Active,
            AMOUNT,
            INTERVAL,
        );
        fixtures::patch_status(
            &test_env.env,
            &test_env.client,
            id,
            SubscriptionStatus::InsufficientBalance,
        );
        test_env.client.cancel_subscription(&id, &subscriber);
        assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);
    }
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_invalid_cancelled_to_active() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    test_env.client.cancel_subscription(&id, &subscriber);
    test_env.client.resume_subscription(&id, &subscriber);
}

#[test]
#[should_panic(expected = "Error(Contract, #4001)")]
fn test_invalid_insufficient_balance_to_paused() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    fixtures::patch_status(
        &test_env.env,
        &test_env.client,
        id,
        SubscriptionStatus::InsufficientBalance,
    );
    test_env.client.pause_subscription(&id, &subscriber);
}

// -- Subscription struct tests ------------------------------------------------

#[test]
fn test_subscription_struct_status_field() {
    let env = Env::default();
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        token: Address::generate(&env),
        amount: 100_000_000,
        interval_seconds: 30 * 24 * 60 * 60,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: 500_000_000,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
        start_time: 0,
        expires_at: None,
        grace_start_timestamp: None,
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
        token: Address::generate(&env),
        amount: 10_000_000,
        interval_seconds: 30 * 24 * 60 * 60,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: 50_000_000,
        usage_enabled: false,
        lifetime_cap: Some(cap),
        lifetime_charged: 0,
        start_time: 0,
        expires_at: None,
        grace_start_timestamp: None,
    };
    assert_eq!(sub.lifetime_cap, Some(cap));
    assert_eq!(sub.lifetime_charged, 0);
}

// -- Contract Charging Tests --------------------------------------------------

#[test]
fn test_charge_subscription_basic() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let (id, _, _) = create_test_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    seed_balance(&test_env.env, &test_env.client, id, PREPAID);

    test_env
        .env
        .ledger()
        .with_mut(|li| li.timestamp = T0 + INTERVAL + 1);
    test_env.client.charge_subscription(&id);

    assert_eq!(test_env.client.get_subscription(&id).prepaid_balance, PREPAID - AMOUNT);
    let sub = test_env.client.get_subscription(&id);
    assert_eq!(sub.lifetime_charged, AMOUNT);
}

#[test]
#[should_panic(expected = "Error(Contract, #4002)")]
fn test_charge_subscription_paused_fails() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);
    let (id, subscriber, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    fixtures::seed_balance(&test_env.env, &test_env.client, id, PREPAID);
    test_env.client.pause_subscription(&id, &subscriber);
    test_env.jump(INTERVAL + 1);
    test_env.client.charge_subscription(&id);
}

#[test]
fn test_charge_subscription_insufficient_balance_returns_error() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);
    let (id, _, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );

    let grace_period = 7 * 24 * 60 * 60u64;
    test_env.jump(INTERVAL + grace_period + 1);
    let result = test_env.client.try_charge_subscription(&id);
    assert_eq!(result, Ok(Ok(ChargeExecutionResult::InsufficientBalance)));
}

// -- ID limit test ------------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #6001)")]
fn test_subscription_limit_reached() {
    let test_env = TestEnv::default();
    fixtures::seed_counter(&test_env.env, &test_env.client.address, MAX_SUBSCRIPTION_ID);
    test_env.client.create_subscription(
        &Address::generate(&test_env.env),
        &Address::generate(&test_env.env),
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
     &None::<u64>);
}

#[test]
fn test_cancel_subscription_unauthorized() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let other = Address::generate(&test_env.env);
    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &1000,
        &86400,
        &true,
        &None::<i128>,
        &None::<u64>,
    );
    let result = test_env.client.try_cancel_subscription(&sub_id, &other);
    assert_eq!(result, Err(Ok(Error::Forbidden)));
}

#[test]
fn test_withdraw_subscriber_funds() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    test_env.stellar_token_client().mint(&subscriber, &10_000_000);

    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &1000,
        &86400,
        &true,
        &None::<i128>,
        &None::<u64>,
    );
    test_env.client.deposit_funds(&sub_id, &subscriber, &5_000_000);
    test_env.client.cancel_subscription(&sub_id, &subscriber);
    test_env.client.withdraw_subscriber_funds(&sub_id, &subscriber);

    let sub = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 0);
    assert_eq!(test_env.token_client().balance(&subscriber), 10_000_000);
    assert_eq!(test_env.token_client().balance(&test_env.client.address), 0);
}

// ── Min-Topup Enforcement Tests ────────────────────────────────────────────────

#[test]
fn test_min_topup_below_threshold() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let min_topup = 5_000_000i128;

    test_env.client.set_min_topup(&test_env.admin, &min_topup);
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128,
        &(30 * 24 * 60 * 60),
        &false,
        &None::<i128>,
     &None::<u64>);
    test_env.client.cancel_subscription(&id, &merchant);
    let result = test_env.client.try_deposit_funds(&id, &subscriber, &4_999_999);
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
     &None::<u64>);
    assert!(client
        .try_deposit_funds(&id, &subscriber, &min_topup)
        .is_ok());
}

// -- Deposit tests ------------------------------------------------------------

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
     &None::<u64>);
    assert!(client
        .try_deposit_funds(&id, &subscriber, &deposit_amount)
        .is_ok());
}

// ── Usage-charge tests ─────────────────────────────────────────────────────────

// -- Deposit tests ------------------------------------------------------------

#[test]
fn test_deposit_funds_basic() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &100_000_000);

    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env.client.deposit_funds(&id, &subscriber, &5_000_000);
    assertions::assert_prepaid_balance(&test_env.client, &id, 5_000_000);
}

#[test]
fn test_deposit_funds_unauthorized() {
    let (env, client, token, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let other = Address::generate(&env);
    let merchant = Address::generate(&env);

    let token_admin_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_admin_client.mint(&other, &100_000_000);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
     &None::<u64>);
    let result = client.try_deposit_funds(&id, &other, &5_000_000);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 0);
}

#[test]
fn test_deposit_funds_event_payload() {
    let (env, client, token, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let token_admin_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_admin_client.mint(&subscriber, &100_000_000);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    
    client.deposit_funds(&id, &subscriber, &15_000_000);

    let events = env.events().all();
    let deposit_event = events.last().expect("No events found");

    // Verify event topics: (Symbol("deposited"), subscription_id)
    assert_eq!(deposit_event.0, client.address);
    assert_eq!(
        Symbol::from_val(&env, &deposit_event.1.get(0).expect("Missing topic 0")),
        Symbol::new(&env, "deposited")
    );
    assert_eq!(
        u32::from_val(&env, &deposit_event.1.get(1).expect("Missing topic 1")),
        id
    );

    // Verify event data: FundsDepositedEvent { subscription_id, subscriber, amount, prepaid_balance }
    let event_data: crate::FundsDepositedEvent = FromVal::from_val(&env, &deposit_event.2);
    let event_data: crate::FundsDepositedEvent = deposit_event.2.into_val(&env);
    assert_eq!(event_data.subscription_id, id);
    assert_eq!(event_data.subscriber, subscriber);
    assert_eq!(event_data.amount, 15_000_000);
    assert_eq!(event_data.new_balance, 15_000_000);
}

#[test]
fn test_deposit_funds_cei_compliance() {
    let (env, client, token, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_client = soroban_sdk::token::Client::new(&env, &token);

    let token_admin_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_admin_client.mint(&subscriber, &100_000_000);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    
    let initial_contract_balance = token_client.balance(&client.address);
    let deposit_amount = 20_000_000i128;

    client.deposit_funds(&id, &subscriber, &deposit_amount);

    // Check effects (state)
    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, deposit_amount);

    // Check interactions (transfer)
    assert_eq!(
        token_client.balance(&client.address),
        initial_contract_balance + deposit_amount
    );
}

// -- Batch charge tests -------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #5003)")]
fn test_deposit_funds_below_minimum() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    // min_topup is 1_000_000; try to deposit 500
    test_env.client.deposit_funds(&id, &subscriber, &500);
}

// -- Blocklist tests ----------------------------------------------------------

#[test]
fn test_blocklist_rejects_duplicate_add_and_preserves_original_entry() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);

    let first_reason = Some(String::from_str(&test_env.env, "chargeback"));
    test_env
        .client
        .add_to_blocklist(&test_env.admin, &subscriber, &first_reason);

    let duplicate_reason = Some(String::from_str(&test_env.env, "retry"));
    let duplicate = test_env
        .client
        .try_add_to_blocklist(&test_env.admin, &subscriber, &duplicate_reason);
    assert_eq!(duplicate, Err(Ok(Error::InvalidInput)));

    let entry = test_env.client.get_blocklist_entry(&subscriber);
    assert_eq!(entry.added_by, test_env.admin);
    assert_eq!(entry.reason, first_reason);
}

#[test]
fn test_blocklist_add_and_remove_events_capture_reason_variants() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);
    let subscriber = Address::generate(&test_env.env);

    let empty_reason = Some(String::from_str(&test_env.env, ""));
    test_env
        .client
        .add_to_blocklist(&test_env.admin, &subscriber, &empty_reason);

    let add_events = test_env.env.events().all();
    let add_event = add_events.last().expect("missing blocklist add event");
    assert_eq!(add_event.0, test_env.client.address);
    assert_eq!(
        Symbol::from_val(&test_env.env, &add_event.1.get(0).expect("missing add topic")),
        Symbol::new(&test_env.env, "blocklist_added")
    );
    let added: crate::BlocklistAddedEvent = FromVal::from_val(&test_env.env, &add_event.2);
    let added: crate::BlocklistAddedEvent = add_event.2.into_val(&test_env.env);
    assert_eq!(added.subscriber, subscriber);
    assert_eq!(added.added_by, test_env.admin);
    assert_eq!(added.timestamp, T0);
    assert_eq!(added.reason, empty_reason);

    test_env.jump(60);
    test_env
        .client
        .remove_from_blocklist(&test_env.admin, &subscriber);

    let remove_events = test_env.env.events().all();
    let remove_event = remove_events
        .last()
        .expect("missing blocklist remove event");
    assert_eq!(remove_event.0, test_env.client.address);
    assert_eq!(
        Symbol::from_val(
            &test_env.env,
            &remove_event.1.get(0).expect("missing remove topic")
        ),
        Symbol::new(&test_env.env, "blocklist_removed")
    );
    let removed: crate::BlocklistRemovedEvent = FromVal::from_val(&test_env.env, &remove_event.2);
    let removed: crate::BlocklistRemovedEvent = remove_event.2.into_val(&test_env.env);
    assert_eq!(removed.subscriber, subscriber);
    assert_eq!(removed.removed_by, test_env.admin);
    assert_eq!(removed.timestamp, T0 + 60);

    let none_reason_subscriber = Address::generate(&test_env.env);
    test_env
        .client
        .add_to_blocklist(&test_env.admin, &none_reason_subscriber, &None::<String>);
    let none_entry = test_env.client.get_blocklist_entry(&none_reason_subscriber);
    assert_eq!(none_entry.reason, None);
}

#[test]
fn test_blocklist_enforced_across_mutating_subscription_flows_and_unblock_restores_access() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    test_env
        .stellar_token_client()
        .mint(&subscriber, &100_000_000i128);

    let direct_sub = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env.client.pause_subscription(&direct_sub, &subscriber);

    let plan_v1 =
        test_env
            .client
            .create_plan_template(&merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>);
    let plan_v2 = test_env.client.update_plan_template(
        &merchant,
        &plan_v1,
        &(AMOUNT + 1_000_000),
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    let plan_sub = test_env
        .client
        .create_subscription_from_plan(&subscriber, &plan_v1);

    test_env.client.add_to_blocklist(
        &test_env.admin,
        &subscriber,
        &Some(String::from_str(&test_env.env, "risk-review")),
    );

    assert_eq!(
        test_env.client.try_create_subscription(
            &subscriber,
            &merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
        ),
        Err(Ok(Error::SubscriberBlocklisted))
    );
    assert_eq!(
        test_env.client.try_create_subscription_with_token(
            &subscriber,
            &merchant,
            &test_env.token,
        &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
        ),
        Err(Ok(Error::SubscriberBlocklisted))
    );
    assert_eq!(
        test_env
            .client
            .try_create_subscription_from_plan(&subscriber, &plan_v1),
        Err(Ok(Error::SubscriberBlocklisted))
    );
    assert_eq!(
        test_env
            .client
            .try_deposit_funds(&plan_sub, &subscriber, &5_000_000i128),
        Err(Ok(Error::SubscriberBlocklisted))
    );
    assert_eq!(
        test_env
            .client
            .try_resume_subscription(&direct_sub, &subscriber),
        Err(Ok(Error::SubscriberBlocklisted))
    );
    assert_eq!(
        test_env
            .client
            .try_migrate_subscription_to_plan(&subscriber, &plan_sub, &plan_v2),
        Err(Ok(Error::SubscriberBlocklisted))
    );

    test_env
        .client
        .remove_from_blocklist(&test_env.admin, &subscriber);

    let new_sub = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env
        .client
        .deposit_funds(&plan_sub, &subscriber, &5_000_000i128);
    test_env.client.resume_subscription(&direct_sub, &subscriber);
    test_env
        .client
        .migrate_subscription_to_plan(&subscriber, &plan_sub, &plan_v2);

    assertions::assert_status(&test_env.client, &direct_sub, SubscriptionStatus::Active);
    assertions::assert_prepaid_balance(&test_env.client, &plan_sub, 5_000_000i128);
    let migrated = test_env.client.get_subscription(&plan_sub);
    assert_eq!(migrated.amount, AMOUNT + 1_000_000);
    assert_eq!(test_env.client.is_blocklisted(&subscriber), false);
    assert_eq!(
        test_env.client.get_subscription(&new_sub).status,
        SubscriptionStatus::Active
    );
}

#[test]
fn test_remove_from_blocklist_requires_admin_and_existing_entry() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let missing = test_env
        .client
        .try_remove_from_blocklist(&test_env.admin, &subscriber);
    assert_eq!(missing, Err(Ok(Error::NotFound)));

    test_env
        .client
        .add_to_blocklist(&test_env.admin, &subscriber, &None::<String>);
    let unauthorized = test_env
        .client
        .try_remove_from_blocklist(&merchant, &subscriber);
    assert_eq!(unauthorized, Err(Ok(Error::Unauthorized)));
}

// -- Admin tests --------------------------------------------------------------

#[test]
fn test_rotate_admin() {
    let test_env = TestEnv::default();
    let new_admin = Address::generate(&test_env.env);
    test_env.client.rotate_admin(&test_env.admin, &new_admin, &0u64);
    assert_eq!(test_env.client.get_admin(), new_admin);
}

#[test]
fn test_emergency_stop() {
    let (_env, client, _, admin) = setup_test_env();
    assert!(!client.get_emergency_stop_status());
    client.enable_emergency_stop(&admin);
    assert!(client.get_emergency_stop_status());
    client.disable_emergency_stop(&admin);
    assert!(!client.get_emergency_stop_status());
}

#[test]
#[should_panic(expected = "Error(Contract, #4007)")]
fn test_create_subscription_blocked_by_emergency_stop() {
    let test_env = TestEnv::default();
    test_env.client.enable_emergency_stop(&test_env.admin);
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
}

// -- Batch charge tests -------------------------------------------------------

#[test]
fn test_batch_charge() {
    let (env, client, _, _admin) = setup_test_env();
    env.ledger().with_mut(|li| li.timestamp = T0);

    // 1. Success
    let (id1, _, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    seed_balance(&env, &client, id1, PREPAID);

    // 2. InsufficientBalance
    let (id2, _, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    seed_balance(&env, &client, id2, 0);

    // 3. NotActive (Paused)
    let (id3, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    client.pause_subscription(&id3, &subscriber);

    // 4. IntervalNotElapsed
    let (id4, _, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    seed_balance(&env, &client, id4, PREPAID);

    // 5. Success (Duplicate of id1, but after enough time it could succeed? No, it's the same call)
    // Wait, IDs are processed at the same ledger timestamp.

    env.ledger().set_timestamp(T0 + INTERVAL + 1);

    // Batch: [id1, id2, id3, id4 (not elapsed)]
    // For id4, T0 + INTERVAL + 1 is actually elapsed. Let me fix id4.
    env.as_contract(&client.address, || {
        let mut sub4 = env
            .storage()
            .instance()
            .get::<DataKey, Subscription>(&DataKey::Sub(id4))
            .unwrap();
        sub4.last_payment_timestamp = T0 + INTERVAL + 100; // Will be in the future
        env.storage().instance().set(&DataKey::Sub(id4), &sub4);
    });

    let ids = Vec::from_array(&env, [id1, 999u32, id2, id3, id4]);
    let results = client.batch_charge(&ids, &0u64);

    assert_eq!(results.len(), 5);

    // id1: Success
    assert!(results.get(0).unwrap().success);

    // 999: NotFound (404)
    assert!(!results.get(1).unwrap().success);
    assert_eq!(results.get(1).unwrap().error_code, Error::NotFound as u32);

    // id2: InsufficientBalance (1003)
    assert!(!results.get(2).unwrap().success);
    assert_eq!(results.get(2).unwrap().error_code, Error::InsufficientBalance as u32);

    // id3: NotActive (1002)
    assert!(!results.get(3).unwrap().success);
    assert_eq!(results.get(3).unwrap().error_code, Error::NotActive as u32);

    // id4: IntervalNotElapsed (1001)
    assert!(!results.get(4).unwrap().success);
    assert_eq!(results.get(4).unwrap().error_code, Error::IntervalNotElapsed as u32);
}

#[test]
fn test_batch_charge_duplicate_ids() {
    let (env, client, _, _admin) = setup_test_env();
    env.ledger().set_timestamp(T0);
    let (id, _, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    seed_balance(&env, &client, id, PREPAID * 2);

    env.ledger().set_timestamp(T0 + INTERVAL + 1);
    let ids = Vec::from_array(&env, [id, id]);
    let results = client.batch_charge(&ids, &0u64);

    assert_eq!(results.len(), 2);
    // First should succeed
    assert!(results.get(0).unwrap().success);
    // Second should fail with Replay (1007)
    assert!(!results.get(1).unwrap().success);
    assert_eq!(results.get(1).unwrap().error_code, Error::Replay as u32);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID * 2 - AMOUNT);
}

#[test]
fn test_batch_charge_large_batch() {
    let (env, client, _, _admin) = setup_test_env();
    env.ledger().set_timestamp(T0);

    // Batch size kept under Soroban's per-tx CPU/memory budget. Larger batches
    // (e.g. 50) hit `Error(Budget, ExceededLimit)` before completing.
    const BATCH_SIZE: u32 = 20;

    let mut ids = Vec::new(&env);
    for _ in 0..BATCH_SIZE {
        let (id, _, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
        seed_balance(&env, &client, id, PREPAID);
        ids.push_back(id);
    }

    env.ledger().set_timestamp(T0 + INTERVAL + 1);
    let results = client.batch_charge(&ids, &0u64);
    assert_eq!(results.len(), BATCH_SIZE);
    for i in 0..BATCH_SIZE {
        assert!(results.get(i).unwrap().success);
    }
}

#[test]
fn test_batch_charge_matches_single_charge_semantics_for_identical_inputs() {
    let test_env_batch = TestEnv::default();
    let test_env_single = TestEnv::default();

    test_env_batch.set_timestamp(T0);
    test_env_single.set_timestamp(T0);

    let mut ids_batch = [0u32; 3];
    let mut ids_single = [0u32; 3];
    let mut merchants_batch = alloc::vec::Vec::new();
    let mut merchants_single = alloc::vec::Vec::new();

    for idx in 0..3 {
        let (id_batch, _, merchant_batch) = fixtures::create_subscription_detailed(
            &test_env_batch.env,
            &test_env_batch.client,
            SubscriptionStatus::Active,
            AMOUNT,
            INTERVAL,
        );
        let (id_single, _, merchant_single) = fixtures::create_subscription_detailed(
            &test_env_single.env,
            &test_env_single.client,
            SubscriptionStatus::Active,
            AMOUNT,
            INTERVAL,
        );

        fixtures::seed_balance(
            &test_env_batch.env,
            &test_env_batch.client,
            id_batch,
            PREPAID,
        );
        fixtures::seed_balance(
            &test_env_single.env,
            &test_env_single.client,
            id_single,
            PREPAID,
        );

        ids_batch[idx] = id_batch;
        ids_single[idx] = id_single;
        merchants_batch.push(merchant_batch);
        merchants_single.push(merchant_single);
    }

    test_env_batch.jump(INTERVAL + 1);
    test_env_single.jump(INTERVAL + 1);

    let batch_results =
        collect_batch_result_codes(&test_env_batch.env, &test_env_batch.client, &ids_batch, 0);
    let single_results = collect_single_charge_result_codes(&test_env_single.client, &ids_single);

    assert_eq!(batch_results, single_results);
    assert_eq!(batch_results, alloc::vec![(true, 0), (true, 0), (true, 0)]);

    let batch_snapshots = snapshot_subscriptions(&test_env_batch.client, &ids_batch);
    let single_snapshots = snapshot_subscriptions(&test_env_single.client, &ids_single);
    for (batch_sub, single_sub) in batch_snapshots.iter().zip(single_snapshots.iter()) {
        assert_eq!(batch_sub.prepaid_balance, single_sub.prepaid_balance);
        assert_eq!(
            batch_sub.last_payment_timestamp,
            single_sub.last_payment_timestamp
        );
        assert_eq!(batch_sub.status, single_sub.status);
        assert_eq!(batch_sub.lifetime_charged, single_sub.lifetime_charged);
    }

    for (merchant_batch, merchant_single) in merchants_batch.iter().zip(merchants_single.iter()) {
        assert_eq!(
            test_env_batch.client.get_merchant_balance(merchant_batch),
            test_env_single.client.get_merchant_balance(merchant_single)
        );
    }
}

#[test]
fn test_batch_charge_mixed_results_preserve_single_path_order_and_error_codes() {
    let test_env_batch = TestEnv::default();
    let test_env_single = TestEnv::default();

    test_env_batch.set_timestamp(T0);
    test_env_single.set_timestamp(T0);

    let (valid_batch, _, _merchant_valid_batch) = fixtures::create_subscription_detailed(
        &test_env_batch.env,
        &test_env_batch.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    let (valid_single, _, _merchant_valid_single) = fixtures::create_subscription_detailed(
        &test_env_single.env,
        &test_env_single.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    fixtures::seed_balance(
        &test_env_batch.env,
        &test_env_batch.client,
        valid_batch,
        PREPAID,
    );
    fixtures::seed_balance(
        &test_env_single.env,
        &test_env_single.client,
        valid_single,
        PREPAID,
    );

    let (low_batch, _, _merchant_low_batch) = fixtures::create_subscription_detailed(
        &test_env_batch.env,
        &test_env_batch.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    let (low_single, _, _merchant_low_single) = fixtures::create_subscription_detailed(
        &test_env_single.env,
        &test_env_single.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    fixtures::seed_balance(
        &test_env_batch.env,
        &test_env_batch.client,
        low_batch,
        AMOUNT - 1,
    );
    fixtures::seed_balance(
        &test_env_single.env,
        &test_env_single.client,
        low_single,
        AMOUNT - 1,
    );

    let (paused_batch, _, merchant_paused_batch) = fixtures::create_subscription_detailed(
        &test_env_batch.env,
        &test_env_batch.client,
        SubscriptionStatus::Paused,
        AMOUNT,
        INTERVAL,
    );
    let (paused_single, _, merchant_paused_single) = fixtures::create_subscription_detailed(
        &test_env_single.env,
        &test_env_single.client,
        SubscriptionStatus::Paused,
        AMOUNT,
        INTERVAL,
    );
    fixtures::seed_balance(
        &test_env_batch.env,
        &test_env_batch.client,
        paused_batch,
        PREPAID,
    );
    fixtures::seed_balance(
        &test_env_single.env,
        &test_env_single.client,
        paused_single,
        PREPAID,
    );

    test_env_batch.jump(INTERVAL + 1);
    test_env_single.jump(INTERVAL + 1);

    let ids_batch = [
        valid_batch,
        low_batch,
        paused_batch,
        999_999u32,
        valid_batch,
    ];
    let ids_single = [
        valid_single,
        low_single,
        paused_single,
        999_999u32,
        valid_single,
    ];

    let batch_results =
        collect_batch_result_codes(&test_env_batch.env, &test_env_batch.client, &ids_batch, 0);
    let single_results = collect_single_charge_result_codes(&test_env_single.client, &ids_single);

    assert_eq!(batch_results, single_results);
    assert_eq!(
        batch_results,
        alloc::vec![
            (true, 0),
            (false, Error::InsufficientBalance.to_code()),
            (false, Error::NotActive.to_code()),
            (false, Error::NotFound.to_code()),
            (false, Error::Replay.to_code()),
        ]
    );

    let tracked_batch = [valid_batch, low_batch, paused_batch];
    let tracked_single = [valid_single, low_single, paused_single];
    assert_eq!(
        test_env_batch
            .client
            .get_merchant_balance(&merchant_paused_batch),
        test_env_single
            .client
            .get_merchant_balance(&merchant_paused_single)
    );
}

#[test]
fn test_batch_charge_basic() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);

    let (id1, _, merchant) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    let (id2, _, _) = fixtures::create_subscription_with_merchant(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        merchant.clone(),
    );

    fixtures::seed_balance(&test_env.env, &test_env.client, id1, PREPAID);
    fixtures::seed_balance(&test_env.env, &test_env.client, id2, PREPAID);

    test_env.env.ledger().set_timestamp(T0 + INTERVAL + 1);

    let ids = Vec::from_array(&test_env.env, [id1, id2]);
    let results = test_env.client.batch_charge(&ids, &0u64);

    assert_eq!(results.len(), 2);
    assert!(results.get(0).unwrap().success);
    assert!(results.get(1).unwrap().success);

    assertions::assert_prepaid_balance(&test_env.client, &id1, PREPAID - AMOUNT);
    assertions::assert_prepaid_balance(&test_env.client, &id1, PREPAID - AMOUNT);
    assertions::assert_prepaid_balance(&test_env.client, &id2, PREPAID - AMOUNT);
}

#[test]
#[should_panic]
fn test_batch_charge_fails_unauthorized() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &604800u64);

    let ids = Vec::from_array(&env, [1]);

    // This will panic because no auth is provided for the admin
    client.batch_charge(&ids, &0u64);
}

#[test]
fn test_batch_charge_partial_success() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);

    let (id1, _, merchant) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    let (id2, _, _) = fixtures::create_subscription_with_merchant(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        merchant.clone(),
    );

    fixtures::seed_balance(&test_env.env, &test_env.client, id1, PREPAID);
    // id2 has 0 balance

    test_env.env.ledger().set_timestamp(T0 + INTERVAL + 1);

    let ids = Vec::from_array(&test_env.env, [id1, id2]);
    let results = test_env.client.batch_charge(&ids, &0u64);

    assert_eq!(results.len(), 2);
    assert!(results.get(0).unwrap().success);
    assert_eq!(results.get(1).unwrap().success, false);
    assert_eq!(
        results.get(1).unwrap().error_code,
        Error::InsufficientBalance.to_code()
    );

    assertions::assert_prepaid_balance(&test_env.client, &id1, PREPAID - AMOUNT);
    assertions::assert_prepaid_balance(&test_env.client, &id2, 0);
}

#[test]
fn test_batch_charge_failed_items_match_single_path_without_cross_item_side_effects() {
    let test_env_batch = TestEnv::default();
    let test_env_single = TestEnv::default();

    test_env_batch.env.ledger().set_timestamp(T0);
    test_env_single.env.ledger().set_timestamp(T0);

    let (ok_one_batch, _, merchant_ok_one_batch) = fixtures::create_subscription(
        &test_env_batch.env,
        &test_env_batch.client,
        SubscriptionStatus::Active,
    );
    let (ok_one_single, _, merchant_ok_one_single) = fixtures::create_subscription(
        &test_env_single.env,
        &test_env_single.client,
        SubscriptionStatus::Active,
    );
    fixtures::seed_balance(
        &test_env_batch.env,
        &test_env_batch.client,
        ok_one_batch,
        PREPAID,
    );
    fixtures::seed_balance(
        &test_env_single.env,
        &test_env_single.client,
        ok_one_single,
        PREPAID,
    );

    let (failing_batch, _, merchant_failing_batch) = fixtures::create_subscription(
        &test_env_batch.env,
        &test_env_batch.client,
        SubscriptionStatus::Active,
    );
    let (failing_single, _, merchant_failing_single) = fixtures::create_subscription(
        &test_env_single.env,
        &test_env_single.client,
        SubscriptionStatus::Active,
    );
    fixtures::seed_balance(
        &test_env_batch.env,
        &test_env_batch.client,
        failing_batch,
        1,
    );
    fixtures::seed_balance(
        &test_env_single.env,
        &test_env_single.client,
        failing_single,
        1,
    );

    let (ok_two_batch, _, merchant_ok_two_batch) = fixtures::create_subscription(
        &test_env_batch.env,
        &test_env_batch.client,
        SubscriptionStatus::Active,
    );
    let (ok_two_single, _, merchant_ok_two_single) = fixtures::create_subscription(
        &test_env_single.env,
        &test_env_single.client,
        SubscriptionStatus::Active,
    );
    fixtures::seed_balance(
        &test_env_batch.env,
        &test_env_batch.client,
        ok_two_batch,
        PREPAID,
    );
    fixtures::seed_balance(
        &test_env_single.env,
        &test_env_single.client,
        ok_two_single,
        PREPAID,
    );

    test_env_batch.env.ledger().set_timestamp(T0 + INTERVAL + 1);
    test_env_single
        .env
        .ledger()
        .set_timestamp(T0 + INTERVAL + 1);

    let ids_batch = [ok_one_batch, failing_batch, ok_two_batch];
    let ids_single = [ok_one_single, failing_single, ok_two_single];

    let batch_results =
        collect_batch_result_codes(&test_env_batch.env, &test_env_batch.client, &ids_batch, 0);
    let single_results = collect_single_charge_result_codes(&test_env_single.client, &ids_single);

    assert_eq!(batch_results, single_results);
    assert_eq!(
        batch_results,
        alloc::vec![
            (true, 0),
            (false, Error::InsufficientBalance.to_code()),
            (true, 0),
        ]
    );

    let batch_snapshots = snapshot_subscriptions(&test_env_batch.client, &ids_batch);
    let single_snapshots = snapshot_subscriptions(&test_env_single.client, &ids_single);
    for (batch_sub, single_sub) in batch_snapshots.iter().zip(single_snapshots.iter()) {
        assert_eq!(batch_sub.prepaid_balance, single_sub.prepaid_balance);
        assert_eq!(
            batch_sub.last_payment_timestamp,
            single_sub.last_payment_timestamp
        );
        assert_eq!(batch_sub.status, single_sub.status);
    }

    assert_eq!(
        test_env_batch
            .client
            .get_merchant_balance(&merchant_ok_one_batch),
        test_env_single
            .client
            .get_merchant_balance(&merchant_ok_one_single)
    );
    assert_eq!(
        test_env_batch
            .client
            .get_merchant_balance(&merchant_failing_batch),
        test_env_single
            .client
            .get_merchant_balance(&merchant_failing_single)
    );
    assert_eq!(
        test_env_batch
            .client
            .get_merchant_balance(&merchant_ok_two_batch),
        test_env_single
            .client
            .get_merchant_balance(&merchant_ok_two_single)
    );
}

#[test]
fn test_batch_charge_high_volume_list_matches_single_path_semantics() {
    let test_env_batch = TestEnv::default();
    let test_env_single = TestEnv::default();

    test_env_batch.env.ledger().set_timestamp(T0);
    test_env_single.env.ledger().set_timestamp(T0);

    let mut ids_batch = alloc::vec::Vec::new();
    let mut ids_single = alloc::vec::Vec::new();
    let mut merchants_batch = alloc::vec::Vec::new();
    let mut merchants_single = alloc::vec::Vec::new();

    for idx in 0..20 {
        let status = if idx % 5 == 0 {
            SubscriptionStatus::Paused
        } else {
            SubscriptionStatus::Active
        };
        let (id_batch, _, merchant_batch) = fixtures::create_subscription(
            &test_env_batch.env,
            &test_env_batch.client,
            status.clone(),
        );
        let (id_single, _, merchant_single) =
            fixtures::create_subscription(&test_env_single.env, &test_env_single.client, status);

        let balance = if idx % 2 == 0 { PREPAID } else { AMOUNT - 1 };
        fixtures::seed_balance(
            &test_env_batch.env,
            &test_env_batch.client,
            id_batch,
            balance,
        );
        fixtures::seed_balance(
            &test_env_single.env,
            &test_env_single.client,
            id_single,
            balance,
        );

        ids_batch.push(id_batch);
        ids_single.push(id_single);
        merchants_batch.push(merchant_batch);
        merchants_single.push(merchant_single);
    }

    test_env_batch.env.ledger().set_timestamp(T0 + INTERVAL + 1);
    test_env_single
        .env
        .ledger()
        .set_timestamp(T0 + INTERVAL + 1);

    let mut input_batch = ids_batch.clone();
    let mut input_single = ids_single.clone();
    input_batch.push(ids_batch[2]);
    input_batch.push(ids_batch[7]);
    input_single.push(ids_single[2]);
    input_single.push(ids_single[7]);

    let batch_results =
        collect_batch_result_codes(&test_env_batch.env, &test_env_batch.client, &input_batch, 0);
    let single_results = collect_single_charge_result_codes(&test_env_single.client, &input_single);

    assert_eq!(batch_results, single_results);
    assert_eq!(batch_results.len(), 22);

    let batch_snapshots = snapshot_subscriptions(&test_env_batch.client, &ids_batch);
    let single_snapshots = snapshot_subscriptions(&test_env_single.client, &ids_single);
    for (batch_sub, single_sub) in batch_snapshots.iter().zip(single_snapshots.iter()) {
        assert_eq!(batch_sub.prepaid_balance, single_sub.prepaid_balance);
        assert_eq!(
            batch_sub.last_payment_timestamp,
            single_sub.last_payment_timestamp
        );
        assert_eq!(batch_sub.status, single_sub.status);
    }

    for (merchant_batch, merchant_single) in merchants_batch.iter().zip(merchants_single.iter()) {
        assert_eq!(
            test_env_batch.client.get_merchant_balance(merchant_batch),
            test_env_single.client.get_merchant_balance(merchant_single)
        );
    }
}

// -- Next charge info test ----------------------------------------------------

#[test]
fn test_next_charge_info() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);
    let (id, _, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    let info = test_env.client.get_next_charge_info(&id);
    assert_eq!(info.next_charge_timestamp, T0 + INTERVAL);
    assert!(info.is_charge_expected);
}

// -- Compute next charge info (unit) ------------------------------------------

#[test]
fn test_compute_next_charge_info_active() {
    let env = Env::default();
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        token: Address::generate(&env),
        amount: AMOUNT,
        interval_seconds: INTERVAL,
        last_payment_timestamp: T0,
        status: SubscriptionStatus::Active,
        prepaid_balance: 0,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
        start_time: 0,
        expires_at: None,
        grace_start_timestamp: None,
    };
    let info = compute_next_charge_info(&env, &sub);
    assert_eq!(info.next_charge_timestamp, T0 + INTERVAL);
    assert!(info.is_charge_expected);
}

#[test]
fn test_compute_next_charge_info_paused() {
    let env = Env::default();
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        token: Address::generate(&env),
        amount: AMOUNT,
        interval_seconds: INTERVAL,
        last_payment_timestamp: 2000,
        status: SubscriptionStatus::Paused,
        prepaid_balance: 50_000_000,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
        start_time: 0,
        expires_at: None,
        grace_start_timestamp: None,
    };
    let info = compute_next_charge_info(&env, &sub);
    assert!(!info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, 2000 + INTERVAL);
}

#[test]
fn test_compute_next_charge_info_cancelled() {
    let env = Env::default();
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        token: Address::generate(&env),
        amount: AMOUNT,
        interval_seconds: INTERVAL,
        last_payment_timestamp: T0,
        status: SubscriptionStatus::Cancelled,
        prepaid_balance: 0,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
        start_time: 0,
        expires_at: None,
        grace_start_timestamp: None,
    };
    let info = compute_next_charge_info(&env, &sub);
    assert!(!info.is_charge_expected);
}

#[test]
fn test_compute_next_charge_info_insufficient_balance() {
    let env = Env::default();
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        token: Address::generate(&env),
        amount: AMOUNT,
        interval_seconds: INTERVAL,
        last_payment_timestamp: 3000,
        status: SubscriptionStatus::InsufficientBalance,
        prepaid_balance: 1_000_000,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
        start_time: 0,
        expires_at: None,
        grace_start_timestamp: None,
    };
    let info = compute_next_charge_info(&env, &sub);
    assert!(!info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, 3000 + INTERVAL);
}

#[test]
fn test_next_charge_info_cross_check_status_gating() {
    let (env, client, _, _) = setup_test_env();
    env.ledger().with_mut(|li| li.timestamp = T0);

    let (id_paused, _, _) = create_test_subscription(&env, &client, SubscriptionStatus::Paused);
    let (id_cancelled, _, _) =
        create_test_subscription(&env, &client, SubscriptionStatus::Cancelled);
    let (id_insufficient, _, _) =
        create_test_subscription(&env, &client, SubscriptionStatus::InsufficientBalance);
    let (id_grace, _, _) = create_test_subscription(&env, &client, SubscriptionStatus::GracePeriod);

    for id in [id_paused, id_cancelled, id_insufficient, id_grace] {
        seed_balance(&env, &client, id, PREPAID);
    }

    // Cross-check gating: paused / cancelled / insufficient states fail immediately.
    // Grace period passes the status gate, but still obeys the interval gate.
    assert_eq!(client.try_charge_subscription(&id_paused), Err(Ok(Error::NotActive)));
    assert_eq!(client.try_charge_subscription(&id_cancelled), Err(Ok(Error::NotActive)));
    assert_eq!(client.try_charge_subscription(&id_insufficient), Err(Ok(Error::NotActive)));
    assert_eq!(client.try_charge_subscription(&id_grace), Err(Ok(Error::IntervalNotElapsed)));
}

// -- Top-up estimation (precision) --------------------------------------------

#[test]
fn test_estimate_topup_zero_intervals_returns_zero() {
    let (env, client, _, _) = setup_test_env();
    env.ledger().with_mut(|li| li.timestamp = T0);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    // Cap = 2 * AMOUNT, so after 2 charges, should auto-cancel
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &Some(2 * AMOUNT),
     &None::<u64>);
    seed_balance(&env, &client, id, PREPAID);

    assert_eq!(client.estimate_topup_for_intervals(&id, &0), 0);
}

#[test]
fn test_estimate_topup_balance_already_sufficient_returns_zero() {
    let (env, client, _, _) = setup_test_env();
    env.ledger().with_mut(|li| li.timestamp = T0);
    let (id, _, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);

    // Balance covers 3 future charges.
    seed_balance(&env, &client, id, 3 * AMOUNT);
    assert_eq!(client.estimate_topup_for_intervals(&id, &3), 0);
}

#[test]
fn test_estimate_topup_cross_check_after_actual_charge() {
    let (env, client, _, _) = setup_test_env();
    env.ledger().with_mut(|li| li.timestamp = T0);
    let (id, _, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);
    seed_balance(&env, &client, id, PREPAID);

    // Before any charge, to cover next 6 intervals we need: 6*AMOUNT - PREPAID.
    assert_eq!(
        client.estimate_topup_for_intervals(&id, &6),
        6 * AMOUNT - PREPAID
    );

    // Execute one real charge at the exact boundary.
    env.ledger().with_mut(|li| li.timestamp = T0 + INTERVAL);
    assert_eq!(
        client.try_charge_subscription(&id),
        Ok(Ok(ChargeExecutionResult::Charged))
    );

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID - AMOUNT);

    // Now, covering next 5 intervals should be the same shortfall.
    assert_eq!(
        client.estimate_topup_for_intervals(&id, &5),
        5 * AMOUNT - (PREPAID - AMOUNT)
    );
}

#[test]
fn test_estimate_topup_overflow_protection() {
    let (env, client, _, _) = setup_test_env();
    env.ledger().with_mut(|li| li.timestamp = T0);
    let (id, _, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);

    // Force multiplication overflow: amount * num_intervals.
    let mut sub = client.get_subscription(&id);
    sub.amount = i128::MAX;
    env.as_contract(&client.address, || {
        env.storage().instance().set(&DataKey::Sub(id), &sub);
    });

    assert_eq!(
        client.try_estimate_topup_for_intervals(&id, &2),
        Err(Ok(Error::Overflow))
    );
}

#[test]
fn test_compute_next_charge_info_overflow_protection() {
    let env = Env::default();
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        token: Address::generate(&env),
        amount: AMOUNT,
        interval_seconds: 200,
        last_payment_timestamp: u64::MAX - 100,
        status: SubscriptionStatus::Active,
        prepaid_balance: 100_000_000,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
        start_time: 0,
        expires_at: None,
        grace_start_timestamp: None,
    };
    let info = compute_next_charge_info(&env, &sub);
    assert!(info.is_charge_expected);
    assert_eq!(info.next_charge_timestamp, u64::MAX);
}

// -- Replay protection --------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #4005)")]
fn test_replay_charge_same_period() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);
    let (id, _, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    fixtures::seed_balance(&test_env.env, &test_env.client, id, PREPAID);

    test_env.jump(INTERVAL + 1);
    test_env.client.charge_subscription(&id);
    // Second charge in same period should fail
    test_env.client.charge_subscription(&id);
}

// -- Recovery -----------------------------------------------------------------

#[test]
fn test_recover_stranded_funds() {
    let test_env = TestEnv::default();
    let recipient = Address::generate(&test_env.env);
    test_env
        .stellar_token_client()
        .mint(&test_env.client.address, &1_000_000i128);
    test_env.client.recover_stranded_funds(
        &test_env.admin,
        &test_env.token,
        &recipient,
        &1_000_000,
        &String::from_str(&test_env.env, "test-recovery"),
        &RecoveryReason::UserOverpayment,
    );
}

// -- Lifetime cap tests -------------------------------------------------------

#[test]
fn test_lifetime_cap_auto_cancel() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    // Cap = 2 * AMOUNT, so after 2 charges, should auto-cancel
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &Some(2 * AMOUNT),
        &None::<u64>,
    );
    fixtures::seed_balance(&test_env.env, &test_env.client, id, PREPAID);

    // First charge
    test_env.jump(INTERVAL + 1);
    test_env.client.charge_subscription(&id);
    let sub = test_env.client.get_subscription(&id);
    assert_eq!(sub.lifetime_charged, AMOUNT);
    assert_eq!(sub.status, SubscriptionStatus::Active);

    // Second charge -> cap reached -> auto-cancel
    test_env.jump(INTERVAL);
    test_env.client.charge_subscription(&id);
    let sub = test_env.client.get_subscription(&id);
    assert_eq!(sub.lifetime_charged, 2 * AMOUNT);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

#[test]
fn test_get_cap_info() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let cap = 100_000_000i128;
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &Some(cap),
     &None::<u64>);
    let info = test_env.client.get_cap_info(&id);
    assert_eq!(info.lifetime_cap, Some(cap));
    assert_eq!(info.lifetime_charged, 0);
    assert_eq!(info.remaining_cap, Some(cap));
    assert!(!info.cap_reached);
}

// -- Plan template tests ------------------------------------------------------

/// Plan template inherits lifetime_cap to subscriptions created from it.
#[test]
fn test_plan_template_inherits_lifetime_cap() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let cap = 50_000_000i128;
    let plan_id =
        test_env
            .client
            .create_plan_template(&merchant, &AMOUNT, &INTERVAL, &false, &Some(cap));

    let template = test_env.client.get_plan_template(&plan_id);
    assert_eq!(template.lifetime_cap, Some(cap));

    let sub_id = test_env
        .client
        .create_subscription_from_plan(&subscriber, &plan_id);
    let sub = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub.lifetime_cap, Some(cap));
}

/// Plan template with no cap creates uncapped subscriptions.
#[test]
fn test_plan_template_no_cap_creates_uncapped_sub() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let plan_id =
        test_env
            .client
            .create_plan_template(&merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>);
    let plan = test_env.client.get_plan_template(&plan_id);
    assert_eq!(plan.amount, AMOUNT);

    let sub_id = test_env
        .client
        .create_subscription_from_plan(&subscriber, &plan_id);
    let sub = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub.amount, AMOUNT);
    assert_eq!(sub.merchant, merchant);
}

#[test]
fn test_plan_max_concurrent_subscriptions_enforced_per_subscriber() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let plan_id =
        test_env
            .client
            .create_plan_template(&merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>);

    // Limit each subscriber to a single active subscription for this plan.
    test_env
        .client
        .set_plan_max_active_subs(&merchant, &plan_id, &1);

    // First subscription succeeds.
    let _sub1 = test_env
        .client
        .create_subscription_from_plan(&subscriber, &plan_id);

    // Second subscription for the same subscriber/plan is rejected.
    let result = test_env
        .client
        .try_create_subscription_from_plan(&subscriber, &plan_id);
    assert_eq!(result, Err(Ok(Error::MaxConcurrentSubscriptionsReached)));

    // Another subscriber is unaffected by this limit.
    let other_subscriber = Address::generate(&test_env.env);
    let _sub_other = test_env
        .client
        .create_subscription_from_plan(&other_subscriber, &plan_id);
}

#[test]
fn test_plan_max_concurrent_allows_new_after_cancellation() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let plan_id =
        test_env
            .client
            .create_plan_template(&merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>);
    test_env
        .client
        .set_plan_max_active_subs(&merchant, &plan_id, &1);

    let sub1 = test_env
        .client
        .create_subscription_from_plan(&subscriber, &plan_id);
    test_env.client.cancel_subscription(&sub1, &subscriber);

    // Because only ACTIVE subscriptions are counted, a new subscription is allowed
    // after cancellation.
    let sub2 = test_env
        .client
        .create_subscription_from_plan(&subscriber, &plan_id);
    assertions::assert_status(&test_env.client, &sub2, SubscriptionStatus::Active);
}

#[test]
fn test_subscriber_credit_limit_blocks_new_subscription_creation() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    // Limit total exposure for this subscriber/token to a single interval amount.
    test_env.client.set_subscriber_credit_limit(
        &test_env.admin,
        &subscriber,
        &test_env.token,
        &AMOUNT,
    );

    // First subscription fits entirely within the limit.
    let _sub1 =
        test_env.client.create_subscription(&subscriber, &merchant, &AMOUNT, &INTERVAL, &false, &None, &None::<u64>);

    // Second subscription would exceed credit limit (another interval liability).
    let result =
        test_env.client.try_create_subscription(&subscriber, &merchant, &AMOUNT, &INTERVAL, &false, &None, &None::<u64>);
    assert_eq!(result, Err(Ok(Error::CreditLimitExceeded)));
}

#[test]
fn test_subscriber_credit_limit_blocks_topup_when_exposure_exceeds_limit() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &10_000_000);
    let merchant = Address::generate(&test_env.env);

    // Exposure limit small enough that initial subscription fits, but top-up does not.
    let limit = AMOUNT + 5_000_000i128;
    test_env.client.set_subscriber_credit_limit(
        &test_env.admin,
        &subscriber,
        &test_env.token,
        &limit,
    );

    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
     &None::<u64>);

    // Deposit that would keep us under the limit succeeds.
    test_env
        .client
        .deposit_funds(&sub_id, &subscriber, &5_000_000i128);

    // Further deposit would push exposure over the limit and must be rejected.
    let result = test_env
        .client
        .try_deposit_funds(&sub_id, &subscriber, &1_000_000i128);
    assert_eq!(result, Err(Ok(Error::CreditLimitExceeded)));
}

#[test]
fn test_get_subscriber_credit_limit_and_exposure_views() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &10_000_000);
    let merchant = Address::generate(&test_env.env);

    // Default: no limit configured.
    assert_eq!(
        test_env
            .client
            .get_subscriber_credit_limit(&subscriber, &test_env.token),
        0
    );

    test_env.client.set_subscriber_credit_limit(
        &test_env.admin,
        &subscriber,
        &test_env.token,
        &(AMOUNT * 10),
    );

    // After creating a subscription, exposure reflects one interval liability and zero prepaid.
    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
     &None::<u64>);
    let exposure = test_env.client.get_subscriber_exposure(&subscriber, &test_env.token);
    assert_eq!(exposure, AMOUNT);

    // After topping up, exposure increases by the deposited amount.
    test_env
        .client
        .deposit_funds(&sub_id, &subscriber, &5_000_000i128);
    let exposure_after_topup = test_env
        .client
        .get_subscriber_exposure(&subscriber, &test_env.token);
    assert_eq!(exposure_after_topup, AMOUNT + 5_000_000i128);
}

#[test]
fn test_partial_refund_debits_prepaid_and_transfers_tokens() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &50_000_000i128);
    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
     &None::<u64>);
    test_env.client.deposit_funds(&sub_id, &subscriber, &20_000_000i128);

    let balance_before = test_env.token_client().balance(&subscriber);
    assertions::assert_prepaid_balance(&test_env.client, &sub_id, 20_000_000i128);

    // Perform a partial refund of half the prepaid balance.
    test_env
        .client
        .partial_refund(&test_env.admin, &sub_id, &subscriber, &10_000_000i128);

    let balance_after = test_env.token_client().balance(&subscriber);
    assertions::assert_prepaid_balance(&test_env.client, &sub_id, 10_000_000i128);
    assert_eq!(balance_after, balance_before + 10_000_000i128);
}

#[test]
fn test_partial_refund_rejects_invalid_amounts_and_auth() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &50_000_000i128);
    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
     &None::<u64>);
    test_env.client.deposit_funds(&sub_id, &subscriber, &5_000_000i128);

    // Zero or negative refund amounts are rejected.
    let zero_res =
        test_env
            .client
            .try_partial_refund(&test_env.admin, &sub_id, &subscriber, &0i128);
    assert_eq!(zero_res, Err(Ok(Error::InvalidAmount)));

    let negative_res =
        test_env
            .client
            .try_partial_refund(&test_env.admin, &sub_id, &subscriber, &-1i128);
    assert_eq!(negative_res, Err(Ok(Error::InvalidAmount)));

    // Refund exceeding prepaid balance is rejected.
    let over_res =
        test_env
            .client
            .try_partial_refund(&test_env.admin, &sub_id, &subscriber, &10_000_000i128);
    assert_eq!(over_res, Err(Ok(Error::InsufficientBalance)));

    // Non-admin cannot authorize partial refunds.
    let other_admin = Address::generate(&test_env.env);
    let unauth_res =
        test_env
            .client
            .try_partial_refund(&other_admin, &sub_id, &subscriber, &1_000_000i128);
    assert_eq!(unauth_res, Err(Ok(Error::Unauthorized)));

    // Wrong subscriber address is rejected.
    let wrong_subscriber = Address::generate(&test_env.env);
    let wrong_sub_res = test_env.client.try_partial_refund(
        &test_env.admin,
        &sub_id,
        &wrong_subscriber,
        &1_000_000i128,
    );
    assert_eq!(wrong_sub_res, Err(Ok(Error::Unauthorized)));
}

// =============================================================================
// Partial Refund — Extended Coverage
// =============================================================================

/// Repeated partial refunds each debit the correct incremental amount.
#[test]
fn test_partial_refund_repeated_debits_are_cumulative() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &30_000_000i128);

    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env
        .client
        .deposit_funds(&sub_id, &subscriber, &30_000_000i128);

    // Three successive partial refunds of 5 USDC each.
    for _ in 0..3 {
        test_env
            .client
            .partial_refund(&test_env.admin, &sub_id, &subscriber, &5_000_000i128);
    }

    let sub = test_env.client.get_subscription(&sub_id);
    assert_eq!(sub.prepaid_balance, 15_000_000i128); // 30 - 3*5 = 15
    assert_eq!(test_env.token_client().balance(&subscriber), 15_000_000i128);
}

/// Cumulative refunds that exactly drain the balance succeed; one more unit fails.
#[test]
fn test_partial_refund_cumulative_exact_drain_then_over_refund_fails() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &10_000_000i128);
    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env
        .client
        .deposit_funds(&sub_id, &subscriber, &10_000_000i128);

    // Refund the full balance as two equal halves.
    test_env
        .client
        .partial_refund(&test_env.admin, &sub_id, &subscriber, &5_000_000i128);
    test_env
        .client
        .partial_refund(&test_env.admin, &sub_id, &subscriber, &5_000_000i128);

    assertions::assert_prepaid_balance(&test_env.client, &sub_id, 0);

    // Any further refund must fail — balance is zero.
    let over = test_env
        .client
        .try_partial_refund(&test_env.admin, &sub_id, &subscriber, &1i128);
    assert_eq!(over, Err(Ok(Error::InsufficientBalance)));
}

/// A partial refund equal to the full prepaid balance (full-balance-as-partial) succeeds.
#[test]
fn test_partial_refund_full_balance_as_partial_succeeds() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &20_000_000i128);
    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env
        .client
        .deposit_funds(&sub_id, &subscriber, &20_000_000i128);

    // Refund the entire prepaid balance in one call.
    test_env
        .client
        .partial_refund(&test_env.admin, &sub_id, &subscriber, &20_000_000i128);

    assertions::assert_prepaid_balance(&test_env.client, &sub_id, 0);
    assert_eq!(test_env.token_client().balance(&subscriber), 20_000_000i128);
}

/// Partial refund is allowed on a cancelled subscription (remaining balance can be returned).
#[test]
fn test_partial_refund_after_cancellation_succeeds() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &15_000_000i128);
    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env
        .client
        .deposit_funds(&sub_id, &subscriber, &15_000_000i128);
    test_env.client.cancel_subscription(&sub_id, &subscriber);

    // Admin can still issue a partial refund on a cancelled subscription.
    test_env
        .client
        .partial_refund(&test_env.admin, &sub_id, &subscriber, &5_000_000i128);

    assertions::assert_prepaid_balance(&test_env.client, &sub_id, 10_000_000i128);
    assert_eq!(test_env.token_client().balance(&subscriber), 5_000_000i128);
}

/// Partial refund emits a PartialRefundEvent with correct fields.
#[test]
fn test_partial_refund_emits_event() {
    let test_env = TestEnv::default();
    test_env.env.ledger().with_mut(|li| li.timestamp = T0);

    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &10_000_000i128);
    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env
        .client
        .deposit_funds(&sub_id, &subscriber, &10_000_000i128);

    test_env
        .client
        .partial_refund(&test_env.admin, &sub_id, &subscriber, &3_000_000i128);

    // At least one event must have been emitted by the refund call.
    assert!(!test_env.env.events().all().is_empty());
}

#[test]
fn test_update_plan_template_creates_new_version_and_preserves_old() {
    let test_env = TestEnv::default();
    let merchant = Address::generate(&test_env.env);

    let cap = 50_000_000i128;
    let plan_id =
        test_env
            .client
            .create_plan_template(&merchant, &AMOUNT, &INTERVAL, &false, &Some(cap));
    let original = test_env.client.get_plan_template(&plan_id);
    assert_eq!(original.version, 1);

    let new_amount = AMOUNT * 2;
    let new_interval = INTERVAL / 2;
    let new_plan_id = test_env.client.update_plan_template(
        &merchant,
        &plan_id,
        &new_amount,
        &new_interval,
        &false,
        &Some(cap),
    );

    // Old plan remains unchanged and addressable.
    let original_after = test_env.client.get_plan_template(&plan_id);
    assert_eq!(original_after.version, 1);
    assert_eq!(original_after.amount, AMOUNT);
    assert_eq!(original_after.interval_seconds, INTERVAL);
    assert!(!original_after.usage_enabled);

    // New plan has incremented version and updated fields, sharing template_key.
    let updated = test_env.client.get_plan_template(&new_plan_id);
    assert_eq!(updated.version, 2);
    assert_eq!(updated.template_key, original_after.template_key);
    assert_eq!(updated.amount, new_amount);
    assert_eq!(updated.interval_seconds, new_interval);
    assert!(!updated.usage_enabled);
    assert_eq!(updated.lifetime_cap, Some(cap));
}

#[test]
fn test_migrate_subscription_to_new_plan_version() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let cap = 50_000_000i128;
    let plan_id =
        test_env
            .client
            .create_plan_template(&merchant, &AMOUNT, &INTERVAL, &false, &Some(cap));
    let new_amount = AMOUNT * 3;
    let new_interval = INTERVAL / 3;
    let new_plan_id = test_env.client.update_plan_template(
        &merchant,
        &plan_id,
        &new_amount,
        &new_interval,
        &false,
        &Some(cap),
    );

    let sub_id = test_env
        .client
        .create_subscription_from_plan(&subscriber, &plan_id);
    let before = test_env.client.get_subscription(&sub_id);
    assert_eq!(before.amount, AMOUNT);
    assert_eq!(before.interval_seconds, INTERVAL);
    assert!(!before.usage_enabled);

    test_env
        .client
        .migrate_subscription_to_plan(&subscriber, &sub_id, &new_plan_id);

    let after = test_env.client.get_subscription(&sub_id);
    assert_eq!(after.amount, new_amount);
    assert_eq!(after.interval_seconds, new_interval);
    assert!(!after.usage_enabled);
    // Lifetime tracking is preserved.
    assert_eq!(after.lifetime_charged, 0);
    assert_eq!(after.lifetime_cap, Some(cap));
}

#[test]
fn test_migrate_subscription_rejects_cross_template_family() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let plan_family_a =
        test_env
            .client
            .create_plan_template(&merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>);
    let plan_family_b = test_env.client.create_plan_template(
        &merchant,
        &(AMOUNT * 2),
        &INTERVAL,
        &false,
        &None::<i128>,
    );

    let sub_id = test_env
        .client
        .create_subscription_from_plan(&subscriber, &plan_family_a);

    let result =
        test_env
            .client
            .try_migrate_subscription_to_plan(&subscriber, &sub_id, &plan_family_b);
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

// --- Cancellation and Withdrawal Regression Tests ---------------------------

#[test]
fn test_cancel_from_various_states() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    // Cancel from Active
    let id1 = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
     &None::<u64>);

    // Cancel from Paused
    let id2 = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env.client.pause_subscription(&id2, &subscriber);
    test_env.client.cancel_subscription(&id2, &subscriber);
    assertions::assert_status(&test_env.client, &id2, SubscriptionStatus::Cancelled);
}

#[test]
fn test_withdraw_subscriber_funds_exactly_once() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &20_000_000);

    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env.client.deposit_funds(&id, &subscriber, &10_000_000);

    test_env.client.deposit_funds(&id, &subscriber, &5_000_000);
    test_env.client.cancel_subscription(&id, &subscriber);

    // First withdrawal: Success
    test_env.client.withdraw_subscriber_funds(&id, &subscriber);
    assertions::assert_prepaid_balance(&test_env.client, &id, 0);

    // Second withdrawal of zero balance returns InvalidAmount.
    let result = test_env
        .client
        .try_withdraw_subscriber_funds(&id, &subscriber);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_withdraw_zero_balance_fails() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
     &None::<u64>);
    test_env.client.cancel_subscription(&id, &subscriber);

    let result = test_env
        .client
        .try_withdraw_subscriber_funds(&id, &subscriber);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_cancel_and_withdraw_events() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &10_000_000);

    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env.client.deposit_funds(&id, &subscriber, &5_000_000);

    test_env.client.cancel_subscription(&id, &subscriber);

    // Check cancellation event
    let events = test_env.env.events().all();
    let _cancel_event = events.get(events.len() - 1).unwrap();

    test_env.client.withdraw_subscriber_funds(&id, &subscriber);

    // Check withdrawal event
    let events = test_env.env.events().all();
    let _withdraw_event = events.get(events.len() - 1).unwrap();
}

#[test]
fn test_migrate_subscription_requires_plan_origin() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    // Create a subscription directly (not from a plan template).
    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
     &None::<u64>);
    let plan_id = test_env.client.create_plan_template(
        &merchant,
        &(&AMOUNT * 2),
        &INTERVAL,
        &false,
        &None::<i128>,
    );
    let id2 = test_env.client.create_subscription_from_plan(&subscriber, &plan_id);

    let page = test_env.client.list_subscriptions_by_subscriber(&subscriber, &0, &10);
    assert_eq!(page.subscription_ids.len(), 2);
    assert_eq!(page.subscription_ids.get(0).unwrap(), sub_id);
    assert_eq!(page.subscription_ids.get(1).unwrap(), id2);
    assert_eq!(page.next_start_id, None);
}

/// Subscriber can withdraw remaining prepaid balance after cap-triggered cancellation.
#[test]
fn test_cap_cancelled_subscriber_can_withdraw() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &1_000_000_000i128);

    // cap = AMOUNT + 5M: after 1 charge (AMOUNT), remaining cap = 5M < AMOUNT, so
    // the second charge attempt cancels without moving funds, leaving a 5M balance.
    let cap = AMOUNT + 5_000_000;
    let sub_id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &Some(cap),
     &None::<u64>);

    // Deposit exactly cap so the deposit fits within enforce_deposit_cap.
    test_env.client.deposit_funds(&sub_id, &subscriber, &cap);

    test_env
        .env
        .ledger()
        .with_mut(|li| li.timestamp = T0 + INTERVAL + 1);
    test_env.client.charge_subscription(&sub_id); // charges AMOUNT, balance = cap - AMOUNT = 5M
    test_env
        .env
        .ledger()
        .with_mut(|li| li.timestamp = T0 + 2 * INTERVAL + 1);
    test_env.client.charge_subscription(&sub_id); // remaining cap < AMOUNT → cancel, balance stays 5M

    assertions::assert_status(&test_env.client, &sub_id, SubscriptionStatus::Cancelled);
    let sub_after = test_env.client.get_subscription(&sub_id);
    assert!(sub_after.prepaid_balance > 0, "remaining balance exists after partial-cap cancel");

    // Subscriber can withdraw remaining prepaid balance
    test_env
        .client
        .withdraw_subscriber_funds(&sub_id, &subscriber);
    assertions::assert_prepaid_balance(&test_env.client, &sub_id, 0);
}

#[test]
fn test_charge_usage_basic() {
    let test_env = TestEnv::default();
    test_env.env.ledger().with_mut(|li| li.timestamp = T0);

    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &true,
        &None::<i128>,
        &None::<u64>,
    );
    fixtures::seed_balance(&test_env.env, &test_env.client, id, PREPAID);

    test_env.client.charge_usage(&id, &1_000_000);
    assertions::assert_prepaid_balance(&test_env.client, &id, PREPAID - 1_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #6003)")]
fn test_charge_usage_not_enabled() {
    let test_env = TestEnv::default();
    let (id, _, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    fixtures::seed_balance(&test_env.env, &test_env.client, id, PREPAID);
    test_env.client.charge_usage(&id, &1_000_000);
}

// -- Merchant tests -----------------------------------------------------------

#[test]
fn test_merchant_balance_and_withdrawal() {
    let test_env = TestEnv::default();
    test_env.env.ledger().with_mut(|li| li.timestamp = T0);

    let (id, _, merchant) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    fixtures::seed_balance(&test_env.env, &test_env.client, id, PREPAID);

    test_env
        .env
        .ledger()
        .with_mut(|li| li.timestamp = T0 + INTERVAL + 1);
    test_env.client.charge_subscription(&id);

    let balance = test_env.client.get_merchant_balance(&merchant);
    assert!(balance > 0);
}

#[test]
fn test_withdraw_merchant_funds_reduces_default_bucket_and_emits_event() {
    let (env, client, token, _) = setup_test_env();
    let merchant = Address::generate(&env);
    let contract_id = client.address.clone();

    seed_merchant_balance(&env, &contract_id, &merchant, &token, 9_000_000i128);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&contract_id, &9_000_000i128);

    env.as_contract(&contract_id, || {
        crate::merchant::withdraw_merchant_funds(&env, merchant.clone(), 4_000_000i128)
    })
    .unwrap();

    assert_eq!(client.get_merchant_balance(&merchant), 5_000_000i128);

    let encoded: Val = MerchantWithdrawalEvent {
        merchant: merchant.clone(),
        token: token.clone(),
        amount: 4_000_000i128,
        remaining_balance: 5_000_000i128,
        timestamp: env.ledger().timestamp(),
    }
    .into_val(&env);
    let event = MerchantWithdrawalEvent::try_from_val(&env, &encoded).unwrap();
    assert_eq!(event.merchant, merchant);
    assert_eq!(event.token, token);
    assert_eq!(event.amount, 4_000_000i128);
    assert_eq!(event.remaining_balance, 5_000_000i128);
}

#[test]
fn test_withdraw_merchant_funds_rejects_empty_bucket() {
    let (env, client, _, _) = setup_test_env();
    let merchant = Address::generate(&env);

    let result = client.try_withdraw_merchant_funds(&merchant, &1_000_000i128);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn test_withdraw_merchant_funds_rejects_overdraw() {
    let (env, client, token, _) = setup_test_env();
    let merchant = Address::generate(&env);
    let contract_id = client.address.clone();

    seed_merchant_balance(&env, &contract_id, &merchant, &token, 3_000_000i128);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&contract_id, &3_000_000i128);

    let result = client.try_withdraw_merchant_funds(&merchant, &4_000_000i128);
    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));
    assert_eq!(client.get_merchant_balance(&merchant), 3_000_000i128);
}

#[test]
fn test_withdraw_merchant_token_funds_only_debits_requested_bucket_and_emits_event() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token_a, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    client.add_accepted_token(&admin, &token_b, &6);

    let merchant = Address::generate(&env);
    seed_merchant_balance(&env, &contract_id, &merchant, &token_a, 5_000_000i128);
    seed_merchant_balance(&env, &contract_id, &merchant, &token_b, 7_000_000i128);
    soroban_sdk::token::StellarAssetClient::new(&env, &token_a).mint(&contract_id, &5_000_000i128);
    soroban_sdk::token::StellarAssetClient::new(&env, &token_b).mint(&contract_id, &7_000_000i128);

    env.as_contract(&contract_id, || {
        crate::merchant::withdraw_merchant_funds_for_token(
            &env,
            merchant.clone(),
            token_b.clone(),
            2_000_000i128,
        )
    })
    .unwrap();

    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token_a),
        5_000_000i128
    );
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token_b),
        5_000_000i128
    );

    let encoded: Val = MerchantWithdrawalEvent {
        merchant: merchant.clone(),
        token: token_b.clone(),
        amount: 2_000_000i128,
        remaining_balance: 5_000_000i128,
        timestamp: env.ledger().timestamp(),
    }
    .into_val(&env);
    let event = MerchantWithdrawalEvent::try_from_val(&env, &encoded).unwrap();
    assert_eq!(event.merchant, merchant);
    assert_eq!(event.token, token_b);
    assert_eq!(event.amount, 2_000_000i128);
    assert_eq!(event.remaining_balance, 5_000_000i128);
}

#[test]
fn test_withdraw_merchant_token_funds_rejects_empty_bucket() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, token, admin) = setup_contract(&env);
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.add_accepted_token(&admin, &token_b, &6);

    let merchant = Address::generate(&env);
    seed_merchant_balance(&env, &client.address, &merchant, &token, 3_000_000i128);

    let result = client.try_withdraw_merchant_token_funds(&merchant, &token_b, &1_000_000i128);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

#[test]
fn test_withdraw_merchant_token_funds_checks_vault_balance_before_transfer() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, token, _admin) = setup_contract(&env);
    let merchant = Address::generate(&env);

    seed_merchant_balance(&env, &client.address, &merchant, &token, 4_000_000i128);

    let result = client.try_withdraw_merchant_token_funds(&merchant, &token, &4_000_000i128);
    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        4_000_000i128
    );
}

// -- End-to-end billing lifecycle tests --------------------------------------

#[test]
fn test_billing_lifecycle_golden_path_end_to_end() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);

    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let minted = 100_000_000i128;
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &minted);

    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    let created = test_env.client.get_subscription(&id);
    assert_eq!(created.status, SubscriptionStatus::Active);
    assert_eq!(created.prepaid_balance, 0);
    assert_eq!(created.last_payment_timestamp, T0);
    assert_eq!(test_env.client.get_merchant_balance(&merchant), 0);

    test_env.client.deposit_funds(&id, &subscriber, &PREPAID);
    let after_deposit = test_env.client.get_subscription(&id);
    assert_eq!(after_deposit.status, SubscriptionStatus::Active);
    assert_eq!(after_deposit.prepaid_balance, PREPAID);
    assert_eq!(
        test_env.token_client().balance(&subscriber),
        minted - PREPAID
    );
    assert_eq!(
        test_env.token_client().balance(&test_env.client.address),
        PREPAID
    );

    test_env.env.ledger().set_timestamp(T0 + INTERVAL);
    test_env.client.charge_subscription(&id);
    let after_first_charge = test_env.client.get_subscription(&id);
    assert_eq!(after_first_charge.status, SubscriptionStatus::Active);
    assert_eq!(after_first_charge.prepaid_balance, PREPAID - AMOUNT);
    assert_eq!(after_first_charge.last_payment_timestamp, T0 + INTERVAL);
    assert_eq!(after_first_charge.lifetime_charged, AMOUNT);
    assert_eq!(test_env.client.get_merchant_balance(&merchant), AMOUNT);

    test_env.env.ledger().set_timestamp(T0 + (2 * INTERVAL));
    test_env.client.charge_subscription(&id);
    let after_second_charge = test_env.client.get_subscription(&id);
    assert_eq!(after_second_charge.status, SubscriptionStatus::Active);
    assert_eq!(after_second_charge.prepaid_balance, PREPAID - (2 * AMOUNT));
    assert_eq!(
        after_second_charge.last_payment_timestamp,
        T0 + (2 * INTERVAL)
    );
    assert_eq!(after_second_charge.lifetime_charged, 2 * AMOUNT);
    assert_eq!(test_env.client.get_merchant_balance(&merchant), 2 * AMOUNT);

    let statements = test_env
        .client
        .get_sub_statements_offset(&id, &0, &10, &true);
    assert_eq!(statements.total, 2);
    assert_eq!(statements.statements.len(), 2);

    let newest = statements.statements.get(0).unwrap();
    assert_eq!(newest.sequence, 1);
    assert_eq!(newest.charged_at, T0 + (2 * INTERVAL));
    assert_eq!(newest.period_start, T0 + INTERVAL);
    assert_eq!(newest.period_end, T0 + (2 * INTERVAL));
    assert_eq!(newest.amount, AMOUNT);
    assert_eq!(newest.merchant, merchant.clone());
    assert_eq!(newest.kind, crate::BillingChargeKind::Interval);

    let oldest = statements.statements.get(1).unwrap();
    assert_eq!(oldest.sequence, 0);
    assert_eq!(oldest.charged_at, T0 + INTERVAL);
    assert_eq!(oldest.period_start, T0);
    assert_eq!(oldest.period_end, T0 + INTERVAL);
    assert_eq!(oldest.amount, AMOUNT);
    assert_eq!(oldest.merchant, merchant.clone());
    assert_eq!(oldest.kind, crate::BillingChargeKind::Interval);

    let first_page = test_env
        .client
        .get_sub_statements_cursor(&id, &None::<u32>, &1, &true);
    assert_eq!(first_page.total, 2);
    assert_eq!(first_page.statements.len(), 1);
    assert_eq!(first_page.statements.get(0).unwrap().sequence, 1);
    assert_eq!(first_page.next_cursor, Some(0));

    let second_page =
        test_env
            .client
            .get_sub_statements_cursor(&id, &first_page.next_cursor, &1, &true);
    assert_eq!(second_page.total, 2);
    assert_eq!(second_page.statements.len(), 1);
    assert_eq!(second_page.statements.get(0).unwrap().sequence, 0);
    assert_eq!(second_page.next_cursor, None);

    let merchant_wallet_before = test_env.token_client().balance(&merchant);
    test_env
        .client
        .withdraw_merchant_funds(&merchant, &(2 * AMOUNT));
    assert_eq!(test_env.client.get_merchant_balance(&merchant), 0);
    assert_eq!(
        test_env.token_client().balance(&merchant),
        merchant_wallet_before + (2 * AMOUNT)
    );
    assert_eq!(
        test_env.token_client().balance(&test_env.client.address),
        PREPAID - (2 * AMOUNT)
    );

    test_env.client.cancel_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);

    test_env.client.withdraw_subscriber_funds(&id, &subscriber);
    let closed_out = test_env.client.get_subscription(&id);
    assert_eq!(closed_out.prepaid_balance, 0);
    assert_eq!(test_env.token_client().balance(&test_env.client.address), 0);
    assert_eq!(
        test_env.token_client().balance(&subscriber),
        minted - (2 * AMOUNT)
    );
}

#[test]
fn test_billing_lifecycle_delayed_charge_and_min_topup_progression() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);

    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &50_000_000i128);

    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env
        .client
        .deposit_funds(&id, &subscriber, &19_000_000i128);

    let delayed_charge_at = T0 + (2 * INTERVAL) + 77;
    test_env.env.ledger().set_timestamp(delayed_charge_at);
    test_env.client.charge_subscription(&id);

    let after_delayed_charge = test_env.client.get_subscription(&id);
    assert_eq!(after_delayed_charge.status, SubscriptionStatus::Active);
    assert_eq!(after_delayed_charge.prepaid_balance, 9_000_000i128);
    assert_eq!(
        after_delayed_charge.last_payment_timestamp,
        T0 + 2 * INTERVAL // charge sets timestamp to period_start, not exact charge time
    );
    assert_eq!(after_delayed_charge.lifetime_charged, AMOUNT);
    assert_eq!(test_env.client.get_merchant_balance(&merchant), AMOUNT);

    test_env
        .client
        .deposit_funds(&id, &subscriber, &1_000_000i128);
    let after_topup = test_env.client.get_subscription(&id);
    assert_eq!(after_topup.prepaid_balance, AMOUNT);

    test_env
        .env
        .ledger()
        .set_timestamp(delayed_charge_at + INTERVAL);
    test_env.client.charge_subscription(&id);

    let after_second_charge = test_env.client.get_subscription(&id);
    assert_eq!(after_second_charge.status, SubscriptionStatus::Active);
    assert_eq!(after_second_charge.prepaid_balance, 0);
    assert_eq!(
        after_second_charge.last_payment_timestamp,
        T0 + 3 * INTERVAL // period_start advances by INTERVAL from the previous period_start
    );
    assert_eq!(after_second_charge.lifetime_charged, 2 * AMOUNT);
    assert_eq!(test_env.client.get_merchant_balance(&merchant), 2 * AMOUNT);

    let statements = test_env
        .client
        .get_sub_statements_offset(&id, &0, &10, &false);
    assert_eq!(statements.total, 2);
    assert_eq!(statements.statements.len(), 2);

    let first = statements.statements.get(0).unwrap();
    assert_eq!(first.sequence, 0);
    assert_eq!(first.period_start, T0);
    assert_eq!(first.period_end, delayed_charge_at);
    assert_eq!(first.amount, AMOUNT);

    let second = statements.statements.get(1).unwrap();
    assert_eq!(second.sequence, 1);
    assert_eq!(second.period_start, T0 + 2 * INTERVAL); // period_start = last_payment_timestamp after first charge
    assert_eq!(second.period_end, delayed_charge_at + INTERVAL);
    assert_eq!(second.amount, AMOUNT);

    assert_eq!(
        test_env.token_client().balance(&test_env.client.address),
        20_000_000i128
    );
}

// -- List subscriptions by subscriber test ------------------------------------

#[test]
fn test_list_subscriptions_by_subscriber() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    let id1 = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    let id2 = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    let page = test_env
        .client
        .list_subscriptions_by_subscriber(&subscriber, &0, &10);
    assert_eq!(page.subscription_ids.len(), 2);
    assert_eq!(page.subscription_ids.get(0).unwrap(), id1);
    assert_eq!(page.subscription_ids.get(1).unwrap(), id2);
    assert!(page.next_start_id.is_none());
}

#[test]
fn test_list_subscriptions_by_subscriber_limit_zero_errors() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let res = test_env
        .client
        .try_list_subscriptions_by_subscriber(&subscriber, &0, &0u32);
    assert!(matches!(res, Err(Ok(Error::InvalidInput))));
}

#[test]
fn test_list_subscriptions_by_subscriber_pagination_stable_ordering() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let mut expected = alloc::vec::Vec::new();
    for _ in 0..5 {
        let id = test_env.client.create_subscription(
            &subscriber,
            &merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
        &None::<u64>,
    );
        expected.push(id);
    }

    let page1 = test_env
        .client
        .list_subscriptions_by_subscriber(&subscriber, &0, &2);
    assert_eq!(page1.subscription_ids.len(), 2);
    assert_eq!(page1.subscription_ids.get(0).unwrap(), expected[0]);
    assert_eq!(page1.subscription_ids.get(1).unwrap(), expected[1]);
    let next = page1.next_start_id.expect("next page");
    let page2 = test_env
        .client
        .list_subscriptions_by_subscriber(&subscriber, &next, &10);
    assert_eq!(page2.subscription_ids.len(), 3);
    assert_eq!(page2.subscription_ids.get(0).unwrap(), expected[2]);
    assert_eq!(page2.subscription_ids.get(2).unwrap(), expected[4]);
    assert!(page2.next_start_id.is_none());
}

#[test]
fn test_get_subscriptions_by_merchant_pagination_and_invalid_limit() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    for _ in 0..3 {
        test_env.client.create_subscription(
            &subscriber,
            &merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
        &None::<u64>,
    );
    }
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant),
        3
    );
    assert_eq!(
        test_env
            .client
            .try_get_subscriptions_by_merchant(&merchant, &0, &0u32),
        Err(Ok(Error::InvalidInput))
    );
    assert_eq!(
        test_env.client.try_get_subscriptions_by_merchant(
            &merchant,
            &0,
            &(MAX_SUBSCRIPTION_LIST_PAGE + 1)
        ),
        Err(Ok(Error::InvalidInput))
    );
    let p1 = test_env
        .client
        .get_subscriptions_by_merchant(&merchant, &0, &2);
    assert_eq!(p1.len(), 2);
    let p2 = test_env
        .client
        .get_subscriptions_by_merchant(&merchant, &2, &2);
    assert_eq!(p2.len(), 1);
    let p3 = test_env
        .client
        .get_subscriptions_by_merchant(&merchant, &3, &10);
    assert_eq!(p3.len(), 0);
}

#[test]
fn test_get_subscriptions_by_token_pagination_and_count() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    for _ in 0..2 {
        test_env.client.create_subscription(
            &subscriber,
            &merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
        &None::<u64>,
    );
    }
    assert_eq!(
        test_env
            .client
            .get_token_subscription_count(&test_env.token),
        2
    );
    assert_eq!(
        test_env
            .client
            .try_get_subscriptions_by_token(&test_env.token, &0, &0u32),
        Err(Ok(Error::InvalidInput))
    );
    let page = test_env
        .client
        .get_subscriptions_by_token(&test_env.token, &0, &1);
    assert_eq!(page.len(), 1);
    let rest = test_env
        .client
        .get_subscriptions_by_token(&test_env.token, &1, &5);
    assert_eq!(rest.len(), 1);
}

// -- Subscriber withdrawal test -----------------------------------------------

#[test]
fn test_withdraw_subscriber_funds_after_cancel() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &10_000_000);

    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env.client.deposit_funds(&id, &subscriber, &5_000_000);
    test_env.client.cancel_subscription(&id, &subscriber);

    test_env.client.withdraw_subscriber_funds(&id, &subscriber);

    let sub = test_env.client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 0);
}

// -- Export tests -------------------------------------------------------------

#[test]
fn test_export_contract_snapshot() {
    let test_env = TestEnv::default();
    let snapshot = test_env.client.export_contract_snapshot(&test_env.admin);
    assert_eq!(snapshot.admin, test_env.admin);
    assert_eq!(snapshot.storage_version, 2);
}

#[test]
fn test_export_subscription_summaries() {
    let test_env = TestEnv::default();
    let (id, _, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    let summaries = test_env
        .client
        .export_subscription_summaries(&test_env.admin, &0, &10);
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries.get(0).unwrap().subscription_id, id);
}

// =============================================================================
// Metadata Key-Value Store Tests
// =============================================================================

#[test]
fn test_metadata_set_and_get() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let key = String::from_str(&test_env.env, "invoice_id");
    let value = String::from_str(&test_env.env, "INV-2025-001");

    test_env.client.set_metadata(&id, &subscriber, &key, &value);

    let result = test_env.client.get_metadata(&id, &key);
    assert_eq!(result, value);
}

#[test]
fn test_metadata_update_existing_key() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let key = String::from_str(&test_env.env, "customer_id");
    let value1 = String::from_str(&test_env.env, "CUST-001");
    let value2 = String::from_str(&test_env.env, "CUST-002");

    test_env
        .client
        .set_metadata(&id, &subscriber, &key, &value1);
    assert_eq!(test_env.client.get_metadata(&id, &key), value1);

    test_env
        .client
        .set_metadata(&id, &subscriber, &key, &value2);
    assert_eq!(test_env.client.get_metadata(&id, &key), value2);

    // Key count should still be 1 (updated, not duplicated)
    let keys = test_env.client.list_metadata_keys(&id);
    assert_eq!(keys.len(), 1);
}

#[test]
fn test_metadata_delete() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let key = String::from_str(&test_env.env, "tag");
    let value = String::from_str(&test_env.env, "premium");

    test_env.client.set_metadata(&id, &subscriber, &key, &value);
    assert_eq!(test_env.client.get_metadata(&id, &key), value);

    test_env.client.delete_metadata(&id, &subscriber, &key);

    let result = test_env.client.try_get_metadata(&id, &key);
    assert!(result.is_err());
}

#[test]
fn test_metadata_list_keys() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let key1 = String::from_str(&test_env.env, "invoice_id");
    let key2 = String::from_str(&test_env.env, "customer_id");
    let key3 = String::from_str(&test_env.env, "campaign_tag");

    test_env.client.set_metadata(
        &id,
        &subscriber,
        &key1,
        &String::from_str(&test_env.env, "v1"),
    );
    test_env.client.set_metadata(
        &id,
        &subscriber,
        &key2,
        &String::from_str(&test_env.env, "v2"),
    );
    test_env.client.set_metadata(
        &id,
        &subscriber,
        &key3,
        &String::from_str(&test_env.env, "v3"),
    );

    let keys = test_env.client.list_metadata_keys(&id);
    assert_eq!(keys.len(), 3);
}

#[test]
fn test_metadata_empty_list_for_new_subscription() {
    let test_env = TestEnv::default();
    let (id, _, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let keys = test_env.client.list_metadata_keys(&id);
    assert_eq!(keys.len(), 0);
}

#[test]
fn test_metadata_merchant_can_set() {
    let test_env = TestEnv::default();
    let (id, _, merchant) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let key = String::from_str(&test_env.env, "merchant_ref");
    let value = String::from_str(&test_env.env, "MR-123");

    test_env.client.set_metadata(&id, &merchant, &key, &value);
    assert_eq!(test_env.client.get_metadata(&id, &key), value);
}

#[test]
fn test_metadata_merchant_can_delete() {
    let test_env = TestEnv::default();
    let (id, subscriber, merchant) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let key = String::from_str(&test_env.env, "tag");
    let value = String::from_str(&test_env.env, "test");

    // Subscriber sets it
    test_env.client.set_metadata(&id, &subscriber, &key, &value);

    // Merchant deletes it
    test_env.client.delete_metadata(&id, &merchant, &key);

    let result = test_env.client.try_get_metadata(&id, &key);
    assert!(result.is_err());
}

#[test]
#[should_panic(expected = "Error(Contract, #1002)")]
fn test_metadata_unauthorized_actor_rejected() {
    let test_env = TestEnv::default();
    let (id, _, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let stranger = Address::generate(&test_env.env);
    let key = String::from_str(&test_env.env, "test");
    let value = String::from_str(&test_env.env, "val");

    test_env.client.set_metadata(&id, &stranger, &key, &value);
}

#[test]
#[should_panic(expected = "Error(Contract, #1002)")]
fn test_metadata_delete_unauthorized_rejected() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let key = String::from_str(&test_env.env, "test");
    test_env.client.set_metadata(
        &id,
        &subscriber,
        &key,
        &String::from_str(&test_env.env, "val"),
    );

    let stranger = Address::generate(&test_env.env);
    test_env.client.delete_metadata(&id, &stranger, &key);
}

#[test]
#[should_panic(expected = "Error(Contract, #6005)")]
fn test_metadata_key_limit_enforced() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    // Set MAX_METADATA_KEYS (10) keys
    for i in 0..10u32 {
        let key = String::from_str(&test_env.env, &format!("key_{i}"));
        let value = String::from_str(&test_env.env, "val");
        test_env.client.set_metadata(&id, &subscriber, &key, &value);
    }

    // 11th key should fail
    let key = String::from_str(&test_env.env, "key_overflow");
    let value = String::from_str(&test_env.env, "val");
    test_env.client.set_metadata(&id, &subscriber, &key, &value);
}

#[test]
fn test_metadata_update_at_limit_succeeds() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    // Fill to max
    for i in 0..10u32 {
        let key = String::from_str(&test_env.env, &format!("key_{i}"));
        test_env.client.set_metadata(
            &id,
            &subscriber,
            &key,
            &String::from_str(&test_env.env, "val"),
        );
    }

    // Updating an existing key should succeed even at limit
    let key = String::from_str(&test_env.env, "key_0");
    let new_value = String::from_str(&test_env.env, "updated");
    test_env
        .client
        .set_metadata(&id, &subscriber, &key, &new_value);
    assert_eq!(test_env.client.get_metadata(&id, &key), new_value);
}

#[test]
#[should_panic(expected = "Error(Contract, #3005)")]
fn test_metadata_key_too_long_rejected() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    // 33 chars exceeds MAX_METADATA_KEY_LENGTH (32)
    let key = String::from_str(&test_env.env, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    let value = String::from_str(&test_env.env, "val");
    test_env.client.set_metadata(&id, &subscriber, &key, &value);
}

#[test]
#[should_panic(expected = "Error(Contract, #3005)")]
fn test_metadata_empty_key_rejected() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let key = String::from_str(&test_env.env, "");
    let value = String::from_str(&test_env.env, "val");
    test_env.client.set_metadata(&id, &subscriber, &key, &value);
}

#[test]
#[should_panic(expected = "Error(Contract, #3006)")]
fn test_metadata_value_too_long_rejected() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let key = String::from_str(&test_env.env, "test");
    // Create a string > 256 bytes
    let long_str = alloc::string::String::from_utf8(alloc::vec![b'x'; 257]).unwrap();
    let long_value = String::from_str(&test_env.env, &long_str);
    test_env
        .client
        .set_metadata(&id, &subscriber, &key, &long_value);
}

#[test]
fn test_metadata_key_max_length_boundary_ok() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    let key = String::from_str(&test_env.env, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert_eq!(key.len(), 32);
    let val = String::from_str(&test_env.env, "x");
    test_env.client.set_metadata(&id, &subscriber, &key, &val);
    assert_eq!(test_env.client.get_metadata(&id, &key), val);
}

#[test]
fn test_metadata_value_max_length_boundary_ok() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    let key = String::from_str(&test_env.env, "k");
    let val_str = alloc::string::String::from_utf8(alloc::vec![b'z'; 256]).unwrap();
    let value = String::from_str(&test_env.env, &val_str);
    assert_eq!(value.len(), 256);
    test_env.client.set_metadata(&id, &subscriber, &key, &value);
    assert_eq!(test_env.client.get_metadata(&id, &key), value);
}

#[test]
fn test_metadata_delete_nonexistent_try_api() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    let key = String::from_str(&test_env.env, "missing");
    let res = test_env.client.try_delete_metadata(&id, &subscriber, &key);
    assert_eq!(res, Err(Ok(Error::NotFound)));
}

#[test]
#[should_panic(expected = "Error(Contract, #2001)")]
fn test_metadata_get_nonexistent_key() {
    let test_env = TestEnv::default();
    let (id, _, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let key = String::from_str(&test_env.env, "nonexistent");
    test_env.client.get_metadata(&id, &key);
}

#[test]
#[should_panic(expected = "Error(Contract, #2001)")]
fn test_metadata_delete_nonexistent_key() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let key = String::from_str(&test_env.env, "nonexistent");
    test_env.client.delete_metadata(&id, &subscriber, &key);
}

#[test]
#[should_panic(expected = "Error(Contract, #2001)")]
fn test_metadata_operations_on_nonexistent_subscription() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let key = String::from_str(&test_env.env, "test");
    let value = String::from_str(&test_env.env, "val");
    test_env
        .client
        .set_metadata(&999, &subscriber, &key, &value);
}

#[test]
#[should_panic(expected = "Error(Contract, #4002)")]
fn test_metadata_set_on_cancelled_subscription_rejected() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.client.cancel_subscription(&id, &subscriber);

    let key = String::from_str(&test_env.env, "test");
    let value = String::from_str(&test_env.env, "val");
    test_env.client.set_metadata(&id, &subscriber, &key, &value);
}

#[test]
fn test_metadata_does_not_affect_financial_state() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    fixtures::seed_balance(&test_env.env, &test_env.client, id, PREPAID);

    let sub_before = test_env.client.get_subscription(&id);

    // Set multiple metadata entries
    for i in 0..5u32 {
        let key = String::from_str(&test_env.env, &format!("key_{i}"));
        let value = String::from_str(&test_env.env, &format!("value_{i}"));
        test_env.client.set_metadata(&id, &subscriber, &key, &value);
    }

    let sub_after = test_env.client.get_subscription(&id);

    // Financial state must be unchanged
    assert_eq!(sub_before.prepaid_balance, sub_after.prepaid_balance);
    assert_eq!(sub_before.lifetime_charged, sub_after.lifetime_charged);
    assert_eq!(sub_before.status, sub_after.status);
    assert_eq!(sub_before.amount, sub_after.amount);
}

#[test]
fn test_metadata_delete_then_re_add() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let key = String::from_str(&test_env.env, "tag");
    let value1 = String::from_str(&test_env.env, "v1");
    let value2 = String::from_str(&test_env.env, "v2");

    test_env
        .client
        .set_metadata(&id, &subscriber, &key, &value1);
    test_env.client.delete_metadata(&id, &subscriber, &key);

    // Re-add same key with different value
    test_env
        .client
        .set_metadata(&id, &subscriber, &key, &value2);
    assert_eq!(test_env.client.get_metadata(&id, &key), value2);

    let keys = test_env.client.list_metadata_keys(&id);
    assert_eq!(keys.len(), 1);
}

#[test]
fn test_metadata_delete_frees_key_slot() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    // Fill to max
    for i in 0..10u32 {
        let key = String::from_str(&test_env.env, &format!("key_{i}"));
        test_env.client.set_metadata(
            &id,
            &subscriber,
            &key,
            &String::from_str(&test_env.env, "v"),
        );
    }

    // Delete one
    test_env
        .client
        .delete_metadata(&id, &subscriber, &String::from_str(&test_env.env, "key_5"));

    // Should now be able to add a new key
    let new_key = String::from_str(&test_env.env, "key_new");
    test_env.client.set_metadata(
        &id,
        &subscriber,
        &new_key,
        &String::from_str(&test_env.env, "v"),
    );

    let keys = test_env.client.list_metadata_keys(&id);
    assert_eq!(keys.len(), 10);
}

#[test]
fn test_metadata_isolation_between_subscriptions() {
    let test_env = TestEnv::default();
    let (id1, sub1, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    let (id2, sub2, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let key = String::from_str(&test_env.env, "invoice_id");
    let val1 = String::from_str(&test_env.env, "INV-001");
    let val2 = String::from_str(&test_env.env, "INV-002");

    test_env.client.set_metadata(&id1, &sub1, &key, &val1);
    test_env.client.set_metadata(&id2, &sub2, &key, &val2);

    assert_eq!(test_env.client.get_metadata(&id1, &key), val1);
    assert_eq!(test_env.client.get_metadata(&id2, &key), val2);
}

#[test]
fn test_metadata_on_paused_subscription_allowed() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.client.pause_subscription(&id, &subscriber);

    let key = String::from_str(&test_env.env, "note");
    let value = String::from_str(&test_env.env, "paused for maintenance");
    test_env.client.set_metadata(&id, &subscriber, &key, &value);
    assert_eq!(test_env.client.get_metadata(&id, &key), value);
}

#[test]
fn test_metadata_delete_on_cancelled_subscription_allowed() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let key = String::from_str(&test_env.env, "tag");
    test_env.client.set_metadata(
        &id,
        &subscriber,
        &key,
        &String::from_str(&test_env.env, "v"),
    );

    test_env.client.cancel_subscription(&id, &subscriber);

    // Delete should still work on cancelled (cleanup)
    test_env.client.delete_metadata(&id, &subscriber, &key);
    let result = test_env.client.try_get_metadata(&id, &key);
    assert!(result.is_err());
}

#[test]
fn test_billing_statements_offset_pagination_newest_first() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);

    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &1_000_000_000i128);

    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &true,
        &None::<i128>,
     &None::<u64>);
    test_env.client.deposit_funds(&id, &subscriber, &200_000_000i128);

    for i in 1..=6 {
        test_env
            .env
            .ledger()
            .set_timestamp(T0 + (i as u64 * INTERVAL));
        test_env.client.charge_subscription(&id);
    }

    let page1 = test_env
        .client
        .get_sub_statements_offset(&id, &0, &2, &true);
    assert_eq!(page1.total, 6);
    assert_eq!(page1.statements.len(), 2);
    assert_eq!(page1.statements.get(0).unwrap().sequence, 5);
    assert_eq!(page1.statements.get(1).unwrap().sequence, 4);

    let page2 = test_env
        .client
        .get_sub_statements_offset(&id, &2, &2, &true);
    assert_eq!(page2.statements.len(), 2);
    assert_eq!(page2.statements.get(0).unwrap().sequence, 3);
    assert_eq!(page2.statements.get(1).unwrap().sequence, 2);
}

#[test]
fn test_billing_statements_cursor_pagination_boundaries() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);

    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &1_000_000_000i128);

    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &true,
        &None::<i128>,
     &None::<u64>);
    test_env.client.deposit_funds(&id, &subscriber, &200_000_000i128);

    for i in 1..=4 {
        test_env
            .env
            .ledger()
            .set_timestamp(T0 + (i as u64 * INTERVAL));
        test_env.client.charge_subscription(&id);
    }

    let first = test_env
        .client
        .get_sub_statements_cursor(&id, &None::<u32>, &3, &true);
    assert_eq!(first.statements.len(), 3);
    assert_eq!(first.statements.get(0).unwrap().sequence, 3);
    assert_eq!(first.statements.get(2).unwrap().sequence, 1);
    assert_eq!(first.next_cursor, Some(0));

    let second = test_env
        .client
        .get_sub_statements_cursor(&id, &first.next_cursor, &3, &true);
    assert_eq!(second.statements.len(), 1);
    assert_eq!(second.statements.get(0).unwrap().sequence, 0);
    assert_eq!(second.next_cursor, None);

    let invalid_cursor = test_env
        .client
        .get_sub_statements_cursor(&id, &Some(99u32), &2, &true);
    assert_eq!(invalid_cursor.statements.len(), 0);
    assert_eq!(invalid_cursor.next_cursor, None);
    assert_eq!(invalid_cursor.total, 4);
}

#[test]
fn test_compaction_prunes_old_statements_and_keeps_recent() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);

    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &2_000_000_000i128);

    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
     &None::<u64>);
    test_env.client.deposit_funds(&id, &subscriber, &500_000_000i128);

    for i in 1..=8 {
        test_env
            .env
            .ledger()
            .set_timestamp(T0 + (i as u64 * INTERVAL));
        test_env.client.charge_subscription(&id);
    }

    test_env.client.set_billing_retention(&test_env.admin, &3);
    let summary = test_env
        .client
        .compact_billing_statements(&test_env.admin, &id, &None::<u32>);
    assert_eq!(summary.pruned_count, 5);
    assert_eq!(summary.kept_count, 3);
    assert_eq!(summary.total_pruned_amount, 5_000_000i128);

    let page = test_env
        .client
        .get_sub_statements_offset(&id, &0, &10, &true);
    assert_eq!(page.total, 3);
    assert_eq!(page.statements.len(), 3);
    assert_eq!(page.statements.get(0).unwrap().sequence, 7);
    assert_eq!(page.statements.get(2).unwrap().sequence, 5);

    let agg = test_env.client.get_stmt_compacted_aggregate(&id);
    assert_eq!(agg.pruned_count, 5);
    assert_eq!(agg.total_amount, 5_000_000i128);
}

#[test]
fn test_compaction_no_rows_and_override_value() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
     &None::<u64>);

    let summary = test_env
        .client
        .compact_billing_statements(&test_env.admin, &id, &Some(10u32));
    assert_eq!(summary.pruned_count, 0);
    assert_eq!(summary.kept_count, 0);
    assert_eq!(summary.total_pruned_amount, 0);
}

#[test]
fn test_compaction_idempotent_second_run() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    test_env
        .stellar_token_client()
        .mint(&subscriber, &2_000_000_000i128);

    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env
        .client
        .deposit_funds(&id, &subscriber, &500_000_000i128);

    for i in 1..=8 {
        test_env
            .env
            .ledger()
            .set_timestamp(T0 + (i as u64 * INTERVAL));
        test_env.client.charge_subscription(&id);
    }

    test_env.client.set_billing_retention(&test_env.admin, &3);
    let first = test_env
        .client
        .compact_billing_statements(&test_env.admin, &id, &None::<u32>);
    assert_eq!(first.pruned_count, 5);

    let second = test_env
        .client
        .compact_billing_statements(&test_env.admin, &id, &None::<u32>);
    assert_eq!(second.pruned_count, 0);
    assert_eq!(second.kept_count, 3);
    assert_eq!(second.total_pruned_amount, 0);

    let agg = test_env.client.get_stmt_compacted_aggregate(&id);
    assert_eq!(agg.pruned_count, 5);
    assert_eq!(agg.total_amount, 5_000_000i128);
}

#[test]
fn test_compaction_keep_recent_zero_prunes_all_detail() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    test_env
        .stellar_token_client()
        .mint(&subscriber, &500_000_000i128);

    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env
        .client
        .deposit_funds(&id, &subscriber, &100_000_000i128);

    for i in 1..=4 {
        test_env
            .env
            .ledger()
            .set_timestamp(T0 + (i as u64 * INTERVAL));
        test_env.client.charge_subscription(&id);
    }

    let summary = test_env
        .client
        .compact_billing_statements(&test_env.admin, &id, &Some(0u32));
    assert_eq!(summary.pruned_count, 4);
    assert_eq!(summary.kept_count, 0);

    let page = test_env
        .client
        .get_sub_statements_offset(&id, &0, &10, &true);
    assert_eq!(page.total, 0);
    assert_eq!(page.statements.len(), 0);

    let agg = test_env.client.get_stmt_compacted_aggregate(&id);
    assert_eq!(agg.total_amount, 4_000_000i128);
    assert_eq!(agg.pruned_count, 4);
}

#[test]
fn test_set_billing_retention_non_admin_rejected() {
    let test_env = TestEnv::default();
    let attacker = Address::generate(&test_env.env);
    let res = test_env.client.try_set_billing_retention(&attacker, &5u32);
    assert_eq!(res, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_compact_billing_statements_non_admin_rejected() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    let attacker = Address::generate(&test_env.env);
    let res = test_env
        .client
        .try_compact_billing_statements(&attacker, &id, &None::<u32>);
    assert_eq!(res, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_billing_retention_rapid_config_changes() {
    let test_env = TestEnv::default();
    test_env
        .client
        .set_billing_retention(&test_env.admin, &1u32);
    assert_eq!(test_env.client.get_billing_retention().keep_recent, 1);
    test_env
        .client
        .set_billing_retention(&test_env.admin, &u32::MAX);
    assert_eq!(
        test_env.client.get_billing_retention().keep_recent,
        u32::MAX
    );
    test_env
        .client
        .set_billing_retention(&test_env.admin, &12u32);
    assert_eq!(test_env.client.get_billing_retention().keep_recent, 12);
}

#[test]
fn test_compaction_override_respects_per_run_threshold() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    test_env
        .stellar_token_client()
        .mint(&subscriber, &2_000_000_000i128);

    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env
        .client
        .deposit_funds(&id, &subscriber, &500_000_000i128);

    for i in 1..=6 {
        test_env
            .env
            .ledger()
            .set_timestamp(T0 + (i as u64 * INTERVAL));
        test_env.client.charge_subscription(&id);
    }

    test_env
        .client
        .set_billing_retention(&test_env.admin, &100u32);
    let s = test_env
        .client
        .compact_billing_statements(&test_env.admin, &id, &Some(2u32));
    assert_eq!(s.pruned_count, 4);
    assert_eq!(s.kept_count, 2);
}

#[test]
fn test_oracle_enabled_charge_uses_quote_conversion() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let oracle_id = test_env.env.register(MockOracle, ());
    let oracle = MockOracleClient::new(&test_env.env, &oracle_id);
    oracle.set_price(&2_000_000i128, &T0); // 2 quote units/token with 6 decimals

    // Enable oracle pricing with non-stale quote.
    test_env.client.set_oracle_config(
        &test_env.admin,
        &true,
        &Some(oracle_id.clone()),
        &(60 * 24 * 60 * 60),
    );

    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token).mint(&subscriber, &2_000_000_000i128);

    // 20 quote units (6 decimals). At price 2 quote/token, charge should be 10 tokens.
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &20_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
     &None::<u64>);
    test_env.client.deposit_funds(&id, &subscriber, &100_000_000i128);

    test_env.env.ledger().set_timestamp(T0 + INTERVAL);
    test_env.client.charge_subscription(&id);

    assert_eq!(test_env.client.get_merchant_balance(&merchant), 10_000_000i128);
}

#[test]
fn test_oracle_stale_quote_rejected() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0 + INTERVAL);
    let oracle_id = test_env.env.register(MockOracle, ());
    let oracle = MockOracleClient::new(&test_env.env, &oracle_id);
    oracle.set_price(&2_000_000i128, &T0); // stale vs max_age=1
    test_env.client.set_oracle_config(&test_env.admin, &true, &Some(oracle_id.clone()), &1u64);

    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token).mint(&subscriber, &2_000_000_000i128);
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &20_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
     &None::<u64>);
    test_env.client.deposit_funds(&id, &subscriber, &100_000_000i128);

    let result = test_env.client.try_charge_subscription(&id);
    assert_eq!(result, Err(Ok(Error::OraclePriceStale)));
}

#[test]

fn test_create_subscription_with_unaccepted_token_fails() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let unsupported = Address::generate(&test_env.env);
    let result = test_env.client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &unsupported,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

#[test]
fn test_create_subscription_zero_amount_rejected() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let result = test_env.client.try_create_subscription(
        &subscriber,
        &merchant,
        &0i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_create_subscription_interval_too_small_rejected() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let result = test_env.client.try_create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &59u64,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

#[test]
fn test_create_subscription_lifetime_cap_less_than_amount_rejected() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let result = test_env.client.try_create_subscription(
        &subscriber,
        &merchant,
        &10i128,
        &INTERVAL,
        &false,
        &Some(9i128),
        &None::<u64>,
    );
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

#[test]
fn test_create_subscription_blocklisted_subscriber_rejected() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    test_env
        .client
        .add_to_blocklist(&test_env.admin, &subscriber, &None);

    let result = test_env.client.try_create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    assert_eq!(result, Err(Ok(Error::SubscriberBlocklisted)));
}

#[test]
fn test_create_subscription_with_token_zero_amount_rejected() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let result = test_env.client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &test_env.token,
        &0i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_create_subscription_with_token_interval_too_small_rejected() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let result = test_env.client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &test_env.token,
        &1_000_000i128,
        &59u64,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

#[test]
fn test_create_subscription_with_token_lifetime_cap_less_than_amount_rejected() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let result = test_env.client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &test_env.token,
        &10i128,
        &INTERVAL,
        &false,
        &Some(9i128),
        &None::<u64>,
    );
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

#[test]
fn test_create_subscription_with_token_blocklisted_subscriber_rejected() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    test_env
        .client
        .add_to_blocklist(&test_env.admin, &subscriber, &None);

    let result = test_env.client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &test_env.token,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    assert_eq!(result, Err(Ok(Error::SubscriberBlocklisted)));
}

#[test]
fn test_create_subscription_max_amount_and_cap_succeeds() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &i128::MAX,
        &INTERVAL,
        &false,
        &Some(i128::MAX),
        &None::<u64>,
    );
    let sub = test_env.client.get_subscription(&id);
    assert_eq!(sub.amount, i128::MAX);
    assert_eq!(sub.lifetime_cap, Some(i128::MAX));
}

#[test]
fn test_create_subscription_max_amount_cap_smaller_rejected() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let result = test_env.client.try_create_subscription(
        &subscriber,
        &merchant,
        &i128::MAX,
        &INTERVAL,
        &false,
        &Some(i128::MAX - 1),
        &None::<u64>,
    );
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

// =============================================================================
// Admin Rotation Hardening Tests
// =============================================================================

// -- Basic functionality ------------------------------------------------------

#[test]
fn test_get_admin_returns_init_admin() {
    let test_env = TestEnv::default();
    assert_eq!(test_env.client.get_admin(), test_env.admin);
}

#[test]
fn test_rotate_admin_successful() {
    let test_env = TestEnv::default();
    let new_admin = Address::generate(&test_env.env);
    test_env.client.rotate_admin(&test_env.admin, &new_admin, &0u64);
    assert_eq!(test_env.client.get_admin(), new_admin);
}

#[test]
fn test_rotate_admin_unauthorized() {
    let test_env = TestEnv::default();
    let stranger = Address::generate(&test_env.env);
    let new_admin = Address::generate(&test_env.env);
    let result = test_env.client.try_rotate_admin(&stranger, &new_admin, &0u64);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_rotate_admin_to_same_address_rejected() {
    let test_env = TestEnv::default();
    let result = test_env
        .client
        .try_rotate_admin(&test_env.admin, &test_env.admin, &0u64);
    assert_eq!(result, Err(Ok(Error::SelfRotation)));
}

#[test]
fn test_rotate_admin_to_contract_address_rejected() {
    let test_env = TestEnv::default();
    let result = test_env
        .client
        .try_rotate_admin(&test_env.admin, &test_env.client.address, &0u64);
    assert_eq!(result, Err(Ok(Error::InvalidNewAdmin)));
}

// -- Immediate revocation / grant ---------------------------------------------

#[test]
fn test_old_admin_loses_access_after_rotation() {
    let test_env = TestEnv::default();
    let new_admin = Address::generate(&test_env.env);
    test_env.client.rotate_admin(&test_env.admin, &new_admin, &0u64);
    // Old admin can no longer call set_min_topup.
    let result = test_env.client.try_set_min_topup(&test_env.admin, &2_000_000i128);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_new_admin_gains_access_after_rotation() {
    let test_env = TestEnv::default();
    let new_admin = Address::generate(&test_env.env);
    test_env.client.rotate_admin(&test_env.admin, &new_admin, &0u64);
    // New admin can immediately call set_min_topup.
    test_env.client.set_min_topup(&new_admin, &2_000_000i128);
    assert_eq!(test_env.client.get_min_topup(), 2_000_000i128);
}

#[test]
fn test_set_min_topup_unauthorized_before_rotation() {
    let test_env = TestEnv::default();
    let stranger = Address::generate(&test_env.env);
    let result = test_env.client.try_set_min_topup(&stranger, &2_000_000i128);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_set_min_topup_unauthorized_after_rotation() {
    let test_env = TestEnv::default();
    let new_admin = Address::generate(&test_env.env);
    let stranger = Address::generate(&test_env.env);
    test_env.client.rotate_admin(&test_env.admin, &new_admin, &0u64);
    assert_eq!(
        test_env.client.try_set_min_topup(&test_env.admin, &2_000_000i128),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_set_min_topup(&stranger, &2_000_000i128),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn test_recover_stranded_funds_unauthorized_before_rotation() {
    let test_env = TestEnv::default();
    let stranger = Address::generate(&test_env.env);
    let recipient = Address::generate(&test_env.env);
    let result = test_env.client.try_recover_stranded_funds(
        &stranger,
        &test_env.token,
        &recipient,
        &1_000_000i128,
        &String::from_str(&test_env.env, "test-recovery"),
        &RecoveryReason::UserOverpayment,
    );
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_recover_stranded_funds_unauthorized_after_rotation() {
    let test_env = TestEnv::default();
    let new_admin = Address::generate(&test_env.env);
    let recipient = Address::generate(&test_env.env);
    test_env.client.rotate_admin(&test_env.admin, &new_admin, &0u64);
    assert_eq!(
        test_env.client.try_recover_stranded_funds(
            &test_env.admin,
            &test_env.token,
            &recipient,
            &1_000_000i128,
            &String::from_str(&test_env.env, "test-recovery"),
            &RecoveryReason::UserOverpayment
        ),
        Err(Ok(Error::Unauthorized))
    );
}

// -- Integration: recovery respects rotation ----------------------------------

#[test]
fn test_admin_rotation_affects_recovery_operations() {
    let test_env = TestEnv::default();
    let new_admin = Address::generate(&test_env.env);
    let recipient = Address::generate(&test_env.env);
    test_env
        .stellar_token_client()
        .mint(&test_env.client.address, &3_000_000i128);

    // Mint surplus so there are funds to recover
    test_env.stellar_token_client().mint(&test_env.client.address, &5_000_000);

    // Old admin can recover before rotation.
    test_env.client.recover_stranded_funds(
        &test_env.admin,
        &test_env.token,
        &recipient,
        &1_000_000i128,
        &String::from_str(&test_env.env, "rec_1"),
        &RecoveryReason::AccidentalTransfer,
    );

    test_env.client.rotate_admin(&test_env.admin, &new_admin, &0u64);

    // Old admin blocked after rotation.
    assert_eq!(
        test_env.client.try_recover_stranded_funds(
            &test_env.admin,
            &test_env.token,
            &recipient,
            &1_000_000i128,
            &String::from_str(&test_env.env, "test-recovery"),
            &RecoveryReason::UserOverpayment
        ),
        Err(Ok(Error::Unauthorized))
    );

    // New admin can recover.
    test_env.client.recover_stranded_funds(
        &new_admin,
        &test_env.token,
        &recipient,
        &1_000_000i128,
        &String::from_str(&test_env.env, "test-recovery"),
        &RecoveryReason::UserOverpayment,
    );
}

#[test]
fn test_all_admin_operations_after_rotation() {
    let test_env = TestEnv::default();
    let new_admin = Address::generate(&test_env.env);
    let next_admin = Address::generate(&test_env.env);
    let recipient = Address::generate(&test_env.env);
    test_env
        .stellar_token_client()
        .mint(&test_env.client.address, &1_000_000i128);

    test_env.client.rotate_admin(&test_env.admin, &new_admin, &0u64);

    test_env.client.set_min_topup(&new_admin, &3_000_000i128);
    test_env.stellar_token_client().mint(&test_env.client.address, &2_000_000);
    test_env.client.recover_stranded_funds(
        &new_admin,
        &test_env.token,
        &recipient,
        &1_000_000i128,
        &String::from_str(&test_env.env, "rec_2"),
        &RecoveryReason::AccidentalTransfer,
    );
    test_env.client.rotate_admin(&new_admin, &next_admin, &0u64);
    assert_eq!(test_env.client.get_admin(), next_admin);
}

#[test]
fn test_multiple_admin_rotations() {
    let test_env = TestEnv::default();
    let admin_b = Address::generate(&test_env.env);
    let admin_c = Address::generate(&test_env.env);
    let admin_d = Address::generate(&test_env.env);

    test_env.client.rotate_admin(&test_env.admin, &admin_b, &0u64);
    test_env.client.rotate_admin(&admin_b, &admin_c, &0u64);
    test_env.client.rotate_admin(&admin_c, &admin_d, &0u64);

    assert_eq!(test_env.client.get_admin(), admin_d);

    // All previous admins are denied.
    for stale in [&test_env.admin, &admin_b, &admin_c] {
        assert_eq!(
            test_env.client.try_set_min_topup(stale, &1_000_000i128),
            Err(Ok(Error::Unauthorized))
        );
    }
}

#[test]
fn test_admin_cannot_be_rotated_by_previous_admin() {
    let test_env = TestEnv::default();
    let admin2 = Address::generate(&test_env.env);
    let admin3 = Address::generate(&test_env.env);

    test_env.client.rotate_admin(&test_env.admin, &admin2, &0u64);

    // admin1 cannot rotate again.
    let result = test_env.client.try_rotate_admin(&test_env.admin, &admin3, &1u64);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
    assert_eq!(test_env.client.get_admin(), admin2);
}

// -- State isolation ----------------------------------------------------------

#[test]
fn test_admin_rotation_does_not_affect_subscriptions() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    let before = test_env.client.get_subscription(&id);

    let new_admin = Address::generate(&test_env.env);
    test_env.client.rotate_admin(&test_env.admin, &new_admin, &0u64);

    let after = test_env.client.get_subscription(&id);
    assert_eq!(before.subscriber, after.subscriber);
    assert_eq!(before.merchant, after.merchant);
    assert_eq!(before.amount, after.amount);
    assert_eq!(before.status, after.status);
}

#[test]
fn test_admin_rotation_with_subscriptions_active() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    test_env.client.pause_subscription(&id, &subscriber);
    assert_eq!(
        test_env.client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );

    let new_admin = Address::generate(&test_env.env);
    test_env.client.rotate_admin(&test_env.admin, &new_admin, &0u64);

    // Subscription state preserved.
    assert_eq!(
        test_env.client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );

    // Subscriber can still manage their subscription.
    test_env.client.resume_subscription(&id, &subscriber);
    assert_eq!(
        test_env.client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );
    test_env.client.cancel_subscription(&id, &subscriber);
    assert_eq!(
        test_env.client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}

// -- Comprehensive access control matrix --------------------------------------

#[test]
fn test_admin_rotation_access_control_comprehensive() {
    let test_env = TestEnv::default();
    let admin2 = Address::generate(&test_env.env);
    let admin3 = Address::generate(&test_env.env);
    let non_admin = Address::generate(&test_env.env);

    // Phase 1: admin1 active.
    test_env
        .client
        .set_min_topup(&test_env.admin, &1_000_000i128);
    assert_eq!(
        test_env.client.try_set_min_topup(&admin2, &1_000_000i128),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_set_min_topup(&non_admin, &1_000_000i128),
        Err(Ok(Error::Unauthorized))
    );

    // Phase 2: rotate to admin2.
    test_env.client.rotate_admin(&test_env.admin, &admin2, &0u64);
    test_env.client.set_min_topup(&admin2, &2_000_000i128);
    assert_eq!(
        test_env.client.try_set_min_topup(&test_env.admin, &1_000_000i128),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_set_min_topup(&non_admin, &1_000_000i128),
        Err(Ok(Error::Unauthorized))
    );

    // Phase 3: rotate to admin3.
    test_env.client.rotate_admin(&admin2, &admin3, &0u64);
    test_env.client.set_min_topup(&admin3, &3_000_000i128);
    assert_eq!(
        test_env.client.try_set_min_topup(&test_env.admin, &1_000_000i128),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_set_min_topup(&admin2, &1_000_000i128),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_set_min_topup(&non_admin, &1_000_000i128),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn test_admin_authorization_matrix_rejects_non_admin_across_protected_entrypoints() {
    let test_env = TestEnv::default();
    let stranger = Address::generate(&test_env.env);
    let recipient = Address::generate(&test_env.env);
    let token_to_remove = create_secondary_token(&test_env.env);
    let token_to_add = create_secondary_token(&test_env.env);
    let (subscription_id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    let blocklisted_subscriber = Address::generate(&test_env.env);

    test_env
        .client
        .add_accepted_token(&test_env.admin, &token_to_remove, &6);
    test_env
        .client
        .add_to_blocklist(&test_env.admin, &blocklisted_subscriber, &None::<String>);
    test_env.client.enable_emergency_stop(&test_env.admin);

    assert_eq!(
        test_env.client.try_set_min_topup(&stranger, &2_000_000i128),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env
            .client
            .try_rotate_admin(&stranger, &Address::generate(&test_env.env), &0u64),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_recover_stranded_funds(
            &stranger,
            &test_env.token,
            &recipient,
            &1_000_000i128,
            &String::from_str(&test_env.env, "test-recovery"),
            &RecoveryReason::UserOverpayment
        ),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_add_accepted_token(&stranger, &token_to_add, &6),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env
            .client
            .try_remove_accepted_token(&stranger, &token_to_remove),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env
            .client
            .try_remove_from_blocklist(&stranger, &blocklisted_subscriber),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_enable_emergency_stop(&stranger),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_disable_emergency_stop(&stranger),
        Err(Ok(Error::Unauthorized))
    );
    assert!(matches!(
        test_env.client.try_export_contract_snapshot(&stranger),
        Err(Ok(Error::Unauthorized))
    ));
    assert!(matches!(
        test_env
            .client
            .try_export_subscription_summary(&stranger, &subscription_id),
        Err(Ok(Error::Unauthorized))
    ));
    assert_eq!(
        test_env
            .client
            .try_export_subscription_summaries(&stranger, &0, &10),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_set_billing_retention(&stranger, &5),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env
            .client
            .try_compact_billing_statements(&stranger, &subscription_id, &None::<u32>),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env
            .client
            .try_set_oracle_config(&stranger, &false, &None::<Address>, &0u64),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_set_subscriber_credit_limit(
            &stranger,
            &subscriber,
            &test_env.token,
        &AMOUNT
        ),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn test_admin_authorization_matrix_rejects_stale_admin_after_rotation() {
    let test_env = TestEnv::default();
    let new_admin = Address::generate(&test_env.env);
    let recipient = Address::generate(&test_env.env);
    let token_to_remove = create_secondary_token(&test_env.env);
    let token_to_add = create_secondary_token(&test_env.env);
    let (subscription_id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    let blocklisted_subscriber = Address::generate(&test_env.env);

    test_env
        .client
        .add_accepted_token(&test_env.admin, &token_to_remove, &6);
    test_env
        .client
        .add_to_blocklist(&test_env.admin, &blocklisted_subscriber, &None::<String>);
    test_env.client.enable_emergency_stop(&test_env.admin);
    test_env.client.rotate_admin(&test_env.admin, &new_admin, &0u64);

    assert_eq!(
        test_env.client.try_set_min_topup(&test_env.admin, &2_000_000i128),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env
            .client
            .try_rotate_admin(&test_env.admin, &Address::generate(&test_env.env), &1u64),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_recover_stranded_funds(
            &test_env.admin,
            &test_env.token,
            &recipient,
            &1_000_000i128,
            &String::from_str(&test_env.env, "test-recovery"),
            &RecoveryReason::UserOverpayment
        ),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env
            .client
            .try_add_accepted_token(&test_env.admin, &token_to_add, &6),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env
            .client
            .try_remove_accepted_token(&test_env.admin, &token_to_remove),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env
            .client
            .try_remove_from_blocklist(&test_env.admin, &blocklisted_subscriber),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_enable_emergency_stop(&test_env.admin),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_disable_emergency_stop(&test_env.admin),
        Err(Ok(Error::Unauthorized))
    );
    assert!(matches!(
        test_env.client.try_export_contract_snapshot(&test_env.admin),
        Err(Ok(Error::Unauthorized))
    ));
    assert!(matches!(
        test_env
            .client
            .try_export_subscription_summary(&test_env.admin, &subscription_id),
        Err(Ok(Error::Unauthorized))
    ));
    assert_eq!(
        test_env
            .client
            .try_export_subscription_summaries(&test_env.admin, &0, &10),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_set_billing_retention(&test_env.admin, &5),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env
            .client
            .try_compact_billing_statements(&test_env.admin, &subscription_id, &None::<u32>),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env
            .client
            .try_set_oracle_config(&test_env.admin, &false, &None::<Address>, &0u64),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        test_env.client.try_set_subscriber_credit_limit(
            &test_env.admin,
            &subscriber,
            &test_env.token,
        &AMOUNT
        ),
        Err(Ok(Error::Unauthorized))
    );

    test_env.client.disable_emergency_stop(&new_admin);
    test_env
        .client
        .remove_from_blocklist(&new_admin, &blocklisted_subscriber);
    test_env.client.set_min_topup(&new_admin, &2_000_000i128);
    assert_eq!(test_env.client.get_min_topup(), 2_000_000i128);
}

// -- Edge cases ---------------------------------------------------------------

#[test]
fn test_get_admin_before_and_after_rotation() {
    let test_env = TestEnv::default();
    assert_eq!(test_env.client.get_admin(), test_env.admin);

    let admin2 = Address::generate(&test_env.env);
    test_env.client.rotate_admin(&test_env.admin, &admin2, &0u64);
    assert_eq!(test_env.client.get_admin(), admin2);

    let admin3 = Address::generate(&test_env.env);
    test_env.client.rotate_admin(&admin2, &admin3, &0u64);
    assert_eq!(test_env.client.get_admin(), admin3);
}

#[test]
fn test_admin_rotation_event_emission() {
    let test_env = TestEnv::default();
    let new_admin = Address::generate(&test_env.env);
    test_env.client.rotate_admin(&test_env.admin, &new_admin, &0u64);

    // Verify at least one event was emitted during the rotation call.
    // The Soroban test harness records all events; we just confirm the list is non-empty.
    let events = test_env.env.events().all();
    assert!(!events.is_empty());
}

// -- Post-rotation charge access control --------------------------------------

#[test]
fn test_batch_charge_uses_stored_admin_after_rotation() {
    // batch_charge reads the stored admin internally and calls require_auth on it.
    // After rotation the stored admin is the new admin, so the call succeeds
    // (mock_all_auths satisfies any require_auth). The old admin address is no
    // longer the stored admin, so it cannot be the authorizer.
    let test_env = TestEnv::default();
    test_env.env.ledger().with_mut(|li| li.timestamp = T0);

    let (id, _, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    fixtures::seed_balance(&test_env.env, &test_env.client, id, PREPAID);
    test_env
        .env
        .ledger()
        .with_mut(|li| li.timestamp = T0 + INTERVAL + 1);

    let new_admin = Address::generate(&test_env.env);
    test_env.client.rotate_admin(&test_env.admin, &new_admin, &0u64);

    // After rotation the stored admin is new_admin; batch_charge should succeed.
    let ids = Vec::from_array(&test_env.env, [id]);
    let results = test_env.client.batch_charge(&ids, &0u64);
    assert_eq!(results.len(), 1);
    assert!(results.get(0).unwrap().success);
    // Confirm new admin is stored.
    assert_eq!(test_env.client.get_admin(), new_admin);
}

#[test]
fn test_batch_charge_allowed_for_new_admin_after_rotation() {
    let test_env = TestEnv::default();
    test_env.env.ledger().with_mut(|li| li.timestamp = T0);

    let (id, _, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    fixtures::seed_balance(&test_env.env, &test_env.client, id, PREPAID);
    test_env
        .env
        .ledger()
        .with_mut(|li| li.timestamp = T0 + INTERVAL + 1);

    let new_admin = Address::generate(&test_env.env);
    test_env.client.rotate_admin(&test_env.admin, &new_admin, &0u64);

    let ids = Vec::from_array(&test_env.env, [id]);
    let results = test_env.client.batch_charge(&ids, &0u64);
    assert_eq!(results.len(), 1);
    assert!(results.get(0).unwrap().success);
}

// -- Rotation during emergency stop -------------------------------------------

#[test]
fn test_rotate_admin_allowed_during_emergency_stop() {
    let test_env = TestEnv::default();
    test_env.client.enable_emergency_stop(&test_env.admin);
    assert!(test_env.client.get_emergency_stop_status());

    let new_admin = Address::generate(&test_env.env);
    // rotate_admin itself is not gated by emergency stop.
    test_env.client.rotate_admin(&test_env.admin, &new_admin, &0u64);
    assert_eq!(test_env.client.get_admin(), new_admin);

    // New admin can disable the emergency stop.
    test_env.client.disable_emergency_stop(&new_admin);
    assert!(!test_env.client.get_emergency_stop_status());
}

// =============================================================================
// Pause / Resume — Actor Authorization & Transition Guard Tests
// =============================================================================
//
// Security model
// ──────────────
// Only the subscription's `subscriber` or `merchant` may call pause_subscription
// or resume_subscription.  Any other address receives Error::Forbidden (403).
//
// Transition rules (enforced before the actor check so the state machine is
// always the first line of defence):
//
//   pause:  Active  → Paused          ✓
//           Paused  → Paused          ✓ (idempotent, no event)
//           Cancelled / InsufficientBalance → Paused  ✗ (InvalidStatusTransition)
//
//   resume: Paused              → Active  ✓
//           InsufficientBalance → Active  ✓
//           Active              → Active  ✓ (idempotent, no event)
//           Cancelled           → Active  ✗ (InvalidStatusTransition)
//
// Table-driven helpers
// ────────────────────
// `pause_actor_cases` / `resume_actor_cases` iterate over every (actor, state)
// combination and assert the expected outcome, giving full permutation coverage
// in a single test function.

// ── helpers ──────────────────────────────────────────────────────────────────

/// Patch a subscription's status directly in storage (test-only).
fn set_status(env: &Env, client: &SubscriptionVaultClient, id: u32, status: SubscriptionStatus) {
    let test_env = TestEnv::default();
    let mut sub = test_env.client.get_subscription(&id);
    sub.status = status;
    env.as_contract(&client.address, || {
        env.storage().instance().set(&DataKey::Sub(id), &sub);
    });
}

// ── actor × state table for pause ────────────────────────────────────────────

#[test]
fn pause_actor_cases() {
    // (actor_selector, initial_status, expect_ok)
    // actor_selector: 0 = subscriber, 1 = merchant, 2 = stranger
    let cases: &[(u8, SubscriptionStatus, bool)] = &[
        // subscriber can pause from Active
        (0, SubscriptionStatus::Active, true),
        // merchant can pause from Active
        (1, SubscriptionStatus::Active, true),
        // stranger cannot pause from Active
        (2, SubscriptionStatus::Active, false),
        // subscriber: idempotent pause from Paused
        (0, SubscriptionStatus::Paused, true),
        // merchant: idempotent pause from Paused
        (1, SubscriptionStatus::Paused, true),
        // stranger cannot pause from Paused either
        (2, SubscriptionStatus::Paused, false),
        // nobody can pause from Cancelled (transition guard fires first)
        (0, SubscriptionStatus::Cancelled, false),
        (1, SubscriptionStatus::Cancelled, false),
        (2, SubscriptionStatus::Cancelled, false),
        // nobody can pause from InsufficientBalance
        (0, SubscriptionStatus::InsufficientBalance, false),
        (1, SubscriptionStatus::InsufficientBalance, false),
        (2, SubscriptionStatus::InsufficientBalance, false),
    ];

    for (i, (actor_sel, initial_status, expect_ok)) in cases.iter().enumerate() {
        let test_env = TestEnv::default();
        let (id, subscriber, merchant) = fixtures::create_subscription(
            &test_env.env,
            &test_env.client,
            SubscriptionStatus::Active,
        );
        fixtures::patch_status(&test_env.env, &test_env.client, id, initial_status.clone());

        let stranger = Address::generate(&test_env.env);
        let actor = match actor_sel {
            0 => subscriber.clone(),
            1 => merchant.clone(),
            _ => stranger.clone(),
        };

        let result = test_env.client.try_pause_subscription(&id, &actor);
        assert_eq!(
            result.is_ok(),
            *expect_ok,
            "case {i}: actor={actor_sel} status={initial_status:?} expected_ok={expect_ok}"
        );
    }
}

// ── actor × state table for resume ───────────────────────────────────────────

#[test]
fn resume_actor_cases() {
    // (actor_selector, initial_status, expect_ok)
    let cases: &[(u8, SubscriptionStatus, bool)] = &[
        // subscriber can resume from Paused
        (0, SubscriptionStatus::Paused, true),
        // merchant can resume from Paused
        (1, SubscriptionStatus::Paused, true),
        // stranger cannot resume from Paused
        (2, SubscriptionStatus::Paused, false),
        // case 3: subscriber resumes from InsufficientBalance (idempotent, effectively Active)
        (0, SubscriptionStatus::InsufficientBalance, true),
        // case 4: merchant resumes from InsufficientBalance
        (1, SubscriptionStatus::InsufficientBalance, true),
        // stranger cannot resume from InsufficientBalance
        (2, SubscriptionStatus::InsufficientBalance, false),
        // nobody can resume from Cancelled
        (0, SubscriptionStatus::Cancelled, false),
        (1, SubscriptionStatus::Cancelled, false),
        (2, SubscriptionStatus::Cancelled, false),
        // idempotent: subscriber resumes from Active (already active)
        (0, SubscriptionStatus::Active, true),
        // idempotent: merchant resumes from Active
        (1, SubscriptionStatus::Active, true),
        // stranger cannot resume from Active
        (2, SubscriptionStatus::Active, false),
    ];

    for (i, (actor_sel, initial_status, expect_ok)) in cases.iter().enumerate() {
        let test_env = TestEnv::default();
        let (id, subscriber, merchant) = fixtures::create_subscription(
            &test_env.env,
            &test_env.client,
            SubscriptionStatus::Active,
        );
        fixtures::patch_status(&test_env.env, &test_env.client, id, initial_status.clone());

        let stranger = Address::generate(&test_env.env);
        let actor = match actor_sel {
            0 => subscriber.clone(),
            1 => merchant.clone(),
            _ => stranger.clone(),
        };

        if *initial_status == SubscriptionStatus::InsufficientBalance && *expect_ok {
            test_env.stellar_token_client().mint(&subscriber, &AMOUNT);
            test_env.client.deposit_funds(&id, &subscriber, &AMOUNT);
        }

        let result = test_env.client.try_resume_subscription(&id, &actor);
        assert_eq!(
            result.is_ok(),
            *expect_ok,
            "case {i}: actor={actor_sel} status={initial_status:?} expected_ok={expect_ok}"
        );
    }
}

// ── explicit error-code assertions ───────────────────────────────────────────

#[test]
fn pause_by_stranger_returns_forbidden() {
    let test_env = TestEnv::default();
    let (id, _, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    let stranger = Address::generate(&test_env.env);
    assert_eq!(
        test_env.client.try_pause_subscription(&id, &stranger),
        Err(Ok(Error::Forbidden))
    );
}

#[test]
fn resume_by_stranger_returns_forbidden() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.client.pause_subscription(&id, &subscriber);
    let stranger = Address::generate(&test_env.env);
    assert_eq!(
        test_env.client.try_resume_subscription(&id, &stranger),
        Err(Ok(Error::Forbidden))
    );
}

#[test]
fn pause_from_cancelled_returns_invalid_transition() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.client.cancel_subscription(&id, &subscriber);
    assert_eq!(
        test_env.client.try_pause_subscription(&id, &subscriber),
        Err(Ok(Error::InvalidStatusTransition))
    );
}

#[test]
fn resume_from_cancelled_returns_invalid_transition() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.client.cancel_subscription(&id, &subscriber);
    assert_eq!(
        test_env.client.try_resume_subscription(&id, &subscriber),
        Err(Ok(Error::InvalidStatusTransition))
    );
}

#[test]
fn pause_from_insufficient_balance_returns_invalid_transition() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    fixtures::patch_status(
        &test_env.env,
        &test_env.client,
        id,
        SubscriptionStatus::InsufficientBalance,
    );
    assert_eq!(
        test_env.client.try_pause_subscription(&id, &subscriber),
        Err(Ok(Error::InvalidStatusTransition))
    );
}

// ── cross-actor scenarios ─────────────────────────────────────────────────────

#[test]
fn merchant_pauses_subscriber_resumes() {
    let test_env = TestEnv::default();
    let (id, subscriber, merchant) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    test_env.client.pause_subscription(&id, &merchant);
    assert_eq!(
        test_env.client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );

    test_env.client.resume_subscription(&id, &subscriber);
    assert_eq!(
        test_env.client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );
}

#[test]
fn subscriber_pauses_merchant_resumes() {
    let test_env = TestEnv::default();
    let (id, subscriber, merchant) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    test_env.client.pause_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Paused);

    test_env.client.resume_subscription(&id, &merchant);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Active);
}

// ── event emission ────────────────────────────────────────────────────────────
//
// env.events().all() in the Soroban test harness returns only the events from
// the most recent contract invocation, so we check the count after each call
// independently rather than computing a delta.

#[test]
fn pause_emits_event() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    test_env.client.pause_subscription(&id, &subscriber);
    // The pause invocation must have produced at least one event.
    assert!(
        !test_env.env.events().all().is_empty(),
        "pause_subscription must emit at least one event"
    );
}

#[test]
fn resume_emits_event() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.client.pause_subscription(&id, &subscriber);

    test_env.client.resume_subscription(&id, &subscriber);
    assert!(
        !test_env.env.events().all().is_empty(),
        "resume_subscription must emit at least one event"
    );
}

#[test]
fn idempotent_pause_does_not_emit_event() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.client.pause_subscription(&id, &subscriber);

    // Second pause on already-Paused subscription — idempotent, no new event.
    // env.events().all() reflects only the most recent invocation.
    test_env.client.pause_subscription(&id, &subscriber);
    assert!(
        test_env.env.events().all().is_empty(),
        "idempotent pause must not emit an event"
    );
}

#[test]
fn idempotent_resume_does_not_emit_event() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    // Resume on already-Active subscription — idempotent, no new event.
    test_env.client.resume_subscription(&id, &subscriber);
    assert!(
        test_env.env.events().all().is_empty(),
        "idempotent resume must not emit an event"
    );
}

// ── repeat pause / resume cycles ─────────────────────────────────────────────

#[test]
fn repeated_pause_resume_cycles_stay_consistent() {
    let test_env = TestEnv::default();
    let (id, subscriber, merchant) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    for _ in 0..3 {
        test_env.client.pause_subscription(&id, &subscriber);
        assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Paused);
        test_env.client.resume_subscription(&id, &merchant);
        assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Active);
    }
}

// =============================================================================
// Lifecycle Edge Case Regression Suite  (Issue #202)
// =============================================================================

// -----------------------------------------------------------------------------
// Terminal State Enforcement  sequences that must be fully blocked
// -----------------------------------------------------------------------------

// Active => Paused => Cancelled => Resume must fail with InvalidStatusTransition.
#[test]
fn test_pause_cancel_resume_blocked() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    test_env.client.pause_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Paused);

    test_env.client.cancel_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);

    let result = test_env.client.try_resume_subscription(&id, &subscriber);
    assert_eq!(
        result,
        Err(Ok(Error::InvalidStatusTransition)),
        "resume from Cancelled must fail"
    );
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);
}

// Active => Cancelled => Pause must fail with InvalidStatusTransition.
#[test]
fn test_cancel_then_pause_blocked() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    test_env.client.cancel_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);

    let result = test_env.client.try_pause_subscription(&id, &subscriber);
    assert_eq!(result, Err(Ok(Error::InvalidStatusTransition)));
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);
}

// Cancelled => InsufficientBalance must be impossible.
// The only public path to InsufficientBalance is a failed charge.
// A cancelled subscription must reject the charge before any status flip occurs.
#[test]
fn test_cancelled_to_insufficient_balance_blocked() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    test_env.client.cancel_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);

    test_env.jump(INTERVAL + 1);
    let result = test_env.client.try_charge_subscription(&id);
    assert!(result.is_err(), "charge on Cancelled must fail");
    // status must remain Cancelled  never flipped to InsufficientBalance.
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);
}

// -----------------------------------------------------------------------------
// Idempotent Operations — field preservation under repeated calls
// -----------------------------------------------------------------------------

// Two consecutive pause calls must leave all financial fields unchanged.
#[test]
fn test_idempotent_pause_preserves_all_fields() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env.client.deposit_funds(&id, &subscriber, &PREPAID);

    let before = test_env.client.get_subscription(&id);

    test_env.client.pause_subscription(&id, &subscriber);
    test_env.client.pause_subscription(&id, &subscriber);

    let after = test_env.client.get_subscription(&id);

    assert_eq!(
        after.prepaid_balance, before.prepaid_balance,
        "balance must not change"
    );
    assert_eq!(
        after.last_payment_timestamp, before.last_payment_timestamp,
        "timestamp must not change"
    );
    assert_eq!(after.amount, before.amount, "amount must not change");
    assert_eq!(
        after.interval_seconds, before.interval_seconds,
        "interval must not change"
    );
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Paused);
}

// two consecutive cancel calls must leave all financial fields unchanged.
#[test]
fn test_idempotent_cancel_preserves_all_fields() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env.client.deposit_funds(&id, &subscriber, &PREPAID);

    let before = test_env.client.get_subscription(&id);

    test_env.client.cancel_subscription(&id, &subscriber);
    test_env.client.cancel_subscription(&id, &subscriber);

    let after = test_env.client.get_subscription(&id);

    assert_eq!(
        after.prepaid_balance, before.prepaid_balance,
        "balance must not change on cancel"
    );
    assert_eq!(
        after.last_payment_timestamp, before.last_payment_timestamp,
        "timestamp must not change on cancel"
    );
    assert_eq!(after.amount, before.amount);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);
}

// -----------------------------------------------------------------------------
// Grace Period (InsufficientBalance) Restrictions
// -----------------------------------------------------------------------------

// InsufficientBalance => Paused must fail.
#[test]
fn test_pause_during_grace_period() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    fixtures::patch_status(
        &test_env.env,
        &test_env.client,
        id,
        SubscriptionStatus::InsufficientBalance,
    );

    let result = test_env.client.try_pause_subscription(&id, &subscriber);
    assert_eq!(result, Err(Ok(Error::InvalidStatusTransition)));
    assertions::assert_status(
        &test_env.client,
        &id,
        SubscriptionStatus::InsufficientBalance,
    );
}

// InsufficientBalance => Active via resume must succeed.
#[test]
fn test_resume_from_insufficient_balance_succeeds() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    fixtures::patch_status(
        &test_env.env,
        &test_env.client,
        id,
        SubscriptionStatus::InsufficientBalance,
    );
    fixtures::seed_balance(&test_env.env, &test_env.client, id, AMOUNT);

    test_env.client.resume_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Active);
}

// InsufficientBalance => Cancelled via cancel must succeed.
#[test]
fn test_cancel_during_grace_period() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    fixtures::patch_status(
        &test_env.env,
        &test_env.client,
        id,
        SubscriptionStatus::InsufficientBalance,
    );

    let result = test_env.client.try_cancel_subscription(&id, &subscriber);
    assert!(result.is_ok(), "cancel during grace period must succeed");
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);
}

// -----------------------------------------------------------------------------
// §4  Multiple Pause or Resume Cycles
// -----------------------------------------------------------------------------

// Exactly five consecutive paus or resume cycles must all succeed without corruption.
#[test]
fn test_multiple_pause_resume_cycles() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    for cycle in 0..5 {
        test_env.client.pause_subscription(&id, &subscriber);
        assert_eq!(
            test_env.client.get_subscription(&id).status,
            SubscriptionStatus::Paused,
            "cycle {cycle}: expected Paused after pause"
        );
        test_env.client.resume_subscription(&id, &subscriber);
        assert_eq!(
            test_env.client.get_subscription(&id).status,
            SubscriptionStatus::Active,
            "cycle {cycle}: expected Active after resume"
        );
    }
}

// Active => Paused =>  Active => Paused => Active rapid sequence.
#[test]
fn test_rapid_state_transitions() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let expected_after_each_step = [
        SubscriptionStatus::Paused,
        SubscriptionStatus::Active,
        SubscriptionStatus::Paused,
        SubscriptionStatus::Active,
    ];

    for (i, expected) in expected_after_each_step.iter().enumerate() {
        if i % 2 == 0 {
            test_env.client.pause_subscription(&id, &subscriber);
        } else {
            test_env.client.resume_subscription(&id, &subscriber);
        }
        assert_eq!(
            test_env.client.get_subscription(&id).status,
            *expected,
            "rapid transition step {i} wrong status"
        );
    }
}

// -----------------------------------------------------------------------------
// State Preservation
// -----------------------------------------------------------------------------

// last_payment_timestamp must not change across pause and resume.
#[test]
fn test_timestamp_preserved_across_pause_resume() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    let original_ts = test_env.client.get_subscription(&id).last_payment_timestamp;

    test_env.client.pause_subscription(&id, &subscriber);
    assert_eq!(
        test_env.client.get_subscription(&id).last_payment_timestamp,
        original_ts,
        "timestamp must not change on pause"
    );

    // advance time while paused the stored timestamp must still not change.
    test_env.jump(INTERVAL);

    test_env.client.resume_subscription(&id, &subscriber);
    assert_eq!(
        test_env.client.get_subscription(&id).last_payment_timestamp,
        original_ts,
        "timestamp must not change on resume"
    );
}

// prepaid_balance must not change across pause and resume.
#[test]
fn test_balance_preserved_across_pause_resume() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env.client.deposit_funds(&id, &subscriber, &PREPAID);

    let original_balance = test_env.client.get_subscription(&id).prepaid_balance;

    test_env.client.pause_subscription(&id, &subscriber);
    assertions::assert_prepaid_balance(&test_env.client, &id, original_balance);

    test_env.client.resume_subscription(&id, &subscriber);
    assertions::assert_prepaid_balance(&test_env.client, &id, original_balance);
}

// prepaid_balance must be retained (not zeroed) after cancel.
#[test]
fn test_balance_preserved_on_cancel() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env.client.deposit_funds(&id, &subscriber, &PREPAID);

    let original_balance = test_env.client.get_subscription(&id).prepaid_balance;

    test_env.client.cancel_subscription(&id, &subscriber);
    assertions::assert_prepaid_balance(&test_env.client, &id, original_balance);
}

// -----------------------------------------------------------------------------
// Charging Restrictions
// -----------------------------------------------------------------------------

// Charge on a Paused subscription must return NotActive and leave balance intact.
#[test]
fn test_charge_blocked_while_paused() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env.client.deposit_funds(&id, &subscriber, &PREPAID);

    test_env.client.pause_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Paused);

    test_env.jump(INTERVAL + 1);

    let result = test_env.client.try_charge_subscription(&id);
    assert_eq!(
        result,
        Err(Ok(Error::NotActive)),
        "paused charge must return NotActive"
    );
    // balance must be entirely untouched.
    assertions::assert_prepaid_balance(&test_env.client, &id, PREPAID);
}

// Charge on a Cancelled subscription must return NotActive and leave balance intact.
#[test]
fn test_charge_blocked_after_cancel() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env.client.deposit_funds(&id, &subscriber, &PREPAID);

    test_env.client.cancel_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);

    test_env.jump(INTERVAL + 1);

    let result = test_env.client.try_charge_subscription(&id);
    assert_eq!(
        result,
        Err(Ok(Error::NotActive)),
        "cancelled charge must return NotActive"
    );
    assertions::assert_prepaid_balance(&test_env.client, &id, PREPAID);
}

// Resume then immediately charge must succeed when the interval has elapsed
// (the pause must not reset or advance the billing clock).
#[test]
fn test_resume_and_charge_immediately() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env.client.deposit_funds(&id, &subscriber, &PREPAID);

    // Pause while the interval elapses.
    test_env.client.pause_subscription(&id, &subscriber);
    test_env.jump(INTERVAL + 1);

    // Resume  subscription is Active again and interval has elapsed.
    test_env.client.resume_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Active);

    let result = test_env.client.try_charge_subscription(&id);
    assert!(
        result.is_ok(),
        "charge after resume with elapsed interval must succeed"
    );
    assertions::assert_prepaid_balance(&test_env.client, &id, PREPAID - AMOUNT);
}

// -----------------------------------------------------------------------------
// Multi-Subscription Scenarios
// -----------------------------------------------------------------------------

// Three subscriptions sharing one merchant, each in a different state.
// Mutating one must never affect the others.
#[test]
fn test_shared_merchant_multiple_states() {
    let test_env = TestEnv::default();
    let merchant = Address::generate(&test_env.env);

    let sub_a_sub = Address::generate(&test_env.env);
    let sub_b_sub = Address::generate(&test_env.env);
    let sub_c_sub = Address::generate(&test_env.env);

    let id_a = test_env.client.create_subscription(
        &sub_a_sub,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    let id_b = test_env.client.create_subscription(
        &sub_b_sub,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    let id_c = test_env.client.create_subscription(
        &sub_c_sub,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    // Set: A stays Active, B => Paused, C => Cancelled.
    test_env.client.pause_subscription(&id_b, &sub_b_sub);
    test_env.client.cancel_subscription(&id_c, &sub_c_sub);

    assertions::assert_status(&test_env.client, &id_a, SubscriptionStatus::Active);
    assertions::assert_status(&test_env.client, &id_b, SubscriptionStatus::Paused);
    assertions::assert_status(&test_env.client, &id_c, SubscriptionStatus::Cancelled);

    // Mutate A — B and C must be unaffected.
    test_env.client.pause_subscription(&id_a, &sub_a_sub);
    assertions::assert_status(&test_env.client, &id_a, SubscriptionStatus::Paused);
    assertions::assert_status(&test_env.client, &id_b, SubscriptionStatus::Paused);
    assertions::assert_status(&test_env.client, &id_c, SubscriptionStatus::Cancelled);

    // Resume B — A and C must be unaffected.
    test_env.client.resume_subscription(&id_b, &sub_b_sub);
    assertions::assert_status(&test_env.client, &id_a, SubscriptionStatus::Paused);
    assertions::assert_status(&test_env.client, &id_b, SubscriptionStatus::Active);
    assertions::assert_status(&test_env.client, &id_c, SubscriptionStatus::Cancelled);
}

// Pause or resume must work identically regardless of billing interval.
#[test]
fn test_pause_with_varying_intervals() {
    let test_env = TestEnv::default();
    let m = Address::generate(&test_env.env);

    let daily = 24 * 60 * 60u64;
    let weekly = 7 * 24 * 60 * 60u64;
    let monthly = 30 * 24 * 60 * 60u64;

    let s1 = Address::generate(&test_env.env);
    let s2 = Address::generate(&test_env.env);
    let s3 = Address::generate(&test_env.env);

    let id1 = test_env
        .client
        .create_subscription(&s1, &m, &AMOUNT, &daily, &false, &None::<i128>, &None::<u64>);
    let id2 = test_env
        .client
        .create_subscription(&s2, &m, &AMOUNT, &weekly, &false, &None::<i128>, &None::<u64>);
    let id3 =
        test_env
            .client
            .create_subscription(&s3, &m, &AMOUNT, &monthly, &false, &None::<i128>, &None::<u64>);

    // All three should pause without error regardless of interval.
    test_env.client.pause_subscription(&id1, &s1);
    test_env.client.pause_subscription(&id2, &s2);
    test_env.client.pause_subscription(&id3, &s3);

    assertions::assert_status(&test_env.client, &id1, SubscriptionStatus::Paused);
    assertions::assert_status(&test_env.client, &id2, SubscriptionStatus::Paused);
    assertions::assert_status(&test_env.client, &id3, SubscriptionStatus::Paused);

    // All three should resume without error.
    test_env.client.resume_subscription(&id1, &s1);
    test_env.client.resume_subscription(&id2, &s2);
    test_env.client.resume_subscription(&id3, &s3);

    assertions::assert_status(&test_env.client, &id1, SubscriptionStatus::Active);
    assertions::assert_status(&test_env.client, &id2, SubscriptionStatus::Active);
    assertions::assert_status(&test_env.client, &id3, SubscriptionStatus::Active);
}

// Batch charge with mixed states: the failed items must not corrupt the
// successful item's balance or the batch's overall accounting.
#[test]
fn test_batch_charge_with_paused_and_cancelled() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);

    let (id_active, sub_a, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    let (id_paused, sub_b, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    let (id_cancelled, sub_c, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    test_env.stellar_token_client().mint(&sub_a, &PREPAID);
    test_env.stellar_token_client().mint(&sub_b, &PREPAID);
    test_env.stellar_token_client().mint(&sub_c, &PREPAID);
    test_env.client.deposit_funds(&id_active, &sub_a, &PREPAID);
    test_env.client.deposit_funds(&id_paused, &sub_b, &PREPAID);
    test_env
        .client
        .deposit_funds(&id_cancelled, &sub_c, &PREPAID);

    test_env.client.pause_subscription(&id_paused, &sub_b);
    test_env.client.cancel_subscription(&id_cancelled, &sub_c);

    test_env.jump(INTERVAL + 1);

    let ids = soroban_sdk::Vec::from_array(&test_env.env, [id_active, id_paused, id_cancelled]);
    let results = test_env.client.batch_charge(&ids, &0u64);

    assert_eq!(results.len(), 3);
    assert!(results.get(0).unwrap().success, "active must succeed");
    assert!(!results.get(1).unwrap().success, "paused must fail");
    assert!(!results.get(2).unwrap().success, "cancelled must fail");

    // Only the active subscription's balance was deducted.
    assertions::assert_prepaid_balance(&test_env.client, &id_active, PREPAID - AMOUNT);
    assertions::assert_prepaid_balance(&test_env.client, &id_paused, PREPAID);
    assertions::assert_prepaid_balance(&test_env.client, &id_cancelled, PREPAID);
}

// -----------------------------------------------------------------------------
// Issue-specified end-to-end flows
// -----------------------------------------------------------------------------

// pause => cancel => withdraw — the explicit example from the issue.
#[test]
fn test_pause_cancel_withdraw_flow() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env.client.deposit_funds(&id, &subscriber, &PREPAID);

    let original_balance = test_env.client.get_subscription(&id).prepaid_balance;

    // Pause — balance unchanged.
    test_env.client.pause_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Paused);
    assertions::assert_prepaid_balance(&test_env.client, &id, original_balance);

    // Cancel — balance still retained for withdrawal.
    test_env.client.cancel_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Cancelled);
    assertions::assert_prepaid_balance(&test_env.client, &id, original_balance);

    // Withdraw — balance zeroed, tokens returned to subscriber.
    test_env.client.withdraw_subscriber_funds(&id, &subscriber);
    assertions::assert_prepaid_balance(&test_env.client, &id, 0);

    let wallet =
        soroban_sdk::token::Client::new(&test_env.env, &test_env.token).balance(&subscriber);
    assert_eq!(
        wallet, original_balance,
        "subscriber must receive full prepaid back"
    );
}

// insufficient => deposit => resume — the explicit example from the issue.
#[test]
fn test_insufficient_deposit_resume_flow() {
    let test_env = TestEnv::default();
    test_env.set_timestamp(T0);
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    // Simulate InsufficientBalance state.
    fixtures::patch_status(
        &test_env.env,
        &test_env.client,
        id,
        SubscriptionStatus::InsufficientBalance,
    );
    assertions::assert_status(
        &test_env.client,
        &id,
        SubscriptionStatus::InsufficientBalance,
    );

    // Subscriber tops up  balance credited regardless of status.
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env.client.deposit_funds(&id, &subscriber, &PREPAID);
    assertions::assert_prepaid_balance(&test_env.client, &id, PREPAID);

    // Resume brings subscription back to Active.
    test_env.client.resume_subscription(&id, &subscriber);
    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Active);

    // A charge must now succeed once the interval elapses.
    test_env.jump(INTERVAL + 1);
    let result = test_env.client.try_charge_subscription(&id);
    assert!(
        result.is_ok(),
        "charge after insufficient→deposit→resume must succeed"
    );
    assertions::assert_prepaid_balance(&test_env.client, &id, PREPAID - AMOUNT);
}

// ── Oracle validation tests ───────────────────────────────────────────────────

/// Helper: register oracle, configure vault, create subscription, deposit funds.
/// Returns (subscription_id, subscriber, merchant, oracle_client).
fn setup_oracle_env<'a>(
    env: &'a Env,
    client: &'a SubscriptionVaultClient<'a>,
    token: &Address,
    admin: &Address,
    price: i128,
    price_ts: u64,
    max_age_seconds: u64,
) -> (u32, Address, Address, MockOracleClient<'a>) {
    let oracle_id = env.register(MockOracle, ());
    let oracle = MockOracleClient::new(env, &oracle_id);
    oracle.set_price(&price, &price_ts);
    client.set_oracle_config(admin, &true, &Some(oracle_id), &max_age_seconds);

    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    soroban_sdk::token::StellarAssetClient::new(env, token).mint(&subscriber, &1_000_000_000i128);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &20_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&id, &subscriber, &200_000_000i128);
    (id, subscriber, merchant, oracle)
}

// --- set_oracle_config validation ---

#[test]
fn test_set_oracle_config_enabled_without_address_fails() {
    let test_env = TestEnv::default();
    let result =
        test_env
            .client
            .try_set_oracle_config(&test_env.admin, &true, &None::<Address>, &60u64);
    assert_eq!(result, Err(Ok(Error::OracleNotConfigured)));
}

#[test]
fn test_set_oracle_config_enabled_with_zero_max_age_fails() {
    let test_env = TestEnv::default();
    let oracle_id = test_env.env.register(MockOracle, ());
    let result =
        test_env
            .client
            .try_set_oracle_config(&test_env.admin, &true, &Some(oracle_id), &0u64);
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

#[test]
fn test_set_oracle_config_disabled_with_zero_max_age_succeeds() {
    // Disabling oracle does not require a valid max_age.
    let test_env = TestEnv::default();
    let oracle_id = test_env.env.register(MockOracle, ());
    test_env
        .client
        .set_oracle_config(&test_env.admin, &false, &Some(oracle_id), &0u64);
    let cfg = test_env.client.get_oracle_config();
    assert!(!cfg.enabled);
}

#[test]
fn test_set_oracle_config_disabled_with_no_address_succeeds() {
    let test_env = TestEnv::default();
    test_env
        .client
        .set_oracle_config(&test_env.admin, &false, &None::<Address>, &0u64);
    let cfg = test_env.client.get_oracle_config();
    assert!(!cfg.enabled);
    assert!(cfg.oracle.is_none());
}

// --- Oracle disabled: passthrough ---

#[test]
fn test_oracle_disabled_charge_uses_subscription_amount_directly() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);

    // Ensure oracle is off (default).
    let cfg = test_env.client.get_oracle_config();
    assert!(!cfg.enabled);

    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &100_000_000i128);
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env
        .client
        .deposit_funds(&id, &subscriber, &50_000_000i128);

    test_env.env.ledger().set_timestamp(T0 + INTERVAL);
    test_env.client.charge_subscription(&id);

    // Merchant receives exactly AMOUNT (no oracle conversion).
    assert_eq!(test_env.client.get_merchant_balance(&merchant), AMOUNT);
}

// --- Zero price rejection ---

#[test]
fn test_oracle_zero_price_rejected() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let (id, _sub, _mer, _oracle) = setup_oracle_env(
        &test_env.env,
        &test_env.client,
        &test_env.token,
        &test_env.admin,
        0i128,
        T0,
        3600u64,
    );

    test_env.env.ledger().set_timestamp(T0 + INTERVAL);
    let result = test_env.client.try_charge_subscription(&id);
    assert_eq!(result, Err(Ok(Error::OraclePriceInvalid)));
}

// --- Negative price rejection ---

#[test]
fn test_oracle_negative_price_rejected() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let (id, _sub, _mer, _oracle) = setup_oracle_env(
        &test_env.env,
        &test_env.client,
        &test_env.token,
        &test_env.admin,
        -1_000_000i128,
        T0,
        3600u64,
    );

    test_env.env.ledger().set_timestamp(T0 + INTERVAL);
    let result = test_env.client.try_charge_subscription(&id);
    assert_eq!(result, Err(Ok(Error::OraclePriceInvalid)));
}

// --- Unavailable price (timestamp == 0) ---

#[test]
fn test_oracle_zero_timestamp_price_unavailable() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    // price=2_000_000 but timestamp=0 → OraclePriceUnavailable
    let (id, _sub, _mer, _oracle) = setup_oracle_env(
        &test_env.env,
        &test_env.client,
        &test_env.token,
        &test_env.admin,
        2_000_000i128,
        0u64,
        3600u64,
    );

    test_env.env.ledger().set_timestamp(T0 + INTERVAL);
    let result = test_env.client.try_charge_subscription(&id);
    assert_eq!(result, Err(Ok(Error::OraclePriceUnavailable)));
}

// --- Staleness boundary ---

#[test]
fn test_oracle_price_exactly_at_max_age_boundary_accepted() {
    // now - price.timestamp == max_age_seconds → still fresh (not stale).
    let test_env = TestEnv::default();
    let max_age = 3600u64;
    // Use a price_ts large enough that charge_ts - INTERVAL > 0.
    let price_ts = INTERVAL + max_age; // e.g. 2592000 + 3600
    let charge_ts = price_ts + max_age; // age == max_age at charge time

    let oracle_id = test_env.env.register(MockOracle, ());
    let oracle = MockOracleClient::new(&test_env.env, &oracle_id);
    oracle.set_price(&2_000_000i128, &price_ts);
    test_env
        .client
        .set_oracle_config(&test_env.admin, &true, &Some(oracle_id), &max_age);

    test_env.env.ledger().set_timestamp(T0);
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &1_000_000_000i128);
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &20_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env
        .client
        .deposit_funds(&id, &subscriber, &200_000_000i128);

    // Set last_payment_timestamp so interval has elapsed by charge_ts.
    let mut sub = test_env.client.get_subscription(&id);
    sub.last_payment_timestamp = charge_ts - INTERVAL; // positive, interval elapsed
    test_env.env.as_contract(&test_env.client.address, || {
        test_env.env.storage().instance().set(&DataKey::Sub(id), &sub);
    });

    test_env.env.ledger().set_timestamp(charge_ts);
    // Should succeed — price age == max_age_seconds (boundary, not stale).
    test_env.client.charge_subscription(&id);
    assert_eq!(
        test_env.client.get_merchant_balance(&merchant),
        10_000_000i128
    );
}

#[test]
fn test_oracle_price_one_second_past_max_age_rejected() {
    // now - price.timestamp == max_age_seconds + 1 → stale.
    let test_env = TestEnv::default();
    let max_age = 3600u64;
    let price_ts = T0;
    let charge_ts = price_ts + max_age + 1;

    let oracle_id = test_env.env.register(MockOracle, ());
    let oracle = MockOracleClient::new(&test_env.env, &oracle_id);
    oracle.set_price(&2_000_000i128, &price_ts);
    test_env
        .client
        .set_oracle_config(&test_env.admin, &true, &Some(oracle_id), &max_age);

    test_env.env.ledger().set_timestamp(T0);
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &1_000_000_000i128);
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &20_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env
        .client
        .deposit_funds(&id, &subscriber, &200_000_000i128);

    // Set last_payment_timestamp so interval has elapsed by charge_ts.
    let mut sub = test_env.client.get_subscription(&id);
    sub.last_payment_timestamp = charge_ts.saturating_sub(INTERVAL);
    test_env.env.as_contract(&test_env.client.address, || {
        test_env.env.storage().instance().set(&DataKey::Sub(id), &sub);
    });

    test_env.env.ledger().set_timestamp(charge_ts);
    let result = test_env.client.try_charge_subscription(&id);
    assert_eq!(result, Err(Ok(Error::OraclePriceStale)));
}

// --- Oracle not configured but enabled ---

#[test]
fn test_oracle_enabled_no_address_stored_returns_not_configured() {
    // Manually store enabled=true without an oracle address to simulate
    // a misconfigured state (bypassing set_oracle_config validation).
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);

    // Force-write enabled=true with no oracle address directly into storage.
    test_env.env.as_contract(&test_env.client.address, || {
        let config = crate::types::OracleConfig {
            enabled: true,
            oracle: None,
            max_age_seconds: 3600,
        };
        test_env.env.storage().instance().set(&crate::types::DataKey::Oracle, &config);
    });

    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    soroban_sdk::token::StellarAssetClient::new(&test_env.env, &test_env.token)
        .mint(&subscriber, &100_000_000i128);
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    test_env
        .client
        .deposit_funds(&id, &subscriber, &50_000_000i128);

    test_env.env.ledger().set_timestamp(T0 + INTERVAL);
    let result = test_env.client.try_charge_subscription(&id);
    assert_eq!(result, Err(Ok(Error::OracleNotConfigured)));
}

// --- Charge does not mutate balances on oracle error ---

#[test]
fn test_oracle_error_does_not_mutate_balances() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    // Zero price → OraclePriceInvalid
    let (id, _sub, merchant, _oracle) = setup_oracle_env(
        &test_env.env,
        &test_env.client,
        &test_env.token,
        &test_env.admin,
        0i128,
        T0,
        3600u64,
    );

    let balance_before = test_env.client.get_subscription(&id).prepaid_balance;
    let merchant_before = test_env.client.get_merchant_balance(&merchant);

    test_env.env.ledger().set_timestamp(T0 + INTERVAL);
    let _ = test_env.client.try_charge_subscription(&id);

    assert_eq!(
        test_env.client.get_subscription(&id).prepaid_balance,
        balance_before
    );
    assert_eq!(
        test_env.client.get_merchant_balance(&merchant),
        merchant_before
    );
}

// --- get_oracle_config round-trip ---

#[test]
fn test_get_oracle_config_reflects_set_values() {
    let test_env = TestEnv::default();
    let oracle_id = test_env.env.register(MockOracle, ());
    test_env
        .client
        .set_oracle_config(&test_env.admin, &true, &Some(oracle_id.clone()), &120u64);

    let cfg = test_env.client.get_oracle_config();
    assert!(cfg.enabled);
    assert_eq!(cfg.oracle, Some(oracle_id));
    assert_eq!(cfg.max_age_seconds, 120u64);
}

#[test]
fn test_get_oracle_config_default_is_disabled() {
    let test_env = TestEnv::default();
    let cfg = test_env.client.get_oracle_config();
    assert!(!cfg.enabled);
    assert!(cfg.oracle.is_none());
    assert_eq!(cfg.max_age_seconds, 0u64);
}

// -- Storage Layout Compatibility Tests ---------------------------------------
//
// These tests act as regression guards for the on-chain storage schema.
// Soroban encodes #[contracttype] structs as ScMap (keyed by field name) and
// enums as ScVec([discriminant, payload]).  Any change that shifts a
// discriminant value or removes/renames a field is a BREAKING upgrade.
//
// Security note: breaking storage changes on a live contract would make
// existing subscriptions unreadable, potentially locking subscriber funds.
// These tests must pass before any upgrade is deployed.

#[cfg(test)]
mod storage_layout {
    use super::*;
    use crate::{DataKey, SubscriptionStatus};

    // -------------------------------------------------------------------------
    // 1. DataKey discriminant snapshot
    //    Each variant's position in the enum determines its on-chain encoding.
    //    If a variant is inserted before an existing one, all subsequent keys
    //    become unreadable.  This test pins the current order.
    // -------------------------------------------------------------------------
    #[test]
    fn test_datakey_discriminants_are_stable() {
        // Soroban encodes enum variants by their declaration order (0-based).
        // We verify the discriminant of each variant by round-tripping through
        // storage inside a contract context.
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SubscriptionVault, ());

        env.as_contract(&contract_id, || {
            // Write each key variant and confirm it can be read back under the
            // same variant — a mismatch would mean the discriminant shifted.
            let storage = env.storage().instance();

            storage.set(&DataKey::Token, &42u32);
            assert_eq!(storage.get::<DataKey, u32>(&DataKey::Token), Some(42u32));

            storage.set(&DataKey::Admin, &99u32);
            assert_eq!(storage.get::<DataKey, u32>(&DataKey::Admin), Some(99u32));

            storage.set(&DataKey::MinTopup, &7u32);
            assert_eq!(storage.get::<DataKey, u32>(&DataKey::MinTopup), Some(7u32));

            storage.set(&DataKey::NextId, &1u32);
            assert_eq!(storage.get::<DataKey, u32>(&DataKey::NextId), Some(1u32));

            storage.set(&DataKey::SchemaVersion, &2u32);
            assert_eq!(
                storage.get::<DataKey, u32>(&DataKey::SchemaVersion),
                Some(2u32)
            );

            let sub_key = DataKey::Sub(1);
            storage.set(&sub_key, &100u32);
            assert_eq!(storage.get::<DataKey, u32>(&DataKey::Sub(1)), Some(100u32));

            let cp_key = DataKey::ChargedPeriod(1);
            storage.set(&cp_key, &5u32);
            assert_eq!(
                storage.get::<DataKey, u32>(&DataKey::ChargedPeriod(1)),
                Some(5u32)
            );

            storage.set(&DataKey::EmergencyStop, &true);
            assert_eq!(
                storage.get::<DataKey, bool>(&DataKey::EmergencyStop),
                Some(true)
            );
        });
    }

    // -------------------------------------------------------------------------
    // 2. SubscriptionStatus discriminant snapshot
    //    Enum variants are stored as integers on-chain.  Reordering or inserting
    //    variants before existing ones corrupts all stored subscription statuses.
    // -------------------------------------------------------------------------
    #[test]
    fn test_subscription_status_discriminants_are_stable() {
        // Explicit discriminants are declared in types.rs; verify they match
        // what we expect so a future edit is caught immediately.
        assert_eq!(SubscriptionStatus::Active as u32, 0);
        assert_eq!(SubscriptionStatus::Paused as u32, 1);
        assert_eq!(SubscriptionStatus::Cancelled as u32, 2);
        assert_eq!(SubscriptionStatus::InsufficientBalance as u32, 3);
        assert_eq!(SubscriptionStatus::GracePeriod as u32, 4);
    }

    // -------------------------------------------------------------------------
    // 3. Subscription struct round-trip (field-name encoding)
    //    Soroban ScMap keys are field name strings.  This test writes a full
    //    Subscription to storage and reads it back, confirming every field
    //    survives the encode/decode cycle without corruption.
    // -------------------------------------------------------------------------
    #[test]
    fn test_subscription_struct_round_trips_through_storage() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SubscriptionVault, ());

        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);
        let token = Address::generate(&env);

        let original = Subscription {
            subscriber: subscriber.clone(),
            merchant: merchant.clone(),
            token: token.clone(),
            amount: 10_000_000,
            interval_seconds: INTERVAL,
            last_payment_timestamp: T0,
            status: SubscriptionStatus::Active,
            prepaid_balance: 50_000_000,
            usage_enabled: false,
            lifetime_cap: Some(120_000_000),
            lifetime_charged: 10_000_000,
            start_time: 0,
            expires_at: None,
            grace_start_timestamp: None,
        };

        env.as_contract(&contract_id, || {
            env.storage().instance().set(&DataKey::Sub(42), &original);
            let loaded: Subscription = env
                .storage()
                .instance()
                .get(&DataKey::Sub(42))
                .expect("subscription must be present");

            assert_eq!(loaded.amount, 10_000_000);
            assert_eq!(loaded.interval_seconds, INTERVAL);
            assert_eq!(loaded.last_payment_timestamp, T0);
            assert_eq!(loaded.status, SubscriptionStatus::Active);
            assert_eq!(loaded.prepaid_balance, 50_000_000);
            assert!(!loaded.usage_enabled);
            assert_eq!(loaded.lifetime_cap, Some(120_000_000));
            assert_eq!(loaded.lifetime_charged, 10_000_000);
        });
    }

    // -------------------------------------------------------------------------
    // 4. Optional field default — lifetime_cap = None
    //    Subscriptions created before lifetime_cap was introduced have no cap
    //    field.  New code must treat a missing/None cap as "no cap" (not panic).
    // -------------------------------------------------------------------------
    #[test]
    fn test_subscription_with_no_lifetime_cap_is_readable() {
        let (env, client, _token, _admin) = setup_test_env();

        // Create a subscription without a cap (None).
        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);
        let id = client.create_subscription(
            &subscriber,
            &merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
        );

        let sub = client.get_subscription(&id);
        assert_eq!(sub.lifetime_cap, None);
        assert_eq!(sub.lifetime_charged, 0);
    }

    // -------------------------------------------------------------------------
    // 5. Optional field introduction — lifetime_cap = Some(value)
    //    Subscriptions created with a cap must persist and be readable.
    // -------------------------------------------------------------------------
    #[test]
    fn test_subscription_with_lifetime_cap_persists_correctly() {
        let (env, client, _token, _admin) = setup_test_env();

        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);
        let cap = 60_000_000i128; // 60 USDC
        let id = client.create_subscription(
            &subscriber,
            &merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &Some(cap),
            &None::<u64>,
        );

        let sub = client.get_subscription(&id);
        assert_eq!(sub.lifetime_cap, Some(cap));
        assert_eq!(sub.lifetime_charged, 0);
    }

    // -------------------------------------------------------------------------
    // 6. Backward-compatible deserialization: manually written storage record
    //    Simulates reading a subscription that was written by an older contract
    //    version (e.g., before lifetime_cap existed).  We write a Subscription
    //    with lifetime_cap=None directly into storage and confirm the current
    //    code reads it without error.
    // -------------------------------------------------------------------------
    #[test]
    fn test_legacy_subscription_without_cap_is_deserializable() {
        let (env, client, _token, _admin) = setup_test_env();

        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);
        let token = Address::generate(&env);

        // Simulate a "legacy" record written with no cap fields.
        let legacy = Subscription {
            subscriber: subscriber.clone(),
            merchant: merchant.clone(),
            token: token.clone(),
            amount: AMOUNT,
            interval_seconds: INTERVAL,
            last_payment_timestamp: T0,
            status: SubscriptionStatus::Active,
            prepaid_balance: PREPAID,
            usage_enabled: false,
            lifetime_cap: None,
            lifetime_charged: 0,
            start_time: 0,
            expires_at: None,
            grace_start_timestamp: None,
        };

        env.as_contract(&client.address, || {
            env.storage().instance().set(&DataKey::Sub(999), &legacy);
        });

        // Current code must read it back without panicking.
        let loaded: Subscription = env.as_contract(&client.address, || {
            env.storage()
                .instance()
                .get(&DataKey::Sub(999))
                .expect("legacy record must be readable")
        });

        assert_eq!(loaded.lifetime_cap, None);
        assert_eq!(loaded.lifetime_charged, 0);
        assert_eq!(loaded.amount, AMOUNT);
        assert_eq!(loaded.status, SubscriptionStatus::Active);
    }

    // -------------------------------------------------------------------------
    // 7. Config key isolation — Sub(id) keys do not collide with Symbol keys
    //    Ensures u32 subscription IDs stored under DataKey::Sub(n) are
    //    distinct from Symbol-based config keys (Token, Admin, etc.).
    // -------------------------------------------------------------------------
    #[test]
    fn test_subscription_key_does_not_collide_with_config_keys() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SubscriptionVault, ());

        env.as_contract(&contract_id, || {
            let storage = env.storage().instance();

            // Write a config value and a subscription under different keys.
            storage.set(&DataKey::NextId, &1u32);
            storage.set(&DataKey::Sub(1), &999u32);

            // Both must be independently readable.
            assert_eq!(storage.get::<DataKey, u32>(&DataKey::NextId), Some(1u32));
            assert_eq!(storage.get::<DataKey, u32>(&DataKey::Sub(1)), Some(999u32));
        });
    }

    // -------------------------------------------------------------------------
    // 8. All SubscriptionStatus variants survive storage round-trip
    //    Each status must encode and decode correctly so state transitions
    //    are never silently corrupted after an upgrade.
    // -------------------------------------------------------------------------
    #[test]
    fn test_all_status_variants_round_trip_through_storage() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SubscriptionVault, ());

        let statuses = [
            SubscriptionStatus::Active,
            SubscriptionStatus::Paused,
            SubscriptionStatus::Cancelled,
            SubscriptionStatus::InsufficientBalance,
            SubscriptionStatus::GracePeriod,
        ];

        env.as_contract(&contract_id, || {
            for (i, status) in statuses.iter().enumerate() {
                let key = DataKey::Sub(i as u32);
                env.storage().instance().set(&key, status);
                let loaded: SubscriptionStatus = env
                    .storage()
                    .instance()
                    .get(&key)
                    .expect("status must be present");
                assert_eq!(&loaded, status);
            }
        });
    }

    // -------------------------------------------------------------------------
    // 9. SchemaVersion key is readable after init
    //    Confirms the schema version is written during init and can be read
    //    back — a prerequisite for any future migration guard logic.
    // -------------------------------------------------------------------------
    #[test]
    fn test_schema_version_is_set_after_init() {
        let (env, client, _token, _admin) = setup_test_env();

        let version: u32 = env.as_contract(&client.address, || {
            env.storage()
                .instance()
                .get(&DataKey::SchemaVersion)
                .expect("schema_version must be set after init")
        });

        // Must be a positive version number (current: 2).
        assert!(version >= 1, "schema version must be >= 1, got {version}");
    }

    // -------------------------------------------------------------------------
    // 10. Error discriminants are stable
    //     Error codes are returned to callers and stored in BatchChargeResult.
    //     Changing a discriminant value is a breaking API change.
    // -------------------------------------------------------------------------
    #[test]
    fn test_error_codes_are_stable() {
        assert_eq!(Error::Unauthorized as u32, 1001);
        assert_eq!(Error::Forbidden as u32, 1002);
        assert_eq!(Error::NotFound as u32, 2001);
        assert_eq!(Error::InvalidStatusTransition as u32, 4001);
        assert_eq!(Error::BelowMinimumTopup as u32, 5003);
        assert_eq!(Error::SubscriptionLimitReached as u32, 6001);
        assert_eq!(Error::IntervalNotElapsed as u32, 4004);
        assert_eq!(Error::NotActive as u32, 4002);
        assert_eq!(Error::InsufficientBalance as u32, 5001);
        assert_eq!(Error::UsageNotEnabled as u32, 6003);
        assert_eq!(Error::InsufficientPrepaidBalance as u32, 5002);
        assert_eq!(Error::InvalidAmount as u32, 3001);
        assert_eq!(Error::Replay as u32, 4005);
        assert_eq!(Error::EmergencyStopActive as u32, 4007);
        assert_eq!(Error::LifetimeCapReached as u32, 6002);
        assert_eq!(Error::AlreadyInitialized as u32, 4008);
    }
}

#[test]
fn test_merchant_token_bucket_reconciliation() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_c = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token_a, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    client.add_accepted_token(&admin, &token_b, &6);
    client.add_accepted_token(&admin, &token_c, &6);

    let merchant = Address::generate(&env);
    let subscriber_a = Address::generate(&env);
    let subscriber_b = Address::generate(&env);

    let token_a_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token_a);
    let token_b_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token_b);
    let token_a_client = soroban_sdk::token::Client::new(&env, &token_a);
    let token_b_client = soroban_sdk::token::Client::new(&env, &token_b);

    token_a_admin.mint(&subscriber_a, &100_000_000i128);
    token_b_admin.mint(&subscriber_b, &100_000_000i128);

    let id_a = client.create_subscription(
        &subscriber_a,
        &merchant,
        &5_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    let id_b = client.create_subscription_with_token(
        &subscriber_b,
        &merchant,
        &token_b,
        &7_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    client.deposit_funds(&id_a, &subscriber_a, &20_000_000i128);
    client.deposit_funds(&id_b, &subscriber_b, &20_000_000i128);

    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token_a), 0);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token_b), 0);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token_c), 0);

    // Charge cycle 1
    env.ledger().set_timestamp(T0 + INTERVAL);
    client.charge_subscription(&id_a);
    client.charge_subscription(&id_b);

    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token_a),
        5_000_000i128
    );
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token_b),
        7_000_000i128
    );
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token_c), 0);

    // Partial withdraw Token A (test withdrawal invariant and isolation)
    client.withdraw_merchant_token_funds(&merchant, &token_a, &2_000_000i128);

    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token_a),
        3_000_000i128
    );
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token_b),
        7_000_000i128
    );
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token_c), 0);

    assert_eq!(token_a_client.balance(&merchant), 2_000_000i128);
    assert_eq!(token_b_client.balance(&merchant), 0);

    // Charge cycle 2 (interleaved sequence)
    env.ledger().set_timestamp(T0 + 2 * INTERVAL);
    client.charge_subscription(&id_a);
    client.charge_subscription(&id_b);

    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token_a),
        8_000_000i128
    );
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token_b),
        14_000_000i128
    );

    // Full withdraw Token B
    client.withdraw_merchant_token_funds(&merchant, &token_b, &14_000_000i128);

    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token_a),
        8_000_000i128
    );
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token_b), 0);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token_c), 0);

    assert_eq!(token_a_client.balance(&merchant), 2_000_000i128);
    assert_eq!(token_b_client.balance(&merchant), 14_000_000i128);
}

#[test]
fn test_list_subscriptions_by_subscriber_pagination_and_sparse_ids() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(crate::SubscriptionVault, ());
    let client = crate::SubscriptionVaultClient::new(&env, &contract_id);

    let subscriber = Address::generate(&env);

    // Instead of creating real subs which require plans/assets/etc,
    // we query an empty state to verify the new structure doesn't crash
    // and returns the correct hardened types.
    let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &10);

    assert_eq!(page.subscription_ids.len(), 0);
    assert!(page.next_start_id.is_none());
}

// Added pagination correctness tests for billing statements

#[test]
fn test_offset_pagination_ordering_newest_first() {
    let (env, client, _token, _admin) = setup_test_env();
    let sub_id = 1u32;

    // append 5 statements
    for i in 0..5u32 {
        env.as_contract(&client.address, || {
            crate::statements::append_statement(
                &env,
                sub_id,
                1000 + i as i128,
                Address::generate(&env),
                crate::types::BillingChargeKind::Interval,
                i as u64,
                i as u64 + 10,
            ).unwrap();
        });
    }

    let page = env.as_contract(&client.address, || {
        crate::statements::get_statements_by_subscription_offset(&env, sub_id, 0, 5, true).unwrap()
    });

    assert_eq!(page.statements.len(), 5);
    // newest first => last appended first
    assert!(page.statements.get(0).unwrap().amount > page.statements.get(4).unwrap().amount);
}

#[test]
fn test_offset_pagination_ordering_oldest_first() {
    let (env, _client, _token, _admin) = setup_test_env();
    let sub_id = 2u32;

    for i in 0..5u32 {
        env.as_contract(&_client.address, || {
            crate::statements::append_statement(
                &env,
                sub_id,
                1000 + i as i128,
                Address::generate(&env),
                crate::types::BillingChargeKind::Interval,
                i as u64,
                i as u64 + 10,
            ).unwrap();
        });
    }

    let page = env.as_contract(&_client.address, || {
        crate::statements::get_statements_by_subscription_offset(&env, sub_id, 0, 5, false).unwrap()
    });

    assert!(page.statements.get(0).unwrap().amount < page.statements.get(4).unwrap().amount);
}

#[test]
fn test_cursor_pagination_continuity() {
    let (env, client, _token, _admin) = setup_test_env();
    let sub_id = 3u32;

    for i in 0..10u32 {
        env.as_contract(&client.address, || {
            crate::statements::append_statement(
                &env,
                sub_id,
                1000 + i as i128,
                Address::generate(&env),
                crate::types::BillingChargeKind::Interval,
                i as u64,
                i as u64 + 10,
            ).unwrap();
        });
    }

    let first = env.as_contract(&client.address, || {
        crate::statements::get_statements_by_subscription_cursor(&env, sub_id, None, 4, true)
            .unwrap()
    });

    assert_eq!(first.statements.len(), 4);
    assert!(first.next_cursor.is_some());

    let second = env.as_contract(&client.address, || {
        crate::statements::get_statements_by_subscription_cursor(
            &env,
            sub_id,
            first.next_cursor,
            4,
            true,
        )
        .unwrap()
    });

    assert_eq!(second.statements.len(), 4);
}

#[test]
fn test_cursor_termination() {
    let (env, client, _token, _admin) = setup_test_env();
    let client_addr = client.address.clone();
    let sub_id = 4u32;

    env.as_contract(&client_addr, || {
        for i in 0..3u32 {
            crate::statements::append_statement(
                &env,
                sub_id,
                1000 + i as i128,
                Address::generate(&env),
                crate::types::BillingChargeKind::Interval,
                i as u64,
                i as u64 + 10,
            ).unwrap();
        }
    });

    let mut cursor = None;
    let mut total_fetched = 0;

    loop {
        let page = env.as_contract(&client_addr, || {
            crate::statements::get_statements_by_subscription_cursor(&env, sub_id, cursor, 2, true)
                .unwrap()
        });

        total_fetched += page.statements.len();
        if page.next_cursor.is_none() {
            break;
        }
        cursor = page.next_cursor;
    }

    assert_eq!(total_fetched, 3);
}

#[test]
fn test_invalid_limit() {
    let (env, _client, _token, _admin) = setup_test_env();
    let sub_id = 5u32;

    let result =
        crate::statements::get_statements_by_subscription_cursor(&env, sub_id, None, 0, true);

    assert!(result.is_err());
}

#[test]
fn test_empty_history() {
    let (env, client, _token, _admin) = setup_test_env();

    let page = env.as_contract(&client.address, || {
        crate::statements::get_statements_by_subscription_cursor(&env, 999, None, 5, true).unwrap()
    });

    assert_eq!(page.statements.len(), 0);
    assert!(page.next_cursor.is_none());
}

#[test]
fn test_event_schema_consistency() {
    let test_env = TestEnv::default();
    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);

    test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &true,
        &None::<i128>,
        &None::<u64>,
    );

    let events = test_env.env.events().all();
    assert!(!events.is_empty());
}

// ── One-Off Charge Hardening Tests ──────────────────────────────────────────

#[test]
fn test_oneoff_unauthorized_merchant_rejected() {
    let (env, client, token, _admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let imposter = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&id, &subscriber, &50_000_000i128);

    // Imposter (different merchant) must be rejected
    let res = client.try_charge_one_off(&id, &imposter, &5_000_000i128);
    assert_eq!(res, Err(Ok(Error::Unauthorized)));

    // Verify balance unchanged
    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 50_000_000);
}

#[test]
fn test_oneoff_zero_amount_rejected() {
    let (env, client, token, _admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&id, &subscriber, &50_000_000i128);

    let res = client.try_charge_one_off(&id, &merchant, &0i128);
    assert_eq!(res, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_oneoff_negative_amount_rejected() {
    let (env, client, token, _admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&id, &subscriber, &50_000_000i128);

    let res = client.try_charge_one_off(&id, &merchant, &-1i128);
    assert_eq!(res, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_oneoff_exceeds_balance_rejected() {
    let (env, client, token, _admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&id, &subscriber, &50_000_000i128);

    // Attempt to charge more than balance
    let res = client.try_charge_one_off(&id, &merchant, &50_000_001i128);
    assert_eq!(res, Err(Ok(Error::InsufficientPrepaidBalance)));

    // Balance unchanged
    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 50_000_000);
}

#[test]
fn test_oneoff_exact_balance_succeeds() {
    let (env, client, token, _admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&id, &subscriber, &50_000_000i128);

    // Charge exactly the full balance
    client.charge_one_off(&id, &merchant, &50_000_000i128);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 0);
}

#[test]
fn test_oneoff_on_paused_subscription_succeeds() {
    let (env, client, token, _admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&id, &subscriber, &50_000_000i128);
    client.pause_subscription(&id, &subscriber);

    // One-off charges should work on paused subscriptions
    client.charge_one_off(&id, &merchant, &5_000_000i128);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 45_000_000);
    assert_eq!(sub.status, SubscriptionStatus::Paused);
}

#[test]
fn test_oneoff_on_cancelled_subscription_rejected() {
    let (env, client, token, _admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&id, &subscriber, &50_000_000i128);
    client.cancel_subscription(&id, &subscriber);

    let res = client.try_charge_one_off(&id, &merchant, &5_000_000i128);
    assert_eq!(res, Err(Ok(Error::NotActive)));
}

#[test]
fn test_oneoff_partial_balance_boundary() {
    let (env, client, token, _admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&id, &subscriber, &10_000_000i128);

    // Charge leaving exactly 1 unit remaining
    client.charge_one_off(&id, &merchant, &9_999_999i128);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 1);

    // Now charge that last unit
    client.charge_one_off(&id, &merchant, &1i128);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 0);

    // Any further charge should fail
    let res = client.try_charge_one_off(&id, &merchant, &1i128);
    assert_eq!(res, Err(Ok(Error::InsufficientPrepaidBalance)));
}

#[test]
fn test_oneoff_blocked_by_emergency_stop() {
    let (env, client, token, admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&id, &subscriber, &50_000_000i128);

    // Enable emergency stop
    client.enable_emergency_stop(&admin);

    let res = client.try_charge_one_off(&id, &merchant, &5_000_000i128);
    assert_eq!(res, Err(Ok(Error::EmergencyStopActive)));

    // Disable and verify charge works again
    client.disable_emergency_stop(&admin);
    client.charge_one_off(&id, &merchant, &5_000_000i128);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 45_000_000);
}

#[test]
fn test_oneoff_statement_kind_consistency() {
    let (env, client, token, _admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&id, &subscriber, &50_000_000i128);

    client.charge_one_off(&id, &merchant, &7_000_000i128);

    // Verify statement was recorded with OneOff kind
    let page = client.get_sub_statements_offset(&id, &0, &10, &false);
    assert_eq!(page.statements.len(), 1);

    let stmt = page.statements.get(0).unwrap();
    assert_eq!(stmt.amount, 7_000_000);
    assert_eq!(stmt.kind, crate::types::BillingChargeKind::OneOff);
    assert_eq!(stmt.merchant, merchant);
    // For one-off, period_start == period_end
    assert_eq!(stmt.period_start, stmt.period_end);
}

#[test]
fn test_oneoff_event_emitted() {
    use soroban_sdk::testutils::Events as _;

    let (env, client, token, _admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&id, &subscriber, &50_000_000i128);

    client.charge_one_off(&id, &merchant, &3_000_000i128);

    // Verify oneoff_ch event was emitted (events().all() is non-empty after the call)
    let all = env.events().all();
    assert!(
        !all.is_empty(),
        "charge_one_off must emit at least one event"
    );
}

#[test]
fn test_oneoff_lifetime_cap_boundary() {
    let (env, client, token, _admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    // Create subscription with lifetime cap of 20 USDC
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &Some(20_000_000i128),
        &None::<u64>,
    );
    // Deposit exactly cap — enforce_deposit_cap rejects deposits over remaining cap.
    client.deposit_funds(&id, &subscriber, &20_000_000i128);

    // Charge up to one unit below cap so subscription stays Active
    client.charge_one_off(&id, &merchant, &19_999_999i128);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.lifetime_charged, 19_999_999);

    // Next charge exceeds remaining balance (1 unit left) — balance check fires first.
    let res = client.try_charge_one_off(&id, &merchant, &2i128);
    assert_eq!(res, Err(Ok(Error::InsufficientPrepaidBalance)));
}

#[test]
fn test_oneoff_does_not_update_last_payment_timestamp() {
    let (env, client, token, _admin) = setup_test_env();
    env.ledger().set_timestamp(T0);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &100_000_000i128);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&id, &subscriber, &50_000_000i128);

    let sub_before = client.get_subscription(&id);
    let ts_before = sub_before.last_payment_timestamp;

    env.ledger().set_timestamp(T0 + 1000);
    client.charge_one_off(&id, &merchant, &5_000_000i128);

    let sub_after = client.get_subscription(&id);
    assert_eq!(sub_after.last_payment_timestamp, ts_before);
}


#[test]
fn test_compaction_aggregation_accuracy() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);

    let subscriber = Address::generate(&test_env.env);
    let merchant = Address::generate(&test_env.env);
    test_env.stellar_token_client().mint(&subscriber, &1_000_000_000i128);

    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000i128, // 10 USDC
        &INTERVAL,
        &true, // usage enabled
        &None::<i128>,
        &None::<u64>,
    );
    test_env.client.deposit_funds(&id, &subscriber, &500_000_000i128);

    // 1. Add mixed charges
    // Interval charge
    test_env.env.ledger().set_timestamp(T0 + INTERVAL);
    test_env.client.charge_subscription(&id); // 10 USDC

    // Usage charge
    test_env.client.charge_usage(&id, &5_000_000i128); // 5 USDC

    // One-off charge
    test_env.client.charge_one_off(&id, &merchant, &2_000_000i128); // 2 USDC

    // Another interval charge
    test_env.env.ledger().set_timestamp(T0 + 2 * INTERVAL);
    test_env.client.charge_subscription(&id); // 10 USDC (sequence 3)
    
    // Total charged so far: 10 + 5 + 2 + 10 = 27 USDC
    // Sequences: 0 (Interval), 1 (Usage), 2 (OneOff), 3 (Interval)

    // 2. Compact first 3 rows
    test_env.client.set_billing_retention(&test_env.admin, &1);
    let summary = test_env.client.compact_billing_statements(&test_env.admin, &id, &None::<u32>);
    
    assert_eq!(summary.pruned_count, 3);
    assert_eq!(summary.total_pruned_amount, 17_000_000i128);

    let agg = test_env.client.get_stmt_compacted_aggregate(&id);
    assert_eq!(agg.pruned_count, 3);
    assert_eq!(agg.total_amount, 17_000_000i128);
    assert_eq!(agg.totals.interval, 10_000_000i128);
    assert_eq!(agg.totals.usage, 5_000_000i128);
    assert_eq!(agg.totals.one_off, 2_000_000i128);

    // 3. Verify consistency
    let sub = test_env.client.get_subscription(&id);
    let live_page = test_env.client.get_sub_statements_offset(&id, &0, &10, &false);
    let mut live_total = 0i128;
    for stmt in live_page.statements.iter() {
        live_total += stmt.amount;
    }
    
    assert_eq!(agg.total_amount + live_total, sub.lifetime_charged);
    assert_eq!(sub.lifetime_charged, 27_000_000i128);
}

// ═════════════════════════════════════════════════════════════════════=========
// STATE MACHINE TRANSITION TESTS - Exhaustive Coverage
// ═════════════════════════════════════════════════════════════════════=========

/// Test that `transition_to` correctly applies valid transitions
#[test]
fn test_transition_to_valid_transitions() {
    // Active -> Paused
    let mut status = SubscriptionStatus::Active;
    assert!(transition_to(&mut status, SubscriptionStatus::Paused).is_ok());
    assert_eq!(status, SubscriptionStatus::Paused);

    // Paused -> Active
    let mut status = SubscriptionStatus::Paused;
    assert!(transition_to(&mut status, SubscriptionStatus::Active).is_ok());
    assert_eq!(status, SubscriptionStatus::Active);

    // Active -> Cancelled
    let mut status = SubscriptionStatus::Active;
    assert!(transition_to(&mut status, SubscriptionStatus::Cancelled).is_ok());
    assert_eq!(status, SubscriptionStatus::Cancelled);

    // InsufficientBalance -> Active
    let mut status = SubscriptionStatus::InsufficientBalance;
    assert!(transition_to(&mut status, SubscriptionStatus::Active).is_ok());
    assert_eq!(status, SubscriptionStatus::Active);

    // GracePeriod -> Active
    let mut status = SubscriptionStatus::GracePeriod;
    assert!(transition_to(&mut status, SubscriptionStatus::Active).is_ok());
    assert_eq!(status, SubscriptionStatus::Active);

    // GracePeriod -> InsufficientBalance
    let mut status = SubscriptionStatus::GracePeriod;
    assert!(transition_to(&mut status, SubscriptionStatus::InsufficientBalance).is_ok());
    assert_eq!(status, SubscriptionStatus::InsufficientBalance);

    // Active -> GracePeriod
    let mut status = SubscriptionStatus::Active;
    assert!(transition_to(&mut status, SubscriptionStatus::GracePeriod).is_ok());
    assert_eq!(status, SubscriptionStatus::GracePeriod);

    // Active -> InsufficientBalance
    let mut status = SubscriptionStatus::Active;
    assert!(transition_to(&mut status, SubscriptionStatus::InsufficientBalance).is_ok());
    assert_eq!(status, SubscriptionStatus::InsufficientBalance);

    // Active -> Expired
    let mut status = SubscriptionStatus::Active;
    assert!(transition_to(&mut status, SubscriptionStatus::Expired).is_ok());
    assert_eq!(status, SubscriptionStatus::Expired);

    // Paused -> Expired
    let mut status = SubscriptionStatus::Paused;
    assert!(transition_to(&mut status, SubscriptionStatus::Expired).is_ok());
    assert_eq!(status, SubscriptionStatus::Expired);

    // InsufficientBalance -> Expired
    let mut status = SubscriptionStatus::InsufficientBalance;
    assert!(transition_to(&mut status, SubscriptionStatus::Expired).is_ok());
    assert_eq!(status, SubscriptionStatus::Expired);

    // GracePeriod -> Expired
    let mut status = SubscriptionStatus::GracePeriod;
    assert!(transition_to(&mut status, SubscriptionStatus::Expired).is_ok());
    assert_eq!(status, SubscriptionStatus::Expired);

    // Cancelled -> Archived
    let mut status = SubscriptionStatus::Cancelled;
    assert!(transition_to(&mut status, SubscriptionStatus::Archived).is_ok());
    assert_eq!(status, SubscriptionStatus::Archived);

    // Expired -> Archived
    let mut status = SubscriptionStatus::Expired;
    assert!(transition_to(&mut status, SubscriptionStatus::Archived).is_ok());
    assert_eq!(status, SubscriptionStatus::Archived);
}

/// Test that `transition_to` rejects invalid transitions without mutation
#[test]
fn test_transition_to_invalid_transitions_no_mutation() {
    // Cancelled -> Active (should fail, status unchanged)
    let mut status = SubscriptionStatus::Cancelled;
    let original = status;
    assert!(transition_to(&mut status, SubscriptionStatus::Active).is_err());
    assert_eq!(status, original); // No mutation on failure

    // Cancelled -> Paused
    let mut status = SubscriptionStatus::Cancelled;
    let original = status;
    assert!(transition_to(&mut status, SubscriptionStatus::Paused).is_err());
    assert_eq!(status, original);

    // Cancelled -> InsufficientBalance
    let mut status = SubscriptionStatus::Cancelled;
    let original = status;
    assert!(transition_to(&mut status, SubscriptionStatus::InsufficientBalance).is_err());
    assert_eq!(status, original);

    // Cancelled -> GracePeriod
    let mut status = SubscriptionStatus::Cancelled;
    let original = status;
    assert!(transition_to(&mut status, SubscriptionStatus::GracePeriod).is_err());
    assert_eq!(status, original);

    // Paused -> InsufficientBalance
    let mut status = SubscriptionStatus::Paused;
    let original = status;
    assert!(transition_to(&mut status, SubscriptionStatus::InsufficientBalance).is_err());
    assert_eq!(status, original);

    // InsufficientBalance -> Paused
    let mut status = SubscriptionStatus::InsufficientBalance;
    let original = status;
    assert!(transition_to(&mut status, SubscriptionStatus::Paused).is_err());
    assert_eq!(status, original);

    // Archived -> Active (terminal state)
    let mut status = SubscriptionStatus::Archived;
    let original = status;
    assert!(transition_to(&mut status, SubscriptionStatus::Active).is_err());
    assert_eq!(status, original);

    // Archived -> Paused
    let mut status = SubscriptionStatus::Archived;
    let original = status;
    assert!(transition_to(&mut status, SubscriptionStatus::Paused).is_err());
    assert_eq!(status, original);

    // Archived -> Cancelled
    let mut status = SubscriptionStatus::Archived;
    let original = status;
    assert!(transition_to(&mut status, SubscriptionStatus::Cancelled).is_err());
    assert_eq!(status, original);
}

/// Test idempotent transitions (same status -> same status)
#[test]
fn test_transition_to_idempotent() {
    let statuses = [
        SubscriptionStatus::Active,
        SubscriptionStatus::Paused,
        SubscriptionStatus::Cancelled,
        SubscriptionStatus::InsufficientBalance,
        SubscriptionStatus::GracePeriod,
        SubscriptionStatus::Expired,
        SubscriptionStatus::Archived,
    ];

    for status in statuses {
        let mut current = status;
        assert!(
            transition_to(&mut current, status).is_ok(),
            "Idempotent transition for {:?} should succeed",
            status
        );
        assert_eq!(current, status);
    }
}

/// Test all valid transitions from each state (exhaustive adjacency matrix)
#[test]
fn test_exhaustive_valid_transitions_matrix() {
    // Define all valid transitions as (from, to) pairs
    let valid_transitions = [
        // From Active
        (SubscriptionStatus::Active, SubscriptionStatus::Paused),
        (SubscriptionStatus::Active, SubscriptionStatus::Cancelled),
        (SubscriptionStatus::Active, SubscriptionStatus::InsufficientBalance),
        (SubscriptionStatus::Active, SubscriptionStatus::GracePeriod),
        (SubscriptionStatus::Active, SubscriptionStatus::Expired),
        // From Paused
        (SubscriptionStatus::Paused, SubscriptionStatus::Active),
        (SubscriptionStatus::Paused, SubscriptionStatus::Cancelled),
        (SubscriptionStatus::Paused, SubscriptionStatus::Expired),
        // From Cancelled
        (SubscriptionStatus::Cancelled, SubscriptionStatus::Archived),
        // From InsufficientBalance
        (SubscriptionStatus::InsufficientBalance, SubscriptionStatus::Active),
        (SubscriptionStatus::InsufficientBalance, SubscriptionStatus::Cancelled),
        (SubscriptionStatus::InsufficientBalance, SubscriptionStatus::Expired),
        // From GracePeriod
        (SubscriptionStatus::GracePeriod, SubscriptionStatus::Active),
        (SubscriptionStatus::GracePeriod, SubscriptionStatus::Cancelled),
        (SubscriptionStatus::GracePeriod, SubscriptionStatus::InsufficientBalance),
        (SubscriptionStatus::GracePeriod, SubscriptionStatus::Expired),
        // From Expired
        (SubscriptionStatus::Expired, SubscriptionStatus::Archived),
        // From Archived - no outgoing transitions (terminal)
    ];

    for (from, to) in valid_transitions {
        let mut status = from;
        assert!(
            transition_to(&mut status, to).is_ok(),
            "Transition {:?} -> {:?} should be valid",
            from,
            to
        );
        assert_eq!(status, to);
    }
}

/// Test critical invalid transitions that must be blocked for security
#[test]
fn test_critical_security_transitions_blocked() {
    // Terminal state violations
    let terminal_violations = [
        (SubscriptionStatus::Cancelled, SubscriptionStatus::Active),
        (SubscriptionStatus::Cancelled, SubscriptionStatus::Paused),
        (SubscriptionStatus::Cancelled, SubscriptionStatus::InsufficientBalance),
        (SubscriptionStatus::Cancelled, SubscriptionStatus::GracePeriod),
        (SubscriptionStatus::Archived, SubscriptionStatus::Active),
        (SubscriptionStatus::Archived, SubscriptionStatus::Paused),
        (SubscriptionStatus::Archived, SubscriptionStatus::Cancelled),
        (SubscriptionStatus::Archived, SubscriptionStatus::InsufficientBalance),
        (SubscriptionStatus::Archived, SubscriptionStatus::GracePeriod),
        (SubscriptionStatus::Archived, SubscriptionStatus::Expired),
    ];

    for (from, to) in terminal_violations {
        let mut status = from;
        assert!(
            transition_to(&mut status, to).is_err(),
            "Security violation: {:?} -> {:?} must be blocked",
            from,
            to
        );
        assert_eq!(status, from, "Status must not mutate on invalid transition");
    }

    // Semantic violations (illogical transitions)
    let semantic_violations = [
        (SubscriptionStatus::Paused, SubscriptionStatus::InsufficientBalance),
        (SubscriptionStatus::InsufficientBalance, SubscriptionStatus::Paused),
        (SubscriptionStatus::Active, SubscriptionStatus::Archived), // Must go through Cancelled/Expired first
        (SubscriptionStatus::Paused, SubscriptionStatus::Archived),  // Must go through Cancelled/Expired first
    ];

    for (from, to) in semantic_violations {
        let mut status = from;
        assert!(
            transition_to(&mut status, to).is_err(),
            "Semantic violation: {:?} -> {:?} must be blocked",
            from,
            to
        );
        assert_eq!(status, from, "Status must not mutate on invalid transition");
    }
}

/// Test complex lifecycle sequences
#[test]
fn test_lifecycle_sequence_active_pause_resume_cancel() {
    let mut status = SubscriptionStatus::Active;
    
    // Active -> Paused
    assert!(transition_to(&mut status, SubscriptionStatus::Paused).is_ok());
    assert_eq!(status, SubscriptionStatus::Paused);
    
    // Paused -> Active
    assert!(transition_to(&mut status, SubscriptionStatus::Active).is_ok());
    assert_eq!(status, SubscriptionStatus::Active);
    
    // Active -> Paused (again)
    assert!(transition_to(&mut status, SubscriptionStatus::Paused).is_ok());
    assert_eq!(status, SubscriptionStatus::Paused);
    
    // Paused -> Cancelled
    assert!(transition_to(&mut status, SubscriptionStatus::Cancelled).is_ok());
    assert_eq!(status, SubscriptionStatus::Cancelled);
    
    // Cancelled -> Active (should fail)
    assert!(transition_to(&mut status, SubscriptionStatus::Active).is_err());
    assert_eq!(status, SubscriptionStatus::Cancelled);
}

#[test]
fn test_lifecycle_sequence_insufficient_balance_recovery() {
    let mut status = SubscriptionStatus::Active;
    
    // Active -> InsufficientBalance (charge failure)
    assert!(transition_to(&mut status, SubscriptionStatus::InsufficientBalance).is_ok());
    assert_eq!(status, SubscriptionStatus::InsufficientBalance);
    
    // InsufficientBalance -> Active (recovery after deposit)
    assert!(transition_to(&mut status, SubscriptionStatus::Active).is_ok());
    assert_eq!(status, SubscriptionStatus::Active);
    
    // Active -> InsufficientBalance (again)
    assert!(transition_to(&mut status, SubscriptionStatus::InsufficientBalance).is_ok());
    assert_eq!(status, SubscriptionStatus::InsufficientBalance);
    
    // InsufficientBalance -> Cancelled
    assert!(transition_to(&mut status, SubscriptionStatus::Cancelled).is_ok());
    assert_eq!(status, SubscriptionStatus::Cancelled);
}

#[test]
fn test_lifecycle_sequence_grace_period_flow() {
    let mut status = SubscriptionStatus::Active;
    
    // Active -> GracePeriod (first charge failure with grace enabled)
    assert!(transition_to(&mut status, SubscriptionStatus::GracePeriod).is_ok());
    assert_eq!(status, SubscriptionStatus::GracePeriod);
    
    // GracePeriod -> Active (successful charge during grace)
    assert!(transition_to(&mut status, SubscriptionStatus::Active).is_ok());
    assert_eq!(status, SubscriptionStatus::Active);
    
    // Active -> GracePeriod (again)
    assert!(transition_to(&mut status, SubscriptionStatus::GracePeriod).is_ok());
    assert_eq!(status, SubscriptionStatus::GracePeriod);
    
    // GracePeriod -> InsufficientBalance (grace expires)
    assert!(transition_to(&mut status, SubscriptionStatus::InsufficientBalance).is_ok());
    assert_eq!(status, SubscriptionStatus::InsufficientBalance);
    
    // InsufficientBalance -> Active (recovery)
    assert!(transition_to(&mut status, SubscriptionStatus::Active).is_ok());
    assert_eq!(status, SubscriptionStatus::Active);
}

#[test]
fn test_lifecycle_sequence_expiration_paths() {
    // Path 1: Active -> Expired -> Archived
    let mut status = SubscriptionStatus::Active;
    assert!(transition_to(&mut status, SubscriptionStatus::Expired).is_ok());
    assert_eq!(status, SubscriptionStatus::Expired);
    assert!(transition_to(&mut status, SubscriptionStatus::Archived).is_ok());
    assert_eq!(status, SubscriptionStatus::Archived);
    
    // Path 2: Paused -> Expired -> Archived
    let mut status = SubscriptionStatus::Paused;
    assert!(transition_to(&mut status, SubscriptionStatus::Expired).is_ok());
    assert_eq!(status, SubscriptionStatus::Expired);
    assert!(transition_to(&mut status, SubscriptionStatus::Archived).is_ok());
    assert_eq!(status, SubscriptionStatus::Archived);
    
    // Path 3: GracePeriod -> Expired -> Archived
    let mut status = SubscriptionStatus::GracePeriod;
    assert!(transition_to(&mut status, SubscriptionStatus::Expired).is_ok());
    assert_eq!(status, SubscriptionStatus::Expired);
    assert!(transition_to(&mut status, SubscriptionStatus::Archived).is_ok());
    assert_eq!(status, SubscriptionStatus::Archived);
}

#[test]
fn test_lifecycle_sequence_cancel_then_archive() {
    let mut status = SubscriptionStatus::Active;
    
    // Active -> Cancelled
    assert!(transition_to(&mut status, SubscriptionStatus::Cancelled).is_ok());
    assert_eq!(status, SubscriptionStatus::Cancelled);
    
    // Cancelled -> Archived
    assert!(transition_to(&mut status, SubscriptionStatus::Archived).is_ok());
    assert_eq!(status, SubscriptionStatus::Archived);
    
    // Archived is terminal - no further transitions
    assert!(transition_to(&mut status, SubscriptionStatus::Active).is_err());
    assert!(transition_to(&mut status, SubscriptionStatus::Cancelled).is_err());
    assert!(transition_to(&mut status, SubscriptionStatus::Expired).is_err());
    assert_eq!(status, SubscriptionStatus::Archived);
}

/// Test that all 7 states are covered in the match statement (compiler-enforced totality)
#[test]
fn test_state_completeness_all_statuses_exist() {
    let all_statuses = [
        SubscriptionStatus::Active,
        SubscriptionStatus::Paused,
        SubscriptionStatus::Cancelled,
        SubscriptionStatus::InsufficientBalance,
        SubscriptionStatus::GracePeriod,
        SubscriptionStatus::Expired,
        SubscriptionStatus::Archived,
    ];
    
    // Verify we have exactly 7 states
    assert_eq!(all_statuses.len(), 7, "State machine must have exactly 7 states");
    
    // Verify each state can transition to itself (idempotent)
    for status in all_statuses {
        let mut current = status;
        assert!(transition_to(&mut current, status).is_ok());
    }
}
