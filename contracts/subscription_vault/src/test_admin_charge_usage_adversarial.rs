#![cfg(test)]

//! Adversarial coverage for [`crate::admin::do_charge_usage`].
//!
//! `do_charge_usage` is the admin-only helper that wraps
//! [`crate::charge_core::charge_usage_one`]. Unlike the public
//! `charge_usage` / `charge_usage_with_reference` entrypoints, it returns
//! `Result<(), Error>` and deliberately discards the `UsageChargeResult`
//! produced by the core, so a "rejected" usage charge (replay, burst/rate
//! limit, per-period cap, over-cap cancellation) is reported to the caller as
//! `Ok(())`. This module pins the observable invariants that make that
//! contract safe:
//!
//! 1. **Auth first, fail closed.** The stored admin must sign; when no admin is
//!    configured the call returns `NotInitialized` instead of charging, and a
//!    missing signature is rejected before the subscription is even looked up.
//! 2. **Rejections never move funds.** Zero/negative/overflowing amounts,
//!    unknown ids, inactive or usage-disabled subscriptions, and insufficient
//!    prepaid balances all leave the prepaid balance, lifetime counter and
//!    status untouched.
//! 3. **The `reference` string is a per-subscription idempotency key.** A
//!    duplicate reference is never charged twice, even though the second call
//!    still returns `Ok(())`, and the key is *not* consumed by a charge that a
//!    usage limit rejects.
//! 4. **Boundaries are explicit.** Charging exactly the whole balance drains it
//!    and parks the subscription in `InsufficientBalance`; crossing the
//!    lifetime cap cancels without debiting; `i128::MIN`/`i128::MAX` are handled
//!    without arithmetic panics.
//!
//! See `docs/deterministic_charging.md` and the usage-charge sections of
//! `docs/subscription_lifecycle.md`.

use crate::{
    types::{DataKey, Error, SubscriptionStatus, UsageState},
    SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, String, Symbol,
};

const T0: u64 = 1_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const AMOUNT: i128 = 10_000_000; // 1.0 unit (6-decimal token)
const PREPAID: i128 = 50_000_000; // 5.0 units

// ── Harness ───────────────────────────────────────────────────────────────────

/// Fresh, initialized vault with a real Stellar asset contract. Returns the
/// deployed token address alongside the client so tests can assert credits.
fn setup() -> (Env, SubscriptionVaultClient<'static>, Address) {
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

    (env, client, token)
}

/// Create a subscription (optionally in a non-`Active` state), force
/// `usage_enabled = true` directly in storage, and optionally seed a prepaid
/// balance. Returns `(id, subscriber, merchant)`.
fn create_usage_sub(
    env: &Env,
    client: &SubscriptionVaultClient,
    status: SubscriptionStatus,
    prepaid: i128,
) -> (u32, Address, Address) {
    let (id, subscriber, merchant) =
        crate::test_utils::fixtures::create_subscription(env, client, status);

    let mut sub = client.get_subscription(&id);
    sub.usage_enabled = true;
    env.as_contract(&client.address, || {
        env.storage().persistent().set(&DataKey::Sub(id), &sub);
    });

    if prepaid != 0 {
        crate::test_utils::fixtures::seed_balance(env, client, id, prepaid);
    }

    (id, subscriber, merchant)
}

/// Rewrite the stored subscription's cap fields. Used to place the
/// subscription on either side of the lifetime-cap boundary.
fn set_lifetime(env: &Env, client: &SubscriptionVaultClient, id: u32, cap: i128, charged: i128) {
    let mut sub = client.get_subscription(&id);
    sub.lifetime_cap = Some(cap);
    sub.lifetime_charged = charged;
    env.as_contract(&client.address, || {
        env.storage().persistent().set(&DataKey::Sub(id), &sub);
    });
}

/// Invoke the internal admin helper exactly as an admin billing worker would.
fn do_charge_usage(
    env: &Env,
    client: &SubscriptionVaultClient,
    id: u32,
    amount: i128,
    reference: &str,
) -> Result<(), Error> {
    let reference = String::from_str(env, reference);
    env.as_contract(&client.address, || {
        crate::admin::do_charge_usage(env, id, amount, reference)
    })
}

