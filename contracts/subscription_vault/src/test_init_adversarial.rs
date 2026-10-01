//! Adversarial coverage for [`SubscriptionVault::init`].
//!
//! `init` is the contract's one-shot initializer (it delegates to
//! [`crate::admin::do_init`]). It is the single place where the settlement
//! token, its decimals, the admin address, the minimum top-up threshold and
//! the grace period are written, so a defect here poisons every later call.
//!
//! This module deliberately pins the *rejection* paths as hard as the happy
//! path:
//!
//! * valid call writes every configuration value verbatim,
//! * `token_decimals` boundaries (`0`, `19` accepted; `20` and `u32::MAX`
//!   rejected with [`Error::InvalidTokenDecimals`]),
//! * `min_topup` boundaries (`1` and `i128::MAX` accepted; `0`, `-1` and
//!   `i128::MIN` rejected with [`Error::InvalidAmount`]),
//! * `grace_period` boundaries (`0` and `u64::MAX` accepted),
//! * a token equal to the contract's own address rejected with
//!   [`Error::InvalidToken`],
//! * re-initialization rejected with [`Error::AlreadyInitialized`] and the
//!   first configuration left completely intact,
//! * after **every** rejected operation the contract is either still fully
//!   uninitialized or still holds the original configuration — never a
//!   half-written mixture.
//!
//! Error discriminants are asserted through `try_*` results (deterministic
//! `Err(Ok(Error::X))`) as well as through the repo's `should_panic`
//! convention, and a dedicated test guards the numeric discriminants
//! themselves.

use crate::types::Error;
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::{Address as _, Events as _};
use soroban_sdk::{Address, Env, Symbol, TryFromVal};

const DECIMALS: u32 = 6;
const MIN_TOPUP: i128 = 1_000_000;
const GRACE: u64 = 7 * 24 * 60 * 60;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Registers a fresh vault on `env` with all auth mocked. The contract is
/// **not** initialized — callers decide whether to call `init`.
fn fresh_vault<'a>(env: &'a Env) -> (SubscriptionVaultClient<'a>, Address) {
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    let admin = Address::generate(env);
    (client, admin)
}

/// Registers a real Stellar asset contract so `token` is a plausible
/// settlement asset rather than a random address.
fn token_for(env: &Env, admin: &Address) -> Address {
    env.register_stellar_asset_contract_v2(admin.clone()).address()
}

/// Reads the stored grace period through the same helper `init` writes with.
fn stored_grace(env: &Env, client: &SubscriptionVaultClient) -> u64 {
    env.as_contract(&client.address, || {
        crate::admin::get_grace_period(env).expect("grace period must be readable")
    })
}

/// Reads the stored decimals for `token` through the admin helper.
fn stored_decimals(env: &Env, client: &SubscriptionVaultClient, token: &Address) -> Option<u32> {
    env.as_contract(&client.address, || {
        crate::admin::get_token_decimals(env, token).ok()
    })
}

/// Reads the persisted schema version (`0` before `init`).
fn stored_schema(env: &Env, client: &SubscriptionVaultClient) -> u32 {
    env.as_contract(&client.address, || crate::admin::get_schema_version(env))
}

/// Asserts the vault has never been initialized: schema still zero, admin and
/// min-topup reads fail with `NotInitialized`, and the candidate token was not
/// registered.
fn assert_uninitialized(env: &Env, client: &SubscriptionVaultClient, token: &Address) {
    assert_eq!(stored_schema(env, client), 0, "schema must not advance");
    assert_eq!(
        client.try_get_admin(),
        Err(Ok(Error::NotInitialized)),
        "admin must remain unset"
    );
    assert_eq!(
        client.try_get_min_topup(),
        Err(Ok(Error::NotInitialized)),
        "min_topup must remain unset"
    );
    assert_eq!(
        stored_decimals(env, client, token),
        None,
        "rejected init must not register the token"
    );
    assert_eq!(stored_grace(env, client), 0, "grace period must remain zero");
}

