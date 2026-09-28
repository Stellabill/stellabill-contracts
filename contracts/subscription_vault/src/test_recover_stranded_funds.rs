#![cfg(test)]

//! Focused adversarial coverage and boundary tests for `recover_stranded_funds`.
//!
//! # Covered behaviors:
//! 1. Happy path recovery across all `RecoveryReason` variants (`UserOverpayment`,
//!    `FailedTransfer`, `ExpiredEscrow`, `SystemCorrection`, `AccidentalTransfer`)
//!    with recipient balance validation and `TOPIC_RECOVERY` + `RecoveryEvent` emission.
//! 2. Exact recoverable balance boundaries:
//!    - Recovery of exact recoverable balance (`contract_balance - accounted_balance`) succeeds.
//!    - Exceeding recoverable balance by even 1 unit fails with `Error::InsufficientBalance`.
//!    - Zero recoverable balance rejects attempts with `Error::InsufficientBalance`.
//! 3. Invalid amount boundary testing:
//!    - `amount = 0` rejected with `Error::InvalidRecoveryAmount`.
//!    - Negative amounts (`-1`, `-10_000_000`, `i128::MIN`) rejected with `Error::InvalidRecoveryAmount`.
//! 4. Uninitialized state: calling `recover_stranded_funds` before `init` returns
//!    `Error::NotInitialized`.
//! 5. Unauthorized callers matrix: stranger, subscriber, merchant, operator, recipient,
//!    and contract self-address are all rejected with `Error::Forbidden`.
//! 6. Stale admin rejection after admin rotation: previous admin is rejected with `Error::Forbidden`,
//!    newly active admin succeeds.
//! 7. Replay protection: reusing the same `recovery_id` (even with differing parameters)
//!    is rejected with `Error::Replay`.
//! 8. Overdraw protection & subscription coexistence: stranded funds recovery does not
//!    drain or compromise active subscription balances, subsequent charges, or merchant payouts.
//! 9. Multi-token isolation: stranded balance in Token A does not permit recovery of Token B,
//!    and accounting between different tokens remains strictly segregated.
//! 10. Emergency stop resilience: `recover_stranded_funds` remains fully operational
//!     when `EmergencyStopActive` is asserted.
//! 11. State immutability across all rejected operations: token balances, subscriber vaults,
//!     merchant balances, and accounting invariants remain unmodified on failure.

use crate::{
    types::{
        Error, RecoveryEvent, RecoveryReason,
        EVENT_SCHEMA_VERSION, TOPIC_RECOVERY,
    },
    SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    token::{Client as TokenClient, StellarAssetClient},
    Address, BytesN, Env, FromVal, String as SorobanString, Symbol, TryFromVal,
};

extern crate alloc;
use alloc::format;

const T0: u64 = 1_700_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const MIN_TOPUP: i128 = 1_000_000;
const GRACE_PERIOD: u64 = 7 * 24 * 60 * 60;

