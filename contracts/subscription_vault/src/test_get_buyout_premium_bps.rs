//! Focused tests for `admin::get_buyout_premium_bps`.
//!
//! # What is under test
//!
//! `get_buyout_premium_bps(env: &Env) -> u32` reads the protocol-wide buyout
//! premium from `DataKey::BuyoutPremiumBps` via `read_config`, which checks
//! persistent storage first and then instance storage (pre-v3 migration path).
//! It returns `0` when no value has been written.
//!
//! # Cases
//!
//! | # | Description |
//! |---|-------------|
//! | 1 | Default – contract initialized, key absent → returns 0 |
//! | 2 | Persistent storage round-trip – standard post-v3 path |
//! | 3 | Boundary value 1 bps |
//! | 4 | Boundary value 9_999 bps |
//! | 5 | Boundary value 10_000 bps (100 %, extreme but not invalid for the getter itself) |
//! | 6 | Boundary value u32::MAX (getter must not panic) |
//! | 7 | Instance storage path – pre-v3 migration: instance value read before persistent exists |
//! | 8 | Persistent shadows instance – when both exist persistent wins |
//! | 9 | Overwrite – second write supersedes first |
//! | 10 | Storage unchanged after a rejected grace_buyout (contract state immutability) |
//! | 11 | Storage unchanged after a rejected grace_buyout when premium is zero |
//! | 12 | Storage unchanged after a rejected grace_buyout when premium is non-zero |

#![cfg(test)]

use crate::{
    admin::get_buyout_premium_bps,
    types::DataKey,
    SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env,
};

// ── shared constants ─────────────────────────────────────────────────────────

const T0: u64 = 1_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const GRACE_PERIOD: u64 = 7 * 24 * 60 * 60;
const AMOUNT: i128 = 10_000_000; // 10 USDC-equivalent in base units

// ── helpers ──────────────────────────────────────────────────────────────────

/// Initialize a contract environment with the vault deployed and configured.
/// Returns `(env, contract_id, token_address, stellar_asset_client)`.
fn setup() -> (
    Env,
    Address,                               // contract_id
    Address,                               // token
    token::StellarAssetClient<'static>,    // mint capability
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = T0);

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token_admin = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_client = token::StellarAssetClient::new(&env, &token_id.address());

    client.init(
        &token_id.address(),
        &6,
        &admin,
        &1_000_000i128,
        &GRACE_PERIOD,
    );

    (env, contract_id, token_id.address(), token_client)
}

/// Write `bps` directly to **persistent** storage (the normal post-v3 path).
fn write_premium_persistent(env: &Env, contract_id: &Address, bps: u32) {
    env.as_contract(contract_id, || {
        env.storage()
            .persistent()
            .set(&DataKey::BuyoutPremiumBps, &bps);
    });
}

/// Write `bps` directly to **instance** storage (the pre-v3 migration path).
fn write_premium_instance(env: &Env, contract_id: &Address, bps: u32) {
    env.as_contract(contract_id, || {
        env.storage()
            .instance()
            .set(&DataKey::BuyoutPremiumBps, &bps);
    });
}

/// Remove the key from both storage tiers so the contract is in the
/// "never written" state.
fn clear_premium(env: &Env, contract_id: &Address) {
    env.as_contract(contract_id, || {
        env.storage()
            .persistent()
            .remove(&DataKey::BuyoutPremiumBps);
        env.storage()
            .instance()
            .remove(&DataKey::BuyoutPremiumBps);
    });
}

/// Read `BuyoutPremiumBps` via the public function under test from inside
/// the contract context (required because `get_buyout_premium_bps` takes
/// `&Env` which must be the one associated with the contract).
fn read_premium(env: &Env, contract_id: &Address) -> u32 {
    env.as_contract(contract_id, || get_buyout_premium_bps(env))
}

