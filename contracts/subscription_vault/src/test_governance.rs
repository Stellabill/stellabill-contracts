use crate::types::{OP_WITHDRAW, OP_REFUND, OP_CHARGE};
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{testutils::Address as _, Address, Env, String};

#[test]
fn test_merchant_config_initialization() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant_a = Address::generate(&env);
    let payout_address = Address::generate(&env);
    let redirect_url = String::from_str(&env, "https://stellabill.io/success");

    // initialize_merchant_config returns MerchantConfig directly (Soroban unwraps Result<T,E> -> T)
    let config = client.initialize_merchant_config(
        &merchant_a,
        &payout_address,
        &500, // 5% fee in bips
        &0x1F, // all operations enabled
        &None,
        &redirect_url,
    );

    assert_eq!(config.fee_bips, 500);
    assert_eq!(config.is_active, true);
    assert_eq!(config.redirect_url, redirect_url);
}

#[test]
fn test_merchant_config_governance_enforced() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant_a = Address::generate(&env);
    let payout_address = Address::generate(&env);
    let redirect_url = String::from_str(&env, "https://stellabill.io/success");

    // Initialize config first
    client.initialize_merchant_config(
        &merchant_a,
        &payout_address,
        &500,
        &0x1F,
        &None,
        &redirect_url,
    );

    // Partial update — update_merchant_config also returns MerchantConfig directly
    let updated = client.update_merchant_config(
        &merchant_a,
        &None,                                                   // payout unchanged
        &Some(1000),                                             // new fee: 10%
        &None,                                                   // ops unchanged
        &None,                                                   // active unchanged
        &None,                                                   // fee_address unchanged
        &Some(String::from_str(&env, "https://new-url.com")),   // new redirect
        &None,                                                   // paused unchanged
    );

    assert_eq!(updated.fee_bips, 1000);
    assert_eq!(updated.redirect_url, String::from_str(&env, "https://new-url.com"));
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_unauthorized_merchant_config_update() {
    let env = Env::default();
    // No mock_all_auths — require_auth() without a signature triggers a host Auth error.
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    let _ = client.initialize_merchant_config(
        &merchant,
        &payout,
        &500,
        &0x1F,
        &None,
        &String::from_str(&env, "https://malicious.com"),
    );
}

// === Edge Cases and Boundary Validation ===

#[test]
fn test_fee_bips_at_maximum_boundary() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    // fee_bips = 10000 is the maximum allowed (100%)
    let config = client.initialize_merchant_config(
        &merchant,
        &payout,
        &10000,
        &0x1F,
        &None,
        &String::from_str(&env, ""),
    );

    assert_eq!(config.fee_bips, 10000);
}

#[test]
#[should_panic(expected = "Error(Contract, #7001)")]
fn test_fee_bips_exceeds_maximum() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    // fee_bips = 10001 exceeds MAX_FEE_BIPS — must return InvalidFeeBips (#1038)
    let _ = client.initialize_merchant_config(
        &merchant,
        &payout,
        &10001,
        &0x1F,
        &None,
        &String::from_str(&env, ""),
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #7003)")]
fn test_operations_without_charge_flag() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    // OP_CHARGE is missing — must return MustAllowChargeOperation (#1041)
    let _ = client.initialize_merchant_config(
        &merchant,
        &payout,
        &0,
        &(OP_WITHDRAW | OP_REFUND),
        &None,
        &String::from_str(&env, ""),
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #7002)")]
fn test_operations_with_invalid_bit() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    // Bit 0x80 is not a valid operation — must return InvalidOperations (#1040)
    let _ = client.initialize_merchant_config(
        &merchant,
        &payout,
        &0,
        &0x80,
        &None,
        &String::from_str(&env, ""),
    );
}

#[test]
fn test_get_merchant_config_not_found() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);

    // get_merchant_config returns Option<MerchantConfig> — None when uninitialized
    let config = client.get_merchant_config(&merchant);
    assert!(config.is_none());
}