/// `true` when the `(usage_ref, subscription_id, reference)` replay marker is
/// present in instance storage.
fn ref_marker_present(
    env: &Env,
    client: &SubscriptionVaultClient,
    id: u32,
    reference: &str,
) -> bool {
    let reference = String::from_str(env, reference);
    env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .has(&(Symbol::new(env, "usage_ref"), id, reference))
    })
}

fn read_usage_state(
    env: &Env,
    client: &SubscriptionVaultClient,
    id: u32,
) -> Option<UsageState> {
    env.as_contract(&client.address, || {
        env.storage().instance().get(&DataKey::UsageState(id))
    })
}

// ── 1. Valid calls ────────────────────────────────────────────────────────────

#[test]
fn valid_usage_charge_debits_prepaid_and_credits_merchant() {
    let (env, client, token) = setup();
    let (id, _subscriber, merchant) = create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);

    let res = do_charge_usage(&env, &client, id, AMOUNT, "meter-1");
    assert_eq!(res, Ok(()));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID - AMOUNT);
    assert_eq!(sub.lifetime_charged, AMOUNT);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        AMOUNT,
        "the full charge is credited to the merchant when no fee is configured"
    );
    assert!(
        ref_marker_present(&env, &client, id, "meter-1"),
        "successful charge must consume the reference so it cannot be replayed"
    );
    assert!(
        read_usage_state(&env, &client, id).is_none(),
        "no UsageState is written when the subscription has no limits configured"
    );
}

#[test]
fn usage_amount_equal_to_full_balance_zeroes_balance_and_marks_insufficient() {
    let (env, client, _token) = setup();
    let (id, _subscriber, _merchant) = create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);

    let res = do_charge_usage(&env, &client, id, PREPAID, "drain");
    assert_eq!(res, Ok(()));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 0);
    assert_eq!(sub.lifetime_charged, PREPAID);
    assert_eq!(
        sub.status,
        SubscriptionStatus::InsufficientBalance,
        "a charge that exhausts the prepaid balance parks the subscription"
    );
    assert!(ref_marker_present(&env, &client, id, "drain"));
}

// ── 2. Boundary / invalid `usage_amount` ──────────────────────────────────────

#[test]
fn zero_usage_amount_is_rejected_and_state_unchanged() {
    let (env, client, token) = setup();
    let (id, _subscriber, merchant) = create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);
    let before = client.get_subscription(&id);

    assert_eq!(
        do_charge_usage(&env, &client, id, 0, "zero"),
        Err(Error::InvalidAmount)
    );

    let after = client.get_subscription(&id);
    assert_eq!(after.prepaid_balance, before.prepaid_balance);
    assert_eq!(after.lifetime_charged, before.lifetime_charged);
    assert_eq!(after.status, before.status);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), 0);
    assert!(!ref_marker_present(&env, &client, id, "zero"));
    assert!(read_usage_state(&env, &client, id).is_none());
}

#[test]
fn negative_usage_amount_is_rejected_and_state_unchanged() {
    let (env, client, _token) = setup();
    let (id, _subscriber, _merchant) = create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);
    let before = client.get_subscription(&id);

    assert_eq!(
        do_charge_usage(&env, &client, id, -1, "negative"),
        Err(Error::InvalidAmount)
    );

    let after = client.get_subscription(&id);
    assert_eq!(after.prepaid_balance, before.prepaid_balance);
    assert_eq!(after.lifetime_charged, before.lifetime_charged);
    assert!(!ref_marker_present(&env, &client, id, "negative"));
}

#[test]
fn i128_min_usage_amount_is_rejected_as_invalid() {
    let (env, client, _token) = setup();
    let (id, _subscriber, _merchant) = create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);

    assert_eq!(
        do_charge_usage(&env, &client, id, i128::MIN, "min"),
        Err(Error::InvalidAmount)
    );
    assert_eq!(client.get_subscription(&id).prepaid_balance, PREPAID);
}

