# Serialization Guarantees: Subscription

This document records the stability expectations for how the `Subscription` user-defined type (UDT) is serialized and stored by Soroban.

- Encoding format: Soroban encodes named-field structs annotated with `#[contracttype]` as an XDR `ScVal` containing an `ScMap` keyed by the field names. See Soroban docs “Custom types” for details.
- Stability invariant: Changing any of the following alters the encoded bytes for every instance and must be treated as a storage-breaking change:
  - Field names (e.g., renaming `subscriber`, `merchant`, `status`, etc.)
  - Field types (e.g., changing `amount: i128` to `u128`)
  - Enum representation (e.g., reordering or renumbering `SubscriptionStatus`)
  - Adding or removing fields

## Upgrade-Safe Layout Guidance

- Additive changes: Prefer adding new fields as `Option<T>` with conservative defaults, appended at the end of the struct in source. This still changes the encoded ScMap (new keys appear) and therefore the bytes differ from previous versions.
- Migration plan: When introducing new optional fields you should:
  - Bump contract version and migration docs as needed.
  - Add new golden vectors for the new version and keep the old vectors for reference as long as needed.
  - Provide code-paths that initialize new fields for existing data as appropriate.

## Test Coverage

The test suite exercises:
- Round-trip encode/decode for `Subscription` across all `SubscriptionStatus` values and `usage_enabled` combinations.
- Golden test vectors for a canonical `Subscription` sample, designed to detect unintended changes to the encoded form.
- A negative test ensuring corrupted bytes are rejected.
- A compatibility guard demonstrating that adding optional fields changes the encoded bytes (and therefore requires deliberate versioning decisions).

Run:

```bash
cargo test -p subscription_vault
```

Golden vectors intentionally break when serialization-affecting changes are introduced, forcing a conscious update with review.
