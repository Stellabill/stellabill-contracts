//! Adversarial coverage for [`crate::admin::do_migrate`] (issue #1025).
//!
//! `do_migrate` is the storage-schema migration engine behind the `migrate`
//! entrypoint. Its contract surface:
//!
//! - **Authorization** — the caller must pass `require_admin_auth` (auth check
//!   first, then address equality against the stored admin).
//! - **Version guard** — a stored version greater than the requested
//!   `binary_version` (a downgrade) is rejected with
//!   [`Error::SchemaVersionMismatch`], and no storage write may occur.
//! - **No-op boundary** — `stored == binary_version` returns `Ok` without
//!   emitting a `schema_migrated` event.
//! - **Upgrade ladder** — versions below 2 jump straight to 2, then hop
//!   `2→3` (config to persistent), `3→4`, `4→5`, `5→6` (subscription record
//!   rewrites), finally persisting the new version and emitting
//!   [`crate::SchemaMigratedEvent`].
//!
//! Every rejected operation is checked for **state invariance**: the stored
//! schema version, admin, token, and any subscription records must be
//! byte-for-byte unchanged afterwards, and no event may be emitted.

#![cfg(test)]

extern crate std;

use crate::types::{DataKey, Error, SchemaMigratedEvent, SUB_TTL_EXTEND_TO};
use crate::{SubscriptionVault, SubscriptionVaultClient, STORAGE_VERSION};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    Address, Env, FromVal, Symbol,
};

/// Read the schema version exactly as `admin::get_schema_version` does
/// (persistent tier first, then instance tier, defaulting to 0).
fn read_schema_version(env: &Env, contract_id: &Address) -> u32 {
    env.as_contract(contract_id, || crate::admin::get_schema_version(env))
}

/// Forcibly write the schema version to the authoritative persistent tier
/// (the tier `do_init` writes and `get_schema_version` reads first).
fn write_schema_version(env: &Env, contract_id: &Address, version: u32) {
    env.as_contract(contract_id, || {
        env.storage()
            .persistent()
            .set(&DataKey::SchemaVersion, &version);
    });
}

/// Snapshot of the persistent-tier keys a rejected migration must never touch.
struct StorageSnapshot {
    schema_version: Option<u32>,
    admin: Option<Address>,
    token: Option<Address>,
    min_topup: Option<i128>,
}

fn snapshot(env: &Env, contract_id: &Address) -> StorageSnapshot {
    env.as_contract(contract_id, || {
        let storage = env.storage();
        StorageSnapshot {
            schema_version: storage.persistent().get(&DataKey::SchemaVersion),
            admin: storage.persistent().get(&DataKey::Admin),
            token: storage.persistent().get(&DataKey::Token),
            min_topup: storage.persistent().get(&DataKey::MinTopup),
        }
    })
}

fn assert_snapshot_unchanged(before: &StorageSnapshot, after: &StorageSnapshot) {
    assert_eq!(
        before.schema_version, after.schema_version,
        "SchemaVersion must be unchanged after a rejected migration"
    );
    assert_eq!(
        before.admin, after.admin,
        "Admin must be unchanged after a rejected migration"
    );
    assert_eq!(
        before.token, after.token,
        "Token must be unchanged after a rejected migration"
    );
    assert_eq!(
        before.min_topup, after.min_topup,
        "MinTopup must be unchanged after a rejected migration"
    );
}

/// `true` if any recorded event carries `symbol` as its first topic.
fn has_event_with_symbol(
    env: &Env,
    events: &soroban_sdk::Vec<(Address, soroban_sdk::Vec< soroban_sdk::Val>, soroban_sdk::Val)>,
    sym_name: &str,
) -> bool {
    let target = Symbol::new(env, sym_name);
    for (_, topics, _) in events.iter() {
        if let Some(first) = topics.get(0) {
            if Symbol::from_val(env, &first) == target {
                return true;
            }
        }
    }
    false
}

/// Decode the single `schema_migrated` event payload, asserting exactly one fired.
fn expect_single_migrate_event(
    env: &Env,
) -> SchemaMigratedEvent {
    let events = env.events().all();
    let mut found: Option<SchemaMigratedEvent> = None;
    let mut count = 0u32;
    for (_, topics, data) in events.iter() {
        if let Some(first) = topics.get(0) {
            if Symbol::from_val(env, &first) == Symbol::new(env, "schema_migrated") {
                count += 1;
                found = Some(FromVal::from_val(env, &data));
            }
        }
    }
    assert_eq!(count, 1, "exactly one schema_migrated event must be emitted");
    found.unwrap()
}

