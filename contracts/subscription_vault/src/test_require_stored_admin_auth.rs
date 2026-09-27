//! Adversarial coverage for `admin::require_stored_admin_auth`.
//!
//! # Function under test
//!
//! ```ignore
//! pub fn require_stored_admin_auth(env: &Env) -> Result<Address, Error> {
//!     let stored_admin = require_admin(env)?;   // fails closed: NotInitialized when unset
//!     stored_admin.require_auth();              // host-layer auth enforcement
//!     Ok(stored_admin)
//! }
//! ```
//!
//! # Test matrix
//!
//! | Case | Setup | Expected outcome |
//! |------|-------|-----------------|
//! | not_initialized | contract registered but never `init`-ed | `Error::NotInitialized` (fails closed) |
//! | missing_auth | contract initialized, zero mock_auths | `Error(Auth, InvalidAction)` (host panic) |
//! | wrong_signer | contract initialized, stranger's auth mock | `Error(Auth, InvalidAction)` (host rejects) |
//! | correct_admin | contract initialized, admin's auth mock | `Ok(admin_address)` |
//! | stale_admin_after_rotation | contract initialized, old admin rotated away | host-layer auth panic |
//! | new_admin_after_rotation | contract initialized, new admin authorized | succeeds and reflects new admin |
//!
//! State-invariant assertions verify that **no subscriber balance or storage is
//! mutated** when an authorization call is rejected.

#![cfg(test)]

use crate::{
    types::{DataKey, Error},
    SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _, MockAuth, MockAuthInvoke},
    Address, BytesN, Env, IntoVal, Vec as SorobanVec,
};

// ── Constants ─────────────────────────────────────────────────────────────────

const T0: u64 = 1_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const AMOUNT: i128 = 10_000_000; // 10 USDC (6-decimal)
const PREPAID: i128 = 50_000_000; // 50 USDC

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Fully-initialized vault with `mock_all_auths`. Returns `(env, client, admin, token)`.
fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, client, admin, token)
}

/// Advance the ledger timestamp by `seconds`.
fn jump(env: &Env, seconds: u64) {
    let t = env.ledger().timestamp();
    env.ledger().with_mut(|l| l.timestamp = t + seconds);
}

/// Seed a subscription's prepaid balance directly in storage (bypasses token transfer).
fn seed_balance(env: &Env, client: &SubscriptionVaultClient, id: u32, balance: i128) {
    let mut sub = client.get_subscription(&id);
    sub.prepaid_balance = balance;
    env.as_contract(&client.address, || {
        env.storage().persistent().set(&DataKey::Sub(id), &sub);
    });
}

/// Create one active subscription with a prepaid balance ready for charging.
/// Returns `(id, subscriber, merchant)`.
fn make_funded_subscription(env: &Env, client: &SubscriptionVaultClient) -> (u32, Address, Address) {
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
    seed_balance(env, client, id, PREPAID);
    (id, subscriber, merchant)
}

/// Install a mock auth that satisfies exactly one `fn_name` call signed by `signer`.
/// Any call not matching this descriptor will fail at the host layer.
fn mock_only_signer(
    env: &Env,
    client: &SubscriptionVaultClient,
    signer: &Address,
    fn_name: &str,
    args: SorobanVec<soroban_sdk::Val>,
) {
    env.mock_auths(&[MockAuth {
        address: signer,
        invoke: &MockAuthInvoke {
            contract: &client.address,
            fn_name,
            args,
            sub_invokes: &[],
        },
    }]);
}

/// Build the argument list for `charge_subscription(id, None)`.
fn charge_subscription_args(env: &Env, id: u32) -> SorobanVec<soroban_sdk::Val> {
    let mut args = SorobanVec::new(env);
    args.push_back(id.into_val(env));
    args.push_back(None::<BytesN<32>>.into_val(env));
    args
}

/// Build the argument list for `batch_charge(ids, nonce)`.
fn batch_charge_args(env: &Env, ids: &SorobanVec<u32>, nonce: u64) -> SorobanVec<soroban_sdk::Val> {
    let mut args = SorobanVec::new(env);
    args.push_back(ids.clone().into_val(env));
    args.push_back(nonce.into_val(env));
    args
}

