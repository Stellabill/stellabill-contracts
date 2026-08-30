//! Integration tests for the per-key admin-config cooldown feature.
//!
//! Covers:
//! - Basic enforcement: within window -> rejected; after window -> succeeds.
//! - Orthogonal keys are updated independently.
//! - Exact-boundary retry (at exactly COOLDOWN_SECS).
//! - Governance proposal bypass.
//! - All covered config key labels.
//! - Event emission verification.
//! - prev_ts correctness across multiple mutations.

#![cfg(test)]

extern crate alloc;

use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    Address, Env, String, Symbol,
};
use subscription_vault::{
    AdminConfigChangedEvent, DataKey, Error, SubscriptionVault, SubscriptionVaultClient,
};

/// 6 hours in seconds -- matches CONFIG_COOLDOWN_SECS.
const COOLDOWN: u64 = 6 * 60 * 60;

// -- Helpers ------------------------------------------------------------------

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, client, token, admin)
}

/// Read the raw cooldown timestamp for a key label from persistent storage.
fn read_cooldown_ts(env: &Env, client: &SubscriptionVaultClient, key_label: &str) -> u64 {
    env.as_contract(&client.address, || {
        let label_bytes = soroban_sdk::Bytes::from_array(env, key_label.as_bytes());
        let hash = env.crypto().sha256(&label_bytes);
        let storage_key = DataKey::AdminConfigLastChangedAt(hash);
        env.storage()
            .persistent()
            .get::<_, u64>(&storage_key)
            .unwrap_or(0)
    })
}

// -- Basic enforcement --------------------------------------------------------

/// First call to set_min_topup succeeds (no prior cooldown record).
#[test]
fn first_mutation_always_succeeds() {
    let (env, client, _token, admin) = setup();
    let res = client.try_set_min_topup(&admin, &2_000_000i128);
    assert_eq!(res, Ok(Ok(())));
}

/// A second call within the cooldown window is rejected with CooldownActive.
#[test]
fn second_mutation_within_window_rejected() {
    let (env, client, _token, admin) = setup();

    // First call at t=1000 succeeds.
    env.ledger().with_mut(|li| li.timestamp = 1000);
    assert_eq!(client.try_set_min_topup(&admin, &2_000_000i128), Ok(Ok(())));

    // Second call at t=1000 + 1 second (< COOLDOWN) is rejected.
    env.ledger().with_mut(|li| li.timestamp = 1000 + 1);
    assert_eq!(
        client.try_set_min_topup(&admin, &3_000_000i128),
        Ok(Err(Error::CooldownActive))
    );
}

/// After the cooldown window expires, mutation succeeds again.
#[test]
fn mutation_after_cooldown_window_succeeds() {
    let (env, client, _token, admin) = setup();

    env.ledger().with_mut(|li| li.timestamp = 1000);
    assert_eq!(client.try_set_min_topup(&admin, &2_000_000i128), Ok(Ok(())));

    // Advance past the cooldown.
    env.ledger().with_mut(|li| li.timestamp = 1000 + COOLDOWN);
    assert_eq!(client.try_set_min_topup(&admin, &3_000_000i128), Ok(Ok(())));
}

// -- Orthogonal keys ----------------------------------------------------------

/// Orthogonal key labels can be mutated independently within the cooldown.
/// Uses set_min_topup (key "MinTopup") and set_operator (key "Operator").
#[test]
fn orthogonal_keys_independent() {
    let (env, client, _token, admin) = setup();
    let op1 = Address::generate(&env);

    env.ledger().with_mut(|li| li.timestamp = 1000);

    // Mutate MinTopup at t=1000.
    assert_eq!(client.try_set_min_topup(&admin, &2_000_000i128), Ok(Ok(())));

    // Mutate Operator at t=1000 -- same timestamp, different key. Should succeed.
    assert_eq!(client.try_set_operator(&admin, &op1), Ok(Ok(())));

    // Mutate MinTopup again at t=1000 -- same key, still within cooldown. Rejected.
    assert_eq!(
        client.try_set_min_topup(&admin, &4_000_000i128),
        Ok(Err(Error::CooldownActive))
    );
}

