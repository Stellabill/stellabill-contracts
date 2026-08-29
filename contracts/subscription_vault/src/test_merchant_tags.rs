//! Tests for merchant compliance-category tags (#564).
//!
//! Covers the admin-controlled tag allowlist (`set_tag_allowlist` /
//! `get_tag_allowlist`) and per-merchant tag assignment (`set_merchant_tags` /
//! `get_merchant_tags`): authorization, the `MAX_MERCHANT_TAGS` bound,
//! unknown/duplicate tag rejection, replace-not-append semantics, event
//! emission, and the "clearing must work even on a blocked/paused merchant"
//! requirement from the issue.

use crate::types::MAX_MERCHANT_TAGS;
use crate::{Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::{Address as _, Events as _};
use soroban_sdk::{Address, Env, Symbol, Vec};

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

fn tag(env: &Env, s: &str) -> Symbol {
    Symbol::new(env, s)
}

fn base_allowlist(env: &Env) -> Vec<Symbol> {
    Vec::from_array(
        env,
        [tag(env, "saas"), tag(env, "media"), tag(env, "nonprofit")],
    )
}

// ── Tag allowlist ────────────────────────────────────────────────────────────

#[test]
fn allowlist_defaults_to_empty() {
    let (_env, client, _admin) = setup();
    assert_eq!(client.get_tag_allowlist().len(), 0);
}

#[test]
fn admin_can_set_tag_allowlist() {
    let (env, client, admin) = setup();
    let allowlist = base_allowlist(&env);
    client.set_tag_allowlist(&admin, &allowlist);
    assert_eq!(client.get_tag_allowlist(), allowlist);
}

#[test]
fn admin_can_replace_tag_allowlist() {
    let (env, client, admin) = setup();
    client.set_tag_allowlist(&admin, &base_allowlist(&env));
    let smaller = Vec::from_array(&env, [tag(&env, "saas")]);
    client.set_tag_allowlist(&admin, &smaller);
    assert_eq!(client.get_tag_allowlist(), smaller);
}

#[test]
fn non_admin_cannot_set_tag_allowlist() {
    let (env, client, _admin) = setup();
    let non_admin = Address::generate(&env);
    let result = client.try_set_tag_allowlist(&non_admin, &base_allowlist(&env));
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn duplicate_tag_in_allowlist_rejected() {
    let (env, client, admin) = setup();
    let dup = Vec::from_array(&env, [tag(&env, "saas"), tag(&env, "saas")]);
    let result = client.try_set_tag_allowlist(&admin, &dup);
    assert_eq!(result, Err(Ok(Error::DuplicateMerchantTag)));
    // Rejected call must not have partially applied.
    assert_eq!(client.get_tag_allowlist().len(), 0);
}

#[test]
fn empty_allowlist_is_valid() {
    let (env, client, admin) = setup();
    client.set_tag_allowlist(&admin, &base_allowlist(&env));
    client.set_tag_allowlist(&admin, &Vec::new(&env));
    assert_eq!(client.get_tag_allowlist().len(), 0);
}

#[test]
fn set_tag_allowlist_emits_event() {
    let (env, client, admin) = setup();
    let before = env.events().all().len();
    client.set_tag_allowlist(&admin, &base_allowlist(&env));
    assert_eq!(env.events().all().len(), before + 1);
}

// ── Per-merchant tags: happy path ────────────────────────────────────────────

#[test]
fn merchant_tags_default_to_empty() {
    let (env, client, _admin) = setup();
    let merchant = Address::generate(&env);
    assert_eq!(client.get_merchant_tags(&merchant).len(), 0);
}

#[test]
fn admin_can_assign_allowlisted_tags_to_merchant() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);
    client.set_tag_allowlist(&admin, &base_allowlist(&env));

    let tags = Vec::from_array(&env, [tag(&env, "saas"), tag(&env, "nonprofit")]);
    client.set_merchant_tags(&admin, &merchant, &tags);

    assert_eq!(client.get_merchant_tags(&merchant), tags);
}

