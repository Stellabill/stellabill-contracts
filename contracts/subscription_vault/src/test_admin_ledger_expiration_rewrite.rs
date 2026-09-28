//! Adversarial coverage for `admin::rewrite_subscriptions_for_ledger_expiration`
//! (issue #1020).
//!
//! # Subject under test
//!
//! `rewrite_subscriptions_for_ledger_expiration` is the v3 → v4 rung of
//! `admin::do_migrate`. It walks `DataKey::Sub(0..NextId)` and, for every record
//! that is live in persistent storage, writes the record back — so that the
//! `expires_at_ledger: Option<u32>` field introduced in schema v4 is present in
//! the stored payload — and refreshes that record's persistent TTL through
//! `extend_ttl(SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO)`. It returns the number of
//! records it touched.
//!
//! # Why these tests seed storage directly
//!
//! The pre-migration states that matter here cannot be produced through the
//! public entrypoints of the current binary: records with holes in the id
//! range, `NextId` still living in instance storage because the contract has not
//! run the config-to-persistent step yet, orphan records at or beyond `NextId`,
//! records whose TTL has decayed below the threshold, and records whose payload
//! is not deserializable as a `Subscription`. The tests therefore seed and read
//! `DataKey::Sub`, `DataKey::NextId`, `DataKey::SchemaVersion` and
//! `DataKey::Admin` through `Env::as_contract` — precisely the storage surface
//! the migration operates on.
//!
//! # Coverage
//!
//! 1. **Happy path** — every live record in `0..NextId` is rewritten with a
//!    field-for-field identical payload, and the returned count covers only live
//!    records.
//! 2. **TTL** — a record whose remaining TTL has decayed below
//!    `SUB_TTL_THRESHOLD` is re-extended to `SUB_TTL_EXTEND_TO`; a record whose
//!    remaining TTL is still above the threshold keeps its exact TTL, because
//!    the host's `extend_ttl` is conditional.
//! 3. **Boundaries** — absent `NextId`, `NextId == 0`, sparse id ranges,
//!    records stored at/after `NextId`, instance-tier records, and the pre-v3
//!    instance-storage fallback in `read_config`.
//! 4. **Authorization and validation** — `migrate` rejects non-admins and schema
//!    downgrades; every rejected call leaves storage unchanged.
//! 5. **Determinism** — repeated runs are idempotent, an already-current schema
//!    is a true no-op, and a payload that cannot be read aborts the rewrite
//!    deterministically instead of silently rewriting a partial set.
//!
//! `Persistent::get` panics (`unwrap_optimized`) when a stored value cannot be
//! converted to the requested type, so the unreadable-payload case is asserted
//! with `catch_unwind`.

#![cfg(test)]

use crate::admin::{get_schema_version, rewrite_subscriptions_for_ledger_expiration};
use crate::types::{
    DataKey, Error, Subscription, SubscriptionStatus, SUB_TTL_EXTEND_TO, SUB_TTL_THRESHOLD,
};
use crate::{SubscriptionVault, SubscriptionVaultClient, STORAGE_VERSION};
use soroban_sdk::testutils::storage::Persistent as _;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env, Symbol};

// ── Shared constants ─────────────────────────────────────────────────────────

/// Starting ledger sequence for all tests.
const START_SEQ: u32 = 100;
/// Starting ledger timestamp.
const START_TS: u64 = 1_700_000_000;
/// Persistent-entry floor used by the fake ledger.
const MIN_ENTRY_TTL: u32 = 4_096;
/// One day in seconds.
const DAY: u64 = 24 * 60 * 60;

// ── Fixtures ─────────────────────────────────────────────────────────────────

/// A record in the shape produced *before* schema v4: each field added after the
/// original layout sits at the value the migration is expected to preserve.
fn legacy_shaped_subscription(env: &Env) -> Subscription {
    Subscription {
        subscriber: Address::generate(env),
        merchant: Address::generate(env),
        token: Address::generate(env),
        amount: 5_000_000,
        interval_seconds: 30 * DAY,
        last_payment_timestamp: START_TS,
        status: SubscriptionStatus::Active,
        prepaid_balance: 0,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
        start_time: START_TS,
        expires_at: None,
        grace_start_timestamp: None,
        cancel_at: None,
        expires_at_ledger: None,
        sub_account_label: None,
        auto_renew: true,
        auto_renew_disabled_at: None,
        arrears: 0,
    }
}

