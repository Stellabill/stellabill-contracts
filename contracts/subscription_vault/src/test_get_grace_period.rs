//! Adversarial coverage for `admin::get_grace_period` (issue #986).
//!
//! # What is under test
//!
//! ```text
//! pub fn get_grace_period(env: &Env) -> Result<u64, Error> {
//!     Ok(env.storage().instance().get(&DataKey::GracePeriod).unwrap_or(0))
//! }
//! ```
//!
//! `get_grace_period` reads `DataKey::GracePeriod` from **instance** storage
//! and returns `Ok(0)` when the key is absent.  Because it wraps the result in
//! `Ok(…)` unconditionally it can never return an error — callers that
//! propagate its `Result` (e.g. `read_cached_admin_config`) therefore rely on
//! this infallibility implicitly.
//!
//! The setter `do_set_grace_period` also writes to instance storage and is
//! gated by `require_admin_auth` + `enforce_config_cooldown`; the getter must
//! observe every mutation, and a rejected setter must leave the stored value
//! unchanged.
//!
//! Neither function is exposed as a direct contract entry-point, so both are
//! exercised through `env.as_contract(…)` (matching the pattern in
//! `test_admin_auto_pause_threshold.rs` and `test_get_buyout_premium_bps.rs`).
//!
//! # Test matrix
//!
//! | # | Description |
//! |---|-------------|
//! | 1 | Default absent — uninitialised vault returns `Ok(0)` |
//! | 2 | Init writes the supplied grace period and getter reflects it |
//! | 3 | Round-trip — zero (feature effectively disabled) |
//! | 4 | Round-trip — 1 second (minimal non-zero value) |
//! | 5 | Round-trip — 7 days (standard default) |
//! | 6 | Round-trip — u64::MAX (boundary; no overflow in getter) |
//! | 7 | Overwrite — second `do_set_grace_period` supersedes the first |
//! | 8 | Repeated reads are pure — no side-effects on storage |
//! | 9 | Non-admin caller rejected with `Forbidden`; stored value unchanged |
//! | 10 | Wrong admin (correct address format, wrong identity) rejected |
//! | 11 | `do_set_grace_period` gated by cooldown — second call within window fails |
//! | 12 | Cooldown boundary — call succeeds exactly at `CONFIG_COOLDOWN_SECS` elapsed |
//! | 13 | Rejected setter leaves stored value unchanged (cooldown) |
//! | 14 | `do_set_grace_period` does not disturb other admin config keys |
//! | 15 | `read_cached_admin_config` uses the value returned by `get_grace_period` |
//! | 16 | Key lives in instance storage, not persistent storage |
//! | 17 | Getter is not affected by values written to persistent storage |
//! | 18 | Uninitialised vault — caller gets `Ok(0)` without panic |

#![cfg(test)]

use crate::{
    admin::{
        do_set_grace_period, get_grace_period, get_min_topup, get_protocol_fee_bps,
        read_cached_admin_config, CONFIG_COOLDOWN_SECS,
    },
    types::DataKey,
    Error, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env,
};

// ── shared constants ──────────────────────────────────────────────────────────

const SEVEN_DAYS: u64 = 7 * 24 * 60 * 60;
const T0: u64 = 1_000_000;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Minimal setup: vault deployed + initialized.
/// Returns `(env, client, admin, contract_id)`.
fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = T0);

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &SEVEN_DAYS);

    (env, client, admin, contract_id)
}

/// Read `DataKey::GracePeriod` via the internal getter from inside the
/// contract context.
fn read_grace(env: &Env, contract_id: &Address) -> Result<u64, Error> {
    env.as_contract(contract_id, || get_grace_period(env))
}

/// Write `grace_period` directly to instance storage, bypassing auth and
/// cooldown checks (used to set up adversarial preconditions).
fn write_grace_instance(env: &Env, contract_id: &Address, value: u64) {
    env.as_contract(contract_id, || {
        env.storage().instance().set(&DataKey::GracePeriod, &value);
    });
}

/// Clear `DataKey::GracePeriod` from both storage tiers.
fn clear_grace(env: &Env, contract_id: &Address) {
    env.as_contract(contract_id, || {
        env.storage().instance().remove(&DataKey::GracePeriod);
        env.storage().persistent().remove(&DataKey::GracePeriod);
    });
}