#[test]
fn i128_max_usage_amount_is_rejected_as_insufficient_without_overflow() {
    let (env, client, _token) = setup();
    let (id, _subscriber, _merchant) = create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);

    assert_eq!(
        do_charge_usage(&env, &client, id, i128::MAX, "max"),
        Err(Error::InsufficientPrepaidBalance)
    );

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, PREPAID);
    assert_eq!(sub.lifetime_charged, 0, "no counter may saturate past i128");
    assert!(!ref_marker_present(&env, &client, id, "max"));
}

#[test]
fn insufficient_prepaid_balance_is_rejected_without_debit() {
    let (env, client, token) = setup();
    let (id, _subscriber, merchant) = create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);
    let before = client.get_subscription(&id);

    assert_eq!(
        do_charge_usage(&env, &client, id, PREPAID + 1, "short"),
        Err(Error::InsufficientPrepaidBalance)
    );

    let after = client.get_subscription(&id);
    assert_eq!(after.prepaid_balance, before.prepaid_balance);
    assert_eq!(after.lifetime_charged, before.lifetime_charged);
    assert_eq!(after.status, before.status);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), 0);
    assert!(
        !ref_marker_present(&env, &client, id, "short"),
        "a rejected charge must not burn the reference"
    );
}

// ── 3. Unknown ids and inactive / usage-disabled subscriptions ────────────────

#[test]
fn unknown_subscription_id_is_rejected_with_not_found() {
    let (env, client, _token) = setup();
    // Initialized vault, but no subscription 7 exists.
    assert_eq!(
        do_charge_usage(&env, &client, 7, AMOUNT, "ghost"),
        Err(Error::NotFound)
    );
}

#[test]
fn usage_not_enabled_is_rejected_and_state_unchanged() {
    let (env, client, token) = setup();
    let (id, _subscriber, merchant) =
        crate::test_utils::fixtures::create_subscription(&env, &client, SubscriptionStatus::Active);
    crate::test_utils::fixtures::seed_balance(&env, &client, id, PREPAID);
    let before = client.get_subscription(&id);

    assert_eq!(
        do_charge_usage(&env, &client, id, AMOUNT, "no-usage"),
        Err(Error::UsageNotEnabled)
    );

    let after = client.get_subscription(&id);
    assert_eq!(after.prepaid_balance, before.prepaid_balance);
    assert_eq!(after.lifetime_charged, 0);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), 0);
    assert!(!ref_marker_present(&env, &client, id, "no-usage"));
}

#[test]
fn cancelled_subscription_is_rejected_as_not_active() {
    let (env, client, token) = setup();
    let (id, _subscriber, merchant) =
        create_usage_sub(&env, &client, SubscriptionStatus::Cancelled, PREPAID);
    let before = client.get_subscription(&id);

    assert_eq!(
        do_charge_usage(&env, &client, id, AMOUNT, "cancelled"),
        Err(Error::NotActive)
    );

    let after = client.get_subscription(&id);
    assert_eq!(after.prepaid_balance, before.prepaid_balance);
    assert_eq!(after.status, SubscriptionStatus::Cancelled);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), 0);
}

#[test]
fn paused_subscription_is_rejected_as_not_active() {
    let (env, client, _token) = setup();
    let (id, _subscriber, _merchant) =
        create_usage_sub(&env, &client, SubscriptionStatus::Paused, PREPAID);
    let before = client.get_subscription(&id);

    assert_eq!(
        do_charge_usage(&env, &client, id, AMOUNT, "paused"),
        Err(Error::NotActive)
    );

    let after = client.get_subscription(&id);
    assert_eq!(after.prepaid_balance, before.prepaid_balance);
    assert_eq!(after.status, SubscriptionStatus::Paused);
}

// ── 4. Reference is a per-subscription idempotency key ────────────────────────

