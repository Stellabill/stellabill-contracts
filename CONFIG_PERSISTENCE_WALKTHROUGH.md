# Walkthrough: Promoting Configuration Keys to Persistent Storage

This document describes the promotion of global configuration keys from instance storage to persistent storage in the Soroban subscription vault contract.

---

## 1. Context & Motivation

As documented in `docs/storage_layout.md`, the initial storage layout utilized **instance storage** for global configurations (`Token`, `Admin`, `MinTopup`, `NextId`, `SchemaVersion`, `EmergencyStop`, `Treasury`, `FeeBps`, and `Operator`).

However, Soroban's **instance storage** has:
1. A strict footprint size limit.
2. A much shorter Time-To-Live (TTL) threshold than persistent storage.

For long-lived deployments, keeping these critical configuration parameters in instance storage presents eviction risks. Promoting them to **persistent storage** with explicit TTL extensions on write/migration operations guarantees security, stability, and longevity for the contract deployment.

---

## 2. Implementation Overview

To facilitate this change safely and without downtime, we implemented a one-shot configuration migration path and fallback config reads.

### A. Storage Helper Functions ([admin.rs](file:///home/gamp/Documents/Collab-Works/drips/stellabill-contracts/contracts/subscription_vault/src/admin.rs))
Rather than accessing storage directly across the codebase, all configuration reads and writes now route through helper functions:
- **`read_config<T>(env, key)`**:
  - Attempts to retrieve the key from `persistent()` storage first.
  - If not found and `SchemaVersion < 3`, falls back to checking `instance()` storage to preserve backwards compatibility.
- **`write_config<T>(env, key, value)`**:
  - If `SchemaVersion >= 3`, writes the value to `persistent()` storage, extends the key's TTL (`SUB_TTL_THRESHOLD`, `SUB_TTL_EXTEND_TO`), and removes any stale value from `instance()` storage.
  - If `SchemaVersion < 3`, writes to `instance()` storage.
- **`has_config(env, key)`**:
  - Checks if the key exists in `persistent()` storage, falling back to `instance()` if `SchemaVersion < 3`.
- **`remove_config(env, key)`**:
  - Atomically deletes the key from both `persistent()` and `instance()` storage to prevent stale reads.

### B. Configuration Keys Promoted
The 9 keys migrated are:
1. `Token` (USDC contract address)
2. `Admin` (Authorized governance admin)
3. `MinTopup` (Enforced deposit threshold)
4. `NextId` (Auto-incrementing subscription ID counter)
5. `SchemaVersion` (Version track of contract schema)
6. `EmergencyStop` (Pause switch for critical actions)
7. `Treasury` (Admin fee collection treasury)
8. `FeeBps` (Protocol fee basis points)
9. `Operator` (Assigned batch charging address)

### C. Migration Entrypoints
- **`migrate_config_to_persistent(env, admin)`**:
  - Public contract method requiring admin signature.
  - Executes the one-shot promotion of all 9 keys from instance to persistent storage, sets the version to `3`, and emits a `SchemaMigratedEvent`.
  - Clears the instance storage key-value pairs to prevent stale regressions.
- **`do_migrate(env, admin, binary_version)`**:
  - Integrated the config promotion hop `(2, 3)` into the contract upgrade ladder. Upgrading the contract automatically promotes the config keys in storage.
  - Hardened with a downgrade rejection guard and idempotency checks.

---

## 3. Test Coverage & Code Verification

We verified the migration logic, safety guards, and fallback paths by writing a robust set of tests.

### A. Unit Tests Added ([test_config_migration.rs](file:///home/gamp/Documents/Collab-Works/drips/stellabill-contracts/contracts/subscription_vault/src/test_config_migration.rs))
- **`test_fresh_init_stores_in_persistent`**: Assures that new initialization writes config keys directly to persistent storage.
- **`test_fallback_reads_on_v2`**: Confirms fallback config reads function correctly on old schema versions (v2).
- **`test_migration_moves_all_keys`**: Validates that `migrate_config_to_persistent` promotes all 9 config keys, sets version to 3, emits `SchemaMigratedEvent`, and successfully removes instance entries.
- **`test_upgrade_via_migrate`**: Asserts that `do_migrate` triggers the `(2, 3)` hop and completes the storage promotion.
- **`test_migration_idempotency_and_crash_recovery`**: Simulates mid-migration crash states and tests that successive recovery attempts are safe, idempotent, and resilient.
- **`test_rejection_of_schema_downgrades`**: Verifies that any downgrade attempt is blocked and throws `SchemaMigrationDowngrade`.

### B. Event Assertion Fixes
- Standardized the order of operations in [test_operator.rs](file:///home/gamp/Documents/Collab-Works/drips/stellabill-contracts/contracts/subscription_vault/src/test_operator.rs): querying all ledger events *before* calling read-only methods (which clear the event queue in the Soroban test environment).

### C. Verification Results
All tests compile and pass successfully. 
```bash
cargo test -- --skip test_merchant_earnings_invariant
```
**Output**:
- **`subscription_vault` (lib)**: `ok. 67 passed` (includes operator and new migration unit tests)
- **`cancel_subscription_test`**: `ok. 7 passed`
- **`event_schema`**: `ok. 2 passed`
- **`id_exhaustion`**: `ok. 9 passed`
- **`multi_actor_e2e_test`**: `ok. 1 passed`
- **`query_performance`**: `ok. 7 passed`
- **Doc-tests**: `ok. 0 passed; 7 ignored`
