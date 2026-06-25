//! Negative and positive authorization tests for all entrypoints that call
//! `require_auth()`.
//!
//! # Auth matrix (mirrors docs/admin_authorization_matrix.md)
//!
//! | Entrypoint               | Authorizer         | Missing-auth result            | Wrong-signer result        |
//! |--------------------------|--------------------|---------------------------------|----------------------------|
//! | `create_subscription`    | subscriber         | `Error(Auth, InvalidAction)`    | n/a (no ownership check)   |
//! | `deposit_funds`          | subscriber         | `Error(Auth, InvalidAction)`    | `Error::Unauthorized` 1001 |
//! | `cancel_subscription`    | subscriber/merchant| `Error(Auth, InvalidAction)`    | `Error::Forbidden` 1002    |
//! | `pause_subscription`     | subscriber/merchant| `Error(Auth, InvalidAction)`    | `Error::Forbidden` 1002    |
//! | `withdraw_merchant_funds`| merchant           | `Error(Auth, InvalidAction)`    | `Error::NotFound` 2001     |
//! | `set_min_topup`          | stored admin       | `Error(Auth, InvalidAction)`    | `Error::Unauthorized` 1001 |
//!
//! Each entrypoint has three test cases:
//!  1. **missing_auth** — no `mock_all_auths`, host panics at the first
//!     `require_auth()` call with `Error(Auth, InvalidAction)`.
//!  2. **wrong_signer** — `mock_all_auths` satisfies `require_auth()`, but the
//!     contract's own ownership check returns the error shown above.
//!  3. **correct_auth** — `mock_all_auths` + correct address → call succeeds.

use crate::{DataKey, Error, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{testutils::Address as _, Address, Env};

// ── Constants ─────────────────────────────────────────────────────────────────

const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const AMOUNT: i128 = 10_000_000; // 10 USDC (6-decimal)
const MIN_TOPUP: i128 = 1_000_000; // 1 USDC
const DEPOSIT: i128 = 5_000_000; // 5 USDC

// ── Shared setup helpers ──────────────────────────────────────────────────────

/// Fully-initialized vault environment with `mock_all_auths` enabled.
///
/// Returns `(env, client, token_address, admin_address)`.
fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &MIN_TOPUP, &(7 * 24 * 60 * 60));
    (env, client, token, admin)
}

/// Create one active subscription; returns `(sub_id, subscriber, merchant)`.
fn make_subscription(env: &Env, client: &SubscriptionVaultClient) -> (u32, Address, Address) {
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
    );
    (id, subscriber, merchant)
}

// ═════════════════════════════════════════════════════════════════════════════
// 1. create_subscription — subscriber must authorize
//
// The subscriber's `require_auth()` is the first statement in
// `do_create_subscription`, so a missing auth panics before any storage access.
// There is no secondary ownership check beyond `require_auth()` itself, so the
// only two meaningful cases are missing-auth and correct-auth.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn create_subscription_missing_auth() {
    // No mock_all_auths.  `init` has no `require_auth()` so it succeeds and
    // makes `get_token` return the registered token.  `do_create_subscription`
    // then calls `do_create_subscription_with_token` which opens with
    // `subscriber.require_auth()` → host auth failure.
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let _ = client.init(&token, &6, &admin, &MIN_TOPUP, &(7 * 24 * 60 * 60));
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let _ = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
}

#[test]
fn create_subscription_correct_auth() {
    let (env, client, _, _) = setup();
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
    assert_eq!(sub.subscriber, subscriber);
    assert_eq!(sub.merchant, merchant);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

// ═════════════════════════════════════════════════════════════════════════════
// 2. deposit_funds — subscriber must authorize AND match sub.subscriber
//
// `do_deposit_funds` calls `subscriber.require_auth()` first, then checks
// `subscriber != sub.subscriber` and returns `Error::Unauthorized` (1001).
// ═════════════════════════════════════════════════════════════════════════════

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn deposit_funds_missing_auth() {
    // No mock_all_auths → subscriber.require_auth() fires before any storage read.
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let subscriber = Address::generate(&env);
    let _ = client.deposit_funds(&0u32, &subscriber, &DEPOSIT, &None::<soroban_sdk::BytesN<32>>, &None::<soroban_sdk::BytesN<32>>);
}

#[test]
#[should_panic(expected = "Error(Contract, #1001)")] // Error::Unauthorized
fn deposit_funds_wrong_subscriber() {
    let (env, client, token, _) = setup();
    let (id, _, _) = make_subscription(&env, &client);
    let attacker = Address::generate(&env);
    // Give the attacker a token balance so the transfer could theoretically succeed,
    // proving the rejection is the subscriber mismatch check, not a balance issue.
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&attacker, &DEPOSIT);
    // mock_all_auths satisfies require_auth(), but the contract rejects because
    // attacker != sub.subscriber.
    let _ = client.deposit_funds(&id, &attacker, &DEPOSIT, &None::<soroban_sdk::BytesN<32>>, &None::<soroban_sdk::BytesN<32>>);
}