// ═════════════════════════════════════════════════════════════════════════════
// 1. NOT INITIALIZED — require_admin returns NotInitialized immediately
//
// When the contract has never been `init`-ed, `DataKey::Admin` is absent from
// storage. `require_admin` returns `Err(Error::NotInitialized)` before any
// `require_auth()` call can be made, ensuring the function fails closed.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn require_stored_admin_auth_not_initialized_returns_not_initialized() {
    // Contract registered but never init-ed.
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    // charge_subscription is one of several callers of require_stored_admin_auth.
    // Before any init, it must return NotInitialized (Error code 1004).
    let res = client.try_charge_subscription(&0u32, &None::<BytesN<32>>);
    assert_eq!(
        res,
        Err(Ok(Error::NotInitialized)),
        "uninitialized contract must return NotInitialized, not panic"
    );
}

#[test]
fn require_stored_admin_auth_not_initialized_batch_charge() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let ids: SorobanVec<u32> = SorobanVec::from_array(&env, [0u32]);
    let res = client.try_batch_charge(&ids, &0u64);
    assert_eq!(
        res,
        Err(Ok(Error::NotInitialized)),
        "batch_charge on uninitialized contract must return NotInitialized"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// 2. MISSING AUTH — no mock_auths, host panics at require_auth()
//
// `require_stored_admin_auth` calls `stored_admin.require_auth()`. When no
// mock auth is present the host rejects the call immediately with
// `Error(Auth, InvalidAction)`. This happens before any state write.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn charge_subscription_missing_auth_panics() {
    // Initialize without mock_all_auths so we can control auth precisely.
    let env = Env::default();
    env.mock_all_auths(); // only for init
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    // Remove all mocked auths — subsequent calls have no auth context.
    env.mock_auths(&[]);

    // Host panics at stored_admin.require_auth() inside require_stored_admin_auth.
    let _ = client.charge_subscription(&0u32, &None::<BytesN<32>>);
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn batch_charge_missing_auth_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    env.mock_auths(&[]);

    let ids: SorobanVec<u32> = SorobanVec::from_array(&env, [0u32]);
    let _ = client.batch_charge(&ids, &0u64);
}

// ═════════════════════════════════════════════════════════════════════════════
// 3. WRONG SIGNER — a stranger's signature cannot authorize the stored admin
//
// `mock_only_signer` installs a mock auth for `stranger` only. The host
// accepts `stranger.require_auth()` but the stored admin is a *different*
// address, so `stored_admin.require_auth()` finds no matching credential and
// the host panics with `Error(Auth, InvalidAction)`.
//
// Critical state invariant: subscriber balances must be unchanged after
// any rejected call.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn charge_subscription_wrong_signer_panics() {
    let (env, client, _admin, _token) = setup();
    let (id, _, _) = make_funded_subscription(&env, &client);
    jump(&env, INTERVAL + 1);

    let stranger = Address::generate(&env);
    mock_only_signer(
        &env,
        &client,
        &stranger,
        "charge_subscription",
        charge_subscription_args(&env, id),
    );

    // Host fails: stored admin's require_auth() has no matching credential.
    let _ = client.charge_subscription(&id, &None::<BytesN<32>>);
}