/// Multiple key labels: rapid toggling of two orthogonal keys interleaved.
#[test]
fn interleaved_orthogonal_keys() {
    let (env, client, _token, admin) = setup();
    let op1 = Address::generate(&env);

    env.ledger().with_mut(|li| li.timestamp = 1000);

    // Alternating between two keys at the same timestamp.
    assert_eq!(client.try_set_min_topup(&admin, &2_000_000i128), Ok(Ok(())));
    assert_eq!(client.try_set_operator(&admin, &op1), Ok(Ok(())));
    assert_eq!(
        client.try_set_min_topup(&admin, &3_000_000i128),
        Ok(Err(Error::CooldownActive))
    );
}

// -- Boundary tests -----------------------------------------------------------

/// Exact boundary: mutation at exactly COOLDOWN_SECS after previous succeeds.
#[test]
fn exact_boundary_retry_succeeds() {
    let (env, client, _token, admin) = setup();

    env.ledger().with_mut(|li| li.timestamp = 1000);
    assert_eq!(client.try_set_min_topup(&admin, &2_000_000i128), Ok(Ok(())));

    // At exactly COOLDOWN_SECS later: `now - prev_ts == COOLDOWN_SECS`,
    // which is NOT less than COOLDOWN, so it should succeed.
    env.ledger().with_mut(|li| li.timestamp = 1000 + COOLDOWN);
    assert_eq!(client.try_set_min_topup(&admin, &5_000_000i128), Ok(Ok(())));
}

/// Just before the boundary: mutation at COOLDOWN_SECS - 1 is still rejected.
#[test]
fn one_second_before_boundary_rejected() {
    let (env, client, _token, admin) = setup();

    env.ledger().with_mut(|li| li.timestamp = 1000);
    assert_eq!(client.try_set_min_topup(&admin, &2_000_000i128), Ok(Ok(())));

    env.ledger().with_mut(|li| li.timestamp = 1000 + COOLDOWN - 1);
    assert_eq!(
        client.try_set_min_topup(&admin, &5_000_000i128),
        Ok(Err(Error::CooldownActive))
    );
}

// -- prev_ts tracking ---------------------------------------------------------

/// Cooldown records the correct prev_ts across multiple mutation cycles.
#[test]
fn prev_ts_tracked_correctly() {
    let (env, client, _token, admin) = setup();

    // First mutation at t=1000: prev_ts should be 0.
    env.ledger().with_mut(|li| li.timestamp = 1000);
    assert_eq!(client.try_set_min_topup(&admin, &2_000_000i128), Ok(Ok(())));

    let ts1 = read_cooldown_ts(&env, &client, "MinTopup");
    assert_eq!(ts1, 1000);

    // Second mutation at t=1000+COOLDOWN: prev_ts should be 1000.
    env.ledger().with_mut(|li| li.timestamp = 1000 + COOLDOWN);
    assert_eq!(client.try_set_min_topup(&admin, &3_000_000i128), Ok(Ok(())));

    let ts2 = read_cooldown_ts(&env, &client, "MinTopup");
    assert_eq!(ts2, 1000 + COOLDOWN);
}

// -- Emergency stop cooldown --------------------------------------------------

/// Enable/disable emergency stop respects cooldown per key.
#[test]
fn emergency_stop_cooldown() {
    let (env, client, _token, admin) = setup();

    env.ledger().with_mut(|li| li.timestamp = 1000);

    // Enable emergency stop.
    assert_eq!(client.try_enable_emergency_stop(&admin), Ok(Ok(())));

    // Immediately try to disable -- should be rejected (same key: "EmergencyStop").
    assert_eq!(
        client.try_disable_emergency_stop(&admin),
        Ok(Err(Error::CooldownActive))
    );

    // Advance past cooldown and disable succeeds.
    env.ledger().with_mut(|li| li.timestamp = 1000 + COOLDOWN);
    assert_eq!(client.try_disable_emergency_stop(&admin), Ok(Ok(())));
}

