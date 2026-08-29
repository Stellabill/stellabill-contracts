# `DataKey` feature groups

`contracts/subscription_vault/src/types.rs` holds a single flat `DataKey` enum.
Its variant **declaration order is the on-chain wire format** — the Soroban
`#[contracttype]` macro serialises each variant by its 0-indexed position — so
the enum doubles as a canonical discriminant registry that must never be
reordered or thinned.

That makes the enum hard to browse but unsafe to physically split. This document
and `contracts/subscription_vault/tests/data_key_feature_groups.rs` give the
readability win **without** moving a single variant:

| Feature group | Sub-enum | What it covers |
| --- | --- | --- |
| Subscription | `SubscriptionKey` | Per-subscription lifecycle, replay/idempotency, metering, metadata, per-subscriber caps and escrow |
| Merchant | `MerchantKey` | Merchant config, earnings and balances, payout schedule, multi-sig, compliance tags, fee overrides |
| Governance | `GovernanceKey` | Admin/operator authority, protocol fee and treasury policy, guardian proposals, emergency stop, schema version |

## How drift is prevented

Each sub-enum is `#[repr(u32)]` and each variant carries the **frozen**
discriminant of its `DataKey` arm. Every variant also knows how to wrap itself
back into a real `DataKey`. The test module then asserts, for every variant, that

```text
DataKey::<arm>.canonical_discriminant() == <SubEnum>::<Variant> as u32
```

On top of that it asserts:

- the three groups are pairwise disjoint;
- the sorted union of all grouped discriminants equals a longhand frozen
  snapshot array, so any renumbering shows up as an explicit diff;
- every merchant-group key is still on the instance-tier allowlist
  (`KNOWN_INSTANCE_KEY_DISCRIMINANTS`);
- per-subscription record keys stay **off** that allowlist, catching an
  accidental storage-tier move;
- an empty variant set is handled without panicking.

## Scope notes

- **Nothing existing is modified.** No variant, discriminant, storage tier,
  entrypoint, or existing test is touched, which is what guarantees the
  encoding order is preserved for this change.
- **`TransferIntent` is deliberately ungrouped.** The registry currently reports
  discriminant `54` for both `PendingTreasuryChange` and `TransferIntent`. Only
  `PendingTreasuryChange` is grouped, so the disjointness assertion states a
  true invariant. Resolving that collision changes live storage semantics and is
  a maintainer decision, not something this PR should quietly rewrite.
- Arms whose payloads cannot be built from a bare `Env` without unrelated test
  fixtures are left for a follow-up.