/// Standard fixture: mocked auth, registered contract, `init`-ed with a real
/// token, 1 USDC min top-up, 7-day grace.
///
/// The ledger's `max_entry_ttl` is raised above `SUB_TTL_EXTEND_TO` so the
/// `extend_ttl` calls inside `do_migrate` (and `write_config`) are never
/// clamped/aborted — mirroring the fixture used by `test_ttl_billing_statements`.
fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.ledger().with_mut(|li| {
        li.sequence_number = 100;
        li.min_persistent_entry_ttl = 4096;
        li.min_temp_entry_ttl = 4096;
        li.max_entry_ttl = SUB_TTL_EXTEND_TO + 5_000_000;
    });
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

// ── Valid calls ───────────────────────────────────────────────────────────────

/// Migrating from `STORAGE_VERSION - 1` to `STORAGE_VERSION` (the exact hop the
/// `migrate` entrypoint requests) succeeds and lands on `STORAGE_VERSION`.
#[test]
fn do_migrate_success_from_current_minus_one() {
    let (env, client, _token, admin) = setup();
    let contract_id = client.address.clone();

    write_schema_version(&env, &contract_id, STORAGE_VERSION - 1);
    let result = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), STORAGE_VERSION)
    });

    assert_eq!(result, Ok(()), "one-hop migration must succeed");

    // Events must be captured immediately: `env.events().all()` only reflects
    // the most recent invocation, so any later read would clear the buffer.
    let evt = expect_single_migrate_event(&env);
    assert_eq!(evt.from_version, STORAGE_VERSION - 1);
    assert_eq!(evt.to_version, STORAGE_VERSION);
    assert_eq!(evt.admin, admin);
    assert_eq!(evt.timestamp, env.ledger().timestamp());

    assert_eq!(read_schema_version(&env, &contract_id), STORAGE_VERSION);
}

/// A multi-hop ladder `0 → STORAGE_VERSION` runs every intermediate step and
/// lands exactly on `STORAGE_VERSION` (deterministic final state).
#[test]
fn do_migrate_full_ladder_from_zero() {
    let (env, client, _token, admin) = setup();
    let contract_id = client.address.clone();

    write_schema_version(&env, &contract_id, 0);
    let result = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), STORAGE_VERSION)
    });

    assert_eq!(result, Ok(()));
    expect_single_migrate_event(&env);
    assert_eq!(read_schema_version(&env, &contract_id), STORAGE_VERSION);
}

/// Every intermediate version boundary from `1` upward migrates cleanly to
/// `STORAGE_VERSION` — deterministically exercising each ladder rung.
#[test]
fn do_migrate_from_every_intermediate_version() {
    for from in 1..STORAGE_VERSION {
        let (env, client, _token, admin) = setup();
        let contract_id = client.address.clone();

        write_schema_version(&env, &contract_id, from);
        let result = env.as_contract(&contract_id, || {
            crate::admin::do_migrate(&env, admin.clone(), STORAGE_VERSION)
        });

        assert_eq!(result, Ok(()), "migration from v{from} must succeed");
        assert_eq!(
            read_schema_version(&env, &contract_id),
            STORAGE_VERSION,
            "migration from v{from} must land on STORAGE_VERSION"
        );
    }
}