/// A record with every optional field populated, so the rewrite has to carry
/// real values (not just defaults) across the read/write round-trip.
fn rich_subscription(env: &Env) -> Subscription {
    Subscription {
        status: SubscriptionStatus::Paused,
        prepaid_balance: 1_000_000,
        usage_enabled: true,
        lifetime_cap: Some(50_000_000),
        lifetime_charged: 12_345_678,
        expires_at: Some(START_TS + 90 * DAY),
        grace_start_timestamp: Some(START_TS + 3 * DAY),
        cancel_at: Some(START_TS + 10 * DAY),
        expires_at_ledger: Some(START_SEQ + 1_000_000),
        sub_account_label: Some(Symbol::new(env, "eu_west_1")),
        auto_renew: false,
        auto_renew_disabled_at: Some(START_TS + DAY),
        arrears: 250_000,
        ..legacy_shaped_subscription(env)
    }
}

// ── Harness helpers ──────────────────────────────────────────────────────────

/// A registered contract in a fake ledger whose `max_entry_ttl` sits above
/// `SUB_TTL_EXTEND_TO`, so `extend_ttl` calls are never clamped by the host.
fn setup() -> (Env, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|li| {
        li.sequence_number = START_SEQ;
        li.timestamp = START_TS;
        li.min_persistent_entry_ttl = MIN_ENTRY_TTL;
        li.min_temp_entry_ttl = MIN_ENTRY_TTL;
        li.max_entry_ttl = SUB_TTL_EXTEND_TO + 5_000_000;
    });
    let contract_id = env.register(SubscriptionVault, ());
    // Keep the contract's instance entry alive across the long ledger jumps used
    // by the TTL tests: the test host archives entries whose TTL has elapsed and
    // aborts the call with a host error before the contract logic runs.
    env.as_contract(&contract_id, || {
        env.storage()
            .instance()
            .extend_ttl(SUB_TTL_EXTEND_TO, SUB_TTL_EXTEND_TO);
    });
    let admin = Address::generate(&env);
    (env, contract_id, admin)
}

/// Move the ledger sequence without touching the timestamp.
fn set_seq(env: &Env, seq: u32) {
    env.ledger().with_mut(|li| li.sequence_number = seq);
}

/// Store `id`'s payload verbatim (no TTL extension beyond the host's floor).
fn put_sub(env: &Env, cid: &Address, id: u32, sub: &Subscription) {
    env.as_contract(cid, || {
        env.storage().persistent().set(&DataKey::Sub(id), sub);
    });
}

/// Read `id`'s payload, or `None` when the key is absent.
fn get_sub(env: &Env, cid: &Address, id: u32) -> Option<Subscription> {
    env.as_contract(cid, || env.storage().persistent().get(&DataKey::Sub(id)))
}

/// Remaining TTL of `DataKey::Sub(id)`, in ledgers.
fn sub_ttl(env: &Env, cid: &Address, id: u32) -> u32 {
    env.as_contract(cid, || {
        env.storage().persistent().get_ttl(&DataKey::Sub(id))
    })
}

/// Force `DataKey::Sub(id)`'s remaining TTL up to `extend_to` ledgers.
///
/// The host only applies `extend_ttl` when the current TTL is *below* the
/// threshold, never shortens a live TTL, and rejects `threshold > extend_to`, so
/// the threshold is set equal to the target: a fresh entry (which the host seeds
/// with the `min_persistent_entry_ttl` floor) qualifies and is raised to the
/// full window. Ledger advances build shorter windows from this one.
fn force_sub_ttl(env: &Env, cid: &Address, id: u32, extend_to: u32) {
    env.as_contract(cid, || {
        env.storage()
            .persistent()
            .extend_ttl(&DataKey::Sub(id), extend_to, extend_to);
    });
}

