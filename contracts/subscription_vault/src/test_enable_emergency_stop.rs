#![cfg(test)]

//! Focused adversarial coverage and boundary tests for `enable_emergency_stop`.
//!
//! # Covered behaviors:
//! 1. Happy path activation by authorized admin, status update, and event emission
//!    (both `EmergencyStopEnabledEvent` and `AdminConfigChangedEvent`).
//! 2. Idempotency: calling `enable_emergency_stop` when already active returns `Ok(())`
//!    immediately without triggering cooldown checks or emitting duplicate events.
//! 3. Uninitialized state: calling `enable_emergency_stop` before `init` returns
//!    `Error::NotInitialized` with state left intact.
//! 4. Unauthorized callers: stranger, subscriber, merchant, operator, and contract self-address
//!    are rejected with `Error::Forbidden` and state/events remain untouched.
//! 5. Stale admin after rotation: former admin is rejected with `Error::Forbidden`;
//!    newly rotated admin succeeds.
//! 6. Cooldown enforcement & exact time boundaries: `CONFIG_COOLDOWN_SECS = 21_600`
//!    is strictly enforced between state changes:
//!    - Rejected at delta = 0
//!    - Rejected at delta = 1
//!    - Rejected at delta = 21_599 (CONFIG_COOLDOWN_SECS - 1)
//!    - Succeeded at delta = 21_600 (exact boundary)
//! 7. State preservation on rejected operations: subscriptions, prepaid balances,
//!    merchant balances, and operational flows remain intact after failed adversarial attempts.
//! 8. Mutation gating: once active, critical operations are blocked with `Error::EmergencyStopActive`
//!    while view/read functions continue to function.

use crate::{
    admin::CONFIG_COOLDOWN_SECS,
    types::{
        AdminConfigChangedEvent, EmergencyStopEnabledEvent, Error,
        EVENT_SCHEMA_VERSION,
    },
    ChargeExecutionResult, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    token::StellarAssetClient,
    Address, BytesN, Env, FromVal, String as SorobanString, Symbol, TryFromVal,
};

const T0: u64 = 1_700_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const MIN_TOPUP: i128 = 1_000_000;
const GRACE_PERIOD: u64 = 7 * 24 * 60 * 60;

/// Fixture containing initialized contract client and key actors.
struct TestContext<'a> {
    env: Env,
    client: SubscriptionVaultClient<'a>,
    contract_id: Address,
    admin: Address,
    token: Address,
}

