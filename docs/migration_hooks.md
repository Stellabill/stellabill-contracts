# Migration Hooks (Subscription Vault)

This document describes the migration-friendly hooks added to the contract to support
future upgrades while preserving security and minimizing risk.

## Goals and scope

- Provide **admin-only**, **read-only** export hooks for contract and subscription state.
- Keep exports **bounded** and **auditable** via events.
- Avoid any mechanism that could **move funds**, **corrupt state**, or **weaken auth**.

These hooks are intended for carefully managed upgrades only. They do not enable
cross-contract transfers.

## Schema version migration

### Overview

The contract stores a `DataKey::SchemaVersion` key in instance storage. Its value
must always equal the binary's `STORAGE_VERSION` constant (currently `2`).

- `init` writes `SchemaVersion = STORAGE_VERSION` on first call.
- The `migrate(admin)` entrypoint compares the on-chain stored version against the
  binary version and runs registered upgrade closures for the `(from, to)` pair.

### `migrate(admin)` entrypoint

Implemented in `contracts/subscription_vault/src/lib.rs` (delegates to `admin::do_migrate`).

| Stored version | Binary version | Result |
|:---:|:---:|:---|
| `stored > binary` | — | `Err(SchemaMigrationDowngrade)` — downgrade rejected |
| `stored == binary` | — | `Ok(())` — idempotent no-op, no event emitted |
| `stored < binary` | — | Runs upgrade ladder, writes new version, emits `SchemaMigratedEvent` |

**Auth:** Admin only. `admin.require_auth()` is called before any state is read.

**Event:** `SchemaMigratedEvent { admin, from_version, to_version, timestamp }` is
emitted on the `schema_migrated` topic only when an actual upgrade is performed.

**Atomicity:** The `SchemaVersion` key is written **after** all upgrade steps succeed.
A mid-migration panic leaves the stored version unchanged.

### Adding a future migration path

When `STORAGE_VERSION` is bumped to `N`, add a new arm to the `match (current, binary_version)`
ladder in `admin::do_migrate`:

```rust
(N - 1, N) => {
    // perform any required state-shape changes here
    current = N;
}
```

Each arm must be self-contained and must not assume any prior arm ran in the same call.

### Security properties

- **Downgrade guard:** if the on-chain version is newer than the binary, the call is
  rejected immediately with `SchemaMigrationDowngrade`, preventing accidental rollback
  corruption.
- **Idempotent:** calling `migrate` when already at the current version is a safe no-op.
- **Admin-only:** non-admin callers are rejected with `Unauthorized`.
- **No fund movement:** `migrate` only reads and writes the `SchemaVersion` instance key.
  No token transfers, subscription mutations, or balance changes occur.
- **Audit trail:** every successful upgrade emits a `SchemaMigratedEvent` with the
  admin address, version pair, and ledger timestamp.

## Export hooks

The following entrypoints are implemented in `contracts/subscription_vault/src/lib.rs`:

- `export_contract_snapshot(admin)`
  - Returns `ContractSnapshot` containing `admin`, `token`, `min_topup`, `next_id`,
    `storage_version`, and a `timestamp`.
  - Emits a `migration_contract_snapshot` event.

- `export_subscription_summary(admin, subscription_id)`
  - Returns `SubscriptionSummary` for a single subscription.
  - Emits a `migration_export` event.

- `export_subscription_summaries(admin, start_id, limit)`
  - Returns a paginated list of `SubscriptionSummary` records.
  - `limit` is capped at `MAX_EXPORT_LIMIT` (currently 100) to keep responses bounded.
  - Emits a `migration_export` event that includes `start_id`, `limit`, and `exported`.

All export functions require **admin authentication** and are read-only.

## Control and authorization

- Only the stored admin address can invoke export hooks or the migrate entrypoint.
- Each export produces an event for auditability.
- Export hooks do not alter balances, subscription status, or any storage keys.

## Suggested migration flow