/// Seed the keys `do_migrate` reads: the admin used by `require_admin_auth`, the
/// stored schema version, and the migration counter.
///
/// `schema < 3` writes the config keys to the instance tier and `schema >= 3` to
/// the persistent tier, mirroring `admin::write_config`'s tier policy (and
/// `read_config`'s version-gated fallback). The seeded entries are given a full
/// TTL window so that the long ledger jumps used by the TTL tests do not archive
/// them out from under the migration.
fn seed_migration_state(
    env: &Env,
    cid: &Address,
    admin: &Address,
    schema: u32,
    next_id: Option<u32>,
) {
    env.as_contract(cid, || {
        env.storage().persistent().set(&DataKey::Admin, admin);
        env.storage().persistent().extend_ttl(
            &DataKey::Admin,
            SUB_TTL_EXTEND_TO,
            SUB_TTL_EXTEND_TO,
        );
        if schema >= 3 {
            env.storage()
                .persistent()
                .set(&DataKey::SchemaVersion, &schema);
            env.storage().persistent().extend_ttl(
                &DataKey::SchemaVersion,
                SUB_TTL_EXTEND_TO,
                SUB_TTL_EXTEND_TO,
            );
            if let Some(next_id) = next_id {
                env.storage().persistent().set(&DataKey::NextId, &next_id);
                env.storage().persistent().extend_ttl(
                    &DataKey::NextId,
                    SUB_TTL_EXTEND_TO,
                    SUB_TTL_EXTEND_TO,
                );
            }
        } else {
            env.storage()
                .instance()
                .set(&DataKey::SchemaVersion, &schema);
            if let Some(next_id) = next_id {
                env.storage().instance().set(&DataKey::NextId, &next_id);
            }
        }
    });
}

/// Run the v3 → v4 rewrite on the seeded contract and return `touched`.
fn run_rewrite(env: &Env, cid: &Address) -> u32 {
    env.as_contract(cid, || rewrite_subscriptions_for_ledger_expiration(env))
}

/// Stored schema version as observed by the migration code.
fn stored_schema(env: &Env, cid: &Address) -> u32 {
    env.as_contract(cid, || get_schema_version(env))
}

/// Raw instance-tier schema version, bypassing `get_schema_version`'s
/// persistent-first lookup. `None` once the migration has cleared that tier.
fn instance_schema(env: &Env, cid: &Address) -> Option<u32> {
    env.as_contract(cid, || {
        env.storage().instance().get(&DataKey::SchemaVersion)
    })
}

/// `true` when `DataKey::Sub(id)` still lives in the instance tier.
fn instance_sub_present(env: &Env, cid: &Address, id: u32) -> bool {
    env.as_contract(cid, || env.storage().instance().has(&DataKey::Sub(id)))
}

/// `Sub(id)` read back as a `u32` — proves a non-subscription payload was not
/// clobbered by a failed rewrite.
fn raw_sub_u32(env: &Env, cid: &Address, id: u32) -> Option<u32> {
    env.as_contract(cid, || env.storage().persistent().get(&DataKey::Sub(id)))
}

/// Runs `f` and returns its panic message, silencing the panic hook while it
/// runs. Panics if `f` completes normally.
fn panic_message<F: FnOnce()>(f: F) -> String {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    std::panic::set_hook(prev);
    match result {
        Ok(()) => panic!("expected the call to abort, but it completed"),
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_else(|| "<non-string panic payload>".to_string()),
    }
}

// ── 1. Happy path ────────────────────────────────────────────────────────────

/// Every live record in `0..NextId` is rewritten with an identical payload, and
/// the returned count covers exactly the live records.
#[test]
fn rewrites_every_live_record_and_counts_only_live_records() {
    let (env, cid, admin) = setup();
    seed_migration_state(&env, &cid, &admin, 4, Some(3));

    let legacy = legacy_shaped_subscription(&env);
    let rich = rich_subscription(&env);
    put_sub(&env, &cid, 0, &legacy);
    put_sub(&env, &cid, 1, &rich);
    // id 2 is deliberately absent: a hole must not be counted.
    put_sub(&env, &cid, 3, &legacy); // orphan: at or beyond NextId

    set_seq(&env, START_SEQ + 10);
    let touched = run_rewrite(&env, &cid);

    assert_eq!(touched, 2, "only ids 0 and 1 are live below NextId");
    assert_eq!(get_sub(&env, &cid, 0), Some(legacy.clone()));
    assert_eq!(get_sub(&env, &cid, 1), Some(rich.clone()));
}

