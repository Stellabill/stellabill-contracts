#![cfg(test)]

//! Admin transfer authorization tests.
//!
//! This test suite verifies that only the current contract admin can initiate
//! transfer_admin (rotate_admin). Any non-admin caller must be rejected with
//! Error::Unauthorized, and the contract state must remain unchanged.
//!
//! Test scenarios:
//! - Happy path: Admin A can successfully transfer to Admin B
//! - Unauthorized: Admin B cannot initiate transfer
//! - Edge cases: missing auth, invalid addresses, multiple unauthorized attempts

use crate::{Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{testutils::Address as _, Address, Env};

// ── Constants ─────────────────────────────────────────────────────────────────

const MIN_TOPUP: i128 = 1_000_000; // 1 USDC

// ── Shared setup helpers ──────────────────────────────────────────────────────

/// Helper to initialize contract with admin and token.
///
/// Returns `(env, client, token_address, admin_address)`.
fn setup_vault<'a>(env: &'a Env, admin: &Address) -> (Address, SubscriptionVaultClient<'a>) {
    let token_admin = Address::generate(env);
    let token_address = env.register_stellar_asset_contract_v2(token_admin).address();
    
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);

    client.init(
        &token_address,
        &6,  // decimals
        admin,
        &MIN_TOPUP,
        &86400,  // grace period
    );

    (token_address, client)
}

// ═════════════════════════════════════════════════════════════════════════════
// Happy path verification
//
// Verify that the current admin can successfully transfer admin privileges.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn happy_path_admin_a_can_transfer_to_admin_b() {
    let env = Env::default();
    env.mock_all_auths();

    let admin_a = Address::generate(&env);
    let admin_b = Address::generate(&env);

    let (_, client) = setup_vault(&env, &admin_a);

    // Confirm Admin A is the active admin before the test
    assert_eq!(client.get_admin(), admin_a);

    // Admin A successfully transfers to Admin B
    client.rotate_admin(&admin_a, &admin_b, &0u64);

    // Verify Admin B is now the active admin
    assert_eq!(client.get_admin(), admin_b);

    // Verify Admin A can no longer perform admin operations
    let result = client.try_set_min_topup(&admin_a, &2_000_000i128);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));

    // Verify Admin B can perform admin operations
    client.set_min_topup(&admin_b, &2_000_000i128);
    assert_eq!(client.get_min_topup(), 2_000_000i128);
}

// ═════════════════════════════════════════════════════════════════════════════
// Unauthorized transfer tests
//
// Verify that non-admin callers are rejected with Error::Unauthorized.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn unauthorized_admin_b_cannot_initiate_transfer() {
    let env = Env::default();
    env.mock_all_auths();

    let admin_a = Address::generate(&env);
    let admin_b = Address::generate(&env);
    let admin_c = Address::generate(&env);

    let (_, client) = setup_vault(&env, &admin_a);

    // Confirm Admin A is the active admin
    assert_eq!(client.get_admin(), admin_a);

    // Admin B attempts to transfer to Admin C
    let result = client.try_rotate_admin(&admin_b, &admin_c, &0u64);

    // Assert the call returns Error::Unauthorized
    assert_eq!(result, Err(Ok(Error::Unauthorized)));

    // Assert Admin A remains the current admin after the failed attempt
    assert_eq!(client.get_admin(), admin_a);

    // Assert no pending admin is created (there is no pending admin concept
    // in this contract - rotation is atomic)
    assert_eq!(client.get_admin(), admin_a);
}

#[test]
fn unauthorized_stranger_cannot_initiate_transfer() {
    let env = Env::default();
    env.mock_all_auths();

    let admin_a = Address::generate(&env);
    let stranger = Address::generate(&env);
    let admin_b = Address::generate(&env);

    let (_, client) = setup_vault(&env, &admin_a);

    // Confirm Admin A is the active admin
    assert_eq!(client.get_admin(), admin_a);

    // A stranger attempts to transfer to Admin B
    let result = client.try_rotate_admin(&stranger, &admin_b, &0u64);

    // Assert the call returns Error::Unauthorized
    assert_eq!(result, Err(Ok(Error::Unauthorized)));

    // Assert Admin A remains the current admin
    assert_eq!(client.get_admin(), admin_a);
}

// ═════════════════════════════════════════════════════════════════════════════
// Edge case: transfer without require_auth
//
// Verify that authorization fails when require_auth is not satisfied.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn transfer_without_require_auth_fails() {
    // No mock_all_auths - require_auth() will fail at the host level
    let env = Env::default();
    // Note: NOT calling env.mock_all_auths()

    let admin_a = Address::generate(&env);
    let admin_b = Address::generate(&env);

    let (_, client) = setup_vault(&env, &admin_a);

    // Attempt transfer without auth - this should panic at require_auth()
    let _ = client.rotate_admin(&admin_a, &admin_b, &0u64);
}

