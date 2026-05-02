# Migration Hooks (Subscription Vault)

This document describes the migration-friendly hooks added to the contract to support
future upgrades while preserving security and minimizing risk.

## Goals and scope

- Provide **admin-only**, **read-only** export hooks for contract and subscription state.
- Keep exports **bounded** and **auditable** via events.
- Avoid any mechanism that could **move funds**, **corrupt state**, or **weaken auth**.

These hooks are intended for carefully managed upgrades only. They do not implement
an automatic migration, and they do not enable cross-contract transfers.

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

- Only the stored admin address can invoke export hooks.
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
