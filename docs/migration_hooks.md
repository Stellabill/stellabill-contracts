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
- Storage versioning is exposed as a constant (`STORAGE_VERSION = 1`) to support
  migration tooling decisions.

## Caveats

- Export pagination is based on `next_id` and will skip missing IDs.
- Event contents are meant for audit logs, not for replay-based migrations.
- Any migration must be reviewed and validated off-chain before use.