#[test]
fn deposit_funds_correct_auth() {
    let (env, client, token, _) = setup();
    let (id, subscriber, _) = make_subscription(&env, &client);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &DEPOSIT);
    client.deposit_funds(&id, &subscriber, &DEPOSIT, &None::<soroban_sdk::BytesN<32>>, &None::<soroban_sdk::BytesN<32>>);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, DEPOSIT);
}

// ═════════════════════════════════════════════════════════════════════════════
// 3. cancel_subscription — subscriber OR merchant must authorize
//
// `do_cancel_subscription` calls `authorizer.require_auth()` first, then checks
// `authorizer != sub.subscriber && authorizer != sub.merchant` and returns
// `Error::Forbidden` (1002).
// ═════════════════════════════════════════════════════════════════════════════

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn cancel_subscription_missing_auth() {
    // No mock_all_auths → authorizer.require_auth() fires before the sub is loaded.
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let authorizer = Address::generate(&env);
    let _ = client.cancel_subscription(&0u32, &authorizer);
}

#[test]
#[should_panic(expected = "Error(Contract, #1002)")] // Error::Forbidden
fn cancel_subscription_wrong_authorizer() {
    let (env, client, _, _) = setup();
    let (id, _, _) = make_subscription(&env, &client);
    let third_party = Address::generate(&env);
    // Third-party is neither subscriber nor merchant → Forbidden.
    let _ = client.cancel_subscription(&id, &third_party);
}

#[test]
fn cancel_subscription_by_subscriber() {
    let (env, client, _, _) = setup();
    let (id, subscriber, _) = make_subscription(&env, &client);
    client.cancel_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}

#[test]
fn cancel_subscription_by_merchant() {
    let (env, client, _, _) = setup();
    let (id, _, merchant) = make_subscription(&env, &client);
    client.cancel_subscription(&id, &merchant);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// 4. pause_subscription — subscriber OR merchant must authorize
//
// `do_pause_subscription` calls `authorizer.require_auth()` first, then checks
// `authorizer != sub.subscriber && authorizer != sub.merchant` and returns
// `Error::Forbidden` (1002).
// ═════════════════════════════════════════════════════════════════════════════

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn pause_subscription_missing_auth() {
    // No mock_all_auths → authorizer.require_auth() fires before the sub is loaded.
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let authorizer = Address::generate(&env);
    let _ = client.pause_subscription(&0u32, &authorizer);
}

#[test]
#[should_panic(expected = "Error(Contract, #1002)")] // Error::Forbidden
fn pause_subscription_wrong_authorizer() {
    let (env, client, _, _) = setup();
    let (id, _, _) = make_subscription(&env, &client);
    let third_party = Address::generate(&env);
    // Third-party is neither subscriber nor merchant → Forbidden.
    let _ = client.pause_subscription(&id, &third_party);
}

#[test]
fn pause_subscription_by_subscriber() {
    let (env, client, _, _) = setup();
    let (id, subscriber, _) = make_subscription(&env, &client);
    client.pause_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );
}

#[test]
fn pause_subscription_by_merchant() {
    let (env, client, _, _) = setup();
    let (id, _, merchant) = make_subscription(&env, &client);
    client.pause_subscription(&id, &merchant);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// 5. withdraw_merchant_funds — merchant must authorize
//
// `withdraw_merchant_funds_for_token` calls `merchant.require_auth()` first.
// Unlike the subscriber/merchant pair checks, each merchant address maps to its
// own balance slot; a caller passing a different address can only access that
// address's own (zero) balance, returning `Error::NotFound` (2001).
//
// Note: `withdraw_merchant_funds` delegates to `withdraw_merchant_funds_for_token`
// via `get_token → withdraw_merchant_funds_for_token`, so `init` must be called
// before the missing-auth test to get past the `get_token` lookup that precedes
// `require_auth()`.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn withdraw_merchant_funds_missing_auth() {
    // No mock_all_auths.  `init` itself has no `require_auth()` call, so it
    // succeeds and makes `get_token` return the real token, allowing the call
    // to reach `merchant.require_auth()` in `withdraw_merchant_funds_for_token`.
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let _ = client.init(&token, &6, &admin, &MIN_TOPUP, &(7 * 24 * 60 * 60));
    let merchant = Address::generate(&env);
    // merchant.require_auth() fires here → host auth failure.
    let _ = client.withdraw_merchant_funds(&merchant, &AMOUNT);
}