/// Invoke `do_set_grace_period` from inside the contract context.
fn set_grace(env: &Env, contract_id: &Address, admin: &Address, value: u64) -> Result<(), Error> {
    env.as_contract(contract_id, || {
        do_set_grace_period(env, admin.clone(), value)
    })
}

// ── Test 1: default absent — uninitialised vault ──────────────────────────────

/// A freshly registered contract (no `init` call) must return `Ok(0)` because
/// `DataKey::GracePeriod` has never been written.
#[test]
fn get_grace_period_returns_ok_zero_before_initialization() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());

    // No init, no storage write.
    let result = read_grace(&env, &contract_id);
    assert_eq!(result, Ok(0));
}

// ── Test 2: init writes the supplied grace period ─────────────────────────────

/// `do_init` stores the `grace_period` argument in instance storage; the
/// getter must reflect that value immediately after initialization.
#[test]
fn get_grace_period_reflects_value_set_by_init() {
    let (env, _client, _admin, contract_id) = setup();
    // TestEnv::default() initializes with SEVEN_DAYS.
    assert_eq!(read_grace(&env, &contract_id), Ok(SEVEN_DAYS));
}

// ── Test 3: round-trip — zero ─────────────────────────────────────────────────

/// Zero is a legal value (disabling the grace-period window) and must
/// round-trip without modification.
#[test]
fn get_grace_period_round_trips_zero() {
    let (env, _client, _admin, contract_id) = setup();
    write_grace_instance(&env, &contract_id, 0);
    assert_eq!(read_grace(&env, &contract_id), Ok(0));
}

// ── Test 4: round-trip — 1 second ────────────────────────────────────────────

/// The minimum meaningful grace window is 1 second; the getter must return it
/// without truncation.
#[test]
fn get_grace_period_round_trips_one_second() {
    let (env, _client, _admin, contract_id) = setup();
    write_grace_instance(&env, &contract_id, 1);
    assert_eq!(read_grace(&env, &contract_id), Ok(1));
}

// ── Test 5: round-trip — 7 days ───────────────────────────────────────────────

/// The standard 7-day default is stored and retrieved exactly.
#[test]
fn get_grace_period_round_trips_seven_days() {
    let (env, _client, _admin, contract_id) = setup();
    write_grace_instance(&env, &contract_id, SEVEN_DAYS);
    assert_eq!(read_grace(&env, &contract_id), Ok(SEVEN_DAYS));
}

// ── Test 6: round-trip — u64::MAX ────────────────────────────────────────────

/// The getter applies no upper-bound; `u64::MAX` must be returned as-is
/// without overflow or panic.
#[test]
fn get_grace_period_round_trips_u64_max_without_overflow() {
    let (env, _client, _admin, contract_id) = setup();
    write_grace_instance(&env, &contract_id, u64::MAX);
    assert_eq!(read_grace(&env, &contract_id), Ok(u64::MAX));
}

// ── Test 7: overwrite supersedes previous value ───────────────────────────────

/// A second call to `do_set_grace_period` (after advancing past the cooldown)
/// must overwrite the previous value; the getter must report the latest write.
#[test]
fn get_grace_period_reflects_latest_overwrite() {
    let (env, _client, admin, contract_id) = setup();

    // First write is already done by init (SEVEN_DAYS). Advance past cooldown
    // and overwrite.
    env.ledger()
        .with_mut(|l| l.timestamp = T0 + CONFIG_COOLDOWN_SECS + 1);
    set_grace(&env, &contract_id, &admin, 3 * 24 * 60 * 60)
        .expect("second set_grace_period must succeed after cooldown");

    assert_eq!(read_grace(&env, &contract_id), Ok(3 * 24 * 60 * 60));
}

// ── Test 8: repeated reads are pure ──────────────────────────────────────────

/// Calling `get_grace_period` multiple times in sequence must return the same
/// value each time without mutating storage.
#[test]
fn get_grace_period_repeated_reads_are_pure() {
    let (env, _client, _admin, contract_id) = setup();

    for _ in 0..5 {
        assert_eq!(read_grace(&env, &contract_id), Ok(SEVEN_DAYS));
    }
}

// ── Test 9: non-admin caller rejected, stored value unchanged ─────────────────