/// Config promotion is part of the ladder: after a migration that crosses v3,
/// admin config lives in persistent storage and the instance copies are gone.
#[test]
fn do_migrate_promotes_config_to_persistent_tier() {
    let (env, _client, token, admin) = setup();

    // Simulate a pre-v3 deployment: register the contract WITHOUT calling
    // `init` (init would persist SchemaVersion 6), and place all config in
    // instance storage exactly as a v2 deployment would have it.
    let contract_id = env.register(SubscriptionVault, ());
    let _client = SubscriptionVaultClient::new(&env, &contract_id);
    env.mock_all_auths();
    env.as_contract(&contract_id, || {
        let storage = env.storage();
        storage.instance().set(&DataKey::SchemaVersion, &2u32);
        storage.instance().set(&DataKey::Token, &token);
        storage.instance().set(&DataKey::Admin, &admin.clone());
        storage.instance().set(&DataKey::MinTopup, &1_000_000i128);
    });

    let result = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), STORAGE_VERSION)
    });
    assert_eq!(result, Ok(()));

    env.as_contract(&contract_id, || {
        let storage = env.storage();
        assert_eq!(
            storage.persistent().get::<_, u32>(&DataKey::SchemaVersion),
            Some(STORAGE_VERSION)
        );
        assert_eq!(storage.persistent().get::<_, Address>(&DataKey::Token), Some(token));
        assert_eq!(storage.persistent().get::<_, Address>(&DataKey::Admin), Some(admin));
        assert_eq!(
            storage.persistent().get::<_, i128>(&DataKey::MinTopup),
            Some(1_000_000i128)
        );
        // Instance copies must be gone.
        assert!(!storage.instance().has(&DataKey::SchemaVersion));
        assert!(!storage.instance().has(&DataKey::Token));
        assert!(!storage.instance().has(&DataKey::Admin));
        assert!(!storage.instance().has(&DataKey::MinTopup));
    });
}

/// Migration is idempotent: a second identical call is a silent `Ok` no-op
/// (no event, no version change).
#[test]
fn do_migrate_is_idempotent() {
    let (env, client, _token, admin) = setup();
    let contract_id = client.address.clone();

    write_schema_version(&env, &contract_id, 0);
    let first = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), STORAGE_VERSION)
    });
    assert_eq!(first, Ok(()));
    assert_eq!(read_schema_version(&env, &contract_id), STORAGE_VERSION);

    let second = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), STORAGE_VERSION)
    });
    assert_eq!(second, Ok(()), "repeat migration must be a no-op success");
    assert_eq!(read_schema_version(&env, &contract_id), STORAGE_VERSION);
    assert!(
        !has_event_with_symbol(&env, &env.events().all(), "schema_migrated"),
        "no-op migration must not emit schema_migrated"
    );
}

/// A no-op (`stored == binary_version`) returns `Ok` and emits nothing.
#[test]
fn do_migrate_same_version_is_silent_noop() {
    let (env, client, _token, admin) = setup();
    let contract_id = client.address.clone();

    let before = snapshot(&env, &contract_id);
    let result = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), STORAGE_VERSION)
    });

    assert_eq!(result, Ok(()));
    let after = snapshot(&env, &contract_id);
    assert_snapshot_unchanged(&before, &after);
    assert!(!has_event_with_symbol(&env, &env.events().all(), "schema_migrated"));
}

// ── Boundary values for `binary_version` ─────────────────────────────────────

/// `binary_version = 0` on a fresh contract (stored 0) is a valid no-op.
#[test]
fn do_migrate_boundary_zero_version_noop() {
    let (env, _client, _token, admin) = setup();
    let contract_id = _client.address.clone();

    // Force stored version to 0 so binary_version == 0 is a no-op, not a downgrade.
    write_schema_version(&env, &contract_id, 0);

    let before = snapshot(&env, &contract_id);
    let result = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), 0u32)
    });

    assert_eq!(result, Ok(()), "0 → 0 is a legitimate no-op");
    let after = snapshot(&env, &contract_id);
    assert_snapshot_unchanged(&before, &after);
}

/// `binary_version = u32::MAX` is an accepted upper boundary: the ladder must
/// terminate deterministically on `u32::MAX` rather than overflow-loop.
#[test]
fn do_migrate_boundary_u32_max_succeeds_deterministically() {
    let (env, client, _token, admin) = setup();
    let contract_id = client.address.clone();

    write_schema_version(&env, &contract_id, 0);
    let result = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), u32::MAX)
    });

    assert_eq!(result, Ok(()), "u32::MAX target must terminate successfully");
    let evt = expect_single_migrate_event(&env);
    assert_eq!(evt.to_version, u32::MAX);
    assert_eq!(read_schema_version(&env, &contract_id), u32::MAX);
}

/// A version-2 contract asked to stay on 2 is the exact `stored == binary`
/// boundary of the ladder's first rung.
#[test]
fn do_migrate_boundary_at_ladder_rung_two() {
    let (env, client, _token, admin) = setup();
    let contract_id = client.address.clone();

    write_schema_version(&env, &contract_id, 2);
    let result = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), 2u32)
    });

    assert_eq!(result, Ok(()));
    assert_eq!(read_schema_version(&env, &contract_id), 2);
    assert!(!has_event_with_symbol(&env, &env.events().all(), "schema_migrated"));
}

