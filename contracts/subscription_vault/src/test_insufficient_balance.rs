//! Invariant tests for deposit_funds: insufficient token balance and credit limit enforcement.
//!
//! Verifies that:
//! 1. When the subscriber lacks token balance, the deposit reverts without mutating state.
//! 2. When the subscriber has a credit limit, a deposit exceeding it is rejected cleanly.
//!
//! # CEI Invariant
//! Both tests validate that `prepaid_balance` is never modified when the operation fails,
//! confirming the Checks-Effects-Interactions pattern is followed.

use crate::{
    Error, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient, types::DataKey,
};
use soroban_sdk::{testutils::Address as _, Address, Env};

// ── constants ────────────────────────────────────────────────────────────────
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const AMOUNT: i128 = 10_000_000;
const PREPAID: i128 = 50_000_000;

// ── setup helpers ────────────────────────────────────────────────────────────

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    (env, client, token, admin)
}

fn create_sub(
    env: &Env,
    client: &SubscriptionVaultClient,
    token: &Address,
) -> (u32, Address, Address) {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    // Do NOT mint tokens to subscriber — tests verify behavior with zero balance
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    (id, subscriber, merchant)
}

// =============================================================================
// 1. Insufficient token balance — revert atomicity
// =============================================================================

/// Invariant: deposit_funds reverts cleanly when the subscriber has zero token
/// balance. prepaid_balance must NOT be modified by a failed deposit.
#[test]
fn test_deposit_insufficient_token_balance_reverts() {
    let (env, client, token, _) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);
    let token_client = soroban_sdk::token::Client::new(&env, &token);

    let vault_before = token_client.balance(&client.address);
    let sub_before = client.get_subscription(&id);

    // Subscriber has 0 tokens — the token.transfer inside deposit_funds must revert
    let result = client.try_deposit_funds(&id, &subscriber, &5_000_000i128, &None::<soroban_sdk::BytesN<32>>);
    assert!(result.is_err(), "deposit with zero subscriber balance must fail");

    // State invariant: prepaid_balance and vault balance unchanged
    let sub_after = client.get_subscription(&id);
    assert_eq!(
        sub_after.prepaid_balance,
        sub_before.prepaid_balance,
        "prepaid_balance must not change on failed deposit"
    );
    assert_eq!(
        token_client.balance(&client.address),
        vault_before,
        "vault token balance must not change on failed deposit"
    );
}

/// Invariant: deposit_funds reverts when subscriber has some tokens but not enough
/// to cover the full deposit amount. No partial state mutation.
#[test]
fn test_deposit_insufficient_partial_balance_reverts() {
    let (env, client, token, _) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);
    let token_client = soroban_sdk::token::Client::new(&env, &token);

    // Mint just enough for authorization but not the full deposit
    let short_amount = 1_000i128;
    soroban_sdk::token::StellarAssetClient::new(&env, &token)
        .mint(&subscriber, &short_amount);

    let vault_before = token_client.balance(&client.address);
    let sub_before = client.get_subscription(&id);

    // Try to deposit more than the subscriber holds
    let result = client.try_deposit_funds(&id, &subscriber, &5_000_000i128, &None::<soroban_sdk::BytesN<32>>);
    assert!(result.is_err(), "deposit exceeding subscriber balance must fail");

    // State invariant: nothing changed
    let sub_after = client.get_subscription(&id);
    assert_eq!(
        sub_after.prepaid_balance,
        sub_before.prepaid_balance,
        "prepaid_balance must not change on failed deposit"
    );
    assert_eq!(
        token_client.balance(&client.address),
        vault_before,
        "vault token balance must not change on failed deposit"
    );
}

// =============================================================================
// 2. Credit limit enforcement — deposit rejection
// =============================================================================

/// Invariant: deposit_funds is rejected when the subscriber has a credit limit
/// set and the deposit would cause aggregate exposure to exceed that limit.
#[test]
fn test_deposit_rejected_when_credit_limit_exceeded() {
    let (env, client, token, admin) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);

    // Set a very low credit limit: 1_000_000
    // Current exposure = 0 (prepaid) + AMOUNT (next interval liability for active sub)
    // AMOUNT = 10_000_000 already exceeds 1_000_000, so any deposit fails.
    client.set_subscriber_credit_limit(&admin, &subscriber, &token, &1_000_000i128);

    let sub_before = client.get_subscription(&id);

    // Deposit min_topup (1_000_000) should be rejected since
    // exposure (10_000_000) + 1_000_000 > limit (1_000_000)
    let result = client.try_deposit_funds(&id, &subscriber, &1_000_000i128, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(
        result,
        Err(Ok(Error::CreditLimitExceeded)),
        "deposit must be rejected when credit limit is exceeded"
    );

    // State unchanged
    let sub_after = client.get_subscription(&id);
    assert_eq!(
        sub_after.prepaid_balance,
        sub_before.prepaid_balance,
        "prepaid_balance must not change on credit-limit rejection"
    );
}

/// Invariant: deposit is allowed when credit limit is set but not exceeded.
#[test]
fn test_deposit_allowed_within_credit_limit() {
    let (env, client, token, admin) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);
    let token_client = soroban_sdk::token::Client::new(&env, &token);

    // Mint enough tokens to the subscriber
    soroban_sdk::token::StellarAssetClient::new(&env, &token)
        .mint(&subscriber, &PREPAID);

    // Set a generous credit limit: 100_000_000
    // Exposure = 0 (prepaid) + AMOUNT (10_000_000) = 10_000_000
    // Depositing PREPAID (50_000_000) → new exposure = 60_000_000 < 100_000_000, OK
    client.set_subscriber_credit_limit(&admin, &subscriber, &token, &100_000_000i128);

    let vault_before = token_client.balance(&client.address);

    let result = client.try_deposit_funds(&id, &subscriber, &PREPAID, &None::<soroban_sdk::BytesN<32>>);
    assert!(result.is_ok(), "deposit within credit limit should succeed");

    // Verify deposit was applied
    let sub_after = client.get_subscription(&id);
    assert_eq!(sub_after.prepaid_balance, PREPAID);
    assert_eq!(
        token_client.balance(&client.address),
        vault_before + PREPAID
    );
}

/// Invariant: credit limit enforcement considers aggregate exposure across
/// multiple subscriptions. Creating a second subscription reduces remaining capacity.
#[test]
fn test_deposit_credit_limit_aggregate_two_subs() {
    let (env, client, token, admin) = setup();
    let (id1, subscriber, _) = create_sub(&env, &client, &token);

    // Create a second subscription for the same subscriber
    let merchant2 = Address::generate(&env);
    let id2 = client.create_subscription(
        &subscriber,
        &merchant2,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    // Set credit limit: 15_000_000
    // Two active subs → exposure = 0 + AMOUNT (10M) + 0 + AMOUNT (10M) = 20_000_000
    // Already exceeds 15_000_000, so any deposit should fail
    client.set_subscriber_credit_limit(&admin, &subscriber, &token, &15_000_000i128);

    let sub1_before = client.get_subscription(&id1);
    let sub2_before = client.get_subscription(&id2);

    let result = client.try_deposit_funds(&id1, &subscriber, &1_000_000i128, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(
        result,
        Err(Ok(Error::CreditLimitExceeded)),
        "deposit must fail when aggregate exposure exceeds limit"
    );

    // Neither subscription's balance changed
    assert_eq!(
        client.get_subscription(&id1).prepaid_balance,
        sub1_before.prepaid_balance
    );
    assert_eq!(
        client.get_subscription(&id2).prepaid_balance,
        sub2_before.prepaid_balance
    );
}