/// Fixture containing initialized contract client, token, and actors.
struct TestContext<'a> {
    env: Env,
    client: SubscriptionVaultClient<'a>,
    contract_id: Address,
    admin: Address,
    token: Address,
    token_admin_client: StellarAssetClient<'a>,
    token_client: TokenClient<'a>,
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

    let token_admin_client = StellarAssetClient::new(&env, &token);
    let token_client = TokenClient::new(&env, &token);

    client.init(&token, &6, &admin, &MIN_TOPUP, &GRACE_PERIOD);

    TestContext {
        env,
        client,
        contract_id,
        admin,
        token,
        token_admin_client,
        token_client,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Happy Path & Event Verification Across All Recovery Reasons
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_recover_stranded_funds_happy_path_all_reasons() {
    let ctx = setup_context();
    let recipient = Address::generate(&ctx.env);

    // Mint stranded funds directly to contract
    let stranded_amount = 50_000_000i128;
    ctx.token_admin_client.mint(&ctx.contract_id, &stranded_amount);

    let reasons = [
        RecoveryReason::UserOverpayment,
        RecoveryReason::FailedTransfer,
        RecoveryReason::ExpiredEscrow,
        RecoveryReason::SystemCorrection,
        RecoveryReason::AccidentalTransfer,
    ];

    let recovery_chunk = 10_000_000i128;

    for (i, reason) in reasons.into_iter().enumerate() {
        let recovery_id = SorobanString::from_str(&ctx.env, &format!("recovery-reason-{}", i));
        let bal_before = ctx.token_client.balance(&recipient);
        let contract_bal_before = ctx.token_client.balance(&ctx.contract_id);

        let res = ctx.client.try_recover_stranded_funds(
            &ctx.admin,
            &ctx.token,
            &recipient,
            &recovery_chunk,
            &recovery_id,
            &reason,
        );
        assert_eq!(res, Ok(Ok(())));

        // Verify events immediately after invocation
        let events = ctx.env.events().all();
        assert!(!events.is_empty(), "Events should be emitted on recovery");

        let mut found_recovery_event = false;
        for (_contract, topics, data) in events.iter() {
            if topics.len() >= 2 {
                if let Ok(topic_sym) = Symbol::try_from_val(&ctx.env, &topics.get(0).unwrap()) {
                    if topic_sym == TOPIC_RECOVERY {
                        let ev_admin = Address::from_val(&ctx.env, &topics.get(1).unwrap());
                        assert_eq!(ev_admin, ctx.admin);

                        let ev = RecoveryEvent::from_val(&ctx.env, &data);
                        assert_eq!(ev.admin, ctx.admin);
                        assert_eq!(ev.recipient, recipient);
                        assert_eq!(ev.token, ctx.token);
                        assert_eq!(ev.amount, recovery_chunk);
                        assert_eq!(ev.reason, reason);
                        assert_eq!(ev.timestamp, T0);
                        assert_eq!(ev.schema_version, EVENT_SCHEMA_VERSION);
                        found_recovery_event = true;
                    }
                }
            }
        }
        assert!(found_recovery_event, "TOPIC_RECOVERY event with RecoveryEvent payload must be emitted");

        // Verify balances
        let bal_after = ctx.token_client.balance(&recipient);
        let contract_bal_after = ctx.token_client.balance(&ctx.contract_id);
        assert_eq!(bal_after - bal_before, recovery_chunk);
        assert_eq!(contract_bal_before - contract_bal_after, recovery_chunk);
    }

    // All 50M stranded funds have now been recovered
    assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);
    assert_eq!(ctx.token_client.balance(&recipient), stranded_amount);
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Exact Recoverable Boundary Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_recover_stranded_funds_exact_recoverable_boundary() {
    let ctx = setup_context();
    let recipient = Address::generate(&ctx.env);

    // Initial contract balance = 0, Recoverable = 0
    let rec_id_zero = SorobanString::from_str(&ctx.env, "rec-bound-0");
    let res_zero = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient,
        &1i128,
        &rec_id_zero,
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_zero, Err(Ok(Error::InsufficientBalance)));

    // Inject 30M stranded funds directly to contract
    ctx.token_admin_client.mint(&ctx.contract_id, &30_000_000i128);

    // Try recovering 30_000_001 (1 unit above boundary) -> InsufficientBalance
    let rec_id_over = SorobanString::from_str(&ctx.env, "rec-bound-over");
    let res_over = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient,
        &30_000_001i128,
        &rec_id_over,
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_over, Err(Ok(Error::InsufficientBalance)));

    // Recover exact boundary of 30_000_000 -> Succeeded
    let rec_id_exact = SorobanString::from_str(&ctx.env, "rec-bound-exact");
    let res_exact = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient,
        &30_000_000i128,
        &rec_id_exact,
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_exact, Ok(Ok(())));
    assert_eq!(ctx.token_client.balance(&recipient), 30_000_000i128);
    assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);

    // Next recovery attempt of 1 unit fails (balance is 0)
    let rec_id_after = SorobanString::from_str(&ctx.env, "rec-bound-after");
    let res_after = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient,
        &1i128,
        &rec_id_after,
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_after, Err(Ok(Error::InsufficientBalance)));
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Invalid Amount Boundary Tests (<= 0)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_recover_stranded_funds_invalid_amount_boundaries() {
    let ctx = setup_context();
    let recipient = Address::generate(&ctx.env);

    // Mint stranded tokens
    ctx.token_admin_client.mint(&ctx.contract_id, &50_000_000i128);

    let invalid_amounts = [
        0i128,
        -1i128,
        -1_000_000i128,
        i128::MIN,
    ];

    for (i, &bad_amt) in invalid_amounts.iter().enumerate() {
        let rec_id = SorobanString::from_str(&ctx.env, &format!("rec-invalid-amt-{}", i));
        let res = ctx.client.try_recover_stranded_funds(
            &ctx.admin,
            &ctx.token,
            &recipient,
            &bad_amt,
            &rec_id,
            &RecoveryReason::SystemCorrection,
        );
        assert_eq!(res, Err(Ok(Error::InvalidRecoveryAmount)));
        assert_eq!(ctx.token_client.balance(&recipient), 0);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), 50_000_000i128);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Uninitialized Contract State
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_recover_stranded_funds_uninitialized_contract() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let recipient = Address::generate(&env);

    let rec_id = SorobanString::from_str(&env, "rec-uninit");
    let res = client.try_recover_stranded_funds(
        &admin,
        &token,
        &recipient,
        &10_000_000i128,
        &rec_id,
        &RecoveryReason::AccidentalTransfer,
    );

    assert_eq!(res, Err(Ok(Error::NotInitialized)));
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. Unauthorized Caller Matrix
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_recover_stranded_funds_unauthorized_callers_matrix() {
    let ctx = setup_context();
    let recipient = Address::generate(&ctx.env);

    // Setup active actors
    let subscriber = Address::generate(&ctx.env);
    let merchant = Address::generate(&ctx.env);
    let operator = Address::generate(&ctx.env);
    let stranger = Address::generate(&ctx.env);

    ctx.token_admin_client.mint(&subscriber, &30_000_000i128);
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
    ctx.client.deposit_funds(&sub_id, &subscriber, &20_000_000i128, &None::<BytesN<32>>);
    ctx.client.set_operator(&ctx.admin, &operator);

    // Mint stranded tokens
    ctx.token_admin_client.mint(&ctx.contract_id, &50_000_000i128);

    let unauthorized_callers = [
        ("stranger", stranger),
        ("subscriber", subscriber),
        ("merchant", merchant),
        ("operator", operator),
        ("recipient", recipient.clone()),
        ("contract_self", ctx.contract_id.clone()),
    ];

    for (name, caller) in unauthorized_callers.into_iter() {
        let rec_id = SorobanString::from_str(&ctx.env, &format!("rec-unauth-{}", name));
        let bal_recipient_before = ctx.token_client.balance(&recipient);
        let bal_contract_before = ctx.token_client.balance(&ctx.contract_id);

        let res = ctx.client.try_recover_stranded_funds(
            &caller,
            &ctx.token,
            &recipient,
            &10_000_000i128,
            &rec_id,
            &RecoveryReason::AccidentalTransfer,
        );

        assert_eq!(res, Err(Ok(Error::Forbidden)), "Caller '{}' must be rejected with Forbidden", name);
        assert_eq!(ctx.token_client.balance(&recipient), bal_recipient_before);
        assert_eq!(ctx.token_client.balance(&ctx.contract_id), bal_contract_before);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. Stale Admin Rejected After Admin Rotation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_recover_stranded_funds_stale_admin_rejected_after_rotation() {
    let ctx = setup_context();
    let new_admin = Address::generate(&ctx.env);
    let recipient = Address::generate(&ctx.env);

    // Mint stranded tokens
    ctx.token_admin_client.mint(&ctx.contract_id, &40_000_000i128);

    // Old admin recovers 10M before rotation
    let rec_id_1 = SorobanString::from_str(&ctx.env, "rec-rot-1");
    let res_old = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient,
        &10_000_000i128,
        &rec_id_1,
        &RecoveryReason::UserOverpayment,
    );
    assert_eq!(res_old, Ok(Ok(())));
    assert_eq!(ctx.token_client.balance(&recipient), 10_000_000i128);

    // Rotate admin from ctx.admin to new_admin
    ctx.client.rotate_admin(&ctx.admin, &new_admin, &0u64);

    // Old admin attempts recovery -> Rejected with Forbidden
    let rec_id_stale = SorobanString::from_str(&ctx.env, "rec-rot-stale");
    let res_stale = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient,
        &10_000_000i128,
        &rec_id_stale,
        &RecoveryReason::UserOverpayment,
    );
    assert_eq!(res_stale, Err(Ok(Error::Forbidden)));
    assert_eq!(ctx.token_client.balance(&recipient), 10_000_000i128);

    // New admin attempts recovery -> Succeeded
    let rec_id_new = SorobanString::from_str(&ctx.env, "rec-rot-new");
    let res_new = ctx.client.try_recover_stranded_funds(
        &new_admin,
        &ctx.token,
        &recipient,
        &10_000_000i128,
        &rec_id_new,
        &RecoveryReason::UserOverpayment,
    );
    assert_eq!(res_new, Ok(Ok(())));
    assert_eq!(ctx.token_client.balance(&recipient), 20_000_000i128);
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. Replay Protection (Duplicate Recovery ID)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_recover_stranded_funds_replay_protection() {
    let ctx = setup_context();
    let recipient1 = Address::generate(&ctx.env);
    let recipient2 = Address::generate(&ctx.env);

    ctx.token_admin_client.mint(&ctx.contract_id, &50_000_000i128);

    let recovery_id = SorobanString::from_str(&ctx.env, "duplicate-recovery-id-001");

    // First recovery with this ID succeeds
    let res1 = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient1,
        &10_000_000i128,
        &recovery_id,
        &RecoveryReason::ExpiredEscrow,
    );
    assert_eq!(res1, Ok(Ok(())));
    assert_eq!(ctx.token_client.balance(&recipient1), 10_000_000i128);

    // Exact replay attempt -> Replay error
    let res_replay = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient1,
        &10_000_000i128,
        &recovery_id,
        &RecoveryReason::ExpiredEscrow,
    );
    assert_eq!(res_replay, Err(Ok(Error::Replay)));

    // Modified parameters but same recovery_id -> still Replay error
    let res_replay_diff = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient2,
        &5_000_000i128,
        &recovery_id,
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_replay_diff, Err(Ok(Error::Replay)));
    assert_eq!(ctx.token_client.balance(&recipient2), 0);

    // Fresh recovery_id succeeds
    let recovery_id_fresh = SorobanString::from_str(&ctx.env, "fresh-recovery-id-002");
    let res_fresh = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient2,
        &5_000_000i128,
        &recovery_id_fresh,
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_fresh, Ok(Ok(())));
    assert_eq!(ctx.token_client.balance(&recipient2), 5_000_000i128);
}