/// The rewrite neither invents nor drops payload data: the ledger bound added in
/// v4 survives the round-trip, as do every other optional field.
#[test]
fn preserves_ledger_expiration_bound_and_optional_fields() {
    let (env, cid, admin) = setup();
    seed_migration_state(&env, &cid, &admin, 4, Some(1));

    let rich = rich_subscription(&env);
    put_sub(&env, &cid, 0, &rich);

    assert_eq!(run_rewrite(&env, &cid), 1);

    let after = get_sub(&env, &cid, 0).expect("record must still be readable");
    assert_eq!(after, rich);
    assert_eq!(after.expires_at_ledger, Some(START_SEQ + 1_000_000));
    assert_eq!(
        after.sub_account_label,
        Some(Symbol::new(&env, "eu_west_1"))
    );
    assert_eq!(after.arrears, 250_000);
    assert!(!after.auto_renew);
}

// ── 2. TTL semantics ─────────────────────────────────────────────────────────

/// The case the migration exists for: a record whose remaining TTL has decayed
/// below `SUB_TTL_THRESHOLD` is re-extended to `SUB_TTL_EXTEND_TO`.
#[test]
fn extends_ttl_that_decayed_below_threshold() {
    let (env, cid, admin) = setup();
    seed_migration_state(&env, &cid, &admin, 4, Some(1));
    put_sub(&env, &cid, 0, &legacy_shaped_subscription(&env));

    // Decay: advance the ledger until only 10 ledgers of TTL remain.
    let seeded_ttl = sub_ttl(&env, &cid, 0);
    assert!(
        seeded_ttl > 10,
        "host should seed new persistent entries with more than 10 ledgers, got {seeded_ttl}"
    );
    set_seq(&env, START_SEQ + seeded_ttl - 10);
    let decayed = sub_ttl(&env, &cid, 0);
    assert!(
        decayed < SUB_TTL_THRESHOLD,
        "precondition: TTL must have decayed below the threshold, got {decayed}"
    );

    assert_eq!(run_rewrite(&env, &cid), 1);
    assert_eq!(
        sub_ttl(&env, &cid, 0),
        SUB_TTL_EXTEND_TO,
        "decayed record must be re-extended to the full window"
    );
}

/// A record whose remaining TTL is still above `SUB_TTL_THRESHOLD` keeps its
/// exact TTL: `extend_ttl` only fires when the threshold is breached, so the
/// rewrite must not silently reset every record to a fresh window.
#[test]
fn leaves_healthy_ttl_untouched() {
    let (env, cid, admin) = setup();
    seed_migration_state(&env, &cid, &admin, 4, Some(1));
    put_sub(&env, &cid, 0, &legacy_shaped_subscription(&env));

    // A distinct, still-healthy window: build the full window, then decay it by
    // a known amount. It differs from SUB_TTL_EXTEND_TO, so a blanket extension
    // would be visible in the assertion below.
    force_sub_ttl(&env, &cid, 0, SUB_TTL_EXTEND_TO);
    let full_window = sub_ttl(&env, &cid, 0);
    let healthy = full_window - 1_000;
    set_seq(&env, START_SEQ + 1_000);
    assert_eq!(sub_ttl(&env, &cid, 0), healthy);
    assert!(healthy >= SUB_TTL_THRESHOLD);
    assert_ne!(healthy, SUB_TTL_EXTEND_TO);

    assert_eq!(run_rewrite(&env, &cid), 1);
    assert_eq!(
        sub_ttl(&env, &cid, 0),
        healthy,
        "a healthy TTL must not be reset by the rewrite"
    );
}