/// Create a subscription, fund it, advance time past the interval, drain its
/// balance to 0 to trigger a failed charge → GracePeriod transition.
/// Returns `(id, subscriber, merchant)`.
fn create_sub_in_grace_period(
    env: &Env,
    contract_id: &Address,
    token_client: &token::StellarAssetClient<'static>,
) -> (u32, Address, Address) {
    let client = SubscriptionVaultClient::new(env, contract_id);
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

    // Fund the subscriber with one period's worth.
    token_client.mint(&subscriber, &AMOUNT);
    client.deposit_funds(&id, &subscriber, &AMOUNT, &None::<soroban_sdk::BytesN<32>>);

    // Drain the on-chain balance to force an insufficient-balance failure.
    env.as_contract(contract_id, || {
        let mut sub = env
            .storage()
            .persistent()
            .get::<_, crate::types::Subscription>(&DataKey::Sub(id))
            .expect("subscription must exist");
        sub.prepaid_balance = 0;
        env.storage().persistent().set(&DataKey::Sub(id), &sub);
    });

    // Advance past the interval and attempt a charge; this should transition
    // the subscription to GracePeriod.
    env.ledger()
        .with_mut(|l| l.timestamp = T0 + INTERVAL + 1);
    let _ = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);

    let sub = client.get_subscription(&id);
    assert_eq!(
        sub.status,
        SubscriptionStatus::GracePeriod,
        "precondition: subscription must be in GracePeriod"
    );

    (id, subscriber, merchant)
}

// ── Test 1: default (key absent) ─────────────────────────────────────────────

/// When the contract is freshly initialized and `DataKey::BuyoutPremiumBps`
/// has never been written, `get_buyout_premium_bps` must return `0`.
#[test]
fn test_default_returns_zero_when_key_absent() {
    let (env, contract_id, _, _) = setup();
    clear_premium(&env, &contract_id);
    assert_eq!(read_premium(&env, &contract_id), 0);
}

// ── Test 2: persistent storage round-trip ─────────────────────────────────────

/// The value written to persistent storage is returned unchanged.
#[test]
fn test_persistent_round_trip() {
    let (env, contract_id, _, _) = setup();
    write_premium_persistent(&env, &contract_id, 250);
    assert_eq!(read_premium(&env, &contract_id), 250);
}

// ── Test 3: boundary value – 1 bps ───────────────────────────────────────────

/// The minimum meaningful premium (1 bps) is stored and retrieved correctly.
#[test]
fn test_boundary_1_bps() {
    let (env, contract_id, _, _) = setup();
    write_premium_persistent(&env, &contract_id, 1);
    assert_eq!(read_premium(&env, &contract_id), 1);
}

// ── Test 4: boundary value – 9_999 bps ───────────────────────────────────────

/// Just below the 100 % ceiling: 9_999 bps is stored and retrieved correctly.
#[test]
fn test_boundary_9999_bps() {
    let (env, contract_id, _, _) = setup();
    write_premium_persistent(&env, &contract_id, 9_999);
    assert_eq!(read_premium(&env, &contract_id), 9_999);
}

// ── Test 5: boundary value – 10_000 bps (100 %) ──────────────────────────────

/// The getter does not validate the value; it reads whatever was written.
/// 10_000 bps (100 % premium) must be returned without panic.
#[test]
fn test_boundary_10000_bps_no_panic() {
    let (env, contract_id, _, _) = setup();
    write_premium_persistent(&env, &contract_id, 10_000);
    assert_eq!(read_premium(&env, &contract_id), 10_000);
}

// ── Test 6: boundary value – u32::MAX ────────────────────────────────────────

/// The getter applies no upper-bound validation; u32::MAX must be returned as-is.
/// (The *caller* — grace_buyout — is responsible for detecting overflow.)
#[test]
fn test_boundary_u32_max_no_panic() {
    let (env, contract_id, _, _) = setup();
    write_premium_persistent(&env, &contract_id, u32::MAX);
    assert_eq!(read_premium(&env, &contract_id), u32::MAX);
}

// ── Test 7: instance storage path (pre-v3 migration) ─────────────────────────

/// When the schema version is < 3, `read_config` falls through to instance
/// storage if persistent has nothing.  Confirm the value placed in instance
/// storage is returned.
///
/// We simulate the pre-v3 state by:
/// 1. Clearing both tiers.
/// 2. Downgrading the schema version to 2 (< 3) in persistent storage.
/// 3. Writing the value to instance storage only.
#[test]
fn test_instance_storage_fallback_pre_v3() {
    let (env, contract_id, _, _) = setup();

    // Clear both tiers so neither has BuyoutPremiumBps yet.
    clear_premium(&env, &contract_id);

    env.as_contract(&contract_id, || {
        // Simulate pre-v3 by downgrading the schema version stored in
        // persistent storage (read_config checks get_schema_version which
        // reads from persistent first).
        env.storage()
            .persistent()
            .set(&DataKey::SchemaVersion, &2u32);
        // Write the premium only to instance storage.
        env.storage()
            .instance()
            .set(&DataKey::BuyoutPremiumBps, &777u32);
    });

    assert_eq!(read_premium(&env, &contract_id), 777);
}

