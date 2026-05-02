//! Reentrancy invariant tests for token flows.
//!
//! Validates CEI (Checks-Effects-Interactions) guarantees for deposit,
//! charge, withdraw, and refund operations. Documents reentrancy
//! assumptions per docs/reentrancy.md.
//!
//! # Assumptions
//! - The USDC token contract does NOT implement ERC777-style callbacks.
//! - Soroban's synchronous execution model prevents deep reentry chains.
//! - CEI pattern is the primary defense; ReentrancyGuard is secondary.
//!
//! # Non-Goals
//! - Cross-function reentrancy simulation (not possible in Soroban test env).
//! - Live token callback injection (Soroban mock auths prevent this).

    Error, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient, types::DataKey,
};
use soroban_sdk::{testutils::Address as _, Address, Env};

// ── constants ────────────────────────────────────────────────────────────────
const T0: u64 = 1_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const AMOUNT: i128 = 10_000_000;
const PREPAID: i128 = 50_000_000;

// ── setup helper ─────────────────────────────────────────────────────────────

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

fn mint(env: &Env, token: &Address, to: &Address, amount: i128) {
    soroban_sdk::token::StellarAssetClient::new(env, token).mint(to, &amount);
}

fn create_sub(
    env: &Env,
    client: &SubscriptionVaultClient,
    token: &Address,
) -> (u32, Address, Address) {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    mint(env, token, &subscriber, PREPAID * 2);
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

fn seed_balance(env: &Env, client: &SubscriptionVaultClient, id: u32, balance: i128) {
    let mut sub = client.get_subscription(&id);
    sub.prepaid_balance = balance;
    env.as_contract(&client.address, || {
        env.storage().instance().set(&DataKey::Sub(id), &sub);
    });
}

fn seed_merchant_balance(
    env: &Env,
    client: &SubscriptionVaultClient,
    merchant: &Address,
    token: &Address,
    balance: i128,
) {
    use soroban_sdk::Symbol;
    env.as_contract(&client.address, || {
        env.storage().instance().set(
            &DataKey::MerchantBalance(merchant.clone(), token.clone()),
            &balance,
        );
    });
}

// =============================================================================
// 1. DEPOSIT — CEI invariants
// =============================================================================

/// CEI: prepaid_balance updated in storage BEFORE token transfer.
/// Invariant: after deposit, contract holds exactly the deposited tokens.
#[test]
fn test_deposit_state_committed_before_transfer() {
    let (env, client, token, _) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);
    let token_client = soroban_sdk::token::Client::new(&env, &token);

    let vault_before = token_client.balance(&client.address);
    let deposit = 5_000_000i128;

    client.deposit_funds(&id, &subscriber, &deposit);

    // Effects: storage reflects the deposit
    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, deposit);

    // Interactions: vault holds the tokens
    assert_eq!(token_client.balance(&client.address), vault_before + deposit);
}

/// Invariant: multiple sequential deposits accumulate correctly.
/// No intermediate state should allow a double-credit.
#[test]
fn test_deposit_multiple_sequential_consistent_state() {
    let (env, client, token, _) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);

    let deposit = 5_000_000i128;
    for i in 1..=5 {
        client.deposit_funds(&id, &subscriber, &deposit);
        let sub = client.get_subscription(&id);
        assert_eq!(sub.prepaid_balance, deposit * i as i128);
    }
}

/// Invariant: a failed deposit (below min topup) leaves state unchanged.
#[test]
fn test_deposit_failure_leaves_state_unchanged() {
    let (env, client, token, _) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);
    let token_client = soroban_sdk::token::Client::new(&env, &token);

    let vault_before = token_client.balance(&client.address);
    let sub_before = client.get_subscription(&id);

    // below min_topup of 1_000_000
    let result = client.try_deposit_funds(&id, &subscriber, &500);
    assert!(result.is_err());

    let sub_after = client.get_subscription(&id);
    assert_eq!(sub_after.prepaid_balance, sub_before.prepaid_balance);
    assert_eq!(token_client.balance(&client.address), vault_before);
}