#[test]
fn charge_subscription_wrong_signer_state_unchanged() {
    // Use try_ variant to assert on the error without panicking the test.
    let (env, client, _admin, _token) = setup();
    let (id, _, _) = make_funded_subscription(&env, &client);
    jump(&env, INTERVAL + 1);

    let stranger = Address::generate(&env);
    mock_only_signer(
        &env,
        &client,
        &stranger,
        "charge_subscription",
        charge_subscription_args(&env, id),
    );

    let res = client.try_charge_subscription(&id, &None::<BytesN<32>>);
    assert!(res.is_err(), "wrong signer must be rejected");

    // Balance must be unchanged after the rejection.
    let sub = client.get_subscription(&id);
    assert_eq!(
        sub.prepaid_balance, PREPAID,
        "no funds may move on wrong-signer rejection"
    );
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn batch_charge_wrong_signer_panics() {
    let (env, client, _admin, _token) = setup();
    let (id, _, _) = make_funded_subscription(&env, &client);
    jump(&env, INTERVAL + 1);

    let ids: SorobanVec<u32> = SorobanVec::from_array(&env, [id]);
    let stranger = Address::generate(&env);
    mock_only_signer(
        &env,
        &client,
        &stranger,
        "batch_charge",
        batch_charge_args(&env, &ids, 0),
    );

    let _ = client.batch_charge(&ids, &0u64);
}

#[test]
fn batch_charge_wrong_signer_state_unchanged() {
    let (env, client, _admin, _token) = setup();
    let (id, _, _) = make_funded_subscription(&env, &client);
    jump(&env, INTERVAL + 1);

    let ids: SorobanVec<u32> = SorobanVec::from_array(&env, [id]);
    let stranger = Address::generate(&env);
    mock_only_signer(
        &env,
        &client,
        &stranger,
        "batch_charge",
        batch_charge_args(&env, &ids, 0),
    );

    let res = client.try_batch_charge(&ids, &0u64);
    assert!(res.is_err(), "wrong signer must be rejected for batch_charge");

    let sub = client.get_subscription(&id);
    assert_eq!(
        sub.prepaid_balance, PREPAID,
        "no funds may move on wrong-signer batch_charge rejection"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// 4. CORRECT ADMIN — happy path: stored admin authorized
//
// With `mock_all_auths` (or a matching credential for the stored admin),
// `require_stored_admin_auth` must succeed and return the stored admin address.
// The return value is tested indirectly by verifying that downstream state
// mutations (balance deduction, merchant credit) take effect.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn charge_subscription_correct_admin_succeeds() {
    let (env, client, _admin, token) = setup();
    let (id, _subscriber, merchant) = make_funded_subscription(&env, &client);
    jump(&env, INTERVAL + 1);

    // mock_all_auths (set in setup) covers the stored admin's require_auth().
    let res = client.try_charge_subscription(&id, &None::<BytesN<32>>);
    assert!(res.is_ok(), "correct admin must succeed: {:?}", res);

    let sub = client.get_subscription(&id);
    assert_eq!(
        sub.prepaid_balance,
        PREPAID - AMOUNT,
        "subscriber balance must decrease by exactly one charge amount"
    );
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        AMOUNT,
        "merchant must be credited the charge amount"
    );
}

#[test]
fn batch_charge_correct_admin_succeeds() {
    let (env, client, _admin, token) = setup();
    let (id, _subscriber, merchant) = make_funded_subscription(&env, &client);
    jump(&env, INTERVAL + 1);

    let ids: SorobanVec<u32> = SorobanVec::from_array(&env, [id]);
    let results = client.batch_charge(&ids, &0u64);
    assert_eq!(results.len(), 1);
    assert!(
        results.get(0).unwrap().success,
        "batch_charge with correct admin must succeed"
    );

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID - AMOUNT);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), AMOUNT);
}

// ═════════════════════════════════════════════════════════════════════════════
// 5. STALE ADMIN AFTER ROTATION — rotated-away admin cannot authorize
//
// After `rotate_admin`, `DataKey::Admin` points to the new admin. The old
// admin's credentials no longer satisfy `stored_admin.require_auth()`.
// This is a critical property: a compromised or decommissioned admin key
// must lose the ability to trigger charges the moment rotation is final.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn charge_subscription_stale_admin_after_rotation_panics() {
    let (env, client, old_admin, _token) = setup();
    let (id, _, _) = make_funded_subscription(&env, &client);
    jump(&env, INTERVAL + 1);

    let new_admin = Address::generate(&env);
    // Rotate: old_admin → new_admin (mock_all_auths satisfies the rotation auth).
    client.rotate_admin(&old_admin, &new_admin, &0u64);
    assert_eq!(client.get_admin(), new_admin, "rotation must update stored admin");

    // Now install only old_admin's credential — must not satisfy new stored admin.
    mock_only_signer(
        &env,
        &client,
        &old_admin,
        "charge_subscription",
        charge_subscription_args(&env, id),
    );

    // Host panics: stored admin is now new_admin, old_admin's auth is rejected.
    let _ = client.charge_subscription(&id, &None::<BytesN<32>>);
}

