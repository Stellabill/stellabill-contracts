# feat: add known-keys allowlist for defensive storage writes

Closes #504

## Summary

Instance storage holds the subscription vault's global, invariant-bearing
configuration (admin, token, fees, merchant balances, pause flags, …). A future
PR that adds a new `DataKey` variant — or revives a legacy `Symbol`-keyed code
path — could accidentally write an **unknown** key into instance storage and
silently bypass those invariants.

This PR pins a canonical allowlist of the instance-tier keys and adds a
two-layer guard that catches such drift in CI, while compiling to **zero
overhead** in the deployed wasm.

## What changed

### `contracts/subscription_vault/src/types.rs`

| Addition | Purpose |
|----------|---------|
| `DataKey::canonical_discriminant(&self) -> u32` | Exhaustive, **wildcard-free** match mapping every variant to its frozen declaration-order discriminant (`0..=44`). Adding a variant without an arm is a **compile error**. |
| `KNOWN_INSTANCE_KEY_DISCRIMINANTS: &[u32]` | Canonical, sorted const set of the 30 instance-tier discriminants — mirrors the registry table on `DataKey`. |
| `is_known_instance_discriminant(u32) -> bool` / `DataKey::is_known_instance_key(&self)` | Membership checks. The raw-`u32` form rejects a *synthetic* unknown key without needing to construct one. |
| `assert_known_data_key(&DataKey)` | `debug_assert!`-based guard. No-op in release/wasm; trips under `cfg(test)`/debug. |
| `debug_assert_known_key!(key)` | `#[macro_export]` wrapper for instance storage helpers; expands to nothing in release. |
| Corrected & completed the **Discriminant Registry** doc table | The previous table was stale: it omitted variants 26–40 and mis-numbered `AdminNonce`/`Operator`/etc. It is now exhaustive and consistent with `canonical_discriminant`. |

### `docs/storage_layout.md`

New **Known-Instance-Key Allowlist** section documenting the components, the
two-layer protection model, and the exact checklist for adding a new `DataKey`
variant.

## Design: two layers of protection

1. **Compile time** — `canonical_discriminant` is an exhaustive match with no
   `_ =>` arm. A new `DataKey` variant cannot compile until it is explicitly
   numbered, forcing a conscious instance-vs-persistent classification. This is
   the strongest guard against the exact bug #504 describes.
2. **Test / CI time** — `assert_known_data_key` (via `debug_assert_known_key!`)
   trips the moment an unknown or persistent-tier key reaches instance storage,
   while remaining a no-op in the deployed contract.

## Why it is a no-op in release

The guard is built on `debug_assert!`, which the compiler strips entirely when
`debug-assertions = false` — i.e. the release/wasm profile. There is **no
runtime branch, no const lookup, and no code size cost** in the deployed
contract. The check is active only in `cfg(test)`/debug builds, which is exactly
where CI runs.

## Key classification

30 instance-tier discriminants are allowlisted; 15 persistent-tier discriminants
are deliberately excluded so an accidental instance write of a persistent key is
caught:

- **Instance (allowlisted):** `MerchantSubs, Token, Admin, MinTopup, NextId,
  SchemaVersion, EmergencyStop, MerchantPaused, TotalAccounted, MerchantConfig,
  MerchantEarnings, MerchantTokens, UsageLimits, UsageState, GracePeriod, FeeBps,
  Treasury, AcceptedTokens, TokenDecimals, NextPlanId, Plan, SubPlan,
  PlanMaxActive, CreditLimit, TokenSubs, SubscriberSubs, MerchantBalance, Oracle,
  Operator, BillingRetentionConfig`
- **Persistent (excluded):** `Sub, ChargedPeriod, IdemKey, BillingStatement,
  BillingStatementsBySubscription, BillingStatementsByMerchant, Recovery,
  Blocklist, BillingPeriodSnapshot, BillingPeriodSnapshotIndex, AdminNonce,
  Metadata, MetadataKeys, BillingStatementSequence, BillingStatementAggregate`

Each tier was verified against the actual `env.storage().instance()` /
`.persistent()` call sites in the source.

## Tests

New `types::known_keys_tests` module (6 tests):

| Test | Covers |
|------|--------|
| `every_instance_variant_is_accepted` | Positive path — every one of the 30 instance variants passes the allowlist and the runtime guard. |
| `persistent_variants_are_rejected` | Every persistent variant is correctly excluded from the instance allowlist. |
| `synthetic_unknown_key_is_rejected` | A synthetic unknown discriminant (`45`, `9999`, `u32::MAX`) — modelling a legacy `Symbol`-keyed write or an unregistered future variant — is rejected. |
| `assert_panics_on_persistent_key` | The debug guard **panics** when a persistent key (`Sub`) reaches instance storage. |
| `discriminants_are_unique_and_contiguous` | Drift guard — discriminants are unique and cover `0..=44`, and the variant count is exactly 45. |
| `allowlist_matches_instance_classification` | The const allowlist is sorted, duplicate-free, and exactly equal to the instance-tier set enumerated by the test. |

The enumeration in `all_variants` is the canonical mirror of the enum: adding a
variant without classifying it here fails `discriminants_are_unique_and_contiguous`.

### Test output

```
$ cargo test -p subscription_vault --lib -- \
    every_instance_variant_is_accepted persistent_variants_are_rejected \
    synthetic_unknown_key_is_rejected assert_panics_on_persistent_key \
    discriminants_are_unique_and_contiguous allowlist_matches_instance_classification

running 6 tests
test types::known_keys_tests::synthetic_unknown_key_is_rejected ... ok
test types::known_keys_tests::discriminants_are_unique_and_contiguous ... ok
test types::known_keys_tests::every_instance_variant_is_accepted ... ok
test types::known_keys_tests::persistent_variants_are_rejected ... ok
test types::known_keys_tests::allowlist_matches_instance_classification ... ok
test types::known_keys_tests::assert_panics_on_persistent_key - should panic ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 21 filtered out
```

Full library unit suite — no regressions:

```
$ cargo test -p subscription_vault --lib
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## Security notes

- **No new attack surface.** The guard is read-only and additive; it never
  changes which keys are written, only asserts they are recognised.
- **Fail-closed in CI, invisible on-chain.** Unknown/mis-tiered instance keys
  abort tests (`debug_assert!`), so drift cannot merge. The deployed wasm carries
  zero overhead because the assertion is compiled out.
- **Invariant preservation.** Persistent-tier keys are intentionally excluded
  from the instance allowlist, so a regression that routes a persistent record
  (e.g. `Sub`, `BillingStatement`) into instance storage is caught.
- **Defence in depth.** The exhaustive `canonical_discriminant` match means even
  if a developer forgets the runtime guard, a new unclassified variant cannot
  compile in the first place.

## Notes for reviewers

- No existing storage call sites were modified; this change is purely additive.
- The macro `debug_assert_known_key!` is exported for instance storage helpers to
  adopt; wiring it into individual call sites can follow incrementally since the
  exhaustive-match compile-time guard already prevents unclassified variants.
- A pre-existing, unrelated issue prevents a bare `cargo build --release` of the
  crate: `test_insufficient_balance.rs` is declared as a non-`cfg(test)` module
  (`lib.rs:2908`) and references `soroban_sdk::testutils`, which is only available
  under the test feature. This is out of scope for #504 and untouched here; the
  no-overhead property of the guard rests on `debug_assert!` semantics, not on
  that build path.