/// Asserts the vault holds exactly the configuration written by the first
/// successful `init`.
#[allow(clippy::too_many_arguments)]
fn assert_config(
    env: &Env,
    client: &SubscriptionVaultClient,
    token: &Address,
    admin: &Address,
    decimals: u32,
    min_topup: i128,
    grace: u64,
) {
    assert_eq!(client.get_admin(), admin.clone());
    assert_eq!(client.get_min_topup(), min_topup);
    assert_eq!(stored_decimals(env, client, token), Some(decimals));
    assert_eq!(stored_grace(env, client), grace);
    assert_eq!(stored_schema(env, client), crate::STORAGE_VERSION);
}

// ── Happy path ────────────────────────────────────────────────────────────────

#[test]
fn init_persists_every_configuration_value_verbatim() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);

    assert_eq!(client.try_init(&token, &DECIMALS, &admin, &MIN_TOPUP, &GRACE), Ok(Ok(())));

    assert_config(&env, &client, &token, &admin, DECIMALS, MIN_TOPUP, GRACE);
}

#[test]
fn init_emits_single_initialized_event() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);

    client.init(&token, &DECIMALS, &admin, &MIN_TOPUP, &GRACE);

    let events = env.events().all();
    assert_eq!(events.len(), 1, "init must emit exactly one event");
    let (contract, topics, data) = events.get(0).unwrap();
    assert_eq!(contract, client.address);
    assert_eq!(topics.len(), 1);
    let topic: Symbol = Symbol::try_from_val(&env, &topics.get(0).unwrap()).unwrap();
    assert_eq!(topic, Symbol::new(&env, "initialized"));
    // The payload carries the whole configuration written by `init`.
    let payload: (Address, Address, i128, u64) = TryFromVal::try_from_val(&env, &data).unwrap();
    assert_eq!(payload, (token.clone(), admin.clone(), MIN_TOPUP, GRACE));
}

// ── token_decimals boundaries ─────────────────────────────────────────────────

#[test]
fn init_accepts_token_decimals_lower_bound_zero() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);

    assert_eq!(client.try_init(&token, &0u32, &admin, &MIN_TOPUP, &GRACE), Ok(Ok(())));
    assert_eq!(stored_decimals(&env, &client, &token), Some(0));
}

#[test]
fn init_accepts_token_decimals_upper_bound_nineteen() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);

    assert_eq!(client.try_init(&token, &19u32, &admin, &MIN_TOPUP, &GRACE), Ok(Ok(())));
    assert_eq!(stored_decimals(&env, &client, &token), Some(19));
}

#[test]
fn init_rejects_token_decimals_just_above_upper_bound() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);

    let result = client.try_init(&token, &20u32, &admin, &MIN_TOPUP, &GRACE);
    assert_eq!(result, Err(Ok(Error::InvalidTokenDecimals)));
    assert_uninitialized(&env, &client, &token);
}

#[test]
fn init_rejects_token_decimals_u32_max_and_leaves_state_unchanged() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);

    let result = client.try_init(&token, &u32::MAX, &admin, &MIN_TOPUP, &GRACE);
    assert_eq!(result, Err(Ok(Error::InvalidTokenDecimals)));
    assert_uninitialized(&env, &client, &token);
}

#[test]
#[should_panic(expected = "Error(Contract, #8001)")] // Error::InvalidTokenDecimals
fn init_rejects_token_decimals_twenty_panics() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);
    let _ = client.init(&token, &20u32, &admin, &MIN_TOPUP, &GRACE);
}

// ── min_topup boundaries ──────────────────────────────────────────────────────

#[test]
fn init_accepts_min_topup_lower_bound_one() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);

    assert_eq!(client.try_init(&token, &DECIMALS, &admin, &1i128, &GRACE), Ok(Ok(())));
    assert_eq!(client.get_min_topup(), 1);
}

#[test]
fn init_accepts_min_topup_i128_max() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);

    assert_eq!(
        client.try_init(&token, &DECIMALS, &admin, &i128::MAX, &GRACE),
        Ok(Ok(()))
    );
    assert_eq!(client.get_min_topup(), i128::MAX);
}