/// Running the rewrite twice is idempotent: same count, same payloads, same TTL.
#[test]
fn repeated_runs_are_idempotent() {
    let (env, cid, admin) = setup();
    seed_migration_state(&env, &cid, &admin, 4, Some(2));
    put_sub(&env, &cid, 0, &legacy_shaped_subscription(&env));
    put_sub(&env, &cid, 1, &rich_subscription(&env));

    let seeded_ttl = sub_ttl(&env, &cid, 0);
    set_seq(&env, START_SEQ + seeded_ttl - 5);

    let first = run_rewrite(&env, &cid);
    let payload_after_first = get_sub(&env, &cid, 0);
    let ttl_after_first = sub_ttl(&env, &cid, 0);

    let second = run_rewrite(&env, &cid);

    assert_eq!(second, first, "count must be stable across runs");
    assert_eq!(get_sub(&env, &cid, 0), payload_after_first);
    assert_eq!(
        sub_ttl(&env, &cid, 0),
        ttl_after_first,
        "the second run must not move the TTL again"
    );
}

// ── 3. Boundary: id-range bookkeeping ────────────────────────────────────────

/// With no `NextId` key at all (`read_config(...).unwrap_or(0)`), the rewrite
/// visits nothing and reports `0` — it must neither panic nor touch records.
#[test]
fn absent_next_id_is_a_safe_no_op() {
    let (env, cid, admin) = setup();
    seed_migration_state(&env, &cid, &admin, 4, None);
    put_sub(&env, &cid, 0, &legacy_shaped_subscription(&env));
    let ttl_before = sub_ttl(&env, &cid, 0);

    assert_eq!(run_rewrite(&env, &cid), 0);
    assert!(
        get_sub(&env, &cid, 0).is_some(),
        "record must be left in place"
    );
    assert_eq!(
        sub_ttl(&env, &cid, 0),
        ttl_before,
        "no TTL refresh without a counter"
    );
}

/// `NextId == 0` means "no subscription was ever created": a record sitting at
/// id 0 is orphaned (its counter was lost) and is deliberately left alone.
#[test]
fn zero_next_id_leaves_orphan_record_untouched() {
    let (env, cid, admin) = setup();
    seed_migration_state(&env, &cid, &admin, 4, Some(0));
    let legacy = legacy_shaped_subscription(&env);
    put_sub(&env, &cid, 0, &legacy);
    let ttl_before = sub_ttl(&env, &cid, 0);

    assert_eq!(run_rewrite(&env, &cid), 0);
    assert_eq!(get_sub(&env, &cid, 0), Some(legacy));
    assert_eq!(sub_ttl(&env, &cid, 0), ttl_before);
}

/// Ids are sparse: missing ids are skipped, the record at `NextId - 1` is
/// visited, and records at or beyond `NextId` are left fully alone.
#[test]
fn sparse_ids_are_skipped_and_orphans_are_left_alone() {
    let (env, cid, admin) = setup();
    seed_migration_state(&env, &cid, &admin, 4, Some(50));

    let legacy = legacy_shaped_subscription(&env);
    put_sub(&env, &cid, 0, &legacy);
    put_sub(&env, &cid, 7, &legacy);
    put_sub(&env, &cid, 49, &legacy);
    // Orphan at NextId: no counter will ever point past it again.
    put_sub(&env, &cid, 50, &legacy);
    let orphan_ttl = sub_ttl(&env, &cid, 50);

    assert_eq!(run_rewrite(&env, &cid), 3);

    for id in [0u32, 7, 49] {
        assert_eq!(get_sub(&env, &cid, id), Some(legacy.clone()), "id {id}");
    }
    assert_eq!(get_sub(&env, &cid, 50), Some(legacy));
    assert_eq!(
        sub_ttl(&env, &cid, 50),
        orphan_ttl,
        "an orphan must not be refreshed by the id walk"
    );
}

