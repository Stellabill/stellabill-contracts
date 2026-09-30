//! Adversarial coverage for `do_migrate_config_to_persistent_internal` in
//! `admin.rs` (issue #1023).
//!
//! This internal helper is the v2 -> v3 step of the config migration ladder.
//! It moves a fixed set of configuration keys out of *instance* storage and
//! into *persistent* storage, extending their TTL, then removes the instance
//! copies. It is deliberately **not** an authorisation boundary of its own:
//! its callers (`migrate_config_to_persistent`) apply `require_admin_auth`
//! before invoking it, so the helper itself must be safe to call repeatedly
//! and on partially-migrated state.
//!
//! The issue evidence points at `let val: Address = instance.get(&DataKey::Token).unwrap();`.
//! Every read is guarded by a preceding `has(...)` check, so a *missing* key
//! never reaches the `unwrap`. The cases below pin that guard explicitly, pin
//! the exact key set and value fidelity, and document the (intentional,
//! deterministic) panic when a present key holds a value of the wrong type.
//!
//! Cases covered:
//!   * empty storage -> Ok, and SchemaVersion is still written as 3;
//!   * every one of the nine keys migrated individually and together;
//!   * value fidelity for all types, including u32/i128/bool boundaries;
//!   * `has`-guard: only keys present in instance are touched;
//!   * idempotency: a second run is a safe no-op;
//!   * crash recovery: instance wins over a stale persistent copy;
//!   * persistent-only keys are left untouched;
//!   * no events are emitted (the caller owns the `schema_migrated` event);
//!   * the helper requires no auth when invoked directly; and
//!   * the public wrapper still rejects non-admins and leaves state untouched.

use crate::{
    types::{DataKey, Error},
    SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Events},
    Address, Env, IntoVal, TryFromVal, Val,
};

// ── Harness ──────────────────────────────────────────────────────────────────

/// Register a bare vault with no initialisation, so storage starts empty and
/// the migration behaves like a genuine v2 upgrade rather than a fresh init.
fn setup() -> (Env, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    (env, contract_id)
}

/// Invoke the internal helper exactly as its production callers do.
fn run(env: &Env, contract_id: &Address) -> Result<(), Error> {
    env.as_contract(contract_id, || {
        crate::admin::do_migrate_config_to_persistent_internal(env)
    })
}

fn iset<T>(env: &Env, contract_id: &Address, key: &DataKey, value: &T)
where
    T: IntoVal<Env, Val>,
{
    env.as_contract(contract_id, || env.storage().instance().set(key, value));
}

fn pget<T>(env: &Env, contract_id: &Address, key: &DataKey) -> Option<T>
where
    T: TryFromVal<Env, Val>,
{
    env.as_contract(contract_id, || env.storage().persistent().get(key))
}

fn ihave(env: &Env, contract_id: &Address, key: &DataKey) -> bool {
    env.as_contract(contract_id, || env.storage().instance().has(key))
}

fn phave(env: &Env, contract_id: &Address, key: &DataKey) -> bool {
    env.as_contract(contract_id, || env.storage().persistent().has(key))
}

/// The nine config keys the v2 -> v3 migration is responsible for.
const MIGRATED_KEYS: [DataKey; 9] = [
    DataKey::Token,
    DataKey::Admin,
    DataKey::MinTopup,
    DataKey::NextId,
    DataKey::EmergencyStop,
    DataKey::Treasury,
    DataKey::FeeBps,
    DataKey::Operator,
    DataKey::SchemaVersion,
];

// ── Empty / fresh state ──────────────────────────────────────────────────────