/// EmergencyStop enable -> disable -> enable across cooldown cycles.
#[test]
fn emergency_stop_toggle_across_cycles() {
    let (env, client, _token, admin) = setup();

    env.ledger().with_mut(|li| li.timestamp = 1000);
    assert_eq!(client.try_enable_emergency_stop(&admin), Ok(Ok(())));

    env.ledger().with_mut(|li| li.timestamp = 1000 + COOLDOWN);
    assert_eq!(client.try_disable_emergency_stop(&admin), Ok(Ok(())));

    env.ledger().with_mut(|li| li.timestamp = 1000 + 2 * COOLDOWN);
    assert_eq!(client.try_enable_emergency_stop(&admin), Ok(Ok(())));
}

// -- Operator cooldown --------------------------------------------------------

/// set_operator and remove_operator share the "Operator" cooldown key.
#[test]
fn operator_cooldown_shared() {
    let (env, client, _token, admin) = setup();
    let op1 = Address::generate(&env);

    env.ledger().with_mut(|li| li.timestamp = 1000);

    // Set operator.
    assert_eq!(client.try_set_operator(&admin, &op1), Ok(Ok(())));

    // Immediately remove operator -- same key, rejected.
    assert_eq!(
        client.try_remove_operator(&admin),
        Ok(Err(Error::CooldownActive))
    );

    // Advance past cooldown and remove succeeds.
    env.ledger().with_mut(|li| li.timestamp = 1000 + COOLDOWN);
    assert_eq!(client.try_remove_operator(&admin), Ok(Ok(())));
}

// -- Protocol fee cooldown ----------------------------------------------------

/// set_protocol_fee respects the "ProtocolFee" cooldown key.
#[test]
fn protocol_fee_cooldown() {
    let (env, client, _token, admin) = setup();
    let treasury = Address::generate(&env);

    env.ledger().with_mut(|li| li.timestamp = 1000);

    assert_eq!(
        client.try_set_protocol_fee(&admin, &treasury, &500u32),
        Ok(Ok(()))
    );

    // Immediately again -- rejected.
    assert_eq!(
        client.try_set_protocol_fee(&admin, &treasury, &1000u32),
        Ok(Err(Error::CooldownActive))
    );

    // After cooldown -- succeeds.
    env.ledger().with_mut(|li| li.timestamp = 1000 + COOLDOWN);
    assert_eq!(
        client.try_set_protocol_fee(&admin, &treasury, &1000u32),
        Ok(Ok(()))
    );
}

// -- Billing retention cooldown -----------------------------------------------

/// set_billing_retention respects the "BillingRetention" cooldown key.
#[test]
fn billing_retention_cooldown() {
    let (env, client, _token, admin) = setup();

    env.ledger().with_mut(|li| li.timestamp = 1000);
    assert_eq!(
        client.try_set_billing_retention(&admin, &10u32),
        Ok(Ok(()))
    );

    // Immediately again -- rejected.
    assert_eq!(
        client.try_set_billing_retention(&admin, &20u32),
        Ok(Err(Error::CooldownActive))
    );

    // After cooldown -- succeeds.
    env.ledger().with_mut(|li| li.timestamp = 1000 + COOLDOWN);
    assert_eq!(
        client.try_set_billing_retention(&admin, &20u32),
        Ok(Ok(()))
    );
}

// -- Accepted tokens cooldown -------------------------------------------------

/// add_accepted_token and remove_accepted_token share "AcceptedTokens" cooldown.
#[test]
fn accepted_tokens_cooldown_shared() {
    let (env, client, _token, admin) = setup();
    let new_token_admin = Address::generate(&env);
    let new_token = env
        .register_stellar_asset_contract_v2(new_token_admin.clone())
        .address();

    env.ledger().with_mut(|li| li.timestamp = 1000);

    // Add a new accepted token.
    assert_eq!(
        client.try_add_accepted_token(&admin, &new_token, &6u32),
        Ok(Ok(()))
    );

    // Immediately try to remove -- same key, rejected.
    assert_eq!(
        client.try_remove_accepted_token(&admin, &new_token),
        Ok(Err(Error::CooldownActive))
    );

    // After cooldown -- remove succeeds.
    env.ledger().with_mut(|li| li.timestamp = 1000 + COOLDOWN);
    assert_eq!(
        client.try_remove_accepted_token(&admin, &new_token),
        Ok(Ok(()))
    );
}

