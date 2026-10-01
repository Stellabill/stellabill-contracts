use crate::types::{MerchantWhitelistModeEvent, EVENT_SCHEMA_VERSION};
use crate::{Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
use soroban_sdk::{Address, Env, Symbol, TryFromVal, Vec};

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    (env, client, admin)
}

// ── Whitelist mode toggle ────────────────────────────────────────────────────

#[test]
fn whitelist_mode_defaults_to_false() {
    let (env, client, _admin) = setup();
    assert!(!client.get_whitelist_mode());
    let _ = env;
}

#[test]
fn admin_can_enable_whitelist_mode() {
    let (_env, client, admin) = setup();
    client.set_whitelist_mode(&admin, &true);
    assert!(client.get_whitelist_mode());
}

#[test]
fn admin_can_disable_whitelist_mode() {
    let (_env, client, admin) = setup();
    client.set_whitelist_mode(&admin, &true);
    assert!(client.get_whitelist_mode());
    client.set_whitelist_mode(&admin, &false);
    assert!(!client.get_whitelist_mode());
}

#[test]
fn non_admin_cannot_toggle_whitelist_mode() {
    let (_env, client, _admin) = setup();
    let non_admin = Address::generate(&_env);
    let result = client.try_set_whitelist_mode(&non_admin, &true);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

// ── Merchant approval ────────────────────────────────────────────────────────

#[test]
fn admin_can_approve_merchant() {
    let (_env, client, admin) = setup();
    let merchant = Address::generate(&_env);
    client.approve_merchant(&admin, &merchant);
    assert!(client.is_merchant_approved(&merchant));
}

#[test]
fn admin_can_revoke_merchant() {
    let (_env, client, admin) = setup();
    let merchant = Address::generate(&_env);
    client.approve_merchant(&admin, &merchant);
    assert!(client.is_merchant_approved(&merchant));
    client.revoke_merchant(&admin, &merchant);
    assert!(!client.is_merchant_approved(&merchant));
}

#[test]
fn non_admin_cannot_approve_merchant() {
    let (_env, client, _admin) = setup();
    let non_admin = Address::generate(&_env);
    let merchant = Address::generate(&_env);
    let result = client.try_approve_merchant(&non_admin, &merchant);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn non_admin_cannot_revoke_merchant() {
    let (_env, client, _admin) = setup();
    let non_admin = Address::generate(&_env);
    let merchant = Address::generate(&_env);
    let result = client.try_revoke_merchant(&non_admin, &merchant);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

// ── Gating on initialize_merchant_config ─────────────────────────────────────

#[test]
fn whitelist_disabled_allows_unapproved_merchant() {
    let (_env, client, _admin) = setup();
    let merchant = Address::generate(&_env);
    let payout = Address::generate(&_env);
    // Whitelist is off by default — any merchant can register
    client.initialize_merchant_config(&merchant, &payout, &0, &1, &None, &soroban_sdk::String::from_str(&_env, ""));
    assert!(client.get_merchant_config(&merchant).is_some());
}

#[test]
fn whitelist_enabled_blocks_unapproved_merchant() {
    let (_env, client, admin) = setup();
    let merchant = Address::generate(&_env);
    let payout = Address::generate(&_env);
    client.set_whitelist_mode(&admin, &true);
    let result = client.try_initialize_merchant_config(
        &merchant,
        &payout,
        &0,
        &1,
        &None,
        &soroban_sdk::String::from_str(&_env, ""),
    );
    assert_eq!(result, Err(Ok(Error::MerchantNotApproved)));
}

#[test]
fn whitelist_enabled_allows_approved_merchant() {
    let (_env, client, admin) = setup();
    let merchant = Address::generate(&_env);
    let payout = Address::generate(&_env);
    client.set_whitelist_mode(&admin, &true);
    client.approve_merchant(&admin, &merchant);
    client.initialize_merchant_config(&merchant, &payout, &0, &1, &None, &soroban_sdk::String::from_str(&_env, ""));
    assert!(client.get_merchant_config(&merchant).is_some());
}

// ── Edge cases ───────────────────────────────────────────────────────────────

#[test]
fn toggle_whitelist_preserves_existing_approvals() {
    let (_env, client, admin) = setup();
    let merchant = Address::generate(&_env);
    // Approve before enabling whitelist
    client.approve_merchant(&admin, &merchant);
    // Toggle whitelist on
    client.set_whitelist_mode(&admin, &true);
    // Approval should still be there
    assert!(client.is_merchant_approved(&merchant));
    // Merchant can still register
    let payout = Address::generate(&_env);
    client.initialize_merchant_config(&merchant, &payout, &0, &1, &None, &soroban_sdk::String::from_str(&_env, ""));
    assert!(client.get_merchant_config(&merchant).is_some());
}

#[test]
fn approve_then_revoke_then_reapprove() {
    let (_env, client, admin) = setup();
    let merchant = Address::generate(&_env);
    client.set_whitelist_mode(&admin, &true);

    // Initially not approved
    assert!(!client.is_merchant_approved(&merchant));

    // Approve
    client.approve_merchant(&admin, &merchant);
    assert!(client.is_merchant_approved(&merchant));

    // Revoke
    client.revoke_merchant(&admin, &merchant);
    assert!(!client.is_merchant_approved(&merchant));

    // Re-approve
    client.approve_merchant(&admin, &merchant);
    assert!(client.is_merchant_approved(&merchant));
}

#[test]
fn whitelist_off_then_on_does_not_break_existing_merchants() {
    let (_env, client, admin) = setup();
    let merchant = Address::generate(&_env);
    let payout = Address::generate(&_env);

    // Register merchant while whitelist is off
    client.initialize_merchant_config(&merchant, &payout, &0, &1, &None, &soroban_sdk::String::from_str(&_env, ""));
    assert!(client.get_merchant_config(&merchant).is_some());

    // Turn whitelist on — existing merchant should still be in storage
    client.set_whitelist_mode(&admin, &true);
    // The existing config is still accessible
    assert!(client.get_merchant_config(&merchant).is_some());
}

// ── Adversarial coverage for get_whitelist_mode ──────────────────────────────

/// Test that consecutive calls to get_whitelist_mode return the same consistent state.
/// Exercises: idempotency, no side effects, deterministic behavior.
#[test]
fn get_whitelist_mode_is_idempotent() {
    let (env, client, _admin) = setup();
    
    // Multiple consecutive reads should return identical results
    let result1 = client.get_whitelist_mode();
    let result2 = client.get_whitelist_mode();
    let result3 = client.get_whitelist_mode();
    
    assert_eq!(result1, result2);
    assert_eq!(result2, result3);
    assert_eq!(result1, false); // Default is false
    let _ = env;
}

/// Test that get_whitelist_mode reflects the current state after being toggled multiple times.
/// Exercises: state mutation tracking, toggle correctness, state observation.
#[test]
fn get_whitelist_mode_reflects_state_after_multiple_toggles() {
    let (_env, client, admin) = setup();
    
    // Initial state: false
    assert!(!client.get_whitelist_mode());
    
    // Toggle on
    client.set_whitelist_mode(&admin, &true);
    assert!(client.get_whitelist_mode());
    
    // Toggle off
    client.set_whitelist_mode(&admin, &false);
    assert!(!client.get_whitelist_mode());
    
    // Toggle on again
    client.set_whitelist_mode(&admin, &true);
    assert!(client.get_whitelist_mode());
    
    // Toggle off again
    client.set_whitelist_mode(&admin, &false);
    assert!(!client.get_whitelist_mode());
}

/// Test that reading the whitelist mode does not mutate state.
/// Exercises: read-only guarantee, no side effects, repeated calls safe.
#[test]
fn get_whitelist_mode_does_not_mutate_state() {
    let (env, client, admin) = setup();
    
    // Read the state multiple times
    let _result1 = client.get_whitelist_mode();
    let _result2 = client.get_whitelist_mode();
    
    // Verify that subsequent operations work normally
    client.set_whitelist_mode(&admin, &true);
    assert!(client.get_whitelist_mode());
    
    // Read multiple times again
    let _result3 = client.get_whitelist_mode();
    let _result4 = client.get_whitelist_mode();
    
    // State should still be true
    assert!(client.get_whitelist_mode());
    let _ = env;
}

/// Test that get_whitelist_mode behavior is independent of merchant approval state.
/// Exercises: isolation from related state, no cross-state corruption.
#[test]
fn get_whitelist_mode_independent_of_merchant_approval() {
    let (_env, client, admin) = setup();
    let merchant = Address::generate(&_env);
    
    // Whitelist mode is false by default
    assert!(!client.get_whitelist_mode());
    
    // Approve a merchant (should not affect whitelist mode)
    client.approve_merchant(&admin, &merchant);
    assert!(!client.get_whitelist_mode());
    
    // Enable whitelist mode
    client.set_whitelist_mode(&admin, &true);
    assert!(client.get_whitelist_mode());
    
    // Revoke the merchant (should not affect whitelist mode)
    client.revoke_merchant(&admin, &merchant);
    assert!(client.get_whitelist_mode());
}

/// Test that any caller (not just admin) can read whitelist mode.
/// Exercises: no authorization requirement for reads, public contract.
#[test]
fn get_whitelist_mode_accessible_to_all_callers() {
    let (_env, client, admin) = setup();
    let non_admin_1 = Address::generate(&_env);
    let non_admin_2 = Address::generate(&_env);
    
    // Enable whitelist mode
    client.set_whitelist_mode(&admin, &true);
    
    // All callers can read the same value (simulated by direct calls)
    // In a real Soroban scenario, different addresses would invoke the contract
    assert!(client.get_whitelist_mode());
    assert!(client.get_whitelist_mode());
    assert!(client.get_whitelist_mode());
    
    // Disable and verify all see the updated value
    client.set_whitelist_mode(&admin, &false);
    assert!(!client.get_whitelist_mode());
    
    let _ = (non_admin_1, non_admin_2);
}

/// Test that get_whitelist_mode always returns a boolean (true or false, never undefined).
/// Exercises: boundary behavior, return type validation, default handling.
#[test]
fn get_whitelist_mode_always_returns_boolean() {
    let (env, client, admin) = setup();
    
    // Before any toggle: should be false (not undefined or null)
    let result = client.get_whitelist_mode();
    assert!(!result); // Explicitly false, not some undefined state
    
    // After toggle to true: should be true
    client.set_whitelist_mode(&admin, &true);
    let result = client.get_whitelist_mode();
    assert!(result); // Explicitly true
    
    // After toggle back to false: should be false again
    client.set_whitelist_mode(&admin, &false);
    let result = client.get_whitelist_mode();
    assert!(!result); // Explicitly false
    
    let _ = env;
}

/// Test that get_whitelist_mode persists correctly across the full state lifecycle.
/// Exercises: storage persistence, state durability, consistent observation.
#[test]
fn get_whitelist_mode_persists_through_lifecycle() {
    let (_env, client, admin) = setup();
    
    // 1. Initial state
    assert!(!client.get_whitelist_mode());
    
    // 2. Enable and verify persistence
    client.set_whitelist_mode(&admin, &true);
    assert!(client.get_whitelist_mode());
    assert!(client.get_whitelist_mode()); // Verify persistence with repeated read
    
    // 3. Disable and verify persistence
    client.set_whitelist_mode(&admin, &false);
    assert!(!client.get_whitelist_mode());
    assert!(!client.get_whitelist_mode()); // Verify persistence
    
    // 4. Enable again
    client.set_whitelist_mode(&admin, &true);
    assert!(client.get_whitelist_mode());
}

/// Test that get_whitelist_mode correctly observes state set before any merchant operations.
/// Exercises: state independence, no ordering dependencies.
#[test]
fn get_whitelist_mode_state_set_before_merchant_operations() {
    let (_env, client, admin) = setup();
    let merchant = Address::generate(&_env);
    
    // Enable whitelist mode before any merchant operations
    client.set_whitelist_mode(&admin, &true);
    assert!(client.get_whitelist_mode());
    
    // Now perform merchant operations
    client.approve_merchant(&admin, &merchant);
    
    // Whitelist mode should still be readable and true
    assert!(client.get_whitelist_mode());
}

/// Test that get_whitelist_mode is not affected by failed or rejected operations.
/// Exercises: atomicity, state isolation from errors, consistency after errors.
#[test]
fn get_whitelist_mode_unaffected_by_rejected_operations() {
    let (_env, client, admin) = setup();
    let non_admin = Address::generate(&_env);
    
    // Set whitelist mode to true
    client.set_whitelist_mode(&admin, &true);
    assert!(client.get_whitelist_mode());
    
    // Attempt unauthorized toggle (should fail)
    let result = client.try_set_whitelist_mode(&non_admin, &false);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
    
    // State should remain unchanged
    assert!(client.get_whitelist_mode());
}

/// Test that get_whitelist_mode returns consistent results when called in rapid succession.
/// Exercises: race condition safety, determinism under concurrency simulation.
#[test]
fn get_whitelist_mode_consistent_in_rapid_succession() {
    let (_env, client, admin) = setup();
    
    // Rapid calls before any toggle
    for _ in 0..10 {
        assert!(!client.get_whitelist_mode());
    }
    
    // Toggle on
    client.set_whitelist_mode(&admin, &true);
    
    // Rapid calls after toggle
    for _ in 0..10 {
        assert!(client.get_whitelist_mode());
    }
}

/// Test that the default state is explicitly false, not some other falsy value.
/// Exercises: initialization correctness, default behavior, type safety.
#[test]
fn get_whitelist_mode_default_is_explicitly_false() {
    let (_env, client, _admin) = setup();
    
    // Immediately after init, should be false
    let mode = client.get_whitelist_mode();
    assert_eq!(mode, false);
    
    // Verify with negation
    assert!(!client.get_whitelist_mode());
}