/// With nothing in instance storage the helper is a no-op except for forcing
/// `SchemaVersion` to 3 via its unconditional else-branch.
#[test]
fn empty_storage_succeeds_and_writes_schema_version_three() {
    let (env, cid) = setup();

    assert_eq!(run(&env, &cid), Ok(()));

    assert_eq!(
        pget::<u32>(&env, &cid, &DataKey::SchemaVersion),
        Some(3u32),
        "SchemaVersion must be stamped as 3 even when absent from instance"
    );
    assert!(!ihave(&env, &cid, &DataKey::SchemaVersion));

    // No other config key may be invented out of thin air.
    for key in [
        DataKey::Token,
        DataKey::Admin,
        DataKey::MinTopup,
        DataKey::NextId,
        DataKey::EmergencyStop,
        DataKey::Treasury,
        DataKey::FeeBps,
        DataKey::Operator,
    ] {
        assert!(
            !phave(&env, &cid, &key),
            "absent key with discriminant {} must not be written to persistent storage",
            key.canonical_discriminant()
        );
    }
}

// ── Full migration ───────────────────────────────────────────────────────────

/// All nine keys move across with values preserved byte-for-byte.
#[test]
fn migrates_all_keys_and_removes_instance_copies() {
    let (env, cid) = setup();
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let treasury = Address::generate(&env);
    let operator = Address::generate(&env);

    iset(&env, &cid, &DataKey::Token, &token);
    iset(&env, &cid, &DataKey::Admin, &admin);
    iset(&env, &cid, &DataKey::MinTopup, &1_000_000i128);
    iset(&env, &cid, &DataKey::NextId, &42u32);
    iset(&env, &cid, &DataKey::EmergencyStop, &true);
    iset(&env, &cid, &DataKey::Treasury, &treasury);
    iset(&env, &cid, &DataKey::FeeBps, &250u32);
    iset(&env, &cid, &DataKey::Operator, &operator);
    iset(&env, &cid, &DataKey::SchemaVersion, &2u32);

    assert_eq!(run(&env, &cid), Ok(()));

    assert_eq!(pget::<Address>(&env, &cid, &DataKey::Token), Some(token));
    assert_eq!(pget::<Address>(&env, &cid, &DataKey::Admin), Some(admin));
    assert_eq!(
        pget::<i128>(&env, &cid, &DataKey::MinTopup),
        Some(1_000_000i128)
    );
    assert_eq!(pget::<u32>(&env, &cid, &DataKey::NextId), Some(42u32));
    assert_eq!(
        pget::<bool>(&env, &cid, &DataKey::EmergencyStop),
        Some(true)
    );
    assert_eq!(
        pget::<Address>(&env, &cid, &DataKey::Treasury),
        Some(treasury)
    );
    assert_eq!(pget::<u32>(&env, &cid, &DataKey::FeeBps), Some(250u32));
    assert_eq!(
        pget::<Address>(&env, &cid, &DataKey::Operator),
        Some(operator)
    );
    assert_eq!(pget::<u32>(&env, &cid, &DataKey::SchemaVersion), Some(3u32));

    for key in MIGRATED_KEYS {
        assert!(
            !ihave(&env, &cid, &key),
            "instance copy of discriminant {} must be removed after migration",
            key.canonical_discriminant()
        );
    }
}

/// Each key migrates correctly even when it is the only one present.
///
/// Declared at module scope so the generated `#[test]` functions are picked
/// up by the harness.
macro_rules! single_key_migration_case {
    ($name:ident, $key:expr, $ty:ty, $val:expr) => {
        #[test]
        fn $name() {
            let (env, cid) = setup();
            let value: $ty = $val;
            iset(&env, &cid, &$key, &value);
            assert_eq!(run(&env, &cid), Ok(()));
            assert_eq!(pget::<$ty>(&env, &cid, &$key), Some(value));
            assert!(!ihave(&env, &cid, &$key));
        }
    };
}

