//! Grace-period buyout tests.
//!
//! Verifies the `grace_buyout` entrypoint which combines deposit + charge
//! in one atomic call, allowing a subscriber in GracePeriod to top-up
//! enough funds (charge + premium) and immediately return to Active.

use crate::{
    DataKey, Error, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env,
};

const T0: u64 = 1_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const GRACE_PERIOD: u64 = 7 * 24 * 60 * 60; // 7 days
const AMOUNT: i128 = 10_000_000; // 10 USDC per interval

fn setup() -> (
    Env,
    SubscriptionVaultClient<'static>,
    Address,
    token::StellarAssetClient<'static>,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = T0);

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token_admin_addr = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(token_admin_addr.clone());
    let token_admin = token::StellarAssetClient::new(&env, &token_id.address());

    client.init(
        &token_id.address(),
        &6,
        &admin,
        &1_000_000i128,
        &GRACE_PERIOD,
    );

    (env, client, token_id.address(), token_admin)
}

/// Create a subscription and return (id, subscriber, merchant).
fn create_sub(
    env: &Env,
    client: &SubscriptionVaultClient,
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
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    (id, subscriber, merchant)
}

/// Set the buyout premium in basis points via direct storage write.
fn set_buyout_premium(env: &Env, client: &SubscriptionVaultClient, bps: u32) {
    env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .set(&DataKey::BuyoutPremiumBps, &bps);
    });
}

/// Directly overwrite the prepaid_balance in storage.
fn set_balance(env: &Env, client: &SubscriptionVaultClient, id: u32, balance: i128) {
    let mut sub = client.get_subscription(&id);
    sub.prepaid_balance = balance;
    env.as_contract(&client.address, || {
        env.storage().persistent().set(&DataKey::Sub(id), &sub);
    });
}

/// Force a subscription into GracePeriod by attempting a charge with
/// insufficient balance.
fn force_into_grace_period(
    env: &Env,
    client: &SubscriptionVaultClient,
    id: u32,
) {
    // Set balance to 0 so the charge fails and enters GracePeriod.
    set_balance(env, client, id, 0);
    env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    let _ = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);

    let sub = client.get_subscription(&id);
    assert_eq!(
        sub.status,
        SubscriptionStatus::GracePeriod,
        "subscription should be in GracePeriod after failed charge"
    );
}

// ── Happy path ──────────────────────────────────────────────────────────────

/// Buyout with 500 bps (5%) premium: deposit = charge + 5% premium.
#[test]
fn test_grace_buyout_happy_path() {
    let (env, client, _token, token_admin) = setup();
    let (id, subscriber, _merchant) = create_sub(&env, &client);

    set_buyout_premium(&env, &client, 500); // 5%

    // Mint tokens for the buyout deposit.
    let deposit = AMOUNT + AMOUNT * 5 / 100; // charge + 5% premium
    token_admin.mint(&subscriber, &deposit);

    force_into_grace_period(&env, &client, id);

    let balance_before = client.get_subscription(&id).prepaid_balance;

    let res = client.try_grace_buyout(&id, &subscriber, &deposit, &None::<soroban_sdk::BytesN<32>>);
    assert!(res.is_ok(), "buyout must succeed");

    let (charge_amount, premium_paid) = res.unwrap().unwrap();
    assert_eq!(charge_amount, AMOUNT);
    assert_eq!(premium_paid, AMOUNT * 5 / 100);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(sub.prepaid_balance, balance_before + deposit - charge_amount);
}

// ── Reject: not in GracePeriod ──────────────────────────────────────────────