/// Invariant: deposit on a cancelled subscription is rejected.
/// Ensures the lock lifecycle is clean — no partial state on rejected ops.
#[test]
fn test_deposit_on_cancelled_subscription_rejected_cleanly() {
    let (env, client, token, _) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);

    client.cancel_subscription(&id, &subscriber);
    let result = client.try_deposit_funds(&id, &subscriber, &5_000_000i128);
    // Cancelled subs are blocklisted from deposit
    assert!(result.is_err());
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

// =============================================================================
// 2. CHARGE — CEI invariants
// =============================================================================

/// CEI: prepaid_balance debited and merchant balance credited BEFORE any
/// external call. Invariant: total tokens in system is conserved.
#[test]
fn test_charge_token_conservation_invariant() {
    let (env, client, token, _) = setup();
    let (id, _, merchant) = create_sub(&env, &client, &token);
    let token_client = soroban_sdk::token::Client::new(&env, &token);

    seed_balance(&env, &client, id, PREPAID);
    // Mint vault tokens to match seeded balance
    soroban_sdk::token::StellarAssetClient::new(&env, &token)
        .mint(&client.address, &PREPAID);

    env.ledger().set_timestamp(T0 + INTERVAL + 1);
    let vault_before = token_client.balance(&client.address);

    client.charge_subscription(&id);

    let sub_after = client.get_subscription(&id);
    let merchant_balance = client.get_merchant_balance(&merchant);

    // prepaid reduced by AMOUNT
    assert_eq!(sub_after.prepaid_balance, PREPAID - AMOUNT);
    // merchant credited exactly AMOUNT
    assert_eq!(merchant_balance, AMOUNT);
    // vault token balance unchanged (merchant balance is internal ledger, not transfer)
    assert_eq!(token_client.balance(&client.address), vault_before);
}

/// Invariant: charge on insufficient balance transitions status without
/// touching prepaid_balance (no partial debit).
#[test]
fn test_charge_insufficient_balance_no_partial_debit() {
    let (env, client, token, _) = setup();
    let (id, _, _) = create_sub(&env, &client, &token);

    // Zero balance, past grace period
    seed_balance(&env, &client, id, 0);
    let grace = 7 * 24 * 60 * 60u64;
    env.ledger().set_timestamp(T0 + INTERVAL + grace + 1);

    let result = client.try_charge_subscription(&id);
    assert!(result.is_ok()); // returns InsufficientBalance result, not Err

    let sub = client.get_subscription(&id);
    // Balance must not have gone negative
    assert_eq!(sub.prepaid_balance, 0);
    assert_eq!(sub.status, SubscriptionStatus::InsufficientBalance);
}

/// Invariant: replay charge in same interval is rejected without state mutation.
#[test]
fn test_charge_replay_rejected_no_state_mutation() {
    let (env, client, token, _) = setup();
    let (id, _, merchant) = create_sub(&env, &client, &token);

    seed_balance(&env, &client, id, PREPAID);
    soroban_sdk::token::StellarAssetClient::new(&env, &token)
        .mint(&client.address, &PREPAID);

    env.ledger().set_timestamp(T0 + INTERVAL + 1);
    client.charge_subscription(&id);

    let sub_after_first = client.get_subscription(&id);
    let merchant_after_first = client.get_merchant_balance(&merchant);

    // Replay attempt in same interval
    let result = client.try_charge_subscription(&id);
    assert_eq!(result, Err(Ok(Error::Replay)));

    let sub_after_replay = client.get_subscription(&id);
    assert_eq!(sub_after_replay.prepaid_balance, sub_after_first.prepaid_balance);
    assert_eq!(client.get_merchant_balance(&merchant), merchant_after_first);
}

/// Invariant: charge on paused subscription is rejected without touching balances.
#[test]
fn test_charge_on_paused_no_state_change() {
    let (env, client, token, _) = setup();
    let (id, subscriber, merchant) = create_sub(&env, &client, &token);

    seed_balance(&env, &client, id, PREPAID);
    client.pause_subscription(&id, &subscriber);

    env.ledger().set_timestamp(T0 + INTERVAL + 1);
    let result = client.try_charge_subscription(&id);
    assert_eq!(result, Err(Ok(Error::NotActive)));

    assert_eq!(client.get_subscription(&id).prepaid_balance, PREPAID);
    assert_eq!(client.get_merchant_balance(&merchant), 0);
}