// ═════════════════════════════════════════════════════════════════════════════
// Edge case: transfer using invalid/zero address
//
// Verify that self-rotation and rotation to contract address are rejected.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn self_rotation_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let admin_a = Address::generate(&env);

    let (_, client) = setup_vault(&env, &admin_a);

    // Admin A attempts to rotate to themselves
    let result = client.try_rotate_admin(&admin_a, &admin_a, &0u64);

    // Assert the call returns Error::SelfRotation
    assert_eq!(result, Err(Ok(Error::SelfRotation)));

    // Assert Admin A remains the current admin
    assert_eq!(client.get_admin(), admin_a);
}

#[test]
fn rotation_to_contract_address_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let admin_a = Address::generate(&env);

    let (_, client) = setup_vault(&env, &admin_a);

    // Admin A attempts to rotate to the contract address
    let result = client.try_rotate_admin(&admin_a, &client.address, &0u64);

    // Assert the call returns Error::InvalidNewAdmin
    assert_eq!(result, Err(Ok(Error::InvalidNewAdmin)));

    // Assert Admin A remains the current admin
    assert_eq!(client.get_admin(), admin_a);
}

// ═════════════════════════════════════════════════════════════════════════════
// Edge case: multiple unauthorized transfer calls
//
// Verify that repeated unauthorized attempts all fail and state never changes.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn multiple_unauthorized_transfer_calls_all_fail() {
    let env = Env::default();
    env.mock_all_auths();

    let admin_a = Address::generate(&env);
    let admin_b = Address::generate(&env);
    let admin_c = Address::generate(&env);
    let admin_d = Address::generate(&env);

    let (_, client) = setup_vault(&env, &admin_a);

    // Confirm Admin A is the active admin
    assert_eq!(client.get_admin(), admin_a);

    // First unauthorized attempt: Admin B tries to transfer to Admin C
    let result1 = client.try_rotate_admin(&admin_b, &admin_c, &0u64);
    assert_eq!(result1, Err(Ok(Error::Unauthorized)));
    assert_eq!(client.get_admin(), admin_a);

    // Second unauthorized attempt: Admin B tries to transfer to Admin D
    let result2 = client.try_rotate_admin(&admin_b, &admin_d, &0u64);
    assert_eq!(result2, Err(Ok(Error::Unauthorized)));
    assert_eq!(client.get_admin(), admin_a);

    // Third unauthorized attempt: Admin C tries to transfer to Admin B
    let result3 = client.try_rotate_admin(&admin_c, &admin_b, &0u64);
    assert_eq!(result3, Err(Ok(Error::Unauthorized)));
    assert_eq!(client.get_admin(), admin_a);

    // Fourth unauthorized attempt: Admin D tries to transfer to Admin C
    let result4 = client.try_rotate_admin(&admin_d, &admin_c, &0u64);
    assert_eq!(result4, Err(Ok(Error::Unauthorized)));
    assert_eq!(client.get_admin(), admin_a);

    // Verify that after all unauthorized attempts, Admin A is still the current admin
    assert_eq!(client.get_admin(), admin_a);

    // Verify that Admin A can still successfully perform a legitimate transfer
    client.rotate_admin(&admin_a, &admin_b, &0u64);
    assert_eq!(client.get_admin(), admin_b);
}

#[test]
fn repeated_unauthorized_calls_with_different_nonces_all_fail() {
    let env = Env::default();
    env.mock_all_auths();

    let admin_a = Address::generate(&env);
    let admin_b = Address::generate(&env);
    let admin_c = Address::generate(&env);

    let (_, client) = setup_vault(&env, &admin_a);

    // Confirm Admin A is the active admin
    assert_eq!(client.get_admin(), admin_a);

    // Unauthorized attempts with different nonces
    for nonce in 0u64..5u64 {
        let result = client.try_rotate_admin(&admin_b, &admin_c, &nonce);
        assert_eq!(result, Err(Ok(Error::Unauthorized)));
        assert_eq!(client.get_admin(), admin_a);
    }

    // Verify state never changed
    assert_eq!(client.get_admin(), admin_a);
}

// ═════════════════════════════════════════════════════════════════════════════
// Regression tests
//
// Verify that existing admin functionality is unaffected by authorization tests.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn regression_admin_can_set_min_topup_after_failed_rotation() {
    let env = Env::default();
    env.mock_all_auths();

    let admin_a = Address::generate(&env);
    let admin_b = Address::generate(&env);
    let admin_c = Address::generate(&env);

    let (_, client) = setup_vault(&env, &admin_a);

    // Attempt unauthorized rotation
    let result = client.try_rotate_admin(&admin_b, &admin_c, &0u64);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));

    // Verify Admin A can still perform other admin operations
    client.set_min_topup(&admin_a, &2_000_000i128);
    assert_eq!(client.get_min_topup(), 2_000_000i128);
}

#[test]
fn regression_successful_transfer_still_works() {
    let env = Env::default();
    env.mock_all_auths();

    let admin_a = Address::generate(&env);
    let admin_b = Address::generate(&env);

    let (_, client) = setup_vault(&env, &admin_a);

    // Verify successful transfer still works as expected
    client.rotate_admin(&admin_a, &admin_b, &0u64);
    assert_eq!(client.get_admin(), admin_b);

    // Verify new admin can perform operations
    client.set_min_topup(&admin_b, &3_000_000i128);
    assert_eq!(client.get_min_topup(), 3_000_000i128);
}
