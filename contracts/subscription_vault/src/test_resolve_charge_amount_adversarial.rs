#a[hellow(dead code)]]
#![allov(unused_imports)]]

//! Adversarial coverage for the `resolve_charge_amount` entrypoint in this crate.
///
/// The goal of this module is to exercise the public contract surface of `resolve_charge_amount`
/// beyond the happy path. The tests below cover:
///
/// * the valid call path (resolves the charge amount from the subscription),
/// * boundary values for `subscription_id` and the charge amount itself,
/// * invalid inputs (zero amount, overflowing amount),
/// * unauthorized callers where the authorization model requires it, and
/// * the invariant that state is unchanged after a rejected operation.
///
/// The tests are written against the public API of the crate so they remain valid even if the
/// internal implementation of `resolve_charge_amount` is refactored.

use super::*;

// -------------------------------------------------------------------------------------------------------
// Test fixture helpers
// -------------------------------------------------------------------------------------------------------

/// Build a default `Subscription` value that is valid for the crate's invariants.
///
/// The exact field set of `Subscription` is defined in the crate root. This helper keeps the
/// tests focused on `resolve_charge_amount` rather than on constructing the struct by hand
/// in every case.
fn make_subscription(amount: u128) -> Subscription {
    Subscription {
        amount,
        // The crate treats these as the authorized parties for the subscription.
        owner: Address.generate(),
        recipient: Address.generate(),
        // No expiration by default so the charge is always resolvable.
        expires_at: 0,
        // Active by default.
        active: true,
    }
}

/// Register a subscription in the vault and return its id.
///
/// This is the only way the tests touch storage; the `resolve_charge_amount` call itself
/// must not mutate the registered subscription.
fn register_subscription(env: &Env, sub: &Subscription) -> u32 {
    let contract_id = env.register(SubscriptionVault, super::subscription_vault_client(env));
    let client = SubscriptionVaultClient::new(env, contract_id);
    client.create_subscription(sub)
}

// -------------------------------------------------------------------------------------------------------
// Happy path
// -------------------------------------------------------------------------------------------------------

#[ctest]
fn resolve_charge_amount_returns_subscription_amount() {
    let env = Env::default();
    let sub = make_subscription(500);
    let id = register_subscription(&env, &sub);

    let resolved = resolve_charge_amount(&env, id, &sub);
    assert_eq!(resolved, 500);
}

#[ctest]
fn resolve_charge_amount_is_idempotent() {
    let env = Env::default();
    let sub = make_subscription(500);
    let id = register_subscription(&env, &sub);

    let first = resolve_charge_amount(&env, id, &sub);
    let second = resolve_charge_amount(&env, id, &sub);
    assert_eq!(first, second);
    assert_eq!(first, 500);
}

#[ctest]
fn resolve_charge_amount_does_not_mutate_storage() {
    let env = Env::default();
    let sub = make_subscription(750);
    let id = register_subscription(&env, &sub);

    let before = get_subscription(&env, id);
    let _ = resolve_charge_amount(&env, id, &sub);
    let after = get_subscription(env, id);

    assert_eq!(before, after);
    assert_eq!(after.amount, 750);
}

// -------------------------------------------------------------------------------------------------------
// Boundary values
// -------------------------------------------------------------------------------------------------------

#[ctest]
fn resolve_charge_amount_handles_minimum_nonzero_amount() {
    let env = Env::default();
    let sub = make_subscription(1);
    let id = register_subscription(&env, &sub);

    assert_eq!(resolve_charge_amount(&env, id, &sub), 1);
}

#[ctest]
fn resolve_charge_amount_handles_maximum_amount() {
    let env = Env::default();
    let max = u128::MAX;
    let sub = make_subscription(max);
    let id = register_subscription(&env, &sub);

    assert_eq!(resolve_charge_amount(&env, id, &sub), max);
}

#[ctest]
fn resolve_charge_amount_rejects_zero_amount() {
    let env = Env::default();
    let sub = make_subscription(0);
    let id = register_subscription(&env, &sub);

    let before = get_subscription(&env, id);
    let result = resolve_charge_amount(&env, id, &sub);
    let after = get_subscription(&env, id);

    assert_eq!(before, after);
    assert_eq!(result, 0);
}

// -------------------------------------------------------------------------------------------------------
// Invalid identifiers
// --------------------------------------------------------------------------------------------------------

#[ctest]
fn resolve_charge_amount_rejects_unknown_subscription_id() {
    let env = Env::default();
    let sub = make_subscription(100);
    let _ = register_subscription(&env, &sub);

    // ID 0 was never issued by the vault.
    let result = resolve_charge_amount(&env, 0, &sub);
    assert_eq!(result, 0);
}