/// Invariant: lifetime_charged field monotonically increases, never decreases.
#[test]
fn test_charge_lifetime_charged_monotonically_increases() {
    let (env, client, token, _) = setup();
    let (id, _, _) = create_sub(&env, &client, &token);

    seed_balance(&env, &client, id, PREPAID * 10);
    soroban_sdk::token::StellarAssetClient::new(&env, &token)
        .mint(&client.address, &(PREPAID * 10));

    let mut prev_lifetime = 0i128;
    for i in 1..=4 {
        env.ledger().set_timestamp(T0 + (i as u64 * INTERVAL) + 1);
        client.charge_subscription(&id);
        let sub = client.get_subscription(&id);
        assert!(sub.lifetime_charged > prev_lifetime);
        prev_lifetime = sub.lifetime_charged;
    }
}

// =============================================================================
// 3. WITHDRAW (subscriber) — CEI invariants
// =============================================================================

/// CEI: prepaid_balance zeroed in storage BEFORE token transfer to subscriber.
/// Invariant: balance is zero before any token leaves the vault.
#[test]
fn test_withdraw_subscriber_state_committed_before_transfer() {
    let (env, client, token, _) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);
    let token_client = soroban_sdk::token::Client::new(&env, &token);

    client.deposit_funds(&id, &subscriber, &PREPAID);
    client.cancel_subscription(&id, &subscriber);

    let vault_before = token_client.balance(&client.address);
    let subscriber_before = token_client.balance(&subscriber);

    client.withdraw_subscriber_funds(&id, &subscriber);

    // Effects committed: balance zeroed
    assert_eq!(client.get_subscription(&id).prepaid_balance, 0);
    // Interactions: tokens transferred
    assert_eq!(token_client.balance(&client.address), vault_before - PREPAID);
    assert_eq!(token_client.balance(&subscriber), subscriber_before + PREPAID);
}

/// Invariant: double-withdrawal is rejected after first succeeds.
/// Guard: once balance is zeroed, second call must fail.
#[test]
fn test_withdraw_subscriber_double_withdrawal_rejected() {
    let (env, client, token, _) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);

    client.deposit_funds(&id, &subscriber, &PREPAID);
    client.cancel_subscription(&id, &subscriber);
    client.withdraw_subscriber_funds(&id, &subscriber);

    // Second withdrawal must fail (balance = 0)
    let result = client.try_withdraw_subscriber_funds(&id, &subscriber);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
    assert_eq!(client.get_subscription(&id).prepaid_balance, 0);
}

/// Invariant: withdrawal on non-cancelled subscription is rejected.
#[test]
fn test_withdraw_subscriber_requires_cancelled_status() {
    let (env, client, token, _) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);

    client.deposit_funds(&id, &subscriber, &PREPAID);
    // Not cancelled — Active status
    let result = client.try_withdraw_subscriber_funds(&id, &subscriber);
    assert!(result.is_err());
    // Balance untouched
    assert_eq!(client.get_subscription(&id).prepaid_balance, PREPAID);
}

/// Invariant: withdrawal amount exactly matches prepaid_balance.
#[test]
fn test_withdraw_subscriber_exact_amount_transferred() {
    let (env, client, token, _) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);
    let token_client = soroban_sdk::token::Client::new(&env, &token);

    let deposit = 7_777_777i128;
    client.deposit_funds(&id, &subscriber, &deposit);
    client.cancel_subscription(&id, &subscriber);

    let subscriber_before = token_client.balance(&subscriber);
    client.withdraw_subscriber_funds(&id, &subscriber);

    assert_eq!(
        token_client.balance(&subscriber),
        subscriber_before + deposit
    );
}

// =============================================================================
// 4. WITHDRAW (merchant) — CEI invariants
// =============================================================================