single_key_migration_case!(
    token_alone_migrates,
    DataKey::Token,
    Address,
    Address::generate(&Env::default())
);
single_key_migration_case!(
    admin_alone_migrates,
    DataKey::Admin,
    Address,
    Address::generate(&Env::default())
);
single_key_migration_case!(min_topup_alone_migrates, DataKey::MinTopup, i128, 7i128);
single_key_migration_case!(next_id_alone_migrates, DataKey::NextId, u32, 9u32);
single_key_migration_case!(
    emergency_stop_alone_migrates,
    DataKey::EmergencyStop,
    bool,
    true
);
single_key_migration_case!(
    treasury_alone_migrates,
    DataKey::Treasury,
    Address,
    Address::generate(&Env::default())
);
single_key_migration_case!(
    operator_alone_migrates,
    DataKey::Operator,
    Address,
    Address::generate(&Env::default())
);
single_key_migration_case!(fee_bps_alone_migrates, DataKey::FeeBps, u32, 5u32);
/// Unlike every other key, `SchemaVersion` is **not** copied verbatim: the
/// migration always stamps the target version (3). A stale instance value
/// (e.g. 2) must be normalised, never carried over.
#[test]
fn schema_version_is_normalized_to_three_not_preserved() {
    let (env, cid) = setup();
    iset(&env, &cid, &DataKey::SchemaVersion, &2u32);

    assert_eq!(run(&env, &cid), Ok(()));

    assert_eq!(
        pget::<u32>(&env, &cid, &DataKey::SchemaVersion),
        Some(3u32),
        "a stale version must be replaced by the migration target"
    );
    assert!(!ihave(&env, &cid, &DataKey::SchemaVersion));
}

// ── Value fidelity / boundaries ──────────────────────────────────────────────

/// `NextId` at `u32::MAX` survives without truncation or overflow.
#[test]
fn next_id_u32_max_round_trips() {
    let (env, cid) = setup();
    iset(&env, &cid, &DataKey::NextId, &u32::MAX);

    assert_eq!(run(&env, &cid), Ok(()));

    assert_eq!(pget::<u32>(&env, &cid, &DataKey::NextId), Some(u32::MAX));
    assert!(!ihave(&env, &cid, &DataKey::NextId));
}

/// `NextId == 0` is a *present* value and must be migrated, not mistaken for
/// an absent key.
#[test]
fn next_id_zero_is_migrated_not_treated_as_absent() {
    let (env, cid) = setup();
    iset(&env, &cid, &DataKey::NextId, &0u32);

    assert_eq!(run(&env, &cid), Ok(()));

    assert_eq!(pget::<u32>(&env, &cid, &DataKey::NextId), Some(0u32));
    assert!(
        phave(&env, &cid, &DataKey::NextId),
        "a zero value must still be written to persistent storage"
    );
    assert!(!ihave(&env, &cid, &DataKey::NextId));
}

/// `MinTopup` boundaries: `0`, `i128::MAX` and a negative offset.
#[test]
fn min_topup_boundaries_round_trip() {
    for value in [0i128, i128::MAX, -1i128] {
        let (env, cid) = setup();
        iset(&env, &cid, &DataKey::MinTopup, &value);

        assert_eq!(run(&env, &cid), Ok(()));
        assert_eq!(pget::<i128>(&env, &cid, &DataKey::MinTopup), Some(value));
        assert!(!ihave(&env, &cid, &DataKey::MinTopup));
    }
}

/// `FeeBps` at `u32::MAX` round-trips.
#[test]
fn fee_bps_u32_max_round_trips() {
    let (env, cid) = setup();
    iset(&env, &cid, &DataKey::FeeBps, &u32::MAX);

    assert_eq!(run(&env, &cid), Ok(()));

    assert_eq!(pget::<u32>(&env, &cid, &DataKey::FeeBps), Some(u32::MAX));
}

/// `EmergencyStop` is migrated for both boolean values.
#[test]
fn emergency_stop_false_is_migrated() {
    let (env, cid) = setup();
    iset(&env, &cid, &DataKey::EmergencyStop, &false);

    assert_eq!(run(&env, &cid), Ok(()));

    assert_eq!(
        pget::<bool>(&env, &cid, &DataKey::EmergencyStop),
        Some(false),
        "a `false` flag must be preserved, not dropped as falsy"
    );
    assert!(!ihave(&env, &cid, &DataKey::EmergencyStop));
}