#[test]
fn setting_merchant_tags_replaces_not_appends() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);
    client.set_tag_allowlist(&admin, &base_allowlist(&env));

    client.set_merchant_tags(&admin, &merchant, &Vec::from_array(&env, [tag(&env, "saas")]));
    // Re-set with a disjoint tag; the first tag must not linger.
    let second = Vec::from_array(&env, [tag(&env, "media")]);
    client.set_merchant_tags(&admin, &merchant, &second);

    assert_eq!(client.get_merchant_tags(&merchant), second);
}

#[test]
fn non_admin_cannot_set_merchant_tags() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);
    let non_admin = Address::generate(&env);
    client.set_tag_allowlist(&admin, &base_allowlist(&env));

    let result = client.try_set_merchant_tags(
        &non_admin,
        &merchant,
        &Vec::from_array(&env, [tag(&env, "saas")]),
    );
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn set_merchant_tags_emits_event() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);
    client.set_tag_allowlist(&admin, &base_allowlist(&env));

    let before = env.events().all().len();
    client.set_merchant_tags(&admin, &merchant, &Vec::from_array(&env, [tag(&env, "saas")]));
    assert_eq!(env.events().all().len(), before + 1);
}

// ── Edge cases required by the issue ─────────────────────────────────────────

#[test]
fn exact_max_merchant_tags_is_accepted() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);

    // Build an allowlist and a tag set of exactly MAX_MERCHANT_TAGS entries.
    let mut allowlist: Vec<Symbol> = Vec::new(&env);
    for i in 0..MAX_MERCHANT_TAGS {
        allowlist.push_back(tag(&env, &format!("tag{i}")));
    }
    client.set_tag_allowlist(&admin, &allowlist);

    client.set_merchant_tags(&admin, &merchant, &allowlist);
    assert_eq!(client.get_merchant_tags(&merchant).len(), MAX_MERCHANT_TAGS);
}

#[test]
fn over_limit_tag_count_rejected() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);

    // One more than MAX_MERCHANT_TAGS, all present in the allowlist.
    let mut allowlist: Vec<Symbol> = Vec::new(&env);
    for i in 0..(MAX_MERCHANT_TAGS + 1) {
        allowlist.push_back(tag(&env, &format!("tag{i}")));
    }
    client.set_tag_allowlist(&admin, &allowlist);

    let result = client.try_set_merchant_tags(&admin, &merchant, &allowlist);
    assert_eq!(result, Err(Ok(Error::MerchantTagLimitExceeded)));
    // Rejected call must not have partially applied.
    assert_eq!(client.get_merchant_tags(&merchant).len(), 0);
}

#[test]
fn unknown_tag_rejected() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);
    client.set_tag_allowlist(&admin, &base_allowlist(&env));

    // "gambling" was never added to the allowlist.
    let result = client.try_set_merchant_tags(
        &admin,
        &merchant,
        &Vec::from_array(&env, [tag(&env, "saas"), tag(&env, "gambling")]),
    );
    assert_eq!(result, Err(Ok(Error::UnknownMerchantTag)));
    assert_eq!(client.get_merchant_tags(&merchant).len(), 0);
}

#[test]
fn unknown_tag_rejected_against_empty_allowlist() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);
    // No allowlist configured at all — every tag is "unknown".
    let result = client.try_set_merchant_tags(
        &admin,
        &merchant,
        &Vec::from_array(&env, [tag(&env, "saas")]),
    );
    assert_eq!(result, Err(Ok(Error::UnknownMerchantTag)));
}