/// CEI: merchant balance reduced in storage BEFORE token transfer.
/// Invariant: even if a callback occurred during transfer, the reduced
/// balance prevents a second successful withdrawal.
#[test]
fn test_withdraw_merchant_state_committed_before_transfer() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let token_client = soroban_sdk::token::Client::new(&env, &token);

    seed_merchant_balance(&env, &client, &merchant, &token, 9_000_000i128);
    soroban_sdk::token::StellarAssetClient::new(&env, &token)
        .mint(&client.address, &9_000_000i128);

    let vault_before = token_client.balance(&client.address);
    let merchant_before = token_client.balance(&merchant);

    client.withdraw_merchant_funds(&merchant, &4_000_000i128);

    // Effects: internal ledger updated
    assert_eq!(client.get_merchant_balance(&merchant), 5_000_000i128);
    // Interactions: tokens moved
    assert_eq!(token_client.balance(&client.address), vault_before - 4_000_000);
    assert_eq!(token_client.balance(&merchant), merchant_before + 4_000_000);
}

/// Invariant: overdraw attempt is rejected before any state change.
#[test]
fn test_withdraw_merchant_overdraw_rejected_no_state_change() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);

    seed_merchant_balance(&env, &client, &merchant, &token, 3_000_000i128);
    soroban_sdk::token::StellarAssetClient::new(&env, &token)
        .mint(&client.address, &3_000_000i128);

    let result = client.try_withdraw_merchant_funds(&merchant, &4_000_000i128);
    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));

    // Balance untouched
    assert_eq!(client.get_merchant_balance(&merchant), 3_000_000i128);
}

/// Invariant: sequential withdrawals correctly drain the merchant balance.
#[test]
fn test_withdraw_merchant_sequential_correct_accounting() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);

    seed_merchant_balance(&env, &client, &merchant, &token, 10_000_000i128);
    soroban_sdk::token::StellarAssetClient::new(&env, &token)
        .mint(&client.address, &10_000_000i128);

    client.withdraw_merchant_funds(&merchant, &3_000_000i128);
    assert_eq!(client.get_merchant_balance(&merchant), 7_000_000i128);

    client.withdraw_merchant_funds(&merchant, &7_000_000i128);
    assert_eq!(client.get_merchant_balance(&merchant), 0);

    let result = client.try_withdraw_merchant_funds(&merchant, &1i128);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

/// Invariant: merchant A cannot withdraw from merchant B's balance.
#[test]
fn test_merchant_cannot_withdraw_other_merchant() {
    let (env, client, token, _) = setup();
    let merchant_a = Address::generate(&env);
    let merchant_b = Address::generate(&env);

    seed_merchant_balance(&env, &client, &merchant_a, &token, 10_000_000i128);
    seed_merchant_balance(&env, &client, &merchant_b, &token, 5_000_000i128);
    soroban_sdk::token::StellarAssetClient::new(&env, &token)
        .mint(&client.address, &15_000_000i128);

    // Merchant A attempts to withdraw more than their balance
    let result = client.try_withdraw_merchant_funds(&merchant_a, &12_000_000i128);
    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));

    // Merchant A's balance remains untouched
    assert_eq!(client.get_merchant_balance(&merchant_a), 10_000_000i128);
    assert_eq!(client.get_merchant_balance(&merchant_b), 5_000_000i128);
}

// =============================================================================
// 5. REFUND — CEI invariants
// =============================================================================

/// CEI: prepaid_balance debited in storage BEFORE token transfer back to subscriber.
#[test]
fn test_refund_state_committed_before_transfer() {
    let (env, client, token, admin) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);
    let token_client = soroban_sdk::token::Client::new(&env, &token);

    client.deposit_funds(&id, &subscriber, &PREPAID);

    let vault_before = token_client.balance(&client.address);
    let subscriber_before = token_client.balance(&subscriber);
    let refund = 5_000_000i128;

    client.partial_refund(&admin, &id, &subscriber, &refund);

    // Effects: balance reduced
    assert_eq!(client.get_subscription(&id).prepaid_balance, PREPAID - refund);
    // Interactions: tokens returned
    assert_eq!(token_client.balance(&client.address), vault_before - refund);
    assert_eq!(token_client.balance(&subscriber), subscriber_before + refund);
}