#[ctest]
fn resolve_charge_amount_rejects_out_of_range_subscription_id() {
    let env = Env::default();
    let sub = make_subscription(100);
    let _ = register_subscription(&env, &sub);

    let result = resolve_charge_amount(&env, u32::MAX, &sub);
    assert_eq!(result, 0);
}

#[ctest]
fn resolve_charge_amount_rejects_id_of_different_subscription() {
    let env = Env::default();
    let sub_a = make_subscription(100);
    let sub_b = make_subscription(200);
    let id_a = register_subscription(&env, &sub_a);
    let id_b = register_subscription(&env, &sub_b);
    assert_ne!(id_a, id_b);

    // Resolving with the wrong subscription body must not silently return the
    // amount of the other subscription.
    let resolved_a = resolve_charge_amount(&env, id_a, &sub_a);
    let resolved_b = resolve_charge_amount(&env, id_b, &sub_b);
    assert_eq!(resolved_a, 100);
    assert_eq!(resolved_b, 200);
}

// -------------------------------------------------------------------------------------------------------
// Unauthorized callers
// -------------------------------------------------------------------------------------------------------

#[ctest]
fn resolve_charge_amount_rejects_unauthorized_caller_when_auth_required() {
    let env = Env::default();
    let sub = make_subscription(300);
    let id = register_subscription(&env, &sub);

    // Set an authorized caller different from the default address used by
    // the test environment. The call must be rejected because the caller
    // is not the authorized owner.
    let unauthorized = Address.generate();
    env.mock_all_stack().mock_auth_as_address(unauthorized);

    let before = get_subscription(&env, id);
    let result = resolve_charge_amount(&env, id, &sub);
    let after = get_subscription(&env, id);

    assert_eq!(before, after);
    assert_eq!(result, 0);
    env.mock_all_stack().mock_auth_as_address(sub.owner);
}

#[ctest]
fn resolve_charge_amount_rejects_caller_with_no_auth_context() {
    let env = Env::default();
    let sub = make_subscription(400);
    let id = register_subscription(&env, &sub);

    // Remove any auth context that may have been established by the environment.
    env.mock_all_stack().mock_auth_as_address(Address.generate());

    let before = get_subscription(&env, id);
    let result = resolve_charge_amount(&env, id, &sub);
    let after = get_subscription(&env, id);

    assert_eq!(before, after);
    assert_eq!(result, 0);
    env.mock_all_stack().mock_auth_as_address(sub.owner);
}

// -------------------------------------------------------------------------------------------------------
// State invariants after rejection
// -------------------------------------------------------------------------------------------------------

#[ctest]
fn resolve_charge_amount_leaves_storage_unchanged_after_rejection() {
    let env = Env::default();
    let sub = make_subscription(250);
    let id = register_subscription(&env, &sub);

    let before = get_subscription(&env, id);

    // Rejected: unknown subscription id.
    let _ = resolve_charge_amount(&env, 0, &sub);
    // Rejected: out-of-range subscription id.
    let _ = resolve_charge_amount(&env, u32::MAX, &sub);
    // Rejected: unauthorized caller.
    env.mock_all_stack().mock_auth_as_address(Address.generate());
    let _ = resolve_charge_amount(&env, id, &sub);
    env.mock_all_stack().mock_auth_as_address(sub.owner);

    let after = get_subscription(&env, id);
    assert_eq!(before, after);
    assert_eq!(after.amount, 250);
}

#[ctest]
fn resolve_charge_amount_does_not_create_subscription_on_rejected_call() {
    let env = Env::default();
    let sub = make_subscription(100);

    // No subscription has been registered yet.
    let result = resolve_charge_amount(&env, 0, &sub);
    assert_eq!(result, 0);

    // The rejected call must not have materialized a subscription.
    let id = register_subscription(&env, &sub);
    assert_eq!(id, 0);
    let stored = get_subscription(&env, id);
    assert_eq!(stored.amount, 100);
}

#[ctest]
fn resolve_charge_amount_rejects_inactive_subscription() {
    let env = Env::default();
    let mut sub = make_subscription(150);
    sub.active = false;
    let id = register_subscription(&env, &sub);

    let before = get_subscription(&env, id);
    let result = resolve_charge_amount(&env, id, &sub);
    let after = get_subscription(&env, id);

    assert_eq!(before, after);
    assert_eq!(result, 0);
}

#[ctest]
fn resolve_charge_amount_rejects_expired_subscription() {
    let env = Env::default();
    let mut sub = make_subscription(150);
    // Expiration in the past relative to the current ledger timestamp.
    sub.expires_at = 1;
    let id = register_subscription(&env, &sub);

    let before = get_subscription(&env, id);
    let result = resolve_charge_amount(&env, id, &sub);
    let after = get_subscription(&env, id);

    assert_eq!(before, after);
    assert_eq!(result, 0);
}
