#![cfg(test)]

//! Authorization tests for the charge family of entrypoints.
//!
//! `charge_subscription`, `charge_usage`, `charge_usage_with_reference`, and
//! `batch_charge` move funds from subscriber vaults to merchant earnings and
//! must therefore be admin-only. The stored admin is loaded from
//! `DataKey::Admin` and required to sign; there is no caller-supplied admin
//! parameter, so a signed-but-unauthorized caller is rejected at the host layer
//! and a stale admin is rejected after rotation.
//!
//! See `docs/admin_authorization_matrix.md` and
//! `docs/deterministic_charging.md`.

use crate::{
    types::{DataKey, SubscriptionStatus},
    ChargeExecutionResult, Error, SubscriptionVault, SubscriptionVaultClient, UsageChargeResult,
};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, BytesN, Env, IntoVal, String, Vec as SorobanVec};

const T0: u64 = 1_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const AMOUNT: i128 = 10_000_000;
const PREPAID: i128 = 50_000_000;

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

fn jump(env: &Env, seconds: u64) {
    let t = env.ledger().timestamp();
    env.ledger().with_mut(|l| l.timestamp = t + seconds);
}

/// Build a mock auth descriptor for `fn` signed by `signer`, so the host
/// accepts `signer`'s signature for exactly that call.
#[allow(clippy::type_complexity)]
fn mock_signature_for(
    env: &Env,
    client: &SubscriptionVaultClient,
    signer: &Address,
    fn_name: &str,
    args: SorobanVec<soroban_sdk::Val>,
) {
    use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};
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

fn charge_subscription_args(env: &Env, sub_id: u32) -> SorobanVec<soroban_sdk::Val> {
    let mut args = SorobanVec::new(&env);
    args.push_back(sub_id.into_val(env));
    args.push_back(None::<BytesN<32>>.into_val(env));
    args
}

// ── charge_subscription: admin-only ──────────────────────────────────────────

#[test]
fn test_charge_subscription_rejects_no_signature() {
    let (env, client, _admin, _token) = setup();

    let (id, _, _) =
        crate::test_utils::fixtures::create_subscription(&env, &client, SubscriptionStatus::Active);
    crate::test_utils::fixtures::seed_balance(&env, &client, id, PREPAID);
    jump(&env, INTERVAL + 1);

    // No admin signature present: require_auth fails at the host layer.
    env.mock_auths(&[]);
    let res = client.try_charge_subscription(&id, &None::<BytesN<32>>);
    assert!(res.is_err(), "unsigned charge must be rejected");

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID, "no funds may move on rejection");
}

#[test]
fn test_charge_subscription_rejects_signed_non_admin() {
    let (env, client, _admin, _token) = setup();
    let stranger = Address::generate(&env);

    let (id, _, _) =
        crate::test_utils::fixtures::create_subscription(&env, &client, SubscriptionStatus::Active);
    crate::test_utils::fixtures::seed_balance(&env, &client, id, PREPAID);
    jump(&env, INTERVAL + 1);

    // A signed transaction where the *stranger* (not the stored admin) is the
    // authorizer must still fail: the stored admin is loaded and required to
    // sign, so a stranger's signature cannot authorize the charge.
    mock_signature_for(
        &env,
        &client,
        &stranger,
        "charge_subscription",
        charge_subscription_args(&env, id),
    );
    let res = client.try_charge_subscription(&id, &None::<BytesN<32>>);
    assert!(res.is_err(), "signed non-admin charge must be rejected");

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID, "no funds may move on rejection");
}

#[test]
fn test_charge_subscription_admin_succeeds() {
    let (env, client, _admin, token) = setup();

    let (id, _subscriber, merchant) =
        crate::test_utils::fixtures::create_subscription(&env, &client, SubscriptionStatus::Active);
    crate::test_utils::fixtures::seed_balance(&env, &client, id, PREPAID);
    jump(&env, INTERVAL + 1);

    let res = client.try_charge_subscription(&id, &None::<BytesN<32>>);
    assert_eq!(res, Ok(Ok(ChargeExecutionResult::Charged)));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID - AMOUNT);
    let mb = client.get_merchant_balance_by_token(&merchant, &token);
    assert_eq!(mb, AMOUNT);
}

#[test]
fn test_charge_subscription_admin_unset_fails_closed() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    // Contract never initialized: no stored admin, so charging fails closed.
    let res = client.try_charge_subscription(&1u32, &None::<BytesN<32>>);
    assert_eq!(res, Err(Ok(Error::NotInitialized)));
}

#[test]
fn test_charge_subscription_stale_admin_rejected_after_rotation() {
    let (env, client, admin, token) = setup();

    let (id, _subscriber, merchant) =
        crate::test_utils::fixtures::create_subscription(&env, &client, SubscriptionStatus::Active);
    crate::test_utils::fixtures::seed_balance(&env, &client, id, PREPAID);
    jump(&env, INTERVAL + 1);

    let new_admin = Address::generate(&env);
    client.rotate_admin(&admin, &new_admin, &0u64);
    assert_eq!(client.get_admin(), new_admin);

    // Old admin signature no longer satisfies the stored-admin check.
    mock_signature_for(
        &env,
        &client,
        &admin,
        "charge_subscription",
        charge_subscription_args(&env, id),
    );
    let stale_res = client.try_charge_subscription(&id, &None::<BytesN<32>>);
    assert!(stale_res.is_err(), "stale admin after rotation must be rejected");

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID, "no funds may move on rejection");

    // New admin is authorized and can charge normally.
    env.mock_all_auths();
    let res = client.try_charge_subscription(&id, &None::<BytesN<32>>);
    assert_eq!(res, Ok(Ok(ChargeExecutionResult::Charged)));
    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID - AMOUNT);
    let mb = client.get_merchant_balance_by_token(&merchant, &token);
    assert_eq!(mb, AMOUNT);
}