/// `Treasury` and `Operator` addresses are preserved exactly.
#[test]
fn treasury_and_operator_addresses_preserved() {
    let (env, cid) = setup();
    let treasury = Address::generate(&env);
    let operator = Address::generate(&env);
    iset(&env, &cid, &DataKey::Treasury, &treasury);
    iset(&env, &cid, &DataKey::Operator, &operator);

    assert_eq!(run(&env, &cid), Ok(()));

    assert_eq!(
        pget::<Address>(&env, &cid, &DataKey::Treasury),
        Some(treasury)
    );
    assert_eq!(
        pget::<Address>(&env, &cid, &DataKey::Operator),
        Some(operator)
    );
}

/// A `SchemaVersion` already at 3 in instance storage is still normalised to a
/// persistent 3 and removed from instance.
#[test]
fn schema_version_already_three_is_relocated() {
    let (env, cid) = setup();
    iset(&env, &cid, &DataKey::SchemaVersion, &3u32);

    assert_eq!(run(&env, &cid), Ok(()));

    assert_eq!(pget::<u32>(&env, &cid, &DataKey::SchemaVersion), Some(3u32));
    assert!(!ihave(&env, &cid, &DataKey::SchemaVersion));
}

// ── Idempotency / crash recovery ─────────────────────────────────────────────

/// Running the migration twice is a safe no-op; the second run must not panic
/// and must leave the first run's result exactly as it was.
#[test]
fn second_run_is_a_safe_noop() {
    let (env, cid) = setup();
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    iset(&env, &cid, &DataKey::Token, &token);
    iset(&env, &cid, &DataKey::Admin, &admin);
    iset(&env, &cid, &DataKey::NextId, &5u32);

    assert_eq!(run(&env, &cid), Ok(()));

    let token_after_first = pget::<Address>(&env, &cid, &DataKey::Token);
    let admin_after_first = pget::<Address>(&env, &cid, &DataKey::Admin);
    let next_after_first = pget::<u32>(&env, &cid, &DataKey::NextId);

    assert_eq!(run(&env, &cid), Ok(()));

    assert_eq!(
        pget::<Address>(&env, &cid, &DataKey::Token),
        token_after_first
    );
    assert_eq!(
        pget::<Address>(&env, &cid, &DataKey::Admin),
        admin_after_first
    );
    assert_eq!(pget::<u32>(&env, &cid, &DataKey::NextId), next_after_first);
    assert_eq!(pget::<u32>(&env, &cid, &DataKey::SchemaVersion), Some(3u32));
    for key in MIGRATED_KEYS {
        assert!(!ihave(&env, &cid, &key));
    }
}

/// Simulates a crash after `Token` was already relocated: the helper must
/// finish the remaining keys without touching the already-migrated one.
#[test]
fn resumes_from_partial_crash_state() {
    let (env, cid) = setup();
    let token = Address::generate(&env);
    let admin = Address::generate(&env);

    // Token already in persistent; Admin/MinTopup still in instance.
    iset(&env, &cid, &DataKey::Token, &token);
    run(&env, &cid).unwrap(); // Token now persistent
    iset(&env, &cid, &DataKey::Admin, &admin);
    iset(&env, &cid, &DataKey::MinTopup, &123i128);

    assert_eq!(run(&env, &cid), Ok(()));

    assert_eq!(pget::<Address>(&env, &cid, &DataKey::Token), Some(token));
    assert_eq!(pget::<Address>(&env, &cid, &DataKey::Admin), Some(admin));
    assert_eq!(pget::<i128>(&env, &cid, &DataKey::MinTopup), Some(123i128));
    assert!(!ihave(&env, &cid, &DataKey::Token));
    assert!(!ihave(&env, &cid, &DataKey::Admin));
    assert!(!ihave(&env, &cid, &DataKey::MinTopup));
}

/// When a key exists in *both* stores, the instance value is the source of
/// truth and overwrites the stale persistent copy (instance is the pre-v3
/// authoritative location).
#[test]
fn instance_value_overwrites_stale_persistent_value() {
    let (env, cid) = setup();
    let stale = Address::generate(&env);
    let fresh = Address::generate(&env);

    // Stale persistent copy plus authoritative instance copy.
    env.as_contract(&cid, || {
        env.storage().persistent().set(&DataKey::Token, &stale);
    });
    iset(&env, &cid, &DataKey::Token, &fresh);

    assert_eq!(run(&env, &cid), Ok(()));

    assert_eq!(
        pget::<Address>(&env, &cid, &DataKey::Token),
        Some(fresh),
        "instance value must win over a stale persistent copy"
    );
    assert!(!ihave(&env, &cid, &DataKey::Token));
}