// ── Unauthorized callers ──────────────────────────────────────────────────────

/// A non-admin address (auth mocked) is rejected with `Forbidden` by the
/// stored-admin comparison inside `require_admin_auth`.
#[test]
fn do_migrate_rejects_wrong_caller_address() {
    let (env, client, _token, _admin) = setup();
    let contract_id = client.address.clone();
    let stranger = Address::generate(&env);

    let before = snapshot(&env, &contract_id);
    let result = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, stranger.clone(), STORAGE_VERSION)
    });

    assert_eq!(result, Err(Error::Forbidden));
    let after = snapshot(&env, &contract_id);
    assert_snapshot_unchanged(&before, &after);
    assert!(
        !has_event_with_symbol(&env, &env.events().all(), "schema_migrated"),
        "rejected migration must not emit schema_migrated"
    );
}

/// Missing authorization (no `mock_all_auths`, no auth set) aborts at the
/// `require_auth` stage — before any storage read or write.
#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn do_migrate_without_auth_panics_before_storage_access() {
    let env = Env::default();
    // Deliberately no mock_all_auths and no set_auths.
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    // This call must panic at admin.require_auth().
    let _ = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), STORAGE_VERSION)
    });
}

/// Authorization is checked **before** the version guard: an unauthorized
/// downgrade attempt fails with `Forbidden`, not `SchemaVersionMismatch`.
#[test]
fn do_migrate_auth_precedes_version_guard() {
    let (env, client, _token, _admin) = setup();
    let contract_id = client.address.clone();
    let stranger = Address::generate(&env);

    write_schema_version(&env, &contract_id, 99);

    let result = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, stranger.clone(), 1u32)
    });

    assert_eq!(
        result,
        Err(Error::Forbidden),
        "auth failure must mask the version mismatch"
    );
    assert_eq!(
        read_schema_version(&env, &contract_id),
        99,
        "stored version must be untouched"
    );
}

// ── Downgrades and failure paths ──────────────────────────────────────────────

/// A stored version above `binary_version` (downgrade) is rejected with
/// `SchemaVersionMismatch` and leaves every storage key untouched.
#[test]
fn do_migrate_downgrade_rejected_and_state_invariant() {
    let (env, client, _token, admin) = setup();
    let contract_id = client.address.clone();

    write_schema_version(&env, &contract_id, STORAGE_VERSION + 3);
    let before = snapshot(&env, &contract_id);

    let result = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), STORAGE_VERSION)
    });

    assert_eq!(result, Err(Error::SchemaVersionMismatch));
    let after = snapshot(&env, &contract_id);
    assert_snapshot_unchanged(&before, &after);
    assert!(!has_event_with_symbol(&env, &env.events().all(), "schema_migrated"));
}

/// The extreme adversarial downgrade: stored `u32::MAX`, requested `0`.
#[test]
fn do_migrate_extreme_downgrade_rejected() {
    let (env, client, _token, admin) = setup();
    let contract_id = client.address.clone();

    write_schema_version(&env, &contract_id, u32::MAX);
    let before = snapshot(&env, &contract_id);

    let result = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), 0u32)
    });

    assert_eq!(result, Err(Error::SchemaVersionMismatch));
    assert_eq!(read_schema_version(&env, &contract_id), u32::MAX);
    let after = snapshot(&env, &contract_id);
    assert_snapshot_unchanged(&before, &after);
}

/// A rejected downgrade must not disturb live subscription records.
#[test]
fn do_migrate_downgrade_leaves_subscriptions_untouched() {
    let (env, client, token, admin) = setup();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &50_000_000i128);
    let id = client
        .create_subscription(
            &subscriber,
            &merchant,
            &10_000_000i128,
            &(30 * 24 * 60 * 60),
            &false,
            &None::<i128>,
            &None::<u64>,
            &None::<u32>,
            &None::<soroban_sdk::Symbol>,
        );
    client.deposit_funds(&id, &subscriber, &50_000_000i128, &None::<soroban_sdk::BytesN<32>>);
    let sub_before = client.get_subscription(&id);

    let contract_id = client.address.clone();
    write_schema_version(&env, &contract_id, STORAGE_VERSION + 1);

    let result = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), STORAGE_VERSION)
    });
    assert_eq!(result, Err(Error::SchemaVersionMismatch));

    let sub_after = client.get_subscription(&id);
    assert_eq!(sub_before.status, sub_after.status);
    assert_eq!(sub_before.prepaid_balance, sub_after.prepaid_balance);
    assert_eq!(sub_before.amount, sub_after.amount);
    assert_eq!(sub_before.interval_seconds, sub_after.interval_seconds);
    assert_eq!(
        read_schema_version(&env, &contract_id),
        STORAGE_VERSION + 1,
        "stored version must be unchanged"
    );
}