fn setup_context() -> TestContext<'static> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token, &6, &admin, &MIN_TOPUP, &GRACE_PERIOD);

    TestContext {
        env,
        client,
        contract_id,
        admin,
        token,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Happy Path & Event Verification
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_enable_emergency_stop_happy_path_and_events() {
    let ctx = setup_context();

    // Baseline: emergency stop is inactive initially
    assert_eq!(ctx.client.get_emergency_stop_status(), false);

    // Act: authorized admin enables emergency stop
    let res = ctx.client.try_enable_emergency_stop(&ctx.admin);
    assert_eq!(res, Ok(Ok(())));

    // Verify events immediately after the invocation
    let events = ctx.env.events().all();
    assert!(!events.is_empty(), "Events should be emitted on enable");

    let mut found_enabled_event = false;
    let mut found_config_changed_event = false;

    for (_contract, topics, data) in events.iter() {
        if topics.len() > 0 {
            if let Ok(sym) = Symbol::try_from_val(&ctx.env, &topics.get(0).unwrap()) {
                if sym == Symbol::new(&ctx.env, "emergency_stop_enabled") {
                    let ev = EmergencyStopEnabledEvent::from_val(&ctx.env, &data);
                    assert_eq!(ev.admin, ctx.admin);
                    assert_eq!(ev.timestamp, T0);
                    assert_eq!(ev.schema_version, EVENT_SCHEMA_VERSION);
                    found_enabled_event = true;
                } else if sym == Symbol::new(&ctx.env, "admin_config_changed") {
                    let ev = AdminConfigChangedEvent::from_val(&ctx.env, &data);
                    assert_eq!(ev.key_label, SorobanString::from_str(&ctx.env, "EmergencyStop"));
                    assert_eq!(ev.prev_ts, 0);
                    assert_eq!(ev.timestamp, T0);
                    assert_eq!(ev.schema_version, EVENT_SCHEMA_VERSION);
                    found_config_changed_event = true;
                }
            }
        }
    }

    assert!(found_enabled_event, "EmergencyStopEnabledEvent must be emitted");
    assert!(found_config_changed_event, "AdminConfigChangedEvent must be emitted");

    // Verify state mutation
    assert_eq!(ctx.client.get_emergency_stop_status(), true);
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Idempotency (No redundant state changes or events)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_enable_emergency_stop_idempotency_no_duplicate_events() {
    let ctx = setup_context();

    // First enable
    ctx.client.enable_emergency_stop(&ctx.admin);
    let initial_events = ctx.env.events().all();
    assert!(!initial_events.is_empty(), "Initial enable must emit events");

    // Verify state
    assert_eq!(ctx.client.get_emergency_stop_status(), true);

    // Second enable immediately (within cooldown window) - must be a no-op Ok(())
    let res = ctx.client.try_enable_emergency_stop(&ctx.admin);
    assert_eq!(res, Ok(Ok(())), "Idempotent call must return Ok(()) without error");

    // No new events emitted in the second call
    // Note: in soroban test environment, env.events().all() returns events from the latest invocation
    let latest_events = ctx.env.events().all();
    assert!(
        latest_events.is_empty(),
        "Idempotent enable_emergency_stop must not publish redundant events"
    );

    // Status remains true
    assert_eq!(ctx.client.get_emergency_stop_status(), true);
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Uninitialized Contract State
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_enable_emergency_stop_uninitialized_contract() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let caller = Address::generate(&env);

    // Calling before init must fail with NotInitialized
    let res = client.try_enable_emergency_stop(&caller);
    assert_eq!(res, Err(Ok(Error::NotInitialized)));

    // Emergency stop status must remain false
    assert_eq!(client.get_emergency_stop_status(), false);

    // No events emitted
    assert!(env.events().all().is_empty());
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Unauthorized Callers Rejected & State Unchanged
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_enable_emergency_stop_unauthorized_callers_matrix() {
    let ctx = setup_context();

    let stranger = Address::generate(&ctx.env);
    let subscriber = Address::generate(&ctx.env);
    let merchant = Address::generate(&ctx.env);
    let operator = Address::generate(&ctx.env);
    ctx.client.set_operator(&ctx.admin, &operator);

    let unauthorized_callers = [
        ("stranger", stranger),
        ("subscriber", subscriber),
        ("merchant", merchant),
        ("operator", operator),
        ("contract_id", ctx.contract_id.clone()),
    ];

    for (name, caller) in unauthorized_callers.iter() {
        let res = ctx.client.try_enable_emergency_stop(caller);
        assert_eq!(
            res,
            Err(Ok(Error::Forbidden)),
            "Caller {name} must be rejected with Forbidden"
        );

        // Emergency stop must NOT be enabled
        assert_eq!(
            ctx.client.get_emergency_stop_status(),
            false,
            "Emergency stop must remain false after unauthorized call from {name}"
        );

        // No emergency_stop_enabled event should be published
        let latest_events = ctx.env.events().all();
        let has_enabled_event = latest_events.iter().any(|(_c, topics, _d)| {
            topics.len() > 0
                && Symbol::try_from_val(&ctx.env, &topics.get(0).unwrap())
                    .map(|s| s == Symbol::new(&ctx.env, "emergency_stop_enabled"))
                    .unwrap_or(false)
        });
        assert!(
            !has_enabled_event,
            "No emergency_stop_enabled event for unauthorized caller {name}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. Stale Admin After Rotation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_enable_emergency_stop_stale_admin_rejected_after_rotation() {
    let ctx = setup_context();
    let new_admin = Address::generate(&ctx.env);

    // Advance time past admin rotation cooldown
    ctx.env.ledger().set_timestamp(T0 + CONFIG_COOLDOWN_SECS);

    // Rotate admin from old admin to new admin
    ctx.client.rotate_admin(&ctx.admin, &new_admin, &0u64);
    assert_eq!(ctx.client.get_admin(), new_admin);

    // Former admin attempts to enable emergency stop -> Forbidden
    let res_stale = ctx.client.try_enable_emergency_stop(&ctx.admin);
    assert_eq!(
        res_stale,
        Err(Ok(Error::Forbidden)),
        "Former admin must be rejected after rotation"
    );
    assert_eq!(ctx.client.get_emergency_stop_status(), false);

    // Advance time past EmergencyStop cooldown
    ctx.env.ledger().set_timestamp(T0 + CONFIG_COOLDOWN_SECS * 2);

    // New admin enables emergency stop -> Ok(())
    let res_new = ctx.client.try_enable_emergency_stop(&new_admin);
    assert_eq!(res_new, Ok(Ok(())));
    assert_eq!(ctx.client.get_emergency_stop_status(), true);
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. Cooldown Enforcement & Exact Boundary Testing
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_enable_emergency_stop_cooldown_and_exact_boundaries() {
    let ctx = setup_context();

    // 1. First enable at T0
    ctx.client.enable_emergency_stop(&ctx.admin);
    assert_eq!(ctx.client.get_emergency_stop_status(), true);

    // 2. Advance time exactly CONFIG_COOLDOWN_SECS and disable
    let t_disable = T0 + CONFIG_COOLDOWN_SECS;
    ctx.env.ledger().set_timestamp(t_disable);
    ctx.client.disable_emergency_stop(&ctx.admin);
    assert_eq!(ctx.client.get_emergency_stop_status(), false);

    // 3. Boundary tests for re-enabling:

    // Case A: delta = 0 (same timestamp as disable) -> CooldownActive
    ctx.env.ledger().set_timestamp(t_disable);
    let res_0 = ctx.client.try_enable_emergency_stop(&ctx.admin);
    assert_eq!(res_0, Err(Ok(Error::CooldownActive)));
    assert_eq!(ctx.client.get_emergency_stop_status(), false);

    // Case B: delta = 1 second -> CooldownActive
    ctx.env.ledger().set_timestamp(t_disable + 1);
    let res_1 = ctx.client.try_enable_emergency_stop(&ctx.admin);
    assert_eq!(res_1, Err(Ok(Error::CooldownActive)));
    assert_eq!(ctx.client.get_emergency_stop_status(), false);

    // Case C: delta = CONFIG_COOLDOWN_SECS - 1 (21_599 seconds, 1 sec before boundary) -> CooldownActive
    ctx.env
        .ledger()
        .set_timestamp(t_disable + CONFIG_COOLDOWN_SECS - 1);
    let res_just_before = ctx.client.try_enable_emergency_stop(&ctx.admin);
    assert_eq!(
        res_just_before,
        Err(Ok(Error::CooldownActive)),
        "Must reject 1 second before cooldown expiry"
    );
    assert_eq!(ctx.client.get_emergency_stop_status(), false);

    // Case D: delta = CONFIG_COOLDOWN_SECS (exact boundary, 21_600 seconds) -> Success
    ctx.env
        .ledger()
        .set_timestamp(t_disable + CONFIG_COOLDOWN_SECS);
    let res_exact = ctx.client.try_enable_emergency_stop(&ctx.admin);
    assert_eq!(
        res_exact,
        Ok(Ok(())),
        "Must succeed at exact CONFIG_COOLDOWN_SECS boundary"
    );
    assert_eq!(ctx.client.get_emergency_stop_status(), true);
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. State Preservation Across Rejected Adversarial Calls
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_enable_emergency_stop_preserves_state_on_rejected_calls() {
    let ctx = setup_context();
    let subscriber = Address::generate(&ctx.env);
    let merchant = Address::generate(&ctx.env);

    let asset_client = StellarAssetClient::new(&ctx.env, &ctx.token);
    asset_client.mint(&subscriber, &100_000_000i128);

    // Create subscription (usage_enabled = false)
    let sub_id = ctx.client.create_subscription(
        &subscriber,
        &merchant,
        &5_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    // Deposit funds
    ctx.client.deposit_funds(&sub_id, &subscriber, &20_000_000i128, &None::<BytesN<32>>);

    // Verify baseline state
    let sub_before = ctx.client.get_subscription(&sub_id);
    assert_eq!(sub_before.status, SubscriptionStatus::Active);
    assert_eq!(sub_before.prepaid_balance, 20_000_000i128);

    // Adversarial attempts
    let stranger = Address::generate(&ctx.env);
    for _ in 0..5 {
        assert_eq!(
            ctx.client.try_enable_emergency_stop(&stranger),
            Err(Ok(Error::Forbidden))
        );
        assert_eq!(
            ctx.client.try_enable_emergency_stop(&subscriber),
            Err(Ok(Error::Forbidden))
        );
        assert_eq!(
            ctx.client.try_enable_emergency_stop(&merchant),
            Err(Ok(Error::Forbidden))
        );
    }

    // Verify that subscription state, balance, and emergency stop status are unaffected
    let sub_after = ctx.client.get_subscription(&sub_id);
    assert_eq!(sub_after.status, SubscriptionStatus::Active);
    assert_eq!(sub_after.prepaid_balance, 20_000_000i128);
    assert_eq!(ctx.client.get_emergency_stop_status(), false);

    // Advance time past the interval to perform legitimate charge
    ctx.env.ledger().set_timestamp(T0 + INTERVAL);

    // Normal charge operations continue smoothly
    let charge_res = ctx.client.charge_subscription(&sub_id, &None::<BytesN<32>>);
    assert_eq!(charge_res, ChargeExecutionResult::Charged);

    let sub_charged = ctx.client.get_subscription(&sub_id);
    assert_eq!(sub_charged.prepaid_balance, 15_000_000i128);
}

// ─────────────────────────────────────────────────────────────────────────────
// 8. Mutation Gating & Read Accessibility
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_enable_emergency_stop_mutation_gating_matrix() {
    let ctx = setup_context();
    let subscriber = Address::generate(&ctx.env);
    let merchant = Address::generate(&ctx.env);

    let asset_client = StellarAssetClient::new(&ctx.env, &ctx.token);
    asset_client.mint(&subscriber, &100_000_000i128);

    // Create a subscription before emergency stop (usage_enabled = false)
    let sub_id = ctx.client.create_subscription(
        &subscriber,
        &merchant,
        &5_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    // Enable emergency stop
    ctx.client.enable_emergency_stop(&ctx.admin);
    assert_eq!(ctx.client.get_emergency_stop_status(), true);

    // 1. Mutating operations must be blocked with EmergencyStopActive
    assert_eq!(
        ctx.client.try_create_subscription(
            &subscriber,
            &merchant,
            &5_000_000i128,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
            &None::<u32>,
            &None::<soroban_sdk::Symbol>,
        ),
        Err(Ok(Error::EmergencyStopActive))
    );

    assert_eq!(
        ctx.client.try_create_subscription_with_token(
            &subscriber,
            &merchant,
            &ctx.token,
            &5_000_000i128,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
            &None::<u32>,
            &None::<soroban_sdk::Symbol>,
        ),
        Err(Ok(Error::EmergencyStopActive))
    );

    assert_eq!(
        ctx.client.try_deposit_funds(
            &sub_id,
            &subscriber,
            &10_000_000i128,
            &None::<BytesN<32>>,
        ),
        Err(Ok(Error::EmergencyStopActive))
    );

    assert_eq!(
        ctx.client.try_charge_subscription(&sub_id, &None::<BytesN<32>>),
        Err(Ok(Error::EmergencyStopActive))
    );

    assert_eq!(
        ctx.client.try_charge_usage(&sub_id, &100_000i128),
        Err(Ok(Error::EmergencyStopActive))
    );

    assert_eq!(
        ctx.client.try_partial_refund(&ctx.admin, &sub_id, &subscriber, &1_000_000i128),
        Err(Ok(Error::EmergencyStopActive))
    );

    // 2. View operations remain fully accessible
    let sub = ctx.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(ctx.client.get_admin(), ctx.admin);
    assert_eq!(ctx.client.get_min_topup(), MIN_TOPUP);
    assert_eq!(ctx.client.get_emergency_stop_status(), true);
}