1. Admin calls `export_contract_snapshot` to capture config and storage version.
2. Admin iterates through subscriptions with `export_subscription_summaries` using
   pagination (for example, `start_id = 0` and `limit = 100` until done).
3. Off-chain tooling persists the exported summaries and validates:
   - counts and IDs are consistent
   - balances and statuses are as expected
4. A new contract version is deployed and imported using a controlled, external
   migration process (out of scope for this contract).
5. Admin calls `migrate(admin)` on the new deployment to advance `SchemaVersion`
   and confirm the upgrade ladder ran successfully.

## Security and limitations

- Exports are **read-only** and **admin-only** to avoid weakening security.
- No funds can be moved via these hooks.
- The contract does **not** include a generic import hook; imports are intentionally
  excluded to prevent misuse and to keep the surface area minimal.
- Storage versioning is exposed as a constant (`STORAGE_VERSION = 2`) to support
  migration tooling decisions.

## Caveats

- Export pagination is based on `next_id` and will skip missing IDs.
- Event contents are meant for audit logs, not for replay-based migrations.
- Any migration must be reviewed and validated off-chain before use.

## Schema migration test coverage (issue #435)

The following tests in `contracts/subscription_vault/src/test.rs` cover the
`migrate` entrypoint:

| Test | What it verifies |
|---|---|
| `test_init_writes_schema_version` | `init` writes `SchemaVersion = 2` |
| `test_migrate_same_version_is_noop_success` | Same-version call returns `Ok`, no event |
| `test_migrate_downgrade_is_rejected` | Stored > binary → `SchemaMigrationDowngrade` |
| `test_migrate_non_admin_is_rejected` | Non-admin → `Unauthorized` |
| `test_migrate_forward_upgrade_writes_version_and_emits_event` | v0 → v2: version written, event emitted |
| `test_migrate_forward_from_version_1_to_2` | v1 → v2: succeeds |
| `test_migrate_is_idempotent_after_forward_upgrade` | Second call after upgrade is no-op |
| `test_migrate_does_not_affect_subscriptions` | Subscription state unchanged after migration |
| `test_migrate_event_fields_are_correct` | Event fields match admin, versions, timestamp |
| `test_migrate_downgrade_does_not_emit_event` | Rejected downgrade emits no event |

## Migration golden regression test suite

Golden regression tests are located in:
- `contracts/subscription_vault/tests/migration_goldens.rs` — Cross-version snapshot determinism harness
- `contracts/subscription_vault/tests/snapshots/migration_goldens/*.scval.hex` — Hex-encoded deterministic snapshots

## Suggested migration flow

1. Admin calls `export_contract_snapshot` to capture config and storage version.
2. Admin iterates through subscriptions with `export_subscription_summaries` using
   pagination (for example, `start_id = 0` and `limit = 100` until done).
3. Off-chain tooling persists the exported summaries and validates:
   - counts and IDs are consistent
   - balances and statuses are as expected
4. A new contract version is deployed and imported using a controlled, external
   migration process (out of scope for this contract).

### Integration with golden regression tests

The golden regression test suite provides automated validation of snapshot stability:

1. **Initial setup:** Run `cargo test -- --ignored update_goldens` to generate golden fixtures for your contract version
2. **Development:** As you make changes, `cargo test migration_goldens` validates that exports remain deterministic
3. **Pre-release:** Golden fixtures are committed to version control and serve as regression anchors
4. **Post-upgrade:** Compare old and new golden fixtures to understand snapshot format changes
5. **Rollback safety:** Golden fixtures enable bit-perfect snapshot comparison across version boundaries

## Security and limitations

- Exports are **read-only** and **admin-only** to avoid weakening security.
- No funds can be moved via these hooks.
- The contract does **not** include a generic import hook; imports are intentionally
  excluded to prevent misuse and to keep the surface area minimal.
- Storage versioning is exposed as a constant (`STORAGE_VERSION = 2`) to support
  migration tooling decisions.

## Caveats

- Export pagination is based on `next_id` and will skip missing IDs.
- Event contents are meant for audit logs, not for replay-based migrations.
- Any migration must be reviewed and validated off-chain before use.