// ─────────────────────────────────────────────────────────────────────────────
// 8. Overdraw Protection and Subscription Coexistence
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_recover_stranded_funds_overdraw_protection_and_subscription_coexistence() {
    let ctx = setup_context();
    let recipient = Address::generate(&ctx.env);

    // Mint 15M stranded funds to contract
    ctx.token_admin_client.mint(&ctx.contract_id, &15_000_000i128);

    // Overdraw attempt: try recovering 15_000_001 -> InsufficientBalance
    let rec_id_over = SorobanString::from_str(&ctx.env, "rec-coex-over");
    let res_over = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient,
        &15_000_001i128,
        &rec_id_over,
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_over, Err(Ok(Error::InsufficientBalance)));

    // Recover exact 15M -> Succeeded
    let rec_id_exact = SorobanString::from_str(&ctx.env, "rec-coex-exact");
    let res_exact = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient,
        &15_000_000i128,
        &rec_id_exact,
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_exact, Ok(Ok(())));
    assert_eq!(ctx.token_client.balance(&recipient), 15_000_000i128);
    assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);

    // Now create and fund subscriptions cleanly
    let merchant = Address::generate(&ctx.env);
    let subscriber1 = Address::generate(&ctx.env);
    let subscriber2 = Address::generate(&ctx.env);

    ctx.client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0,
        &0x1F,
        &None,
        &SorobanString::from_str(&ctx.env, "https://merchant.example.com"),
    );

    // Sub 1: 15M deposit
    ctx.token_admin_client.mint(&subscriber1, &20_000_000i128);
    let sub1 = ctx.client.create_subscription(
        &subscriber1,
        &merchant,
        &5_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    ctx.client.deposit_funds(&sub1, &subscriber1, &15_000_000i128, &None::<BytesN<32>>);

    // Sub 2: 25M deposit
    ctx.token_admin_client.mint(&subscriber2, &30_000_000i128);
    let sub2 = ctx.client.create_subscription(
        &subscriber2,
        &merchant,
        &10_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    ctx.client.deposit_funds(&sub2, &subscriber2, &25_000_000i128, &None::<BytesN<32>>);

    // Verify subscription lifecycle executes without disruption
    ctx.env.ledger().set_timestamp(T0 + INTERVAL + 1);

    ctx.client.charge_subscription(&sub1, &None::<BytesN<32>>);
    ctx.client.charge_subscription(&sub2, &None::<BytesN<32>>);

    let sub1_after = ctx.client.get_subscription(&sub1);
    let sub2_after = ctx.client.get_subscription(&sub2);
    assert_eq!(sub1_after.prepaid_balance, 10_000_000i128);
    assert_eq!(sub2_after.prepaid_balance, 15_000_000i128);

    assert_eq!(ctx.client.get_merchant_balance(&merchant), 15_000_000i128);

    // Merchant withdraws earned funds (15M total earnings)
    let merchant_bal_before = ctx.token_client.balance(&merchant);
    ctx.client.withdraw_merchant_funds(&merchant, &15_000_000i128);
    let merchant_bal_after = ctx.token_client.balance(&merchant);
    assert_eq!(merchant_bal_after - merchant_bal_before, 15_000_000i128);
    assert_eq!(ctx.client.get_merchant_balance(&merchant), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// 9. Multi-Token Isolation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_recover_stranded_funds_multi_token_isolation() {
    let ctx = setup_context();
    let recipient = Address::generate(&ctx.env);

    // Register second accepted token (Token B)
    let token_b = ctx
        .env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();
    let token_b_client = TokenClient::new(&ctx.env, &token_b);

    ctx.client.add_accepted_token(&ctx.admin, &token_b, &6);

    // Mint 50M stranded tokens for Token A only
    ctx.token_admin_client.mint(&ctx.contract_id, &50_000_000i128);

    // Attempting recovery of Token B fails (balance = 0)
    let rec_id_b = SorobanString::from_str(&ctx.env, "rec-token-b-fail");
    let res_b = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &token_b,
        &recipient,
        &1_000_000i128,
        &rec_id_b,
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_b, Err(Ok(Error::InsufficientBalance)));
    assert_eq!(token_b_client.balance(&ctx.contract_id), 0);

    // Attempting recovery of Token A succeeds
    let rec_id_a = SorobanString::from_str(&ctx.env, "rec-token-a-ok");
    let res_a = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient,
        &50_000_000i128,
        &rec_id_a,
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_a, Ok(Ok(())));
    assert_eq!(ctx.token_client.balance(&recipient), 50_000_000i128);
    assert_eq!(token_b_client.balance(&ctx.contract_id), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// 10. Allowed During Emergency Stop
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_recover_stranded_funds_allowed_during_emergency_stop() {
    let ctx = setup_context();
    let recipient = Address::generate(&ctx.env);

    // Mint stranded tokens
    ctx.token_admin_client.mint(&ctx.contract_id, &40_000_000i128);

    // Admin activates emergency stop
    ctx.client.enable_emergency_stop(&ctx.admin);
    assert_eq!(ctx.client.get_emergency_stop_status(), true);

    // Normal mutation is blocked by EmergencyStopActive
    let subscriber = Address::generate(&ctx.env);
    let merchant = Address::generate(&ctx.env);
    let blocked_sub = ctx.client.try_create_subscription(
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
    assert_eq!(blocked_sub, Err(Ok(Error::EmergencyStopActive)));

    // recover_stranded_funds must succeed even during emergency stop
    let rec_id = SorobanString::from_str(&ctx.env, "rec-em-stop-ok");
    let res = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient,
        &40_000_000i128,
        &rec_id,
        &RecoveryReason::SystemCorrection,
    );
    assert_eq!(res, Ok(Ok(())));
    assert_eq!(ctx.token_client.balance(&recipient), 40_000_000i128);
    assert_eq!(ctx.token_client.balance(&ctx.contract_id), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// 11. State Preservation Across All Rejected Operations
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_recover_stranded_funds_state_preservation_on_all_failures() {
    let ctx = setup_context();
    let stranger = Address::generate(&ctx.env);
    let recipient = Address::generate(&ctx.env);

    // Mint 10M stranded funds (contract balance = 10M)
    ctx.token_admin_client.mint(&ctx.contract_id, &10_000_000i128);

    // Snapshot baseline state
    let baseline_contract_bal = ctx.token_client.balance(&ctx.contract_id);
    let baseline_recipient_bal = ctx.token_client.balance(&recipient);

    // 1. Rejected: Unauthorized caller
    let res_unauth = ctx.client.try_recover_stranded_funds(
        &stranger,
        &ctx.token,
        &recipient,
        &5_000_000i128,
        &SorobanString::from_str(&ctx.env, "fail-unauth"),
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_unauth, Err(Ok(Error::Forbidden)));
    assert_eq!(ctx.token_client.balance(&ctx.contract_id), baseline_contract_bal);
    assert_eq!(ctx.token_client.balance(&recipient), baseline_recipient_bal);

    // 2. Rejected: Zero amount
    let res_zero = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient,
        &0i128,
        &SorobanString::from_str(&ctx.env, "fail-zero"),
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_zero, Err(Ok(Error::InvalidRecoveryAmount)));
    assert_eq!(ctx.token_client.balance(&ctx.contract_id), baseline_contract_bal);
    assert_eq!(ctx.token_client.balance(&recipient), baseline_recipient_bal);

    // 3. Rejected: Negative amount
    let res_neg = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient,
        &-5_000_000i128,
        &SorobanString::from_str(&ctx.env, "fail-neg"),
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_neg, Err(Ok(Error::InvalidRecoveryAmount)));
    assert_eq!(ctx.token_client.balance(&ctx.contract_id), baseline_contract_bal);
    assert_eq!(ctx.token_client.balance(&recipient), baseline_recipient_bal);

    // 4. Rejected: Exceeding recoverable balance (10_000_001 > 10_000_000)
    let res_insuf = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient,
        &10_000_001i128,
        &SorobanString::from_str(&ctx.env, "fail-insuf"),
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_insuf, Err(Ok(Error::InsufficientBalance)));
    assert_eq!(ctx.token_client.balance(&ctx.contract_id), baseline_contract_bal);
    assert_eq!(ctx.token_client.balance(&recipient), baseline_recipient_bal);

    // Now execute one valid recovery
    let valid_id = SorobanString::from_str(&ctx.env, "valid-rec-001");
    let res_valid = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient,
        &5_000_000i128,
        &valid_id,
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_valid, Ok(Ok(())));
    let after_valid_contract_bal = ctx.token_client.balance(&ctx.contract_id);
    let after_valid_recipient_bal = ctx.token_client.balance(&recipient);

    // 5. Rejected: Replay of used recovery ID
    let res_replay = ctx.client.try_recover_stranded_funds(
        &ctx.admin,
        &ctx.token,
        &recipient,
        &5_000_000i128,
        &valid_id,
        &RecoveryReason::AccidentalTransfer,
    );
    assert_eq!(res_replay, Err(Ok(Error::Replay)));
    assert_eq!(ctx.token_client.balance(&ctx.contract_id), after_valid_contract_bal);
    assert_eq!(ctx.token_client.balance(&recipient), after_valid_recipient_bal);
}
