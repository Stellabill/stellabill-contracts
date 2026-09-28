use crate::{Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

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