/// An address that has never been set as admin must be rejected with
/// `Forbidden`; the stored grace period must be unchanged.
#[test]
fn do_set_grace_period_rejects_non_admin_and_leaves_storage_unchanged() {
    let (env, client, _admin, contract_id) = setup();
    let stranger = Address::generate(&env);

    let before = read_grace(&env, &contract_id).unwrap();

    // The client wraps the call so we can use the `try_` variant for a clean
    // assertion on the error discriminant.
    let _ = client; // held to keep the contract alive
    let res = set_grace(&env, &contract_id, &stranger, 99_999);
    // require_admin_auth checks the stored admin and returns Forbidden for any
    // caller that does not match.
    assert_eq!(res, Err(Error::Forbidden));

    // Storage must be unchanged.
    assert_eq!(read_grace(&env, &contract_id), Ok(before));
}

// ── Test 10: wrong admin identity rejected ────────────────────────────────────

/// A second, separately-generated address (not the stored admin) must be
/// rejected even though it passes `require_auth` (all auths are mocked).
#[test]
fn do_set_grace_period_rejects_wrong_admin_identity() {
    let (env, _client, _admin, contract_id) = setup();
    let impostor = Address::generate(&env);

    let before = read_grace(&env, &contract_id).unwrap();
    let res = set_grace(&env, &contract_id, &impostor, 1_234_567);
    assert_eq!(res, Err(Error::Forbidden));
    assert_eq!(read_grace(&env, &contract_id), Ok(before));
}

// ── Test 11: cooldown gate — second call within window fails ──────────────────

/// Two consecutive calls by the correct admin without advancing the ledger
/// must cause the second call to fail with `CooldownActive`.
#[test]
fn do_set_grace_period_second_call_within_cooldown_fails() {
    let (env, _client, admin, contract_id) = setup();

    // Advance just past the initial cooldown so the first set-after-init
    // succeeds (init wrote at T0; call 1 is at T0 + COOLDOWN + 1).
    env.ledger()
        .with_mut(|l| l.timestamp = T0 + CONFIG_COOLDOWN_SECS + 1);

    set_grace(&env, &contract_id, &admin, 10_000)
        .expect("first set_grace_period must succeed after cooldown");

    // Immediately try a second write — ledger timestamp has not advanced.
    let res = set_grace(&env, &contract_id, &admin, 20_000);
    assert_eq!(res, Err(Error::CooldownActive));
}

// ── Test 12: cooldown boundary — succeeds at exactly CONFIG_COOLDOWN_SECS ────

/// A call that arrives at exactly `last_changed_at + CONFIG_COOLDOWN_SECS`
/// must succeed (the condition is strict: `elapsed < COOLDOWN`).
#[test]
fn do_set_grace_period_succeeds_at_exact_cooldown_boundary() {
    let (env, _client, admin, contract_id) = setup();

    // Advance past the cooldown seeded at init (T0), then make the first
    // explicit set.
    let first_ts = T0 + CONFIG_COOLDOWN_SECS + 1;
    env.ledger().with_mut(|l| l.timestamp = first_ts);
    set_grace(&env, &contract_id, &admin, 10_000).expect("first set must succeed");

    // Advance by exactly CONFIG_COOLDOWN_SECS from `first_ts`.
    env.ledger()
        .with_mut(|l| l.timestamp = first_ts + CONFIG_COOLDOWN_SECS);
    let res = set_grace(&env, &contract_id, &admin, 20_000);
    assert_eq!(
        res,
        Ok(()),
        "call at exactly cooldown boundary (elapsed == COOLDOWN) must succeed"
    );
    assert_eq!(read_grace(&env, &contract_id), Ok(20_000));
}

// ── Test 13: rejected setter (cooldown) leaves stored value unchanged ─────────

/// When `do_set_grace_period` is rejected by the cooldown guard, the value
/// in instance storage must remain the value that was committed by the
/// previous successful write.
#[test]
fn do_set_grace_period_cooldown_rejection_leaves_storage_unchanged() {
    let (env, _client, admin, contract_id) = setup();

    env.ledger()
        .with_mut(|l| l.timestamp = T0 + CONFIG_COOLDOWN_SECS + 1);
    set_grace(&env, &contract_id, &admin, 12_345).expect("first set must succeed");

    // Within the cooldown window.
    let res = set_grace(&env, &contract_id, &admin, 99_999);
    assert_eq!(res, Err(Error::CooldownActive));

    // The stored value must still be the one committed by the successful write.
    assert_eq!(read_grace(&env, &contract_id), Ok(12_345));
}

// ── Test 14: setter does not disturb other admin config keys ──────────────────

