# Migration Golden Snapshots

This directory contains deterministic, hex-encoded golden fixtures for cross-version contract snapshot regression testing.

## File Format

Each golden fixture is named `{fixture_name}.scval.hex` and contains the hex-encoded XDR serialization of a Soroban contract snapshot. The format is stable across contract versions and builds.

## Files

- `contract_snapshot_v2.scval.hex` — Golden fixture for the initial contract configuration (admin, token, version info)
- `subscription_summary_v2.scval.hex` — Golden fixture for a single subscription export
- `paginated_export_v2.scval.hex` — Golden fixture for paginated subscription list export

## Updating Fixtures

To regenerate the golden fixtures after intentional breaking changes:

```bash
cargo test -- --ignored update_goldens
```

This will overwrite the existing fixtures with current serialization.

**Warning:** Only run `update_goldens` when:
1. You have intentionally changed the contract data types
2. You have reviewed and approved the changes in the migration review process
3. You understand the implications for cross-version compatibility

## Validation

Normal regression tests compare current serialization against the stored golden fixtures:

```bash
cargo test --lib --test migration_goldens
```

If a test fails, it means either:
- The contract serialization is non-deterministic (a bug)
- The contract has changed in an unintended way
- The fixtures need updating (if the change was intentional)

## Version Suffix

The `_v2` suffix indicates these fixtures are for storage schema version 2. If the contract is upgraded to schema v3, new fixtures should be created as `{fixture}_v3.scval.hex`.