/// Only the persistent tier is rewritten: an instance-tier `Sub(id)` record (a
/// legacy artifact of the pre-persistent layout) is not read, not rewritten and
/// not counted.
#[test]
fn instance_tier_record_is_ignored() {
    let (env, cid, admin) = setup();
    seed_migration_state(&env, &cid, &admin, 4, Some(2));

    let legacy = legacy_shaped_subscription(&env);
    env.as_contract(&cid, || {
        env.storage().instance().set(&DataKey::Sub(0), &legacy);
    });
    put_sub(&env, &cid, 1, &rich_subscription(&env));

    assert_eq!(
        run_rewrite(&env, &cid),
        1,
        "only the persistent record is in scope"
    );
    assert!(
        instance_sub_present(&env, &cid, 0),
        "the instance-tier record must survive untouched"
    );
    assert_eq!(get_sub(&env, &cid, 0), None);
    assert!(get_sub(&env, &cid, 1).is_some());
}

/// A pre-v3 contract keeps `NextId` in instance storage; `read_config`'s
/// version-gated fallback must still find it, otherwise the rewrite would
/// silently no-op on exactly the oldest contracts it targets.
#[test]
fn reads_instance_next_id_before_schema_v3() {
    let (env, cid, admin) = setup();
    seed_migration_state(&env, &cid, &admin, 2, Some(2));

    let legacy = legacy_shaped_subscription(&env);
    let rich = rich_subscription(&env);
    put_sub(&env, &cid, 0, &legacy);
    put_sub(&env, &cid, 1, &rich);

    assert_eq!(stored_schema(&env, &cid), 2);
    assert_eq!(run_rewrite(&env, &cid), 2);
    assert_eq!(get_sub(&env, &cid, 0), Some(legacy));
    assert_eq!(get_sub(&env, &cid, 1), Some(rich));
}

// ── 4. `migrate` entrypoint: authorization and validation ────────────────────

/// End-to-end: a contract frozen at schema v3 (config already persistent,
/// records pre-v4) migrates to `STORAGE_VERSION`, keeping every record readable
/// and re-extending TTLs that had decayed.
#[test]
fn migrate_advances_schema_and_rewrites_records() {
    let (env, cid, admin) = setup();
    seed_migration_state(&env, &cid, &admin, 3, Some(2));

    let legacy = legacy_shaped_subscription(&env);
    let rich = rich_subscription(&env);
    put_sub(&env, &cid, 0, &legacy);
    put_sub(&env, &cid, 1, &rich);

    // Decay both records below the threshold so the rewrite's effect is visible.
    let seeded_ttl = sub_ttl(&env, &cid, 0);
    set_seq(&env, START_SEQ + seeded_ttl - 10);

    let client = SubscriptionVaultClient::new(&env, &cid);
    assert_eq!(client.try_migrate(&admin), Ok(Ok(())));

    assert_eq!(stored_schema(&env, &cid), STORAGE_VERSION);
    assert_eq!(
        instance_schema(&env, &cid),
        None,
        "v3+ keeps the schema version in the persistent tier only"
    );
    assert_eq!(get_sub(&env, &cid, 0), Some(legacy));
    assert_eq!(get_sub(&env, &cid, 1), Some(rich));
    assert_eq!(sub_ttl(&env, &cid, 0), SUB_TTL_EXTEND_TO);
    assert_eq!(sub_ttl(&env, &cid, 1), SUB_TTL_EXTEND_TO);
}

/// A non-admin cannot trigger the rewrite: `migrate` returns `Forbidden` and
/// leaves schema, counter and records exactly as they were.
#[test]
fn migrate_rejects_non_admin_and_leaves_state_untouched() {
    let (env, cid, admin) = setup();
    seed_migration_state(&env, &cid, &admin, 3, Some(1));

    let legacy = legacy_shaped_subscription(&env);
    put_sub(&env, &cid, 0, &legacy);
    let ttl_before = sub_ttl(&env, &cid, 0);

    // `mock_all_auths` satisfies the signature check, not the stored-role check.
    let stranger = Address::generate(&env);
    let client = SubscriptionVaultClient::new(&env, &cid);
    assert_eq!(client.try_migrate(&stranger), Err(Ok(Error::Forbidden)));

    assert_eq!(stored_schema(&env, &cid), 3);
    assert_eq!(get_sub(&env, &cid, 0), Some(legacy));
    assert_eq!(
        sub_ttl(&env, &cid, 0),
        ttl_before,
        "a rejected call must not refresh TTLs"
    );
}