/// `do_set_grace_period` must only mutate `DataKey::GracePeriod`; other
/// config keys (min-topup, fee bps) must be unchanged.
#[test]
fn do_set_grace_period_does_not_disturb_other_config_keys() {
    let (env, _client, admin, contract_id) = setup();

    let min_topup_before = env
        .as_contract(&contract_id, || get_min_topup(&env))
        .unwrap();
    let fee_bps_before = env.as_contract(&contract_id, || get_protocol_fee_bps(&env));

    env.ledger()
        .with_mut(|l| l.timestamp = T0 + CONFIG_COOLDOWN_SECS + 1);
    set_grace(&env, &contract_id, &admin, 42_000).expect("set_grace_period must succeed");

    let min_topup_after = env
        .as_contract(&contract_id, || get_min_topup(&env))
        .unwrap();
    let fee_bps_after = env.as_contract(&contract_id, || get_protocol_fee_bps(&env));

    assert_eq!(min_topup_before, min_topup_after);
    assert_eq!(fee_bps_before, fee_bps_after);
}

// ── Test 15: read_cached_admin_config uses get_grace_period ──────────────────

/// `read_cached_admin_config` calls `get_grace_period` internally and stores
/// the result in `CachedAdminConfig::grace_duration`. A change to the stored
/// grace period must be reflected in a subsequent call to
/// `read_cached_admin_config`.
#[test]
fn read_cached_admin_config_reflects_current_grace_period() {
    let (env, _client, admin, contract_id) = setup();

    let cached_before = env
        .as_contract(&contract_id, || read_cached_admin_config(&env))
        .expect("read_cached_admin_config must succeed after init");
    assert_eq!(cached_before.grace_duration, SEVEN_DAYS);

    // Overwrite the grace period and read the cache again.
    env.ledger()
        .with_mut(|l| l.timestamp = T0 + CONFIG_COOLDOWN_SECS + 1);
    set_grace(&env, &contract_id, &admin, 3_600).expect("set_grace_period must succeed");

    let cached_after = env
        .as_contract(&contract_id, || read_cached_admin_config(&env))
        .expect("read_cached_admin_config must succeed after update");
    assert_eq!(cached_after.grace_duration, 3_600);
}

// ── Test 16: key lives in instance storage, not persistent ───────────────────

/// `do_init` and `do_set_grace_period` both write to **instance** storage.
/// The persistent tier must have no entry for `DataKey::GracePeriod` after a
/// normal init, and `get_grace_period` must still return the correct value.
#[test]
fn get_grace_period_key_lives_in_instance_not_persistent_storage() {
    let (env, _client, _admin, contract_id) = setup();

    env.as_contract(&contract_id, || {
        // Persistent tier must be empty for GracePeriod.
        assert!(
            !env.storage().persistent().has(&DataKey::GracePeriod),
            "GracePeriod must NOT be in persistent storage"
        );
        // Instance tier must hold the value.
        assert!(
            env.storage().instance().has(&DataKey::GracePeriod),
            "GracePeriod must be in instance storage"
        );
    });

    assert_eq!(read_grace(&env, &contract_id), Ok(SEVEN_DAYS));
}

// ── Test 17: persistent storage for GracePeriod is NOT read by getter ─────────

/// Even if something writes `DataKey::GracePeriod` to the persistent tier
/// (which should not happen in normal operation), the getter reads from
/// instance storage only and must NOT be affected.
#[test]
fn get_grace_period_ignores_persistent_storage_value() {
    let (env, _client, _admin, contract_id) = setup();

    // Manually write a different value to persistent storage.
    env.as_contract(&contract_id, || {
        env.storage()
            .persistent()
            .set(&DataKey::GracePeriod, &999_999_u64);
    });

    // The getter reads instance storage exclusively; it must still return
    // the instance value (SEVEN_DAYS), not the persistent one.
    assert_eq!(read_grace(&env, &contract_id), Ok(SEVEN_DAYS));
}

// ── Test 18: uninitialised vault — Ok(0) without panic ───────────────────────

/// Even on a contract that has never been initialised, `get_grace_period`
/// must return `Ok(0)` without panicking.  This is critical for callers
/// that invoke the getter before checking initialization status.
#[test]
fn get_grace_period_never_errors_on_uninitialised_vault() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());

    // No init, no writes at all.
    let result = env.as_contract(&contract_id, || get_grace_period(&env));
    // Must be Ok — the Result<u64, Error> return type must never carry Err.
    assert!(
        result.is_ok(),
        "get_grace_period must always return Ok, got: {:?}",
        result
    );
    assert_eq!(result, Ok(0));
}