// ── charge_usage / charge_usage_with_reference: admin-only ───────────────────

fn seed_usage_subscription(
    env: &Env,
    client: &SubscriptionVaultClient,
) -> (u32, Address, Address) {
    let (id, subscriber, merchant) =
        crate::test_utils::fixtures::create_subscription(&env, &client, SubscriptionStatus::Active);
    let mut sub = client.get_subscription(&id);
    sub.usage_enabled = true;
    env.as_contract(&client.address, || {
        env.storage().persistent().set(&DataKey::Sub(id), &sub);
    });
    crate::test_utils::fixtures::seed_balance(&env, &client, id, PREPAID);
    (id, subscriber, merchant)
}

fn charge_usage_args(env: &Env, sub_id: u32, amount: i128) -> SorobanVec<soroban_sdk::Val> {
    let mut args = SorobanVec::new(&env);
    args.push_back(sub_id.into_val(env));
    args.push_back(amount.into_val(env));
    args
}

#[test]
fn test_charge_usage_rejects_signed_non_admin() {
    let (env, client, _admin, token) = setup();
    let stranger = Address::generate(&env);

    let (id, _merchant, merchant) = seed_usage_subscription(&env, &client);

    mock_signature_for(
        &env,
        &client,
        &stranger,
        "charge_usage",
        charge_usage_args(&env, id, AMOUNT),
    );
    let res = client.try_charge_usage(&id, &AMOUNT);
    assert!(res.is_err(), "signed non-admin usage charge must be rejected");

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID, "no funds may move on rejection");
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        0,
        "merchant must not be credited"
    );
}

#[test]
fn test_charge_usage_admin_succeeds() {
    let (env, client, _admin, token) = setup();

    let (id, _merchant, merchant) = seed_usage_subscription(&env, &client);

    let res = client.try_charge_usage(&id, &AMOUNT);
    assert_eq!(res, Ok(Ok(UsageChargeResult::Charged)));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID - AMOUNT);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), AMOUNT);
}

#[test]
fn test_charge_usage_with_reference_rejects_signed_non_admin() {
    let (env, client, _admin, _token) = setup();
    let stranger = Address::generate(&env);

    let (id, _merchant, _merchant_addr) = seed_usage_subscription(&env, &client);
    let reference = String::from_str(&env, "meter-1");

    mock_signature_for(
        &env,
        &client,
        &stranger,
        "charge_usage_with_reference",
        charge_usage_args(&env, id, AMOUNT),
    );
    let res = client.try_charge_usage_with_reference(&id, &AMOUNT, &reference);
    assert!(res.is_err(), "signed non-admin usage-with-reference charge must be rejected");

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID, "no funds may move on rejection");
}

#[test]
fn test_charge_usage_with_reference_admin_succeeds() {
    let (env, client, _admin, token) = setup();
    let (id, _merchant, merchant) = seed_usage_subscription(&env, &client);
    let reference = String::from_str(&env, "meter-1");

    let res = client.try_charge_usage_with_reference(&id, &AMOUNT, &reference);
    assert_eq!(res, Ok(Ok(UsageChargeResult::Charged)));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID - AMOUNT);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), AMOUNT);
}

// ── batch_charge: admin-only (regression) ────────────────────────────────────

#[test]
fn test_batch_charge_rejects_signed_non_admin() {
    let (env, client, _admin, _token) = setup();
    let stranger = Address::generate(&env);

    let (id, _, _) =
        crate::test_utils::fixtures::create_subscription(&env, &client, SubscriptionStatus::Active);
    crate::test_utils::fixtures::seed_balance(&env, &client, id, PREPAID);
    jump(&env, INTERVAL + 1);

    let mut args = SorobanVec::new(&env);
    let ids: SorobanVec<u32> = SorobanVec::from_array(&env, [id]);
    args.push_back(ids.into_val(&env));
    args.push_back(0u64.into_val(&env));

    mock_signature_for(&env, &client, &stranger, "batch_charge", args);
    let res = client.try_batch_charge(&ids, &0u64);
    assert!(res.is_err(), "signed non-admin batch charge must be rejected");

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID, "no funds may move on rejection");
}

#[test]
fn test_batch_charge_admin_succeeds() {
    let (env, client, _admin, token) = setup();

    let (id, _subscriber, merchant) =
        crate::test_utils::fixtures::create_subscription(&env, &client, SubscriptionStatus::Active);
    crate::test_utils::fixtures::seed_balance(&env, &client, id, PREPAID);
    jump(&env, INTERVAL + 1);

    let ids: SorobanVec<u32> = SorobanVec::from_array(&env, [id]);
    let results = client.batch_charge(&ids, &0u64);
    assert_eq!(results.len(), 1);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID - AMOUNT);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), AMOUNT);
}