// -- rotate_admin cooldown ----------------------------------------------------

/// rotate_admin uses "Admin" cooldown key.
#[test]
fn rotate_admin_cooldown() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);

    env.ledger().with_mut(|li| li.timestamp = 1000);
    let nonce = client.get_admin_nonce(&admin, 1);
    assert_eq!(
        client.try_rotate_admin(&admin, &new_admin, &nonce),
        Ok(Ok(()))
    );

    // Immediately rotate back -- same "Admin" key, rejected.
    let nonce2 = client.get_admin_nonce(&new_admin, 1);
    assert_eq!(
        client.try_rotate_admin(&new_admin, &admin, &nonce2),
        Ok(Err(Error::CooldownActive))
    );

    // After cooldown -- succeeds.
    env.ledger().with_mut(|li| li.timestamp = 1000 + COOLDOWN);
    let nonce3 = client.get_admin_nonce(&new_admin, 1);
    assert_eq!(
        client.try_rotate_admin(&new_admin, &admin, &nonce3),
        Ok(Ok(()))
    );
}

// -- Governance bypass --------------------------------------------------------

/// Governance proposal execution bypasses the cooldown.
///
/// `execute_proposal` writes config directly via `write_config`, so it
/// intentionally skips the cooldown guard. This test verifies the bypass.
#[test]
fn governance_proposal_bypasses_cooldown() {
    let (env, client, _token, admin) = setup();
    let new_admin = Address::generate(&env);
    let guardian = Address::generate(&env);

    env.mock_all_auths();

    // Set up a guardian.
    assert_eq!(
        client.try_add_guardian(&admin, &guardian, &100u32),
        Ok(Ok(()))
    );

    env.ledger().with_mut(|li| li.timestamp = 1000);

    // Rotate admin first (to set cooldown on "Admin" key).
    let nonce = client.get_admin_nonce(&admin, 1);
    assert_eq!(
        client.try_rotate_admin(&admin, &new_admin, &nonce),
        Ok(Ok(()))
    );

    // Immediately try to rotate again via admin function -- should be rejected.
    let nonce2 = client.get_admin_nonce(&new_admin, 1);
    assert_eq!(
        client.try_rotate_admin(&new_admin, &admin, &nonce2),
        Ok(Err(Error::CooldownActive))
    );

    // Submit a governance proposal to rotate admin back.
    let eta: u64 = 2000;
    env.ledger().with_mut(|li| li.timestamp = 1500);
    let proposal_id = client
        .submit_proposal(
            &subscription_vault::ProposalKind::RotateAdmin,
            &admin,
            &None::<Address>,
            &0u32,
            &5000u32, // 50% quorum
            &eta,
        )
        .unwrap();

    // Vote as guardian.
    assert_eq!(client.try_vote_proposal(&proposal_id, &true), Ok(Ok(())));

    // Execute at ETA -- should succeed even though cooldown is active.
    env.ledger().with_mut(|li| li.timestamp = eta);
    assert_eq!(client.try_execute_proposal(&proposal_id), Ok(Ok(())));

    // Verify admin was actually rotated.
    assert_eq!(client.get_admin(), admin);
}

// -- Auth / validation failures do NOT trigger cooldown -----------------------

/// Auth failures do NOT trigger cooldown (cooldown runs after auth check).
#[test]
fn auth_failure_does_not_trigger_cooldown() {
    let (env, client, _token, admin) = setup();
    let non_admin = Address::generate(&env);

    env.ledger().with_mut(|li| li.timestamp = 1000);

    // Non-admin call fails with Unauthorized (not CooldownActive).
    assert_eq!(
        client.try_set_min_topup(&non_admin, &2_000_000i128),
        Ok(Err(Error::Unauthorized))
    );

    // Admin can still set_min_topup -- no cooldown was set by the failed call.
    assert_eq!(
        client.try_set_min_topup(&admin, &2_000_000i128),
        Ok(Ok(()))
    );
}