#[test]
#[should_panic(expected = "Error(Contract, #2001)")]
fn test_update_nonexistent_config() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);

    // Updating before initialize — must return ConfigNotFound (#1042)
    let _ = client.update_merchant_config(
        &merchant,
        &None,
        &Some(500),
        &None,
        &None,
        &None,
        &None,
        &None,
    );
}

#[test]
fn test_partial_update_preserves_other_fields() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    let initial = client.initialize_merchant_config(
        &merchant,
        &payout,
        &500,   // 5%
        &0x0F,  // no OP_AUTO_RENEWAL
        &None,
        &String::from_str(&env, "https://initial.com"),
    );

    assert_eq!(initial.fee_bips, 500);
    assert_eq!(initial.allowed_operations, 0x0F);

    // Update only fee_bips — everything else must be preserved
    let updated = client.update_merchant_config(
        &merchant,
        &None,
        &Some(1000),
        &None,
        &None,
        &None,
        &None,
        &None,
    );

    assert_eq!(updated.fee_bips, 1000);
    assert_eq!(updated.allowed_operations, 0x0F);
    assert_eq!(updated.redirect_url, String::from_str(&env, "https://initial.com"));
}

#[test]
fn test_set_and_get_merchant_config() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    client.initialize_merchant_config(
        &merchant,
        &payout,
        &250,
        &0x1F,
        &None,
        &String::from_str(&env, "https://example.com"),
    );

    // get_merchant_config returns Option<MerchantConfig>, so .unwrap() is valid here
    let retrieved = client.get_merchant_config(&merchant).unwrap();

    assert_eq!(retrieved.fee_bips, 250);
    assert_eq!(retrieved.is_active, true);
    assert_eq!(retrieved.allowed_operations, 0x1F);
}

#[test]
fn test_update_deactivate_merchant() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    client.initialize_merchant_config(
        &merchant,
        &payout,
        &500,
        &0x1F,
        &None,
        &String::from_str(&env, ""),
    );

    let updated = client.update_merchant_config(
        &merchant,
        &None,
        &None,
        &None,
        &Some(false), // deactivate
        &None,
        &None,
        &None,
    );

    assert_eq!(updated.is_active, false);
}

// ══════════════════════════════════════════════════════════════════════════════
// Admin rotation invariant tests
//
// Security model enforced by these tests:
//   1. Only the current stored admin can rotate the admin key.
//   2. Rotation updates exactly one canonical key (DataKey::Admin).
//   3. Old admin loses all privileges atomically in the same transaction.
//   4. New admin gains all privileges atomically in the same transaction.
//   5. Events carry old_admin, new_admin, and timestamp for audit trails.
//   6. Rotation is replay-protected: wrong nonce is rejected before any
//      state mutation occurs.
//   7. Self-rotation and rotation to the contract address are rejected.
//   8. The emergency stop does not gate rotate_admin itself.
//   9. Active subscriptions and pending charges are unaffected by rotation.
// ══════════════════════════════════════════════════════════════════════════════

mod admin_rotation_invariants {
    use crate::test_utils::{fixtures, setup::TestEnv};
    use crate::{AdminRotatedEvent, Error, SubscriptionStatus};
    use soroban_sdk::{testutils::Ledger as _, Address, IntoVal, Vec};

    const T0: u64 = 1_000;
    const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
    const PREPAID: i128 = 50_000_000;

    // ── Invariant 1: exactly one canonical admin key ──────────────────────────

    #[test]
    fn rotate_updates_exactly_one_canonical_admin_key() {
        // DataKey::Admin is the sole source of truth. get_admin() must return
        // the new address immediately after rotation and never the old one.
        let te = TestEnv::default();
        let new_admin = Address::generate(&te.env);

        assert_eq!(te.client.get_admin(), te.admin);
        te.client.rotate_admin(&te.admin, &new_admin, &0u64);
        assert_eq!(te.client.get_admin(), new_admin);
    }