#[test]
fn duplicate_reference_is_not_charged_twice() {
    let (env, client, token) = setup();
    let (id, _subscriber, merchant) = create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);

    assert_eq!(do_charge_usage(&env, &client, id, AMOUNT, "dup"), Ok(()));
    let after_first = client.get_subscription(&id);

    // The core classifies this as `UsageChargeResult::Replay`, but
    // `do_charge_usage` discards the result and still reports `Ok(())`.
    assert_eq!(do_charge_usage(&env, &client, id, AMOUNT, "dup"), Ok(()));

    let after_second = client.get_subscription(&id);
    assert_eq!(after_second.prepaid_balance, after_first.prepaid_balance);
    assert_eq!(after_second.lifetime_charged, after_first.lifetime_charged);
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        AMOUNT,
        "the merchant is credited exactly once for a repeated reference"
    );
}

#[test]
fn duplicate_reference_is_blocked_even_when_the_amount_differs() {
    let (env, client, token) = setup();
    let (id, _subscriber, merchant) = create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);

    assert_eq!(do_charge_usage(&env, &client, id, AMOUNT, "same-ref"), Ok(()));
    // A smaller amount under the same reference is still a replay: the key,
    // not the amount, is what makes a usage charge unique.
    assert_eq!(do_charge_usage(&env, &client, id, 1, "same-ref"), Ok(()));

    assert_eq!(client.get_subscription(&id).prepaid_balance, PREPAID - AMOUNT);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), AMOUNT);
}

#[test]
fn reference_scope_is_per_subscription() {
    let (env, client, token) = setup();
    let (id_a, _sub_a, merchant_a) =
        create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);
    let (id_b, _sub_b, merchant_b) =
        create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);

    assert_eq!(do_charge_usage(&env, &client, id_a, AMOUNT, "shared"), Ok(()));
    assert_eq!(do_charge_usage(&env, &client, id_b, AMOUNT, "shared"), Ok(()));

    assert_eq!(client.get_subscription(&id_a).prepaid_balance, PREPAID - AMOUNT);
    assert_eq!(client.get_subscription(&id_b).prepaid_balance, PREPAID - AMOUNT);
    assert_eq!(client.get_merchant_balance_by_token(&merchant_a, &token), AMOUNT);
    assert_eq!(client.get_merchant_balance_by_token(&merchant_b, &token), AMOUNT);
}

#[test]
fn empty_reference_still_acts_as_an_idempotency_key() {
    let (env, client, token) = setup();
    let (id, _subscriber, merchant) = create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);

    assert_eq!(do_charge_usage(&env, &client, id, AMOUNT, ""), Ok(()));
    assert_eq!(do_charge_usage(&env, &client, id, AMOUNT, ""), Ok(()));

    assert_eq!(client.get_subscription(&id).prepaid_balance, PREPAID - AMOUNT);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), AMOUNT);
    assert!(ref_marker_present(&env, &client, id, ""));
}

// ── 5. Lifetime-cap boundaries ────────────────────────────────────────────────

#[test]
fn charge_crossing_the_lifetime_cap_cancels_without_debiting() {
    let (env, client, token) = setup();
    let (id, _subscriber, merchant) = create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);
    set_lifetime(&env, &client, id, 5_000_000, 0);

    // 6.0 units would push lifetime_charged past the 0.5-unit cap.
    let res = do_charge_usage(&env, &client, id, 6_000_000, "over-cap");
    assert_eq!(res, Ok(()), "over-cap cancellation is surfaced as Ok by the wrapper");

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(sub.prepaid_balance, PREPAID, "no funds may move on cancellation");
    assert_eq!(sub.lifetime_charged, 0);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), 0);
    assert!(!ref_marker_present(&env, &client, id, "over-cap"));
}

#[test]
fn charge_with_lifetime_cap_already_reached_is_rejected() {
    let (env, client, token) = setup();
    let (id, _subscriber, merchant) = create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);
    set_lifetime(&env, &client, id, 1, 1);

    assert_eq!(
        do_charge_usage(&env, &client, id, 1, "exhausted"),
        Err(Error::LifetimeCapReached)
    );

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(sub.prepaid_balance, PREPAID, "exhausted cap must not debit");
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), 0);
}