#[test]
fn charge_subscription_stale_admin_state_unchanged() {
    let (env, client, old_admin, _token) = setup();
    let (id, _, _) = make_funded_subscription(&env, &client);
    jump(&env, INTERVAL + 1);

    let new_admin = Address::generate(&env);
    client.rotate_admin(&old_admin, &new_admin, &0u64);

    mock_only_signer(
        &env,
        &client,
        &old_admin,
        "charge_subscription",
        charge_subscription_args(&env, id),
    );

    let res = client.try_charge_subscription(&id, &None::<BytesN<32>>);
    assert!(res.is_err(), "stale admin must be rejected");

    let sub = client.get_subscription(&id);
    assert_eq!(
        sub.prepaid_balance, PREPAID,
        "no funds may move when stale admin is rejected"
    );
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn batch_charge_stale_admin_after_rotation_panics() {
    let (env, client, old_admin, _token) = setup();
    let (id, _, _) = make_funded_subscription(&env, &client);
    jump(&env, INTERVAL + 1);

    let new_admin = Address::generate(&env);
    client.rotate_admin(&old_admin, &new_admin, &0u64);

    let ids: SorobanVec<u32> = SorobanVec::from_array(&env, [id]);
    mock_only_signer(
        &env,
        &client,
        &old_admin,
        "batch_charge",
        batch_charge_args(&env, &ids, 0),
    );

    let _ = client.batch_charge(&ids, &0u64);
}

// ═════════════════════════════════════════════════════════════════════════════
// 6. NEW ADMIN AFTER ROTATION — new admin has full charging authority
//
// After `rotate_admin`, the new admin must be able to trigger charges using
// `require_stored_admin_auth` immediately; no additional setup is needed.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn charge_subscription_new_admin_after_rotation_succeeds() {
    let (env, client, old_admin, token) = setup();
    let (id, _subscriber, merchant) = make_funded_subscription(&env, &client);
    jump(&env, INTERVAL + 1);

    let new_admin = Address::generate(&env);
    client.rotate_admin(&old_admin, &new_admin, &0u64);
    assert_eq!(client.get_admin(), new_admin);

    // Restore mock_all_auths so new_admin's require_auth() is satisfied.
    env.mock_all_auths();
    let res = client.try_charge_subscription(&id, &None::<BytesN<32>>);
    assert!(res.is_ok(), "new admin after rotation must succeed: {:?}", res);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID - AMOUNT);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), AMOUNT);
}

#[test]
fn batch_charge_new_admin_after_rotation_succeeds() {
    let (env, client, old_admin, token) = setup();
    let (id, _subscriber, merchant) = make_funded_subscription(&env, &client);
    jump(&env, INTERVAL + 1);

    let new_admin = Address::generate(&env);
    client.rotate_admin(&old_admin, &new_admin, &0u64);

    env.mock_all_auths();
    let ids: SorobanVec<u32> = SorobanVec::from_array(&env, [id]);
    // Nonce starts at 0 for the new admin (rotation does not carry nonces).
    let results = client.batch_charge(&ids, &0u64);
    assert_eq!(results.len(), 1);
    assert!(results.get(0).unwrap().success);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID - AMOUNT);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), AMOUNT);
}

// ═════════════════════════════════════════════════════════════════════════════
// 7. RETURN VALUE IDENTITY — the returned address equals the stored admin
//
// `require_stored_admin_auth` documents that it returns `Ok(stored_admin)`.
// `get_admin()` exposes the same value. The two must agree after every
// successful call.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn require_stored_admin_auth_return_value_matches_get_admin() {
    // We verify this indirectly: rotating the admin changes `get_admin()` and
    // also changes *which* address `require_stored_admin_auth` will accept.
    let (env, client, old_admin, _token) = setup();

    // Before rotation: get_admin returns old_admin.
    assert_eq!(client.get_admin(), old_admin);

    let new_admin = Address::generate(&env);
    client.rotate_admin(&old_admin, &new_admin, &0u64);

    // After rotation: get_admin returns new_admin.
    assert_eq!(client.get_admin(), new_admin);

    // require_stored_admin_auth now demands new_admin's signature.
    // Proving this by showing old_admin's credential no longer works:
    let (id, _, _) = make_funded_subscription(&env, &client);
    jump(&env, INTERVAL + 1);

    mock_only_signer(
        &env,
        &client,
        &old_admin,
        "charge_subscription",
        charge_subscription_args(&env, id),
    );
    let res = client.try_charge_subscription(&id, &None::<BytesN<32>>);
    assert!(
        res.is_err(),
        "old_admin credential must fail after rotation"
    );

    // And new_admin's credential does work:
    env.mock_all_auths();
    let res2 = client.try_charge_subscription(&id, &None::<BytesN<32>>);
    assert!(res2.is_ok(), "new_admin must succeed after rotation");
}