    #[test]
    fn rotate_admin_rejected_for_non_admin() {
        let te = TestEnv::default();
        let stranger = Address::generate(&te.env);
        let target = Address::generate(&te.env);

        let result = te.client.try_rotate_admin(&stranger, &target, &0u64);
        assert_eq!(result, Err(Ok(Error::Unauthorized)));
        // Canonical key must be unchanged.
        assert_eq!(te.client.get_admin(), te.admin);
    }

    #[test]
    fn rotate_admin_self_rotation_rejected() {
        // Rotating to the same address wastes a nonce and could mask
        // misconfiguration; the contract rejects it with SelfRotation.
        let te = TestEnv::default();
        let result = te.client.try_rotate_admin(&te.admin, &te.admin, &0u64);
        assert_eq!(result, Err(Ok(Error::SelfRotation)));
        assert_eq!(te.client.get_admin(), te.admin);
    }

    #[test]
    fn rotate_admin_to_contract_address_rejected() {
        // The contract cannot sign Soroban auth transactions, so rotating to it
        // would permanently lock all admin-only operations.
        let te = TestEnv::default();
        let result = te.client.try_rotate_admin(&te.admin, &te.client.address, &0u64);
        assert_eq!(result, Err(Ok(Error::InvalidNewAdmin)));
        assert_eq!(te.client.get_admin(), te.admin);
    }

    // ── Invariant 2: immediate and complete privilege transfer ────────────────

    #[test]
    fn old_admin_loses_all_admin_only_privileges_after_rotation() {
        let te = TestEnv::default();
        let new_admin = Address::generate(&te.env);
        let other = Address::generate(&te.env);

        te.client.rotate_admin(&te.admin, &new_admin, &0u64);

        // set_min_topup requires explicit admin address check.
        assert_eq!(
            te.client.try_set_min_topup(&te.admin, &5_000_000i128),
            Err(Ok(Error::Unauthorized))
        );
        // Emergency stop management requires admin.
        assert_eq!(
            te.client.try_enable_emergency_stop(&te.admin),
            Err(Ok(Error::Unauthorized))
        );
        assert_eq!(
            te.client.try_disable_emergency_stop(&te.admin),
            Err(Ok(Error::Unauthorized))
        );
        // Cannot re-rotate even with the next nonce value — auth check fires first.
        assert_eq!(
            te.client.try_rotate_admin(&te.admin, &other, &1u64),
            Err(Ok(Error::Unauthorized))
        );
    }

    #[test]
    fn new_admin_gains_all_admin_only_privileges_immediately() {
        let te = TestEnv::default();
        let new_admin = Address::generate(&te.env);

        te.client.rotate_admin(&te.admin, &new_admin, &0u64);

        te.client.set_min_topup(&new_admin, &3_000_000i128);
        assert_eq!(te.client.get_min_topup(), 3_000_000i128);

        te.client.enable_emergency_stop(&new_admin);
        assert!(te.client.get_emergency_stop_status());
        te.client.disable_emergency_stop(&new_admin);
        assert!(!te.client.get_emergency_stop_status());
    }

    // ── Invariant 3: AdminRotatedEvent carries correct payload ────────────────

    #[test]
    fn rotate_admin_emits_event_with_correct_old_admin_new_admin_and_timestamp() {
        // Off-chain indexers rely on AdminRotatedEvent to track the rotation
        // history. Verify every field is correct.
        let te = TestEnv::default();
        let new_admin = Address::generate(&te.env);
        let expected_ts = 42_000u64;
        te.env.ledger().with_mut(|li| li.timestamp = expected_ts);

        te.client.rotate_admin(&te.admin, &new_admin, &0u64);

        // admin_rotated is the last event emitted in the call
        // (nonce_consumed comes first from check_and_advance).
        let events = te.env.events().all();
        let last = events.last().expect("no events emitted after rotate_admin");
        let payload: AdminRotatedEvent = last.2.into_val(&te.env);

        assert_eq!(payload.old_admin, te.admin);
        assert_eq!(payload.new_admin, new_admin);
        assert_eq!(payload.timestamp, expected_ts);
    }

    // ── Invariant 4: nonce protects against replay and out-of-order calls ─────