// ── 6. Usage-limit rejections do not debit or consume the reference ───────────

#[test]
fn burst_limited_usage_charge_does_not_debit_or_consume_reference() {
    let (env, client, token) = setup();
    let (id, _subscriber, merchant) = create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);

    client.configure_usage_limits(&merchant, &id, &None::<u32>, &0u64, &100u64, &None::<i128>);

    assert_eq!(do_charge_usage(&env, &client, id, AMOUNT, "burst-a"), Ok(()));
    let after_first = client.get_subscription(&id);
    assert!(read_usage_state(&env, &client, id).is_some());

    // Same ledger timestamp → within the burst window. The core returns
    // `BurstLimitExceeded`, which the wrapper discards as `Ok(())`.
    assert_eq!(do_charge_usage(&env, &client, id, AMOUNT, "burst-b"), Ok(()));

    let after_second = client.get_subscription(&id);
    assert_eq!(after_second.prepaid_balance, after_first.prepaid_balance);
    assert_eq!(after_second.lifetime_charged, after_first.lifetime_charged);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), AMOUNT);
    assert!(
        !ref_marker_present(&env, &client, id, "burst-b"),
        "a burst-rejected charge must not consume its reference"
    );
}

#[test]
fn per_period_usage_cap_exceeded_does_not_debit_or_consume_reference() {
    let (env, client, token) = setup();
    let (id, _subscriber, merchant) = create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);

    client.configure_usage_limits(
        &merchant,
        &id,
        &None::<u32>,
        &0u64,
        &0u64,
        &Some(AMOUNT),
    );

    // Exactly at the cap succeeds and consumes "cap-a".
    assert_eq!(do_charge_usage(&env, &client, id, AMOUNT, "cap-a"), Ok(()));
    let after_first = client.get_subscription(&id);
    let state = read_usage_state(&env, &client, id).expect("limits were configured");
    assert_eq!(state.current_period_usage_units, AMOUNT);

    // One more unit exceeds the cap; the wrapper reports Ok but nothing moves.
    assert_eq!(do_charge_usage(&env, &client, id, 1, "cap-b"), Ok(()));

    let after_second = client.get_subscription(&id);
    assert_eq!(after_second.prepaid_balance, after_first.prepaid_balance);
    assert_eq!(after_second.lifetime_charged, after_first.lifetime_charged);
    assert_eq!(client.get_merchant_balance_by_token(&merchant, &token), AMOUNT);
    assert!(!ref_marker_present(&env, &client, id, "cap-b"));
    let state_after = read_usage_state(&env, &client, id).expect("limits were configured");
    assert_eq!(
        state_after.current_period_usage_units, AMOUNT,
        "a capped charge must not advance the per-period counter"
    );
}

// ── 7. Authorization ──────────────────────────────────────────────────────────

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn missing_admin_signature_is_rejected() {
    let (env, client, _token) = setup();
    let (id, _subscriber, _merchant) = create_usage_sub(&env, &client, SubscriptionStatus::Active, PREPAID);

    // Remove the blanket mock: the stored admin's `require_auth()` now has no
    // satisfying signature and must abort the call.
    env.mock_auths(&[]);
    let _ = do_charge_usage(&env, &client, id, AMOUNT, "unsigned");
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn auth_is_checked_before_the_subscription_lookup() {
    let (env, client, _token) = setup();
    env.mock_auths(&[]);

    // A non-existent subscription id must not leak `NotFound`: auth runs first.
    let _ = do_charge_usage(&env, &client, 4_242, AMOUNT, "unsigned-ghost");
}

#[test]
fn uninitialized_contract_fails_closed_without_moving_funds() {
    // No `init` call: no stored admin, so `require_stored_admin_auth` returns
    // `NotInitialized` rather than panicking or accruing anything.
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    assert_eq!(
        do_charge_usage(&env, &client, 0, AMOUNT, "no-admin"),
        Err(Error::NotInitialized)
    );
}
