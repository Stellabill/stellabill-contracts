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

// ── set_whitelist_mode: adversarial coverage (#1114) ─────────────────────────
//
// `set_whitelist_mode` flips a global instance-storage flag and is gated on the
// stored admin. The tests below pin down the full observable contract:
//
//   * valid calls in both directions (enable / disable),
//   * idempotency when the requested value already matches storage,
//   * rejection of a non-admin caller (`Error::Forbidden` from
//     `admin::require_admin_auth`) with **state unchanged** and no event,
//   * rejection with `Error::NotInitialized` before `init`,
//   * the `merchant_whitelist_toggled` event payload,
//   * the gate's interaction with `initialize_merchant_config`.

/// Register a fresh, *uninitialised* contract instance (no admin stored).
fn setup_uninitialized() -> (Env, SubscriptionVaultClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    (env, client)
}

/// Decode every `merchant_whitelist_toggled` event currently visible to `env`.
///
/// Works whether the harness accumulates events across calls or resets them per
/// call, so it is safe to assert on delta counts.
fn whitelist_toggle_events(env: &Env) -> Vec<MerchantWhitelistModeEvent> {
    let expected_topic = Symbol::new(env, "merchant_whitelist_toggled");
    let mut found = Vec::new(env);
    for (_contract_id, topics, data) in env.events().all() {
        let topic = match topics.get(0) {
            Some(topic) => topic,
            None => continue,
        };
        if let Ok(sym) = Symbol::try_from_val(env, &topic) {
            if sym == expected_topic {
                if let Ok(event) = MerchantWhitelistModeEvent::try_from_val(env, &data) {
                    found.push_back(event);
                }
            }
        }
    }
    found
}

#[test]
fn set_whitelist_mode_enable_then_disable_round_trip() {
    let (env, client, admin) = setup();
    env.ledger().with_mut(|l| l.timestamp = 1_000);

    assert!(!client.get_whitelist_mode());
    client.set_whitelist_mode(&admin, &true);
    assert!(client.get_whitelist_mode());
    client.set_whitelist_mode(&admin, &false);
    assert!(!client.get_whitelist_mode());

    let events = whitelist_toggle_events(&env);
    assert_eq!(events.len(), 2, "each accepted toggle must emit one event");
    assert!(events.get(0).unwrap().enabled);
    assert!(!events.get(1).unwrap().enabled);
}

#[test]
fn set_whitelist_mode_is_idempotent_when_already_enabled() {
    let (_env, client, admin) = setup();
    client.set_whitelist_mode(&admin, &true);
    // Writing the same value again is a valid, no-error call and must not flip it.
    client.set_whitelist_mode(&admin, &true);
    assert!(client.get_whitelist_mode());
}

#[test]
fn set_whitelist_mode_is_idempotent_when_already_disabled() {
    let (_env, client, admin) = setup();
    // Default is disabled; writing `false` again must still succeed.
    client.set_whitelist_mode(&admin, &false);
    client.set_whitelist_mode(&admin, &false);
    assert!(!client.get_whitelist_mode());
}

#[test]
fn set_whitelist_mode_emits_expected_event_payload() {
    let (env, client, admin) = setup();
    env.ledger().with_mut(|l| l.timestamp = 4_242);

    client.set_whitelist_mode(&admin, &true);

    let events = whitelist_toggle_events(&env);
    assert_eq!(events.len(), 1);
    let event = events.get(0).unwrap();
    assert!(event.enabled);
    assert_eq!(event.admin, admin);
    assert_eq!(event.timestamp, 4_242);
    assert_eq!(event.schema_version, EVENT_SCHEMA_VERSION);
}

#[test]
fn non_admin_set_whitelist_mode_rejected_and_state_unchanged() {
    let (env, client, _admin) = setup();
    let stranger = Address::generate(&env);

    assert!(!client.get_whitelist_mode());
    let result = client.try_set_whitelist_mode(&stranger, &true);
    assert_eq!(result, Err(Ok(Error::Forbidden)));

    // A rejected write must not mutate storage ...
    assert!(!client.get_whitelist_mode());
    // ... nor emit the toggle event.
    assert_eq!(whitelist_toggle_events(&env).len(), 0);
}

#[test]
fn non_admin_cannot_disable_whitelist_mode_and_state_unchanged() {
    let (env, client, admin) = setup();
    let stranger = Address::generate(&env);

    client.set_whitelist_mode(&admin, &true);
    assert!(client.get_whitelist_mode());
    let events_before = whitelist_toggle_events(&env).len();

    let result = client.try_set_whitelist_mode(&stranger, &false);
    assert_eq!(result, Err(Ok(Error::Forbidden)));

    // The enabled flag survives the rejected disable attempt.
    assert!(client.get_whitelist_mode());
    assert_eq!(whitelist_toggle_events(&env).len(), events_before);
}

#[test]
fn set_whitelist_mode_before_init_returns_not_initialized() {
    let (env, client) = setup_uninitialized();
    let anyone = Address::generate(&env);

    let result = client.try_set_whitelist_mode(&anyone, &true);
    assert_eq!(result, Err(Ok(Error::NotInitialized)));
    assert!(!client.get_whitelist_mode());
    assert_eq!(whitelist_toggle_events(&env).len(), 0);
}

#[test]
fn toggling_whitelist_mode_preserves_existing_merchant_approvals() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);

    client.approve_merchant(&admin, &merchant);
    assert!(client.is_merchant_approved(&merchant));

    client.set_whitelist_mode(&admin, &true);
    client.set_whitelist_mode(&admin, &false);

    // Approval state is orthogonal to the global toggle.
    assert!(client.is_merchant_approved(&merchant));
}

#[test]
fn disabling_whitelist_mode_reopens_unapproved_registration() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);
    let redirect = soroban_sdk::String::from_str(&env, "");

    // Whitelist on + merchant not approved → registration is blocked.
    client.set_whitelist_mode(&admin, &true);
    let blocked = client.try_initialize_merchant_config(
        &merchant, &payout, &0, &1, &None, &redirect,
    );
    assert_eq!(blocked, Err(Ok(Error::MerchantNotApproved)));
    assert!(client.get_merchant_config(&merchant).is_none());

    // Turning the whitelist back off re-opens registration for the same merchant.
    client.set_whitelist_mode(&admin, &false);
    client.initialize_merchant_config(&merchant, &payout, &0, &1, &None, &redirect);
    assert!(client.get_merchant_config(&merchant).is_some());
}

#[test]
fn set_whitelist_mode_state_survives_ledger_advance() {
    let (env, client, admin) = setup();
    client.set_whitelist_mode(&admin, &true);

    // The flag lives in instance storage; advancing the ledger must not reset it.
    env.ledger().with_mut(|l| l.timestamp += 1_000_000);

    assert!(client.get_whitelist_mode());
}
