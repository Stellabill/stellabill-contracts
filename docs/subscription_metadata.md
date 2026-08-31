# Subscription Metadata Key-Value Store

## Overview

Each subscription can hold a bounded set of metadata key-value pairs for referencing
off-chain objects such as invoice IDs, customer IDs, or campaign tags. Metadata
operations never affect financial state (balances, statuses, charges).

## Limits

| Constraint                | Value |
|---------------------------|-------|
| Max keys per subscription | 10    |
| Max key length (bytes)    | 32    |
| Max value length (bytes)  | 256   |

These limits are enforced on-chain to prevent storage bloat. Length is measured in
**bytes** (not Unicode characters), consistent with Soroban `String` semantics.

## Authorization

Only the **subscriber** or the **merchant** of a subscription may set, update,
or delete metadata. Unauthorized callers receive `Error::Forbidden` (4002 → code 1002
maps to Forbidden; see [Error Codes](#error-codes)).

Setting metadata is blocked on **Cancelled** subscriptions (`Error::NotActive`).
Deleting metadata is allowed on cancelled subscriptions to permit cleanup.

## Entrypoints

### `set_metadata(env, subscription_id, authorizer, key, value)`

Set or update a metadata key-value pair.

- Requires `authorizer` auth (`require_auth`). The `authorizer` must be the
  subscription's `subscriber` or `merchant`.
- If the key already exists, the value is overwritten; no additional key slot is
  consumed and the key-cap check is bypassed.
- Blocked when the subscription status is `Cancelled` (`Error::NotActive`).
- Empty key (0 bytes) or key longer than 32 bytes: `Error::MetadataKeyTooLong`.
- Value longer than 256 bytes: `Error::MetadataValueTooLong`.
- Adding a new key when 10 are already set: `Error::MetadataKeyLimitReached`.
- Emits `MetadataSetEvent`.

### `delete_metadata(env, subscription_id, authorizer, key)`

Remove a metadata key and its value. Frees a key slot.

- Requires `authorizer` auth. The `authorizer` must be the subscription's
  `subscriber` or `merchant`.
- Allowed on all subscription statuses, including `Cancelled`.
- Returns `Error::NotFound` if the key does not exist.
- Emits `MetadataDeletedEvent` on success.

### `get_metadata(env, subscription_id, key) -> String`

Read a metadata value.

- Returns `Error::NotFound` if the key does not exist.
- No authorization required (read-only).

### `list_metadata_keys(env, subscription_id) -> Vec<String>`

List all metadata keys for a subscription.

- Returns an empty `Vec` if no keys are set.
- No authorization required (read-only).

### `set_metadata_signed(env, signer_pubkey, payload, signature)`

Apply an off-chain signed metadata update. Designed for batching workflows where
a merchant (or any party running an off-chain signer) pre-signs N
`(key, value)` mutations and submits them as individual transactions without
paying one `require_auth` fee per change.

**Auth flow:**

1. `subscription_id` is looked up. `Error::NotFound` if it does not exist.
2. Key/value lengths are validated before the signature check (clean typed
   errors instead of a mid-crypto panic).
3. The supplied ed25519 signature is verified against the canonical byte
   string defined below. A forged signature aborts the transaction
   (host-level panic by design — no typed-error downgrade).
4. The signer public key is mapped to a Soroban `Address` via its XDR
   `AccountId`. The resulting address must equal `sub.subscriber` or
   `sub.merchant`; otherwise `Error::Forbidden`.
5. `now < payload.expires_at` (strict); otherwise `Error::InvalidInput`.
6. `payload.nonce` is consumed against
   `(signer, DOMAIN_METADATA_SIGNED = 3)` via the same reuse-guard used by
   admin nonces. Captured payloads are rejected with
   `Error::NonceAlreadyUsed`. A side `nonce_consumed` event is emitted.
7. The same key/value length and key-cap invariants from `set_metadata`
   are re-enforced and the metadata storage slot is updated.
8. `metadata_set_signed` topic emits `MetadataSetSignedEvent`.

> **Note:** an empty key (0 bytes) or whitespace-only key sent via the
> signed path is rejected with `Error::InvalidInput` (the ABI boundary guard
> runs before signature verification). A 33-byte key is rejected with
> `Error::MetadataKeyTooLong` (caught inside `apply_metadata_value`).

**Canonical message** (the bytes the off-chain signer MUST hash and sign):

```text
[32-byte domain tag "SBL_META_SIGNED_v1\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00"]
|| u32_be(subscription_id)
|| u32_be(key.len())   || key_bytes
|| u32_be(value.len()) || value_bytes
|| u64_be(nonce)
|| u32_be(network_id.len()) || network_id_bytes   (env.ledger().network_id())
|| u64_be(expires_at)
```

The 32-byte domain tag and per-field length prefixes make the encoding
unambiguous: two distinct payloads can never collide, and no struct field
boundary can be confused with another. The network id is mixed in so a
cross-chain replay of the same `(sub, signer, nonce, payload)` is
rejected by `ed25519_verify` on a different network's reconstructed message.

**Replay protection:** the off-chain signer fetches
`get_metadata_signed_nonce(signer_address)` first to learn the next nonce,
signs the payload with that nonce, and submits. The contract consumes
`(signer, DOMAIN_METADATA_SIGNED)` so captured payloads are detected.

**Expiry:** `expires_at` should be set comfortably after the expected
submission window (e.g. `now + max(2 * interval, 3600)`). `now >= expires_at`
is rejected.

### `get_metadata_signed_nonce(env, signer) -> u64`

Read-only. Returns the next-expected nonce the off-chain signer must use when
signing the next `SignedMetadataPayload` for that signer against
`DOMAIN_METADATA_SIGNED`. Returns `0` for a signer's first signed update.

### `delete_metadata_signed` (planned, not yet implemented)

A symmetric `delete_metadata_signed` entrypoint will be added in a follow-up
so batchers can also remove keys off-chain. It will share the same nonce
domain, expiry, and identity checks.

## Events

| Event                   | Topic                              | Data fields                                           |
|-------------------------|------------------------------------|-------------------------------------------------------|
| `MetadataSetEvent`      | `("metadata_set", sub_id)`         | `subscription_id, key, authorizer, schema_version`    |
| `MetadataDeletedEvent`  | `("metadata_deleted", sub_id)`     | `subscription_id, key, authorizer, schema_version`    |
| `MetadataSetSignedEvent`| `("metadata_set_signed", sub_id)`  | `subscription_id, key, signer, nonce, timestamp, schema_version` |

The `metadata_set` vs `metadata_set_signed` topic split lets indexers and audit
pipelines attribute auth (on-chain `require_auth` vs off-chain ed25519) without
ambiguity.

## Error Codes

| Error                    | Code | Condition                                             |
|--------------------------|------|-------------------------------------------------------|
| `MetadataKeyTooLong`     | 3005 | Key is empty (0 bytes) or exceeds 32 bytes            |
| `MetadataValueTooLong`   | 3006 | Value exceeds 256 bytes                               |
| `MetadataKeyLimitReached`| 6005 | Adding a new key would exceed the 10-key cap          |
| `Forbidden`              | 1002 | Caller is not the subscription's subscriber or merchant |
| `NotActive`              | 4002 | `set_metadata` called on a `Cancelled` subscription   |
| `NotFound`               | 2001 | Key or subscription does not exist                    |
| `InvalidInput`           | 3002 | Empty/whitespace key or value (signed path ABI guard) |

## Storage Schema

Metadata is stored in **persistent** storage using composite `DataKey` variants:

| DataKey variant               | Value type    | Purpose                            |
|-------------------------------|---------------|------------------------------------|
| `MetadataKeys(subscription_id)` | `Vec<String>` | Ordered list of active key names   |
| `Metadata(subscription_id, key)` | `String`    | Value for a single key             |

Storage is bounded: at most 10 keys per subscription, each key ≤ 32 bytes, each
value ≤ 256 bytes. Worst-case per-subscription footprint: ~2.9 KB.

Metadata TTL follows the subscription's persistent-storage TTL policy: extended
to `SUB_TTL_EXTEND_TO` (365 days) when the record is read or updated and the
remaining TTL is below `SUB_TTL_THRESHOLD` (30 days).

## Data Isolation

Each subscription's metadata is completely independent. A key named `invoice_id`
on subscription `42` has no relation to the same key on subscription `43`. The
composite key `Metadata(subscription_id, key)` guarantees per-subscription
isolation at the storage layer.

## Behavioral Invariants

1. **Key-cap**: `list_metadata_keys(id).len()` is always `<= MAX_METADATA_KEYS (10)`.
2. **Idempotency**: `set_metadata` on an existing key is a pure update — the key
   list length does not change and the cap check is skipped.
3. **Delete-then-re-add**: deleting a key frees one slot; a subsequent `set_metadata`
   with the same (or a new) key succeeds up to the cap.
4. **Financial isolation**: metadata operations never touch `prepaid_balance`,
   `lifetime_charged`, or any charge/transfer logic.
5. **Cancelled cleanup**: `delete_metadata` is intentionally permitted on
   `Cancelled` subscriptions so callers can free storage.

## Schema Recommendations (off-chain)

- Prefer short ASCII keys (e.g. `invoice_id`, `external_ref`) so they stay within
  the 32-byte limit and remain easy to query in indexers.
- Values should be opaque identifiers or tags, not structured blobs; keep under
  256 bytes so updates stay cheap.
- Treat keys as **case-sensitive**; normalize casing off-chain to avoid
  duplicate-looking keys (`INV` vs `inv`).
- After deleting optional keys you may re-add up to the 10-key cap; updates to
  an existing key do not consume a new slot.

## Recommended Fields

| Key            | Example Value     | Purpose                            |
|----------------|-------------------|------------------------------------|
| `invoice_id`   | `INV-2025-001`    | Link to billing system invoice     |
| `customer_id`  | `cust_abc123`     | External customer reference        |
| `campaign_tag` | `q1_promo`        | Marketing campaign attribution     |
| `plan_name`    | `Pro Monthly`     | Human-readable plan label          |
| `external_ref` | `stripe_sub_xyz`  | Cross-system subscription ID       |

## Anti-Patterns (Do NOT Store)

- **PII**: Names, emails, phone numbers, addresses
- **Secrets**: API keys, tokens, passwords
- **Large blobs**: Base64 images, documents, JSON payloads
- **Financial data**: Credit card numbers, bank accounts
- **Mutable state**: Use on-chain fields for status/balance tracking

> Metadata is visible on-chain to anyone who can read ledger state.
> Treat all metadata values as **public and non-sensitive**.

## Security Model

The signed off-chain path (`set_metadata_signed`) preserves the same security
guarantees as on-chain `set_metadata` while removing the per-key transaction
overhead. Each attack vector and its defence:

| Attack vector | Defence | Rejection surface |
|---|---|---|
| Forged ed25519 signature | `env.crypto().ed25519_verify` over canonical bytes | **Host panic** (no typed-error downgrade) |
| Wrong-key signature | Same — host panic | Host panic |
| Cross-network replay | `network_id` mixed into the canonical message bytes | Host panic |
| Same-chain replay | Nonce consumed on `(signer, DOMAIN_METADATA_SIGNED = 3)` via `nonce::check_and_advance` | `Error::NonceAlreadyUsed` (1005) |
| Out-of-order nonce | Strict `expected == provided` check — no skipping | `Error::NonceAlreadyUsed` (1005) |
| Expired payload (`now >= expires_at`) | Strict-rejection at expiry check | `Error::InvalidInput` (3002) |
| Cross-domain replay (signed → admin batch/rotate) | Domain `3` is baked into the storage key; admin domains are 0/1/2 | `Error::NonceAlreadyUsed` (1005) |
| Signer is neither subscriber nor merchant | Derived `Address` compared to `sub.subscriber` / `sub.merchant` | `Error::Forbidden` (1002) |
| Empty / whitespace key (signed path) | `validation::reject_empty_string` ABI guard runs before crypto | `Error::InvalidInput` (3002) |
| Empty / oversized key (on-chain path) | Length check `(0, 32]` in `apply_metadata_value` | `Error::MetadataKeyTooLong` (3005) |
| Oversized value | Length check `<= 256` in `apply_metadata_value` | `Error::MetadataValueTooLong` (3006) |
| Key-cap overflow (> 10 keys) | Per-subscription 10-key cap checked before new-key insertion | `Error::MetadataKeyLimitReached` (6005) |
| Counter overflow (`u64::MAX`) | `checked_add` in `nonce::check_and_advance` returns `Err` | `Error::Overflow` (5005) |
| Unknown `subscription_id` | `get_subscription` returns `NotFound` before any crypto or storage write | `Error::NotFound` (2001) |
| `set_metadata` on `Cancelled` subscription | Status checked after auth, before storage write | `Error::NotActive` (4002) |

### Auth-path coupling soundness

The off-chain check derives `Address` from `signer_pubkey` via its XDR `AccountId`
representation. This matches the same `AccountId` bytes that Soroban's on-chain
`require_auth` decodes from a Strkey-encoded public key. That is the **one implicit
invariant** tying the two paths together; if Soroban ever introduces an Address
variant whose on-chain and off-chain derivations disagree, this equality check must
be revisited.

### Out-of-scope

- An all-zero ed25519 public key still verifies if the same all-zero key is the
  signer. We do not block this; the contract already refuses to act on a signature
  that the on-chain path would not accept for the same identity pair.
- A compromised subscriber or merchant secret key can move metadata on every
  subscription that key is party to via either path. The environment cannot fix
  this in-protocol.