/// Validation failures (e.g. invalid amount) do NOT trigger cooldown.
#[test]
fn validation_failure_does_not_trigger_cooldown() {
    let (env, client, _token, admin) = setup();

    env.ledger().with_mut(|li| li.timestamp = 1000);

    // Zero amount is invalid.
    assert_eq!(
        client.try_set_min_topup(&admin, &0i128),
        Ok(Err(Error::InvalidAmount))
    );

    // Admin can still set_min_topup -- no cooldown was set.
    assert_eq!(
        client.try_set_min_topup(&admin, &2_000_000i128),
        Ok(Ok(()))
    );
}

// -- Event emission -----------------------------------------------------------

/// AdminConfigChangedEvent is emitted on successful cooldown-gated mutations.
#[test]
fn event_emitted_on_success() {
    let (env, client, _token, admin) = setup();

    env.ledger().with_mut(|li| li.timestamp = 1000);

    let _ = client.set_min_topup(&admin, &2_000_000i128);

    let events = env.events().all();
    // Find the admin_config_changed event.
    let mut found = false;
    for i in 0..events.len() {
        let (topics, data) = events.get_unchecked(i);
        // The event topic is a Symbol.
        let topic0: Symbol = topics.get_unchecked(0);
        if topic0 == Symbol::new(&env, "admin_config_changed") {
            let evt: AdminConfigChangedEvent =
                soroban_sdk::TryFromVal::try_from_val(&env, &data).unwrap();
            assert_eq!(evt.key_label, String::from_str(&env, "MinTopup"));
            assert_eq!(evt.prev_ts, 0); // first mutation
            assert_eq!(evt.timestamp, 1000);
            assert_eq!(
                evt.schema_version,
                subscription_vault::EVENT_SCHEMA_VERSION
            );
            found = true;
            break;
        }
    }
    assert!(
        found,
        "admin_config_changed event not found in emitted events"
    );
}

/// The event for a second mutation carries the correct prev_ts.
#[test]
fn event_prev_ts_on_second_mutation() {
    let (env, client, _token, admin) = setup();

    env.ledger().with_mut(|li| li.timestamp = 1000);
    let _ = client.set_min_topup(&admin, &2_000_000i128);

    env.ledger().with_mut(|li| li.timestamp = 1000 + COOLDOWN);
    let _ = client.set_min_topup(&admin, &3_000_000i128);

    let events = env.events().all();
    let mut found = false;
    for i in 0..events.len() {
        let (topics, data) = events.get_unchecked(i);
        let topic0: Symbol = topics.get_unchecked(0);
        if topic0 == Symbol::new(&env, "admin_config_changed") {
            let evt: AdminConfigChangedEvent =
                soroban_sdk::TryFromVal::try_from_val(&env, &data).unwrap();
            if evt.key_label == String::from_str(&env, "MinTopup") && evt.prev_ts == 1000 {
                assert_eq!(evt.timestamp, 1000 + COOLDOWN);
                found = true;
            }
        }
    }
    assert!(
        found,
        "second admin_config_changed event with prev_ts=1000 not found"
    );
}

// -- Edge cases ---------------------------------------------------------------

/// sat_sub prevents underflow when prev_ts > now (e.g. clock skew).
#[test]
fn timestamp_underflow_safe() {
    let (env, client, _token, admin) = setup();

    env.ledger().with_mut(|li| li.timestamp = 1000);
    assert_eq!(client.try_set_min_topup(&admin, &2_000_000i128), Ok(Ok(())));

    // Set ledger time BEFORE the previous timestamp -- should still reject
    // because saturating_sub(1000, 500) = 500 < COOLDOWN.
    env.ledger().with_mut(|li| li.timestamp = 500);
    assert_eq!(
        client.try_set_min_topup(&admin, &3_000_000i128),
        Ok(Err(Error::CooldownActive))
    );
}

/// CONFIG_COOLDOWN_SECS constant is exactly 21600 (6 hours).
#[test]
fn cooldown_constant_value() {
    assert_eq!(subscription_vault::CONFIG_COOLDOWN_SECS, 21_600);
}