/// A binary older than the stored schema refuses to rewrite anything, so an
/// out-of-date deployment can never downgrade (or half-rewrite) live records.
#[test]
fn migrate_rejects_schema_downgrade_and_leaves_state_untouched() {
    let (env, cid, admin) = setup();
    seed_migration_state(&env, &cid, &admin, STORAGE_VERSION + 1, Some(1));

    let legacy = legacy_shaped_subscription(&env);
    put_sub(&env, &cid, 0, &legacy);
    let ttl_before = sub_ttl(&env, &cid, 0);

    let client = SubscriptionVaultClient::new(&env, &cid);
    assert_eq!(
        client.try_migrate(&admin),
        Err(Ok(Error::SchemaVersionMismatch))
    );

    assert_eq!(stored_schema(&env, &cid), STORAGE_VERSION + 1);
    assert_eq!(get_sub(&env, &cid, 0), Some(legacy));
    assert_eq!(sub_ttl(&env, &cid, 0), ttl_before);
}

/// Re-running `migrate` on a current contract is a true no-op: `do_migrate`
/// returns early, so not even a decayed TTL is refreshed and no record is
/// rewritten.
#[test]
fn migrate_is_a_no_op_when_schema_is_current() {
    let (env, cid, admin) = setup();
    seed_migration_state(&env, &cid, &admin, STORAGE_VERSION, Some(1));

    let legacy = legacy_shaped_subscription(&env);
    put_sub(&env, &cid, 0, &legacy);

    // Decay the record to 5 ledgers: if the early return in `do_migrate` ever
    // regressed, the rewrite would refresh this TTL to a full window. Only the
    // record is decayed — `setup` and `seed_migration_state` keep the instance
    // entry and the config keys alive across the long jump below.
    force_sub_ttl(&env, &cid, 0, SUB_TTL_EXTEND_TO);
    let full_window = sub_ttl(&env, &cid, 0);
    set_seq(&env, START_SEQ + full_window - 5);
    assert_eq!(sub_ttl(&env, &cid, 0), 5);
    assert!(5 < SUB_TTL_THRESHOLD);

    let client = SubscriptionVaultClient::new(&env, &cid);
    assert_eq!(client.try_migrate(&admin), Ok(Ok(())));

    assert_eq!(stored_schema(&env, &cid), STORAGE_VERSION);
    assert_eq!(get_sub(&env, &cid, 0), Some(legacy));
    assert_eq!(
        sub_ttl(&env, &cid, 0),
        5,
        "an already-current contract must not run the rewrite"
    );
}

// ── 5. Failure path: unreadable payload ──────────────────────────────────────

/// `Persistent::get::<Subscription>` panics when a `Sub(id)` payload is not a
/// subscription map. The rewrite aborts at that id rather than silently skipping
/// it, and it does not clobber the payloads it already handled: an operator sees
/// a hard failure and the data intact, never a half-migrated, half-silent state.
#[test]
fn unreadable_record_aborts_rewrite_without_clobbering() {
    let (env, cid, admin) = setup();
    seed_migration_state(&env, &cid, &admin, 4, Some(2));

    let legacy = legacy_shaped_subscription(&env);
    put_sub(&env, &cid, 0, &legacy);
    // id 1 holds a `u32` where a subscription is expected (e.g. a truncated or
    // manually corrupted payload).
    env.as_contract(&cid, || {
        env.storage().persistent().set(&DataKey::Sub(1), &7u32);
    });

    let msg = panic_message(|| {
        env.as_contract(&cid, || {
            rewrite_subscriptions_for_ledger_expiration(&env);
        });
    });

    assert!(
        msg.contains("ConversionError"),
        "expected an XDR conversion failure, got: {msg}"
    );

    // id 0 was rewritten before the abort, and its payload is unchanged.
    assert_eq!(get_sub(&env, &cid, 0), Some(legacy));
    // The unreadable payload is still there for a repair migration to handle.
    assert_eq!(raw_sub_u32(&env, &cid, 1), Some(7));
}