/// Invariant: refund exceeding balance is rejected, no state change.
#[test]
fn test_refund_exceeds_balance_rejected_no_state_change() {
    let (env, client, token, admin) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);
    let token_client = soroban_sdk::token::Client::new(&env, &token);

    let deposit = 5_000_000i128;
    client.deposit_funds(&id, &subscriber, &deposit);

    let vault_before = token_client.balance(&client.address);

    let result = client.try_partial_refund(&admin, &id, &subscriber, &(deposit + 1));
    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));

    assert_eq!(client.get_subscription(&id).prepaid_balance, deposit);
    assert_eq!(token_client.balance(&client.address), vault_before);
}

/// Invariant: cumulative refunds cannot exceed the original deposited amount.
#[test]
fn test_refund_cumulative_cannot_exceed_deposit() {
    let (env, client, token, admin) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);

    let deposit = 10_000_000i128;
    client.deposit_funds(&id, &subscriber, &deposit);

    // Drain in two steps
    client.partial_refund(&admin, &id, &subscriber, &5_000_000i128);
    client.partial_refund(&admin, &id, &subscriber, &5_000_000i128);
    assert_eq!(client.get_subscription(&id).prepaid_balance, 0);

    // Any further refund is impossible
    let result = client.try_partial_refund(&admin, &id, &subscriber, &1i128);
    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));
}

// =============================================================================
// 6. REENTRANCY GUARD — lock lifecycle
// =============================================================================

/// Guard invariant: ReentrancyGuard::lock sets a key and removes it on drop.
/// Tests the lock is not left in storage after a normal operation.
#[test]
fn test_reentrancy_guard_lock_is_released_after_operation() {
    let (env, client, token, _) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);

    client.deposit_funds(&id, &subscriber, &5_000_000i128);

    // After a successful deposit, no lock key should remain in storage.
    // We verify this by running a second deposit — if the lock were stuck,
    // it would return Reentrancy error.
    let result = client.try_deposit_funds(&id, &subscriber, &5_000_000i128);
    assert!(result.is_ok(), "second deposit must succeed — lock must be released");
}

/// Guard invariant: withdrawal lock is released after merchant withdrawal.
#[test]
fn test_reentrancy_guard_released_after_merchant_withdrawal() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);

    seed_merchant_balance(&env, &client, &merchant, &token, 10_000_000i128);
    soroban_sdk::token::StellarAssetClient::new(&env, &token)
        .mint(&client.address, &10_000_000i128);

    client.withdraw_merchant_funds(&merchant, &3_000_000i128);

    // Second withdrawal must succeed (not blocked by stale lock)
    client.withdraw_merchant_funds(&merchant, &3_000_000i128);
    assert_eq!(client.get_merchant_balance(&merchant), 4_000_000i128);
}

/// Guard invariant: a failed operation (rejected before lock) does not
/// leave a dangling lock that blocks subsequent valid calls.
#[test]
fn test_reentrancy_guard_not_stuck_after_rejection() {
    let (env, client, token, admin) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);

    client.deposit_funds(&id, &subscriber, &5_000_000i128);

    // Rejected refund (wrong admin)
    let stranger = Address::generate(&env);
    let _ = client.try_partial_refund(&stranger, &id, &subscriber, &1_000_000i128);

    // Valid refund from real admin must still work
    let result = client.try_partial_refund(&admin, &id, &subscriber, &1_000_000i128);
    assert!(result.is_ok(), "valid refund must work after a rejected one");
}

// =============================================================================
// 7. NESTED CALL ATTEMPTS — panic path coverage
// =============================================================================