#[test]
fn init_rejects_zero_min_topup() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);

    let result = client.try_init(&token, &DECIMALS, &admin, &0i128, &GRACE);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
    assert_uninitialized(&env, &client, &token);
}

#[test]
fn init_rejects_negative_min_topup() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);

    let result = client.try_init(&token, &DECIMALS, &admin, &-1i128, &GRACE);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
    assert_uninitialized(&env, &client, &token);
}

#[test]
fn init_rejects_i128_min_min_topup() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);

    let result = client.try_init(&token, &DECIMALS, &admin, &i128::MIN, &GRACE);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
    assert_uninitialized(&env, &client, &token);
}

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // Error::InvalidAmount
fn init_rejects_negative_min_topup_panics() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);
    let _ = client.init(&token, &DECIMALS, &admin, &-1i128, &GRACE);
}

// ── grace_period boundaries ───────────────────────────────────────────────────

#[test]
fn init_accepts_grace_period_zero() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);

    assert_eq!(client.try_init(&token, &DECIMALS, &admin, &MIN_TOPUP, &0u64), Ok(Ok(())));
    assert_eq!(stored_grace(&env, &client), 0);
}

#[test]
fn init_accepts_grace_period_u64_max() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);

    assert_eq!(
        client.try_init(&token, &DECIMALS, &admin, &MIN_TOPUP, &u64::MAX),
        Ok(Ok(()))
    );
    assert_eq!(stored_grace(&env, &client), u64::MAX);
}

// ── token address boundary ────────────────────────────────────────────────────

#[test]
fn init_rejects_contract_address_as_settlement_token() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    // The contract's own address as the settlement token would make the vault
    // try to transfer from itself; `do_init` must reject it.
    let self_token = client.address.clone();

    let result = client.try_init(&self_token, &DECIMALS, &admin, &MIN_TOPUP, &GRACE);
    assert_eq!(result, Err(Ok(Error::InvalidToken)));
    assert_uninitialized(&env, &client, &self_token);
}

#[test]
#[should_panic(expected = "Error(Contract, #8002)")] // Error::InvalidToken
fn init_rejects_contract_address_as_token_panics() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let self_token = client.address.clone();
    let _ = client.init(&self_token, &DECIMALS, &admin, &MIN_TOPUP, &GRACE);
}

// ── Double init / re-initialization ───────────────────────────────────────────

#[test]
fn init_is_one_shot_and_rejects_a_second_call() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);
    client.init(&token, &DECIMALS, &admin, &MIN_TOPUP, &GRACE);

    let result = client.try_init(&token, &DECIMALS, &admin, &MIN_TOPUP, &GRACE);
    assert_eq!(result, Err(Ok(Error::AlreadyInitialized)));
    assert_config(&env, &client, &token, &admin, DECIMALS, MIN_TOPUP, GRACE);
}

#[test]
#[should_panic(expected = "Error(Contract, #4008)")] // Error::AlreadyInitialized
fn init_rejects_second_call_panics() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);
    client.init(&token, &DECIMALS, &admin, &MIN_TOPUP, &GRACE);
    let _ = client.init(&token, &DECIMALS, &admin, &MIN_TOPUP, &GRACE);
}

#[test]
fn second_init_by_different_caller_cannot_hijack_configuration() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);
    client.init(&token, &DECIMALS, &admin, &MIN_TOPUP, &GRACE);

    // An attacker (or even a legitimate second deployer) calls init again with
    // their own admin/token/decimals and boundary-legal amounts.
    let attacker = Address::generate(&env);
    let evil_token = token_for(&env, &attacker);
    let result =
        client.try_init(&evil_token, &19u32, &attacker, &i128::MAX, &u64::MAX);
    assert_eq!(result, Err(Ok(Error::AlreadyInitialized)));

    // The original configuration is byte-for-byte intact: nothing from the
    // rejected call leaked into storage.
    assert_config(&env, &client, &token, &admin, DECIMALS, MIN_TOPUP, GRACE);
    assert_eq!(
        stored_decimals(&env, &client, &evil_token),
        None,
        "rejected second init must not register an attacker token"
    );
    assert_ne!(client.get_admin(), attacker);
}