// ═════════════════════════════════════════════════════════════════════════════
// 8. IDEMPOTENCY / MULTIPLE CALLS — repeated correct-admin calls succeed
//
// `require_stored_admin_auth` has no side-effects of its own (the nonce
// counter is managed by the callers, not by require_stored_admin_auth itself).
// Multiple charges across different intervals must all succeed with the same
// admin key.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn charge_subscription_multiple_intervals_same_admin() {
    let (env, client, _admin, _token) = setup();
    let (id, _, _) = make_funded_subscription(&env, &client);

    // First charge after interval 1.
    jump(&env, INTERVAL + 1);
    let res1 = client.try_charge_subscription(&id, &None::<BytesN<32>>);
    assert!(res1.is_ok(), "first charge must succeed");

    let sub1 = client.get_subscription(&id);
    assert_eq!(sub1.prepaid_balance, PREPAID - AMOUNT);

    // Second charge after interval 2.
    jump(&env, INTERVAL + 1);
    let res2 = client.try_charge_subscription(&id, &None::<BytesN<32>>);
    assert!(res2.is_ok(), "second charge must succeed");

    let sub2 = client.get_subscription(&id);
    assert_eq!(sub2.prepaid_balance, PREPAID - 2 * AMOUNT);
}

// ═════════════════════════════════════════════════════════════════════════════
// 9. BATCH NONCE REPLAY PROTECTION
//
// `do_batch_charge` calls `require_stored_admin_auth` then checks the nonce.
// Replaying the same nonce must fail after the first successful batch_charge,
// even with valid admin auth. State must be unchanged for the replayed call.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn batch_charge_nonce_replay_rejected() {
    let (env, client, _admin, _token) = setup();
    let (id, _, _) = make_funded_subscription(&env, &client);
    jump(&env, INTERVAL + 1);

    let ids: SorobanVec<u32> = SorobanVec::from_array(&env, [id]);

    // First call with nonce 0 succeeds.
    let results = client.batch_charge(&ids, &0u64);
    assert!(results.get(0).unwrap().success, "first batch_charge must succeed");

    let balance_after_first = client.get_subscription(&id).prepaid_balance;
    assert_eq!(balance_after_first, PREPAID - AMOUNT);

    // Replaying nonce 0 must fail even with correct admin auth.
    let replay_res = client.try_batch_charge(&ids, &0u64);
    assert!(
        replay_res.is_err(),
        "replayed nonce must be rejected by batch_charge"
    );

    // Balance must be unchanged after the replay rejection.
    assert_eq!(
        client.get_subscription(&id).prepaid_balance,
        balance_after_first,
        "balance must not change on nonce-replay rejection"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// 10. MULTIPLE SUBSCRIPTIONS — batch and single both enforce the same admin
//
// When two subscriptions exist, require_stored_admin_auth must protect all
// of them equally. A wrong signer cannot partially charge even one id.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn batch_charge_wrong_signer_protects_all_subscriptions() {
    let (env, client, _admin, _token) = setup();
    let (id0, _, _) = make_funded_subscription(&env, &client);
    let (id1, _, _) = make_funded_subscription(&env, &client);
    jump(&env, INTERVAL + 1);

    let ids: SorobanVec<u32> = SorobanVec::from_array(&env, [id0, id1]);
    let stranger = Address::generate(&env);
    mock_only_signer(
        &env,
        &client,
        &stranger,
        "batch_charge",
        batch_charge_args(&env, &ids, 0),
    );

    let res = client.try_batch_charge(&ids, &0u64);
    assert!(res.is_err(), "wrong signer must fail for entire batch");

    // Both subscriptions must retain their original balance.
    assert_eq!(
        client.get_subscription(&id0).prepaid_balance,
        PREPAID,
        "subscription 0 must be unchanged"
    );
    assert_eq!(
        client.get_subscription(&id1).prepaid_balance,
        PREPAID,
        "subscription 1 must be unchanged"
    );
}
