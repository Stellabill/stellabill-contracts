## feat: define upgrade-safe storage layout and versioning for subscription vault

Closes #22

### Summary

Refactors all storage usage from ad-hoc `Symbol` strings and bare `u32` keys to a canonical `DataKey` enum with explicit discriminant ordering. Introduces on-chain schema versioning and an admin-gated migration entrypoint.

### Changes

**Storage key unification** — Every storage key now flows through `DataKey`, eliminating the risk of key collisions between config values and subscription IDs. All 9 variants have fixed discriminants documented in code.

**Schema versioning** — `STORAGE_VERSION` constant (currently `1`) is written to storage during `init()`. A new `get_storage_version()` entrypoint lets external tooling verify the schema. Pre-versioning contracts return `0`.

**Admin migration** — `admin_migrate(admin, from_version)` handles the v0→v1 transition: re-keys subscriptions from bare `u32` to `DataKey::Sub(u32)`, migrates the `next_id` counter, and stamps the version. Idempotent and admin-only.

**Overflow protection** — `next_id()` now uses `checked_add` and returns `Err(Overflow)` instead of panicking at `u32::MAX`.

**Pre-existing batch test fix** — `setup_batch_env` and 16 inline batch test setups were using `Address::generate()` as a fake token, causing `deposit_funds` to fail on token transfer. Replaced with `register_stellar_asset_contract_v2` + `mint`. This fixes 17 previously-failing tests.

### Files changed

| File                             | What                                                                                           |
| -------------------------------- | ---------------------------------------------------------------------------------------------- |
| `types.rs`                       | `DataKey` enum (9 variants), `STORAGE_VERSION`, upgrade-sensitive field docs on `Subscription` |
| `admin.rs`                       | `DataKey::Token/Admin/MinTopup/SchemaVersion` usage, version written on init                   |
| `subscription.rs`                | `DataKey::Sub/NextId/Token/MerchantSubs`, `checked_add` overflow guard                         |
| `charge_core.rs`                 | `DataKey::Sub/ChargedPeriod/IdemKey`                                                           |
| `queries.rs`                     | `DataKey::Sub/NextId/MerchantSubs`                                                             |
| `lib.rs`                         | `get_storage_version()`, `admin_migrate()`, removed dead `_next_id` helper                     |
| `test.rs`                        | 13 new storage/versioning tests, fixed 17 batch tests (real token setup)                       |
| `docs/storage_layout_upgrade.md` | Key registry, migration procedure, compatibility rules                                         |

### Test results

```
test result: ok. 145 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

**Build proof:**

<!-- Paste your screenshot here showing `cargo test -p subscription_vault` output -->

![Build proof](attachment)

### How to get the attachment

Run this in your terminal and take a screenshot of the output:

```bash
cargo test -p subscription_vault 2>&1 | tail -5
```

On Linux, use `gnome-screenshot`, `flameshot`, or `Shift+PrintScreen` to capture the terminal.
Alternatively, pipe to a file and screenshot that:

```bash
cargo test -p subscription_vault 2>&1 | tail -10 > test_output.txt && cat test_output.txt
```

### Compatibility

- **Existing contracts** (v0): Call `admin_migrate(admin, 0)` once after upgrading WASM.
- **New deployments**: `init()` writes version `1` automatically.
- **Future upgrades**: Append to `DataKey` enum, increment `STORAGE_VERSION`, add migration branch.