/// Buyout must reject when subscription is Active (not GracePeriod).
#[test]
fn test_grace_buyout_rejects_active_subscription() {
    let (env, client, _token, token_admin) = setup();
    let (id, subscriber, _merchant) = create_sub(&env, &client);

    let deposit = AMOUNT * 2;
    token_admin.mint(&subscriber, &deposit);

    // Subscription is still Active — buyout should be rejected.
    let res = client.try_grace_buyout(&id, &subscriber, &deposit, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(res, Err(Ok(Error::NotInGracePeriod)));
}

// ── Reject: insufficient deposit ────────────────────────────────────────────

/// Buyout must reject when deposit < charge + premium.
#[test]
fn test_grace_buyout_rejects_insufficient_deposit() {
    let (env, client, _token, token_admin) = setup();
    let (id, subscriber, _merchant) = create_sub(&env, &client);

    set_buyout_premium(&env, &client, 500); // 5%

    force_into_grace_period(&env, &client, id);

    // Deposit only the charge amount, not enough to cover premium.
    let deposit = AMOUNT; // missing the 5% premium
    token_admin.mint(&subscriber, &deposit);

    let res = client.try_grace_buyout(&id, &subscriber, &deposit, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(res, Err(Ok(Error::InsufficientBalance)));
}

// ── Edge: deposit exactly equal to charge (premium bps = 0) ─────────────────

/// When premium is 0 bps, deposit == charge should succeed.
#[test]
fn test_grace_buyout_zero_premium_exact_amount() {
    let (env, client, _token, token_admin) = setup();
    let (id, subscriber, _merchant) = create_sub(&env, &client);

    set_buyout_premium(&env, &client, 0); // no premium

    force_into_grace_period(&env, &client, id);

    let deposit = AMOUNT;
    token_admin.mint(&subscriber, &deposit);

    let res = client.try_grace_buyout(&id, &subscriber, &deposit, &None::<soroban_sdk::BytesN<32>>);
    assert!(res.is_ok(), "buyout with zero premium must succeed");

    let (charge_amount, premium_paid) = res.unwrap().unwrap();
    assert_eq!(charge_amount, AMOUNT);
    assert_eq!(premium_paid, 0);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

// ── Edge: premium bps = 0, deposit > charge (extra goes to balance) ─────────

/// When premium is 0 and deposit exceeds charge, the excess stays in balance.
#[test]
fn test_grace_buyout_zero_premium_excess_stays() {
    let (env, client, _token, token_admin) = setup();
    let (id, subscriber, _merchant) = create_sub(&env, &client);

    set_buyout_premium(&env, &client, 0);

    force_into_grace_period(&env, &client, id);

    let deposit = AMOUNT * 3;
    token_admin.mint(&subscriber, &deposit);

    let res = client.try_grace_buyout(&id, &subscriber, &deposit, &None::<soroban_sdk::BytesN<32>>);
    assert!(res.is_ok());

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    // prepaid_balance = 0 + 3*AMOUNT - AMOUNT = 2*AMOUNT
    assert_eq!(sub.prepaid_balance, AMOUNT * 2);
}

// ── Edge: premium overflow ──────────────────────────────────────────────────

/// Premium calculation with very large charge_amount and premium_bps
/// must not silently overflow.
#[test]
fn test_grace_buyout_premium_overflow() {
    let (env, client, _token, _token_admin) = setup();
    let (id, subscriber, _merchant) = create_sub(&env, &client);

    // Set an absurdly high premium bps that would overflow when multiplied
    // by a non-trivial charge amount.
    set_buyout_premium(&env, &client, u32::MAX);

    force_into_grace_period(&env, &client, id);

    // Deposit a large amount — the premium calculation should overflow.
    let deposit = i128::MAX;
    let res = client.try_grace_buyout(&id, &subscriber, &deposit, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(res, Err(Ok(Error::Overflow)));
}

// ── Edge: rejected buyout does not mutate state ─────────────────────────────

/// A failed buyout must not change subscription status or balance.
#[test]
fn test_grace_buyout_rejected_is_idempotent() {
    let (env, client, _token, token_admin) = setup();
    let (id, subscriber, _merchant) = create_sub(&env, &client);

    set_buyout_premium(&env, &client, 500);

    force_into_grace_period(&env, &client, id);

    let sub_before = client.get_subscription(&id);

    // Try with insufficient deposit — should fail.
    let deposit = AMOUNT; // missing premium
    token_admin.mint(&subscriber, &deposit);
    let _ = client.try_grace_buyout(&id, &subscriber, &deposit, &None::<soroban_sdk::BytesN<32>>);

    let sub_after = client.get_subscription(&id);
    assert_eq!(sub_after.status, sub_before.status);
    assert_eq!(sub_after.prepaid_balance, sub_before.prepaid_balance);
    assert_eq!(sub_after.lifetime_charged, sub_before.lifetime_charged);
}

// ── Back-to-back: buyout then normal charge ─────────────────────────────────

/// After a buyout, the subscription returns to Active and a normal
/// charge at the next interval must succeed.
#[test]
fn test_grace_buyout_then_normal_charge() {
    let (env, client, _token, token_admin) = setup();
    let (id, subscriber, _merchant) = create_sub(&env, &client);

    set_buyout_premium(&env, &client, 200); // 2%

    let deposit = AMOUNT + AMOUNT * 2 / 100;
    token_admin.mint(&subscriber, &(deposit * 2));

    force_into_grace_period(&env, &client, id);

    // Buyout at T0 + INTERVAL.
    let res = client.try_grace_buyout(&id, &subscriber, &deposit, &None::<soroban_sdk::BytesN<32>>);
    assert!(res.is_ok());

    // Normal charge at T0 + 2*INTERVAL.
    env.ledger().with_mut(|l| l.timestamp = T0 + 2 * INTERVAL);
    let res2 = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
    assert!(res2.is_ok(), "normal charge after buyout must succeed");
    assert_eq!(res2.unwrap(), Ok(crate::ChargeExecutionResult::Charged));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

// ── Edge: buyout with existing prepaid balance ──────────────────────────────

/// Buyout when subscription already has some prepaid balance.
#[test]
fn test_grace_buyout_with_existing_balance() {
    let (env, client, _token, token_admin) = setup();
    let (id, subscriber, _merchant) = create_sub(&env, &client);

    set_buyout_premium(&env, &client, 100); // 1%

    // Give some balance before entering grace period.
    token_admin.mint(&subscriber, &AMOUNT);
    client.deposit_funds(&id, &subscriber, &AMOUNT, &None::<soroban_sdk::BytesN<32>>);

    force_into_grace_period(&env, &client, id);

    let sub = client.get_subscription(&id);
    let existing_balance = sub.prepaid_balance;

    let deposit = AMOUNT + AMOUNT / 100; // charge + 1% premium
    token_admin.mint(&subscriber, &deposit);

    let res = client.try_grace_buyout(&id, &subscriber, &deposit, &None::<soroban_sdk::BytesN<32>>);
    assert!(res.is_ok());

    let sub_after = client.get_subscription(&id);
    assert_eq!(sub_after.status, SubscriptionStatus::Active);
    assert_eq!(
        sub_after.prepaid_balance,
        existing_balance + deposit - AMOUNT
    );
}
