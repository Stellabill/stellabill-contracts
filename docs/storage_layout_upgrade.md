# Storage Layout Upgrade Guide

## Storage Schema v1

All contract state is keyed through the `DataKey` enum defined in `types.rs`. This replaces the previous ad-hoc `Symbol` and bare `u32` keys used in v0.

### Key Registry

| Variant                 | Payload         | Value Type     | Purpose                                |
| ----------------------- | --------------- | -------------- | -------------------------------------- |
| `MerchantSubs(Address)` | merchant addr   | `Vec<u32>`     | Merchant → subscription ID index       |
| `Token`                 | —               | `Address`      | USDC token contract                    |
| `Admin`                 | —               | `Address`      | Authorized admin                       |
| `MinTopup`              | —               | `i128`         | Minimum deposit threshold              |
| `NextId`                | —               | `u32`          | Auto-incrementing subscription counter |
| `SchemaVersion`         | —               | `u32`          | On-chain storage version tag           |
| `Sub(u32)`              | subscription ID | `Subscription` | Subscription record                    |
| `ChargedPeriod(u32)`    | subscription ID | `u64`          | Last charged billing-period index      |
| `IdemKey(u32)`          | subscription ID | `BytesN<32>`   | Idempotency key for replay protection  |

### Subscription Struct (v1)

Field ordering is XDR-serialised and must not change:

| Position | Field                    | Type                 |
| -------- | ------------------------ | -------------------- |
| 0        | `subscriber`             | `Address`            |
| 1        | `merchant`               | `Address`            |
| 2        | `amount`                 | `i128`               |
| 3        | `interval_seconds`       | `u64`                |
| 4        | `last_payment_timestamp` | `u64`                |
| 5        | `status`                 | `SubscriptionStatus` |
| 6        | `prepaid_balance`        | `i128`               |
| 7        | `usage_enabled`          | `bool`               |

### SubscriptionStatus Enum

Discriminant values are immutable. New variants must be appended only:

| Variant               | Discriminant |
| --------------------- | ------------ |
| `Active`              | 0            |
| `Paused`              | 1            |
| `Cancelled`           | 2            |
| `InsufficientBalance` | 3            |

---

## Version Tracking

`init()` writes `STORAGE_VERSION` (currently `1`) to `DataKey::SchemaVersion`.

`get_storage_version()` returns the stored version, or `0` if unset (pre-versioning contracts).

---

## Migration: v0 → v1

### What changed

| Area              | v0                                  | v1                            |
| ----------------- | ----------------------------------- | ----------------------------- |
| Config keys       | `Symbol::new(env, "token")` etc.    | `DataKey::Token` etc.         |
| Subscription keys | bare `u32`                          | `DataKey::Sub(u32)`           |
| Replay keys       | `(symbol_short!("cp"), id)` tuple   | `DataKey::ChargedPeriod(u32)` |
| Idempotency keys  | `(symbol_short!("idem"), id)` tuple | `DataKey::IdemKey(u32)`       |
| Counter key       | `Symbol::new(env, "next_id")`       | `DataKey::NextId`             |
| Version tag       | not stored                          | `DataKey::SchemaVersion = 1`  |

### Procedure

1. Deploy the upgraded WASM via `soroban contract install` / `soroban contract upgrade`.
2. Call `admin_migrate(admin, 0)` once.
   - Re-keys all subscriptions from bare `u32` to `DataKey::Sub(u32)`.
   - Migrates the `next_id` counter from `Symbol` key to `DataKey::NextId`.
   - Sets `SchemaVersion` to `1`.
3. Verify with `get_storage_version()` — must return `1`.
4. Verify subscriptions are readable via `get_subscription(id)`.

The migration is idempotent — calling `admin_migrate(admin, 0)` a second time is a no-op.

---

## Adding New Fields to Subscription

Soroban XDR serialisation is positional. To add a field:

1. Append the field to the end of the `Subscription` struct.
2. Increment `STORAGE_VERSION` in `types.rs`.
3. Add a migration branch in `admin_migrate()` for the new version.
4. Use lazy migration or batch migration to backfill existing records.
5. Mark the field with `⚠️ Upgrade-sensitive: position N` in its doc comment.

Never remove, reorder, or retype existing fields.

---

## Adding New DataKey Variants

1. Append the new variant to the end of the `DataKey` enum.
2. Add a discriminant comment (next sequential number).
3. Never remove or reorder existing variants.

---

## Adding New SubscriptionStatus Variants

1. Append to the end of the enum with the next discriminant value.
2. Update `validate_status_transition()` and `get_allowed_transitions()`.
3. Never insert between existing variants.

---

## Compatibility Rules

### Safe (no migration needed)

- Appending new `DataKey` variants
- Appending new `SubscriptionStatus` variants
- Adding new config keys
- Changing function logic without touching storage schema

### Breaking (requires migration)

- Adding fields to `Subscription`
- Changing field types
- Removing fields
- Reordering enum variants
- Renaming storage keys

---

## Testing Upgrades

The test suite includes:

- `test_init_sets_schema_version` — version written on init
- `test_get_storage_version_before_init_returns_zero` — default for unversioned contracts
- `test_admin_migrate_v0_to_v1_simulation` — full v0→v1 migration with 3 subscriptions
- `test_admin_migrate_is_idempotent` — repeated migration is safe
- `test_admin_migrate_unauthorized` — non-admin cannot migrate
- `test_datakey_sub_does_not_collide_with_config_keys` — key isolation
- `test_many_subscriptions_no_key_collision` — 20 subscriptions + config integrity
- `test_next_id_overflow_protection` — counter overflow returns `Overflow` error
- `test_charged_period_key_isolation` — independent replay tracking per subscription
- `test_subscription_fields_stable_after_lifecycle` — field integrity through state transitions
- `test_version_persists_across_admin_rotation` — version survives admin rotation

For mainnet upgrades, always test on testnet first with production-like data.