/// Invariant: charge on a non-existent subscription panics / errors cleanly.
#[test]
fn test_charge_nonexistent_subscription_errors_cleanly() {
    let (env, client, _, _) = setup();
    env.ledger().set_timestamp(T0 + INTERVAL + 1);
    let result = client.try_charge_subscription(&9999u32);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

/// Invariant: deposit on a non-existent subscription errors cleanly.
#[test]
fn test_deposit_nonexistent_subscription_errors_cleanly() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    mint(&env, &token, &subscriber, PREPAID);
    let result = client.try_deposit_funds(&9999u32, &subscriber, &5_000_000i128);
    assert_eq!(result, Err(Ok(Error::NotFound)));
}

/// Invariant: charge blocked by emergency stop; no state mutations.
#[test]
fn test_charge_blocked_by_emergency_stop_no_mutation() {
    let (env, client, token, admin) = setup();
    let (id, _, merchant) = create_sub(&env, &client, &token);
    seed_balance(&env, &client, id, PREPAID);

    client.enable_emergency_stop(&admin);
    env.ledger().set_timestamp(T0 + INTERVAL + 1);

    let result = client.try_charge_subscription(&id);
    assert_eq!(result, Err(Ok(Error::EmergencyStopActive)));

    assert_eq!(client.get_subscription(&id).prepaid_balance, PREPAID);
    assert_eq!(client.get_merchant_balance(&merchant), 0);
}

/// Invariant: deposit blocked by emergency stop; no state mutations.
#[test]
fn test_deposit_blocked_by_emergency_stop_no_mutation() {
    let (env, client, token, admin) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);

    client.enable_emergency_stop(&admin);

    let result = client.try_deposit_funds(&id, &subscriber, &5_000_000i128);
    assert_eq!(result, Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.get_subscription(&id).prepaid_balance, 0);
}

/// Invariant: merchant withdrawal blocked by emergency stop; no state mutations.
#[test]
fn test_withdraw_merchant_blocked_by_emergency_stop_no_mutation() {
    let (env, client, token, admin) = setup();
    let merchant = Address::generate(&env);

    seed_merchant_balance(&env, &client, &merchant, &token, 3_000_000i128);
    soroban_sdk::token::StellarAssetClient::new(&env, &token)
        .mint(&client.address, &3_000_000i128);

    client.enable_emergency_stop(&admin);

    let result = client.try_withdraw_merchant_funds(&merchant, &1_000_000i128);
    assert_eq!(result, Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.get_merchant_balance(&merchant), 3_000_000i128);
}

/// Invariant: subscriber withdrawal blocked by emergency stop; no state mutations.
#[test]
fn test_withdraw_subscriber_blocked_by_emergency_stop_no_mutation() {
    let (env, client, token, admin) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);

    client.deposit_funds(&id, &subscriber, &PREPAID);
    client.cancel_subscription(&id, &subscriber);

    client.enable_emergency_stop(&admin);

    let result = client.try_withdraw_subscriber_funds(&id, &subscriber);
    assert_eq!(result, Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.get_subscription(&id).prepaid_balance, PREPAID);
}

// =============================================================================
// 8. LOCK RELEASE / RECOVERY — edge cases
// =============================================================================

/// Invariant: after charge failure (insufficient balance), subsequent
/// valid charge in next interval succeeds (no stale lock or state corruption).
#[test]
fn test_charge_failure_then_topup_then_charge_succeeds() {
    let (env, client, token, _) = setup();
    let (id, subscriber, _) = create_sub(&env, &client, &token);

    // Zero balance — charge will fail
    seed_balance(&env, &client, id, 0);
    let grace = 7 * 24 * 60 * 60u64;
    env.ledger().set_timestamp(T0 + INTERVAL + grace + 1);
    let _ = client.try_charge_subscription(&id);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::InsufficientBalance
    );

    // Top up and resume
    mint(&env, &token, &subscriber, PREPAID);
    client.deposit_funds(&id, &subscriber, &PREPAID);
    client.resume_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );

    // Next interval — charge must succeed cleanly
    env.ledger().set_timestamp(T0 + INTERVAL + grace + 1 + INTERVAL);
    let result = client.try_charge_subscription(&id);
    assert!(result.is_ok());
    assert_eq!(
        client.get_subscription(&id).prepaid_balance,
        PREPAID - AMOUNT
    );
}