#[test]
fn duplicate_tag_in_merchant_call_rejected() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);
    client.set_tag_allowlist(&admin, &base_allowlist(&env));

    let dup = Vec::from_array(&env, [tag(&env, "saas"), tag(&env, "saas")]);
    let result = client.try_set_merchant_tags(&admin, &merchant, &dup);
    assert_eq!(result, Err(Ok(Error::DuplicateMerchantTag)));
    assert_eq!(client.get_merchant_tags(&merchant).len(), 0);
}

#[test]
fn clearing_tags_succeeds_on_blocklisted_merchant() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);
    client.set_tag_allowlist(&admin, &base_allowlist(&env));
    client.set_merchant_tags(&admin, &merchant, &Vec::from_array(&env, [tag(&env, "saas")]));
    assert_eq!(client.get_merchant_tags(&merchant).len(), 1);

    // Block the merchant, then confirm both re-tagging and clearing still work —
    // compliance metadata must not be gated by the enforcement state it informs.
    client.add_to_blocklist(&admin, &merchant, &None);
    assert!(client.is_blocklisted(&merchant));

    client.set_merchant_tags(&admin, &merchant, &Vec::new(&env));
    assert_eq!(client.get_merchant_tags(&merchant).len(), 0);
}

#[test]
fn clearing_tags_succeeds_on_paused_merchant() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);
    client.set_tag_allowlist(&admin, &base_allowlist(&env));
    client.initialize_merchant_config(
        &merchant,
        &payout,
        &0,
        &1,
        &None,
        &soroban_sdk::String::from_str(&env, ""),
    );
    client.set_merchant_tags(&admin, &merchant, &Vec::from_array(&env, [tag(&env, "media")]));

    client.pause_merchant(&merchant);
    assert!(client.get_merchant_paused(&merchant));

    client.set_merchant_tags(&admin, &merchant, &Vec::new(&env));
    assert_eq!(client.get_merchant_tags(&merchant).len(), 0);
}

#[test]
fn shrinking_allowlist_does_not_retroactively_clear_existing_tags() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);
    client.set_tag_allowlist(&admin, &base_allowlist(&env));
    client.set_merchant_tags(&admin, &merchant, &Vec::from_array(&env, [tag(&env, "nonprofit")]));

    // Remove "nonprofit" from the allowlist.
    client.set_tag_allowlist(&admin, &Vec::from_array(&env, [tag(&env, "saas")]));

    // Already-assigned tag is untouched...
    assert_eq!(
        client.get_merchant_tags(&merchant),
        Vec::from_array(&env, [tag(&env, "nonprofit")])
    );
    // ...but re-using the now-removed tag on a future call is rejected.
    let result = client.try_set_merchant_tags(
        &admin,
        &merchant,
        &Vec::from_array(&env, [tag(&env, "nonprofit")]),
    );
    assert_eq!(result, Err(Ok(Error::UnknownMerchantTag)));
}

#[test]
fn zero_tags_is_valid_even_with_populated_allowlist() {
    let (env, client, admin) = setup();
    let merchant = Address::generate(&env);
    client.set_tag_allowlist(&admin, &base_allowlist(&env));
    client.set_merchant_tags(&admin, &merchant, &Vec::new(&env));
    assert_eq!(client.get_merchant_tags(&merchant).len(), 0);
}

#[test]
fn tags_are_independent_per_merchant() {
    let (env, client, admin) = setup();
    let merchant_a = Address::generate(&env);
    let merchant_b = Address::generate(&env);
    client.set_tag_allowlist(&admin, &base_allowlist(&env));

    client.set_merchant_tags(&admin, &merchant_a, &Vec::from_array(&env, [tag(&env, "saas")]));
    client.set_merchant_tags(&admin, &merchant_b, &Vec::from_array(&env, [tag(&env, "media")]));

    assert_eq!(
        client.get_merchant_tags(&merchant_a),
        Vec::from_array(&env, [tag(&env, "saas")])
    );
    assert_eq!(
        client.get_merchant_tags(&merchant_b),
        Vec::from_array(&env, [tag(&env, "media")])
    );
}