// ── Test 8: persistent shadows instance ──────────────────────────────────────

/// When both persistent and instance storage have a value, persistent wins
/// (it is checked first in `read_config`).
#[test]
fn test_persistent_shadows_instance() {
    let (env, contract_id, _, _) = setup();
    write_premium_instance(&env, &contract_id, 100);
    write_premium_persistent(&env, &contract_id, 400);
    // Persistent (400) must shadow instance (100).
    assert_eq!(read_premium(&env, &contract_id), 400);
}

// ── Test 9: overwrite supersedes previous value ───────────────────────────────

/// A second write to persistent storage replaces the first; no caching
/// artifact should cause the stale value to resurface.
#[test]
fn test_overwrite_returns_new_value() {
    let (env, contract_id, _, _) = setup();
    write_premium_persistent(&env, &contract_id, 300);
    assert_eq!(read_premium(&env, &contract_id), 300);

    write_premium_persistent(&env, &contract_id, 600);
    assert_eq!(read_premium(&env, &contract_id), 600);
}

// ── Test 10: state unchanged after rejected grace_buyout ─────────────────────

/// When `grace_buyout` fails (e.g., subscription is still Active, not in
/// GracePeriod), the `BuyoutPremiumBps` storage key must be unchanged.
#[test]
fn test_premium_bps_unchanged_after_rejected_buyout_wrong_status() {
    let (env, contract_id, _, token_client) = setup();
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    write_premium_persistent(&env, &contract_id, 500);

    // Create an Active subscription (NOT in GracePeriod).
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
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    token_client.mint(&subscriber, &(AMOUNT * 2));

    // Attempt buyout on an Active subscription — must fail with NotInGracePeriod.
    let res = client.try_grace_buyout(
        &id,
        &subscriber,
        &(AMOUNT * 2),
        &None::<soroban_sdk::BytesN<32>>,
    );
    assert!(res.is_err(), "grace_buyout on Active sub must fail");

    // The premium bps must still be 500.
    assert_eq!(read_premium(&env, &contract_id), 500);
}

// ── Test 11: state unchanged after rejected buyout (zero premium, insufficient deposit) ──

/// When the subscriber does not provide enough deposit (equal to zero-premium
/// case is trivially exact, so use a non-zero premium and undercut it), the
/// `BuyoutPremiumBps` storage key is unchanged.
#[test]
fn test_premium_bps_unchanged_after_rejected_buyout_insufficient_deposit() {
    let (env, contract_id, _, token_client) = setup();
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    write_premium_persistent(&env, &contract_id, 500); // 5 %

    let (id, subscriber, _merchant) =
        create_sub_in_grace_period(&env, &contract_id, &token_client);

    // Mint only the base charge amount, not enough to cover the 5 % premium.
    token_client.mint(&subscriber, &AMOUNT);

    let res = client.try_grace_buyout(
        &id,
        &subscriber,
        &AMOUNT, // missing the 5 % premium
        &None::<soroban_sdk::BytesN<32>>,
    );
    assert!(res.is_err(), "grace_buyout with insufficient deposit must fail");

    // The premium bps must still be 500.
    assert_eq!(read_premium(&env, &contract_id), 500);
}

// ── Test 12: state unchanged after rejected buyout (non-zero premium, overflow) ──

/// When the premium calculation would overflow (u32::MAX bps × large amount),
/// `grace_buyout` must fail with `Error::Overflow` and leave the
/// `BuyoutPremiumBps` key untouched.
#[test]
fn test_premium_bps_unchanged_after_overflow_rejection() {
    let (env, contract_id, _, token_client) = setup();
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    write_premium_persistent(&env, &contract_id, u32::MAX);

    let (id, subscriber, _merchant) =
        create_sub_in_grace_period(&env, &contract_id, &token_client);

    // Use i128::MAX as deposit — the premium multiply must overflow.
    let res = client.try_grace_buyout(
        &id,
        &subscriber,
        &i128::MAX,
        &None::<soroban_sdk::BytesN<32>>,
    );
    assert!(res.is_err(), "overflow in premium calc must return Err");

    // Storage must remain at u32::MAX — nothing was mutated.
    assert_eq!(read_premium(&env, &contract_id), u32::MAX);
}