/// The v3→v4/4→5/5→6 rewrite steps must preserve subscription field values:
/// migrating across the whole ladder leaves every field readable and equal.
#[test]
fn do_migrate_rewrite_steps_preserve_subscription_fields() {
    let (env, client, token, admin) = setup();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &50_000_000i128);
    let id = client
        .create_subscription(
            &subscriber,
            &merchant,
            &10_000_000i128,
            &(30 * 24 * 60 * 60),
            &false,
            &None::<i128>,
            &None::<u64>,
            &None::<u32>,
            &None::<soroban_sdk::Symbol>,
        );
    client.deposit_funds(&id, &subscriber, &50_000_000i128, &None::<soroban_sdk::BytesN<32>>);
    let before = client.get_subscription(&id);

    let contract_id = client.address.clone();
    write_schema_version(&env, &contract_id, 0);

    let result = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), STORAGE_VERSION)
    });
    assert_eq!(result, Ok(()));

    let after = client.get_subscription(&id);
    assert_eq!(before.subscriber, after.subscriber);
    assert_eq!(before.merchant, after.merchant);
    assert_eq!(before.token, after.token);
    assert_eq!(before.amount, after.amount);
    assert_eq!(before.interval_seconds, after.interval_seconds);
    assert_eq!(before.status, after.status);
    assert_eq!(before.prepaid_balance, after.prepaid_balance);
    assert_eq!(before.lifetime_charged, after.lifetime_charged);
}

// ── Event observability ───────────────────────────────────────────────────────

/// Event field fidelity: `from_version`, `to_version`, `admin`, and the ledger
/// timestamp must be deterministic and exactly observable.
#[test]
fn do_migrate_event_fields_are_deterministic() {
    let (env, client, _token, admin) = setup();
    let contract_id = client.address.clone();

    let ts: u64 = 1_234_567;
    env.ledger().with_mut(|li| li.timestamp = ts);

    write_schema_version(&env, &contract_id, 3);
    let result = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), 5u32)
    });

    assert_eq!(result, Ok(()));
    let evt = expect_single_migrate_event(&env);
    assert_eq!(evt.from_version, 3);
    assert_eq!(evt.to_version, 5);
    assert_eq!(evt.admin, admin);
    assert_eq!(evt.timestamp, ts);
    assert_eq!(
        evt.schema_version,
        crate::types::EVENT_SCHEMA_VERSION,
        "event envelope schema_version must equal EVENT_SCHEMA_VERSION"
    );
}

/// A rejected migration emits **no** event of any kind from this invocation.
#[test]
fn do_migrate_rejection_emits_no_events() {
    let (env, client, _token, admin) = setup();
    let contract_id = client.address.clone();

    write_schema_version(&env, &contract_id, STORAGE_VERSION + 1);
    let _ = env.as_contract(&contract_id, || {
        crate::admin::do_migrate(&env, admin.clone(), STORAGE_VERSION)
    });

    let events = env.events().all();
    assert!(
        events.is_empty(),
        "a rejected migration must not emit any events, saw {}",
        events.len()
    );
}

// ── Entry-point parity ────────────────────────────────────────────────────────

/// The public `migrate` entrypoint delegates to `do_migrate` with
/// `STORAGE_VERSION` — the observable behavior must match a direct call.
#[test]
fn migrate_entrypoint_matches_do_migrate_semantics() {
    let (env, client, _token, admin) = setup();
    let contract_id = client.address.clone();

    write_schema_version(&env, &contract_id, 1);
    client.migrate(&admin);
    expect_single_migrate_event(&env);
    assert_eq!(read_schema_version(&env, &contract_id), STORAGE_VERSION);

    // And a second call via the entrypoint is the same silent no-op:
    // its own invocation emits no events at all.
    client.migrate(&admin);
    assert!(env.events().all().is_empty(), "no-op migrate must emit nothing");
}