#[test]
#[should_panic(expected = "Error(Contract, #2001)")] // Error::NotFound (zero balance)
fn withdraw_merchant_funds_wrong_merchant() {
    let (env, client, _, _) = setup();
    let wrong_merchant = Address::generate(&env);
    // mock_all_auths satisfies require_auth(), but wrong_merchant has no earned
    // balance → balance == 0 → NotFound.  Demonstrates per-address balance
    // isolation: the wrong_merchant address cannot drain any other merchant's funds.
    let _ = client.withdraw_merchant_funds(&wrong_merchant, &AMOUNT);
}

#[test]
fn withdraw_merchant_funds_correct_auth() {
    let (env, client, token, _) = setup();
    let (id, subscriber, merchant) = make_subscription(&env, &client);

    // Deposit so the vault holds real tokens.
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &DEPOSIT);
    client.deposit_funds(&id, &subscriber, &DEPOSIT, &None::<soroban_sdk::BytesN<32>>, &None::<soroban_sdk::BytesN<32>>);

    // Directly credit the merchant's ledger balance and mint matching vault tokens
    // so the withdrawal transfer can complete.  (A real charge flow would do this
    // automatically, but that requires advancing the ledger clock.)
    let withdraw_amount: i128 = 1_000_000;
    env.as_contract(&client.address, || {
        env.storage().instance().set(
            &DataKey::MerchantBalance(merchant.clone(), token.clone()),
            &withdraw_amount,
        );
    });
    soroban_sdk::token::StellarAssetClient::new(&env, &token)
        .mint(&client.address, &withdraw_amount);

    let balance_before = client.get_merchant_balance(&merchant);
    assert_eq!(balance_before, withdraw_amount);
    client.withdraw_merchant_funds(&merchant, &withdraw_amount);
    assert_eq!(client.get_merchant_balance(&merchant), 0);
}

// ═════════════════════════════════════════════════════════════════════════════
// 6. set_min_topup — stored admin must authorize
//
// `require_admin_auth` calls `admin.require_auth()` first, then compares the
// argument against the stored admin address and returns `Error::Unauthorized`
// (1001) if they differ.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn set_min_topup_missing_auth() {
    // No mock_all_auths → admin.require_auth() fires as the first operation in
    // `require_admin_auth`, before the stored admin is even read.
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let _ = client.set_min_topup(&admin, &500_000i128);
}

#[test]
#[should_panic(expected = "Error(Contract, #1001)")] // Error::Unauthorized
fn set_min_topup_wrong_admin() {
    let (env, client, _, _) = setup();
    let wrong_admin = Address::generate(&env);
    // mock_all_auths satisfies require_auth() for wrong_admin, but the stored
    // admin doesn't match → Unauthorized.
    let _ = client.set_min_topup(&wrong_admin, &500_000i128);
}

#[test]
fn set_min_topup_correct_auth() {
    let (env, client, _, admin) = setup();
    let new_topup = 2_000_000i128;
    client.set_min_topup(&admin, &new_topup);
    assert_eq!(client.get_min_topup(), new_topup);
}

// ═════════════════════════════════════════════════════════════════════════════
// 7. init / set_min_topup — min_topup must be strictly positive
//
// A non-positive min_topup would make the `BelowMinimumTopup` guard in
// `deposit_funds` meaningless and could permit zero-value deposits.
// Both `do_init` and `do_set_min_topup` must reject min_topup <= 0 with
// `Error::InvalidAmount` (3001).
// ═════════════════════════════════════════════════════════════════════════════

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // Error::InvalidAmount
fn init_rejects_zero_min_topup() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    // min_topup = 0 must be rejected.
    let _ = client.init(&token, &6, &admin, &0i128, &(7 * 24 * 60 * 60));
}

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // Error::InvalidAmount
fn init_rejects_negative_min_topup() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    // min_topup = -1 must be rejected.
    let _ = client.init(&token, &6, &admin, &-1i128, &(7 * 24 * 60 * 60));
}

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // Error::InvalidAmount
fn set_min_topup_rejects_zero() {
    let (_, client, _, admin) = setup();
    // admin is correct, but min_topup = 0 must be rejected.
    let _ = client.set_min_topup(&admin, &0i128);
}

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // Error::InvalidAmount
fn set_min_topup_rejects_negative() {
    let (_, client, _, admin) = setup();
    // admin is correct, but min_topup < 0 must be rejected.
    let _ = client.set_min_topup(&admin, &-1i128);
}

#[test]
fn get_min_topup_returns_stored_value_unchanged() {
    // Verify that get_min_topup is a pure read: it returns exactly what init stored.
    let (_, client, _, admin) = setup();
    assert_eq!(client.get_min_topup(), MIN_TOPUP);
    // Update and confirm the new value is returned verbatim.
    let new_topup = 3_000_000i128;
    client.set_min_topup(&admin, &new_topup);
    assert_eq!(client.get_min_topup(), new_topup);
}