    #[test]
    fn rotate_admin_wrong_nonce_rejected_state_unchanged() {
        // Providing nonce=1 when 0 is expected must be rejected after auth passes
        // but before any state mutation. The admin key must remain unchanged.
        let te = TestEnv::default();
        let new_admin = Address::generate(&te.env);

        let result = te.client.try_rotate_admin(&te.admin, &new_admin, &1u64);
        assert_eq!(result, Err(Ok(Error::NonceAlreadyUsed)));

        // Admin key unchanged; nonce not advanced.
        assert_eq!(te.client.get_admin(), te.admin);
        assert_eq!(te.client.get_admin_nonce(&te.admin, &1u32), 0u64);
    }

    // ── Invariant 5: subscription storage is isolated from admin rotation ─────

    #[test]
    fn rotate_admin_does_not_mutate_subscription_state() {
        let te = TestEnv::default();
        let (id, _, _) =
            fixtures::create_subscription(&te.env, &te.client, SubscriptionStatus::Active);
        let before = te.client.get_subscription(&id);

        te.client.rotate_admin(&te.admin, &Address::generate(&te.env), &0u64);

        let after = te.client.get_subscription(&id);
        assert_eq!(before.subscriber, after.subscriber);
        assert_eq!(before.merchant, after.merchant);
        assert_eq!(before.amount, after.amount);
        assert_eq!(before.status, after.status);
        assert_eq!(before.prepaid_balance, after.prepaid_balance);
    }

    // ── Invariant 6: pending charges remain chargeable after rotation ─────────

    #[test]
    fn new_admin_can_batch_charge_subscriptions_pending_before_rotation() {
        // batch_charge uses require_stored_admin_auth (reads admin from storage).
        // After rotation the new admin is stored so the call succeeds. This ensures
        // that pending charges are never dropped or locked by an admin rotation.
        let te = TestEnv::default();
        te.env.ledger().with_mut(|li| li.timestamp = T0);

        let (id, _, _) =
            fixtures::create_subscription(&te.env, &te.client, SubscriptionStatus::Active);
        fixtures::seed_balance(&te.env, &te.client, id, PREPAID);

        // Advance time past the billing interval so a charge is due.
        te.env.ledger().with_mut(|li| li.timestamp = T0 + INTERVAL + 1);

        let new_admin = Address::generate(&te.env);
        te.client.rotate_admin(&te.admin, &new_admin, &0u64);

        let ids = Vec::from_array(&te.env, [id]);
        let results = te.client.batch_charge(&ids, &0u64);
        assert_eq!(results.len(), 1);
        assert!(results.get(0).unwrap().success);
    }

    // ── Invariant 7: rotate_admin is not gated by the emergency stop ──────────

    #[test]
    fn rotate_admin_succeeds_while_emergency_stop_is_active() {
        // rotate_admin must not be blocked by the emergency stop circuit breaker.
        // Rotation is the primary recovery path when the contract is paused.
        let te = TestEnv::default();
        te.client.enable_emergency_stop(&te.admin);
        assert!(te.client.get_emergency_stop_status());

        let new_admin = Address::generate(&te.env);
        te.client.rotate_admin(&te.admin, &new_admin, &0u64);
        assert_eq!(te.client.get_admin(), new_admin);
    }

    #[test]
    fn new_admin_can_disable_emergency_stop_after_rotation() {
        // After rotating during an active emergency stop, the new admin must be
        // able to clear it. This is the critical recovery path.
        let te = TestEnv::default();
        te.client.enable_emergency_stop(&te.admin);

        let new_admin = Address::generate(&te.env);
        te.client.rotate_admin(&te.admin, &new_admin, &0u64);

        te.client.disable_emergency_stop(&new_admin);
        assert!(!te.client.get_emergency_stop_status());

        // Old admin cannot re-enable after privilege was transferred.
        assert_eq!(
            te.client.try_enable_emergency_stop(&te.admin),
            Err(Ok(Error::Unauthorized))
        );
    }
}