#[test]
fn rejected_double_init_does_not_advance_schema_version() {
    let env = Env::default();
    let (client, admin) = fresh_vault(&env);
    let token = token_for(&env, &admin);
    client.init(&token, &DECIMALS, &admin, &MIN_TOPUP, &GRACE);
    let schema_before = stored_schema(&env, &client);

    let _ = client.try_init(&token, &DECIMALS, &admin, &MIN_TOPUP, &GRACE);

    assert_eq!(stored_schema(&env, &client), schema_before);
}

// ── Unauthorized / permissionless-first-caller semantics ──────────────────────

#[test]
fn first_caller_can_init_without_auth_and_init_is_then_immutable() {
    // `init` intentionally performs no `require_auth` (it is the deployment
    // initializer; the deployer is expected to initialize atomically in the
    // deployment transaction). This test pins both halves of that contract:
    // the first caller succeeds even with auth un-mocked, and no later caller
    // can ever run it again.
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(admin.clone()).address();

    // No `mock_all_auths`: init must still succeed.
    assert_eq!(client.try_init(&token, &DECIMALS, &admin, &MIN_TOPUP, &GRACE), Ok(Ok(())));

    // Now, with auth mocked so nothing is blocked by an auth host error, a
    // different caller still cannot re-initialize.
    env.mock_all_auths();
    let other = Address::generate(&env);
    let other_token = env.register_stellar_asset_contract_v2(other.clone()).address();
    let result = client.try_init(&other_token, &DECIMALS, &other, &MIN_TOPUP, &GRACE);
    assert_eq!(result, Err(Ok(Error::AlreadyInitialized)));
    assert_eq!(client.get_admin(), admin);
}

// ── Rejected configs leave the contract fully uninitialized ───────────────────

#[test]
fn every_rejected_init_variant_leaves_contract_uninitialized() {
    // Each rejected variant gets a fresh vault, then we assert *all* the
    // observable state (schema, admin, min_topup, decimals, grace) is
    // untouched. A partial write anywhere in `do_init` would fail this.
    let env = Env::default();

    enum Bad {
        Decimals,
        ZeroTopup,
        NegativeTopup,
        SelfToken,
    }

    for bad in [Bad::Decimals, Bad::ZeroTopup, Bad::NegativeTopup, Bad::SelfToken] {
        let (client, admin) = fresh_vault(&env);
        let token = token_for(&env, &admin);
        let probe = match bad {
            Bad::Decimals => client.try_init(&token, &20u32, &admin, &MIN_TOPUP, &GRACE),
            Bad::ZeroTopup => client.try_init(&token, &DECIMALS, &admin, &0i128, &GRACE),
            Bad::NegativeTopup => client.try_init(&token, &DECIMALS, &admin, &-42i128, &GRACE),
            Bad::SelfToken => {
                client.try_init(&client.address, &DECIMALS, &admin, &MIN_TOPUP, &GRACE)
            }
        };
        assert!(probe.is_err(), "adversarial init variant must be rejected");
        assert_uninitialized(&env, &client, &token);
    }
}

// ── Discriminant guards ───────────────────────────────────────────────────────

#[test]
fn init_error_discriminants_are_stable() {
    // The assertions above stringify these codes through the host
    // (`Error(Contract, #N)`); this test pins the numeric discriminants
    // themselves so an enum renumbering cannot silently change the ABI the
    // tests rely on.
    assert_eq!(Error::NotInitialized.to_code(), 2002);
    assert_eq!(Error::InvalidAmount.to_code(), 3001);
    assert_eq!(Error::AlreadyInitialized.to_code(), 4008);
    assert_eq!(Error::InvalidTokenDecimals.to_code(), 8001);
    assert_eq!(Error::InvalidToken.to_code(), 8002);
}
