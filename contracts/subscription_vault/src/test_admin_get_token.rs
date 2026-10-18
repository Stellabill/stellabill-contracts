//! Adversarial coverage for `admin::get_token` (issue #989).
//!
//! `get_token` returns the vault's *default* settlement token and is the
//! source of truth used by `remove_accepted_token` to protect that token from
//! removal. It is a small function, but every branch matters:
//!
//! * it is the only reader of `DataKey::Token`, so a swap between the
//!   persistent and instance stores (schema migration) must not change what
//!   callers observe;
//! * it must return `Error::NotFound` — not a panic — on an uninitialized
//!   vault;
//! * adding or removing *other* accepted tokens must never change it;
//! * the default token must remain un-removable, which is only possible if
//!   `get_token` is correct.
//!
//! `get_token` has no public contract entry point, so it (and the accepted-token
//! bookkeeping it protects) is read through the internal helpers inside the
//! vault's storage context.

use crate::admin::{get_token, get_token_decimals, is_token_accepted};
use crate::test_utils::setup::TestEnv;
use crate::types::{DataKey, Error};
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

/// Read the default token through the internal getter inside the vault's
/// storage context.
fn default_token(env: &Env, client: &SubscriptionVaultClient) -> Result<Address, Error> {
    env.as_contract(&client.address, || get_token(env))
}

fn accepted(env: &Env, client: &SubscriptionVaultClient, token: &Address) -> bool {
    env.as_contract(&client.address, || is_token_accepted(env, token))
}

#[test]
fn returns_the_token_supplied_at_init() {
    let te = TestEnv::default();
    assert_eq!(default_token(&te.env, &te.client), Ok(te.token.clone()));
}

#[test]
fn returns_not_found_before_initialization() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    // `read_config` must fall through both storage tiers and surface the
    // typed error rather than panicking on missing state.
    assert_eq!(default_token(&env, &client), Err(Error::NotFound));
}

#[test]
fn adding_an_accepted_token_does_not_change_the_default_token() {
    let te = TestEnv::default();
    let other = Address::generate(&te.env);

    te.client.add_accepted_token(&te.admin, &other, &7u32);

    assert_eq!(default_token(&te.env, &te.client), Ok(te.token.clone()));
    assert!(accepted(&te.env, &te.client, &other));
}

#[test]
fn removing_a_non_default_accepted_token_leaves_the_default_token_intact() {
    let te = TestEnv::default();
    let other = Address::generate(&te.env);

    te.client.add_accepted_token(&te.admin, &other, &7u32);
    // add/remove share the "AcceptedTokens" cooldown key, so space them out.
    te.jump(6 * 60 * 60 + 1);
    te.client.remove_accepted_token(&te.admin, &other);

    assert!(!accepted(&te.env, &te.client, &other));
    assert_eq!(default_token(&te.env, &te.client), Ok(te.token.clone()));
}

#[test]
fn the_default_token_cannot_be_removed() {
    let te = TestEnv::default();

    let res = te.client.try_remove_accepted_token(&te.admin, &te.token);

    assert_eq!(res, Err(Ok(Error::InvalidInput)));
    assert!(accepted(&te.env, &te.client, &te.token));
    assert_eq!(default_token(&te.env, &te.client), Ok(te.token.clone()));
}

#[test]
fn repeated_reads_are_pure() {
    let te = TestEnv::default();
    for _ in 0..5 {
        assert_eq!(default_token(&te.env, &te.client), Ok(te.token.clone()));
    }
}

#[test]
fn unknown_accepted_token_has_no_decimals_entry() {
    let te = TestEnv::default();
    let unknown = Address::generate(&te.env);

    // get_token stays authoritative for the default; an arbitrary address is
    // not implicitly accepted just because `get_token` returned `Ok`.
    assert_eq!(default_token(&te.env, &te.client), Ok(te.token.clone()));
    assert_eq!(
        te.env
            .as_contract(&te.client.address, || get_token_decimals(&te.env, &unknown)),
        Err(Error::NotFound),
    );
    assert!(!accepted(&te.env, &te.client, &unknown));
}

#[test]
fn token_key_is_stored_in_the_persistent_tier_after_init() {
    let te = TestEnv::default();
    // Init writes the schema version first, which makes `write_config` route
    // the token to persistent storage; the getter must read it back from there.
    let version = te
        .env
        .as_contract(&te.client.address, || crate::admin::get_schema_version(&te.env));
    assert!(version >= 3);

    te.env.as_contract(&te.client.address, || {
        let storage = te.env.storage();
        assert!(storage.persistent().has(&DataKey::Token));
        assert!(!storage.instance().has(&DataKey::Token));
    });

    assert_eq!(default_token(&te.env, &te.client), Ok(te.token.clone()));
}

#[test]
fn get_token_is_not_affected_by_a_rejected_remove_call() {
    let te = TestEnv::default();
    let other = Address::generate(&te.env);
    te.client.add_accepted_token(&te.admin, &other, &7u32);

    // A non-admin caller must be rejected before any state changes.
    let stranger = Address::generate(&te.env);
    let res = te.client.try_remove_accepted_token(&stranger, &other);

    assert_eq!(res, Err(Ok(Error::Forbidden)));
    assert!(accepted(&te.env, &te.client, &other));
    assert_eq!(default_token(&te.env, &te.client), Ok(te.token.clone()));
}
