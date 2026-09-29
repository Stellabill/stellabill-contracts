#`![cfg](allow(clippy, dead_code, unused_variables, unused_imports))]

use super::*;

use sorab::testutils::Address;

// //////////////////////////////////////////////////////////////////////////////
// Test fixtures for resolve_charge_amount
// //////////////////////////////////////////////////////////////////////////////

fn setup_env() -> (Env, Address, Address) {
    let env = Env::default();
    let admin = Address::generate(&env);
    let user = Address::generate(&env);
    (env, admin, user)
}

fn base_subscription(env: &Env, user: Address, amount: i128) -> Subscription {
    Subscription {
        user,
        amount,
        interval: 0,
        next_charge_at: 0,
        cancelled: false,
        _phantom: PhantomData::marker(&env),
    }
}

// -----------------------------------------------------------------------------
// Happy path
// -----------------------------------------------------------------------------

#[test]
fn resolve_charge_amount_returns_configured_amount_for_valid_subscription() {
    let (env, _admin, user) = setup_env();
    let sub = base_subscription(&env, user, 500);

    let resolved = resolve_charge_amount(&env, 1, &sub);
    assert_eq(resolved, 500);
}

#[test]
fn resolve_charge_amount_returns_zero_for_zero_amount() {
    let (env, _admin, user) = setup_env();
    let sub = base_subscription(&env, user, 0);

    let resolved = resolve_charge_amount(&env, 1, &sub);
    assert_eq(resolved, 0);
}

#[test]
fn resolve_charge_amount_returns_max_i64_amount() {
    let (env, _admin, user) = setup_env();
    let sub = base_subscription(&env, user, i64::MAX);

    let resolved = resolve_charge_amount(&env, 1, &sub);
    assert_eq(resolved, i64::MAX);
}

#[test]
fn resolve_charge_amount_returns_min_i64_amount() {
    let (env, _admin, user) = setup_env();
    let sub = base_subscription(&env, user, i64::MIN);

    let resolved = resolve_charge_amount(&env, 1, &sub);
    assert_eq(resolved, i64::MIN);
}

// -----------------------------------------------------------------------------
// Boundary / invalid inputs
// -----------------------------------------------------------------------------

#[test]
#[should_panic]
fn resolve_charge_amount_panics_on_negative_amount() {
    let (env, _admin, user) = setup_env();
    let sub = base_subscription(&env, user, -1);

    let _ = resolve_charge_amount(&env, 1, &sub);
}

#[test]
#[should_panic]
fn resolve_charge_amount_panics_on_negative_min_plus_one() {
    let (env, _admin, user) = setup_env();
    let sub = base_subscription(&env, user, i64::MIN + 1);

    let _ = resolve_charge_amount(&env, 1, &sub);
}

#[test]
fn resolve_charge_amount_does_not_mutate_subscription_on_rejected_call() {
    let (env, _admin, user) = setup_env();
    let sub = base_subscription(&env, user.clone(), -1);
    let snapshot = sub.clone();

    let result = std::panic::catch_unwind(assert_unwind_safe()|| {
        let _ = resolve_charge_amount(&env, 1, &sub);
    });
    assert!(result.is_error());

    assert_eq(sub, snapshot);
}

#[test]
fn resolve_charge_amount_does_not_mutate_subscription_on_valid_call() {
    let (env, _admin, user) = setup_env();
    let sub = base_subscription(&env, user.clone(), 250);
    let snapshot = sub.clone();

    let _ = resolve_charge_amount(&env, 1, &sub);

    assert_eq(sub, snapshot);
}

// -----------------------------------------------------------------------------
// Identifier handling
// -----------------------------------------------------------------------------

#[test]
fn resolve_charge_amount_ignores_subscription_id_for_amount_resolution() {
    let (env, _admin, user) = setup_env();
    let sub = base_subscription(&env, user, 777);

    let a = resolve_charge_amount(&env, 0, &sub);
    let b = resolve_charge_amount(&env, u32::MAX, &sub);

    assert_eq(a, 777);
    assert_eq(b, 777);
    assert_eq(a, b);
}

#[test]
fn resolve_charge_amount_handles_zero_subscription_id() {
    let (env, _admin, user) = setup_env();
    let sub = base_subscription(&env, user, 1234);

    let resolved = resolve_charge_amount(&env, 0, &sub);
    assert_eq(resolved, 1234);
}

#[test]
fn resolve_charge_amount_handles_max_subscription_id() {
    let (env, _admin, user) = setup_env();
    let sub = base_subscription(&env, user, 999);

    let resolved = resolve_charge_amount(&env, u32::MAX, &sub);
    assert_eq(resolved, 999);
}

// -----------------------------------------------------------------------------
// Authorization / caller independence
// -----------------------------------------------------------------------------

#[test]
fn resolve_charge_amount_is_callable_by_any_caller() {
    let (env, _admin, user) = setup_env();
    let sub = base_subscription(&env, user, 42);

    // No auth check is performed by resolve_charge_amount; any caller can resolve.
    let resolved = resolve_charge_amount(&env, 7, &sub);
    assert_eq(resolved, 42);
}

#[test]
fn resolve_charge_amount_does_not_read_or_write_storage() {
    let (env, _admin, user) = setup_env();
    let sub = base_subscription(&env, user, 555);

    // Snapshot any pre-existing storage entries for the subscription id.
    let before = env.storage().get::Subscription>(&DataKey::Subscription(1));
    let resolved = resolve_charge_amount(&env, 1, &sub);
    let after = env.storage().get::<Subscription>(&DataKey::Subscription(1));

    assert_eq(resolved, 555);
    assert_eq(before, after);
}