/// Keys that exist only in persistent storage (already migrated by an earlier
/// run) must be left untouched — the helper must not clear or default them.
#[test]
fn persistent_only_keys_are_left_untouched() {
    let (env, cid) = setup();
    let min_topup = 999i128;

    env.as_contract(&cid, || {
        env.storage()
            .persistent()
            .set(&DataKey::MinTopup, &min_topup);
    });

    assert_eq!(run(&env, &cid), Ok(()));

    assert_eq!(
        pget::<i128>(&env, &cid, &DataKey::MinTopup),
        Some(min_topup)
    );
    assert!(!ihave(&env, &cid, &DataKey::MinTopup));
}

// ── Observability / auth ─────────────────────────────────────────────────────

/// The helper itself emits no events; the `schema_migrated` event is the
/// caller's responsibility and must be emitted exactly once by the wrapper.
#[test]
fn internal_helper_emits_no_events() {
    let (env, cid) = setup();
    iset(&env, &cid, &DataKey::Admin, &Address::generate(&env));

    let before = env.events().all().len();
    assert_eq!(run(&env, &cid), Ok(()));
    let after = env.events().all().len();

    assert_eq!(before, after, "internal migration must not emit events");
}

/// Called directly (as the authorized wrapper does) the helper performs no
/// auth check of its own and must succeed with no auth entries present.
#[test]
fn internal_helper_requires_no_auth() {
    let (env, cid) = setup();
    iset(&env, &cid, &DataKey::Token, &Address::generate(&env));

    env.set_auths(&[]);
    assert_eq!(
        run(&env, &cid),
        Ok(()),
        "auth is enforced by callers, not by the internal helper"
    );
}

/// A present key whose stored value has the wrong type panics deterministically
/// at the `.unwrap()` noted in the issue evidence. This pins the current
/// behaviour (see follow-up: replace the untyped `.unwrap()`s with typed
/// errors so corrupted storage can be recovered instead of trapping).
#[test]
#[should_panic]
fn wrong_type_value_panics_at_unwrap() {
    let (env, cid) = setup();
    // Token is read as an `Address`; store a u32 instead.
    iset(&env, &cid, &DataKey::Token, &7u32);

    let _ = run(&env, &cid);
}

// ── Public wrapper boundary ──────────────────────────────────────────────────

/// The only supported way to invoke the migration is via
/// `migrate_config_to_persistent`, which must reject a non-admin *before* any
/// storage is touched, then succeed for the real admin.
#[test]
fn public_wrapper_rejects_non_admin_and_migrates_for_admin() {
    let (env, cid) = setup();
    let client = SubscriptionVaultClient::new(&env, &cid);
    let admin = Address::generate(&env);
    let outsider = Address::generate(&env);
    let token = Address::generate(&env);

    // v2 layout: admin + schema version persisted, token still in instance.
    env.as_contract(&cid, || {
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage()
            .persistent()
            .set(&DataKey::SchemaVersion, &2u32);
    });
    iset(&env, &cid, &DataKey::Token, &token);

    // Non-admin: rejected, and nothing is moved.
    assert_eq!(
        client.try_migrate_config_to_persistent(&outsider),
        Err(Ok(Error::Forbidden))
    );
    assert!(
        ihave(&env, &cid, &DataKey::Token),
        "token must still be in instance"
    );
    assert!(
        !phave(&env, &cid, &DataKey::Token),
        "token must not be migrated"
    );

    // Real admin: migration proceeds and the token lands in persistent storage.
    client.migrate_config_to_persistent(&admin);
    assert_eq!(pget::<Address>(&env, &cid, &DataKey::Token), Some(token));
    assert!(!ihave(&env, &cid, &DataKey::Token));
    assert_eq!(pget::<u32>(&env, &cid, &DataKey::SchemaVersion), Some(3u32));
}