## Migration fixture test suite

The file `contracts/subscription_vault/src/test_migration_fixtures.rs` contains
31 tests that verify migration correctness. They cover:

### Contract snapshot invariants
- `test_migration_snapshot_captures_all_config_fields` — verifies admin, token, min_topup, next_id, storage_version, timestamp are all correct after init.
- `test_migration_snapshot_next_id_increments_with_subscriptions` — confirms next_id tracks created subscriptions.
- `test_migration_snapshot_does_not_mutate_state` — repeated snapshot calls leave subscription balances and statuses unchanged.
- `test_migration_snapshot_requires_admin` — non-admin callers are rejected.

### Single-subscription export
- `test_migration_single_summary_preserves_all_fields` — all 14 fields (subscriber, merchant, token, amount, interval, balance, status, etc.) round-trip correctly.
- `test_migration_single_summary_preserves_lifetime_cap_and_charged` — cap and charged counters survive a real charge cycle.
- `test_migration_single_summary_not_found_returns_error` — missing subscription_id returns `NotFound`.
- `test_migration_single_summary_requires_admin` — non-admin rejected.

### Paginated export
- `test_migration_paginated_export_all_subscriptions` — all IDs exported in order.
- `test_migration_paginated_export_respects_limit` — `limit=3` returns exactly 3 records.
- `test_migration_paginated_export_cursor_resumes_correctly` — two pages are disjoint and contiguous.
- `test_migration_paginated_export_empty_when_no_subscriptions` — empty vault returns empty list.
- `test_migration_paginated_export_start_beyond_range_returns_empty` — cursor past next_id returns empty.
- `test_migration_paginated_export_limit_zero_returns_empty` — limit=0 returns empty.
- `test_migration_paginated_export_limit_exceeds_max_returns_error` — limit>100 returns `InvalidExportLimit`.
- `test_migration_paginated_export_requires_admin` — non-admin rejected.

### Status preservation
All seven subscription statuses are verified to export faithfully:
- `Active`, `Paused`, `Cancelled` — via live contract transitions.
- `InsufficientBalance`, `Expired` — via direct storage patch (error-returning contract calls roll back state in the test environment).

### Balance accounting invariants
- `test_migration_export_does_not_inflate_balances` — three successive export calls leave prepaid_balance and lifetime_charged unchanged.
- `test_migration_summary_balance_matches_subscription_record` — exported balance fields match `get_subscription` after two charges.
- `test_migration_full_walk_balances_sum_matches_individual_queries` — sum of balances from paginated export equals sum from direct queries.

### Role security
- `test_migration_export_does_not_change_admin` — repeated exports do not rotate or escalate the admin address.

### Lifetime cap accounting
- `test_migration_lifetime_cap_fully_exhausted_shows_cancelled` — cap = 1 charge → status Cancelled, lifetime_charged = cap.
- `test_migration_lifetime_cap_partially_charged_preserved` — partial charge tracked; subscription stays Active.

### Expiration fields
- `test_migration_active_expiring_subscription_preserves_expires_at` — expires_at is present in summary before expiry triggers.

### Partial migration simulation
- `test_migration_full_walk_covers_all_subscriptions` — paged walk over 7 subscriptions (page size 3) collects all 7 IDs.

### Emergency stop compatibility
- `test_migration_exports_work_during_emergency_stop` — all three export hooks remain callable when emergency stop is active (exports are read-only and not blocked).

## Verified security properties

The test suite explicitly confirms:

| Property | How tested |
|---|---|
| No balance inflation | Multiple export calls; balance unchanged |
| No role escalation | Admin address identical before and after exports |
| Read-only | No state mutation observable after any export call |
| Admin-only access | Non-admin callers rejected on all three hooks |
| Status fidelity | All 7 statuses preserved in export output |
| Accounting fidelity | prepaid_balance + lifetime_charged match direct storage reads |
| Emergency stop safe | Exports unblocked during emergency stop |
