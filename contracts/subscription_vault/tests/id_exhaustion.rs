//! ID-space exhaustion boundary tests.
//!
//! Seeds `DataKey::NextId` to `u32::MAX - 1` via `env.as_contract` so that
//! exactly one more allocation succeeds, then asserts that every subsequent
//! call returns the correct limit error without wrapping the counter.
//!
//! # Errors under test
//!
//! | Entry-point | Error at u32::MAX |
//! |---|---|
//! | `create_subscription` | `SubscriptionLimitReached` (#6001) |
//! | `create_subscription_with_token` | `SubscriptionLimitReached` (#6001) |
//! | `create_subscription_from_plan` | `Overflow` (#1) — no `MAX_SUBSCRIPTION_ID` guard in `do_create_subscription_from_plan` |
//!
//! The `from_plan` discrepancy is intentional documentation: the test pins the
//! current behaviour so a future fix (aligning it to `SubscriptionLimitReached`)
//! will cause a deliberate test failure rather than a silent regression.

#![cfg(test)]

use soroban_sdk::{
    testutils::Address as _,
    Address, Env,
};
use subscription_vault::{DataKey, Error, SubscriptionVault, SubscriptionVaultClient};

// ── shared constants ──────────────────────────────────────────────────────────

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days

// ── helpers ───────────────────────────────────────────────────────────────────

/// Minimal contract setup: register, init with a real SAC token, return client + token address.
fn setup() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    let vault_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &vault_id);
    client.init(&token, &6u32, &admin, &1_000_000i128, &(7 * 24 * 60 * 60u64));

    (env, client, token)
}

/// Seed `DataKey::NextId` to `value` directly in contract storage.
fn seed_next_id(env: &Env, contract: &Address, value: u32) {
    env.as_contract(contract, || {
        env.storage().persistent().set(&DataKey::NextId, &value);
    });
}

/// Read `DataKey::NextId` from contract storage.
fn read_next_id(env: &Env, contract: &Address) -> u32 {
    env.as_contract(contract, || {
        env.storage()
            .persistent()
            .get(&DataKey::NextId)
            .unwrap_or(0u32)
    })
}

// ── create_subscription ───────────────────────────────────────────────────────

/// At `u32::MAX - 1` exactly one allocation succeeds and returns id `u32::MAX - 1`.
#[test]
fn create_subscription_last_id_succeeds() {
    let (env, client, _) = setup();
    seed_next_id(&env, &client.address, u32::MAX - 1);

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

    assert_eq!(id, u32::MAX - 1, "last valid id must be u32::MAX - 1");
    assert_eq!(
        read_next_id(&env, &client.address),
        u32::MAX,
        "counter must advance to u32::MAX after final allocation"
    );
}

/// After the counter reaches `u32::MAX`, `create_subscription` returns `SubscriptionLimitReached`.
#[test]
fn create_subscription_at_max_returns_limit_reached() {
    let (env, client, _) = setup();
    seed_next_id(&env, &client.address, u32::MAX);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let err = client
        .try_create_subscription(
            &subscriber,
            &merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
        )
        .expect_err("must fail when counter is at u32::MAX");

    assert_eq!(err, Ok(Error::SubscriptionLimitReached));
}

/// Counter must not change after a failed allocation (no wrap, no increment).
#[test]
fn create_subscription_counter_unchanged_after_failure() {
    let (env, client, _) = setup();
    seed_next_id(&env, &client.address, u32::MAX);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let _ = client.try_create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    assert_eq!(
        read_next_id(&env, &client.address),
        u32::MAX,
        "counter must stay at u32::MAX after failed allocation"
    );
}

// ── create_subscription_with_token ───────────────────────────────────────────

/// At `u32::MAX - 1` exactly one allocation succeeds via the token-specific path.
#[test]
fn create_subscription_with_token_last_id_succeeds() {
    let (env, client, token) = setup();
    seed_next_id(&env, &client.address, u32::MAX - 1);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    assert_eq!(id, u32::MAX - 1);
    assert_eq!(read_next_id(&env, &client.address), u32::MAX);
}

/// After the counter reaches `u32::MAX`, `create_subscription_with_token` returns `SubscriptionLimitReached`.
#[test]
fn create_subscription_with_token_at_max_returns_limit_reached() {
    let (env, client, token) = setup();
    seed_next_id(&env, &client.address, u32::MAX);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let err = client
        .try_create_subscription_with_token(
            &subscriber,
            &merchant,
            &token,
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
        )
        .expect_err("must fail when counter is at u32::MAX");

    assert_eq!(err, Ok(Error::SubscriptionLimitReached));
}

/// Counter must not change after a failed `create_subscription_with_token`.
#[test]
fn create_subscription_with_token_counter_unchanged_after_failure() {
    let (env, client, token) = setup();
    seed_next_id(&env, &client.address, u32::MAX);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let _ = client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &token,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    assert_eq!(read_next_id(&env, &client.address), u32::MAX);
}

// ── create_subscription_from_plan ────────────────────────────────────────────

/// At `u32::MAX - 1` exactly one allocation succeeds via the plan path.
#[test]
fn create_subscription_from_plan_last_id_succeeds() {
    let (env, client, _) = setup();

    let merchant = Address::generate(&env);
    let plan_id = client.create_plan_template(&merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>);

    seed_next_id(&env, &client.address, u32::MAX - 1);

    let subscriber = Address::generate(&env);
    let id = client.create_subscription_from_plan(&subscriber, &plan_id);

    assert_eq!(id, u32::MAX - 1);
    assert_eq!(read_next_id(&env, &client.address), u32::MAX);
}

/// After the counter reaches `u32::MAX`, `create_subscription_from_plan` returns `Overflow`
/// (not `SubscriptionLimitReached`) because `do_create_subscription_from_plan` uses
/// `checked_add(1).ok_or(Error::Overflow)` without a `MAX_SUBSCRIPTION_ID` guard.
///
/// This test pins the current behaviour. If the implementation is aligned to return
/// `SubscriptionLimitReached` in the future, update this assertion accordingly.
#[test]
fn create_subscription_from_plan_at_max_returns_overflow() {
    let (env, client, _) = setup();

    let merchant = Address::generate(&env);
    let plan_id = client.create_plan_template(&merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>);

    seed_next_id(&env, &client.address, u32::MAX);

    let subscriber = Address::generate(&env);
    let err = client
        .try_create_subscription_from_plan(&subscriber, &plan_id)
        .expect_err("must fail when counter is at u32::MAX");

    assert_eq!(err, Ok(Error::Overflow));
}

/// Counter must not change after a failed `create_subscription_from_plan`.
#[test]
fn create_subscription_from_plan_counter_unchanged_after_failure() {
    let (env, client, _) = setup();

    let merchant = Address::generate(&env);
    let plan_id = client.create_plan_template(&merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>);

    seed_next_id(&env, &client.address, u32::MAX);

    let subscriber = Address::generate(&env);
    let _ = client.try_create_subscription_from_plan(&subscriber, &plan_id);

    assert_eq!(read_next_id(&env, &client.address), u32::MAX);
}
