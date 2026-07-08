# Subscription Metadata Key-Value Store

## Overview

Each subscription can hold a bounded set of metadata key-value pairs for referencing
off-chain objects such as invoice IDs, customer IDs, or campaign tags. Metadata
operations never affect financial state (balances, statuses, charges).

## Limits

| Constraint            | Value |
|-----------------------|-------|
| Max keys per subscription | 10    |
| Max key length (bytes)    | 32    |
| Max value length (bytes)  | 256   |

These limits are enforced on-chain to prevent storage bloat.

## Authorization

Only the **subscriber** or the **merchant** of a subscription may set, update,
or delete metadata. Unauthorized callers receive `Error::Forbidden (403)`.

Setting metadata is blocked on **Cancelled** subscriptions (`Error::NotActive`).
Deleting metadata is allowed on cancelled subscriptions to permit cleanup.

## Entrypoints

### `set_metadata(subscription_id, authorizer, key, value)`

Set or update a metadata key-value pair. If the key already exists, the value is
overwritten (no additional key slot consumed). Emits `MetadataSetEvent`.

### `delete_metadata(subscription_id, authorizer, key)`

Remove a metadata key and its value. Frees a key slot. Returns `Error::NotFound`
if the key does not exist. Emits `MetadataDeletedEvent`.

### `get_metadata(subscription_id, key) -> String`

Read a metadata value. Returns `Error::NotFound` if the key does not exist.
No authorization required (read-only).

### `list_metadata_keys(subscription_id) -> Vec<String>`

List all metadata keys for a subscription. Returns an empty vector if none are set.
No authorization required (read-only).

### `set_metadata_signed(signer_pubkey, payload, signature)`

Apply an off-chain signed metadata update. Designed for batching workflows where
a merchant (or any party running an off-chain signer) pre-signs N
`(key, value)` mutations and submits them as individual transactions without
paying one `require_auth` fee per change.

**Auth flow:**

1. `subscription_id` is looked up. NotFound if it does not exist.
2. The supplied ed25519 signature is verified against the canonical byte
   string defined below. A forged signature aborts the transaction
   (`panic` at the host crypto boundary by design — no typed-error downgrade).
3. The signer public key is mapped to a Soroban Address via
   `Address::from_account`. The resulting address must equal
   `sub.subscriber` or `sub.merchant`; otherwise `Error::Forbidden`.
4. `now < payload.expires_at` (strict); otherwise `Error::InvalidInput`.
5. `payload.nonce` is consumed against
   `(signer, DOMAIN_METADATA_SIGNED = 3)` via the same reuse-guard used by
   admin nonces. Captured payloads are rejected with
   `Error::NonceAlreadyUsed`. A side `nonce_consumed` event is emitted.
6. The same key/value length and key-cap invariants from `set_metadata`
   are re-enforced and the metadata storage slot is updated.
7. `metadata_set_signed` topic emits `MetadataSetSignedEvent`.

**Canonical message** (the bytes the off-chain signer MUST hash and sign):

```
[32-byte domain tag "SBL_META_SIGNED_v1\x00..."]
|| u32_be(subscription_id)
|| u32_be(key.len())  || key_bytes
|| u32_be(value.len())|| value_bytes
|| u64_be(nonce)
|| u32_be(chain_id.len()) || chain_id_bytes   (env.ledger().chain_id())
|| u64_be(expires_at)
```

The 32-byte domain tag and per-field length prefixes make the encoding
unambiguous: two distinct payloads can never collide and no struct field
boundary can be confused with another. The chain id is mixed in so a
cross-chain replay of the same `(sub, signer, nonce, payload)` is
rejected by ed25519_verify on the wrong chain id's reconstructed
message.

**Replay protection:** the off-chain signer fetches
`get_metadata_signed_nonce(signer_address)` first to learn the next
nonce, signs the payload with that nonce, and submits. The contract
consumes `(signer, DOMAIN_METADATA_SIGNED)` so captured payloads are
detected.

**Expiry:** `expires_at` should be set comfortably after the expected
submission window. Pick e.g. `now + max(2 * interval, 1 hour)`.
`now >= expires_at` is rejected.

### `get_metadata_signed_nonce(signer) -> u64`

Read-only. Returns the next-expected nonce the off-chain signer must use
when signing the next [`SignedMetadataPayload`] for that signer against
`DOMAIN_METADATA_SIGNED`. Returns `0` on a signer's first signed update.

### `delete_metadata_signed` (planned, not yet implemented)

A symmetric `delete_metadata_signed` entrypoint will be added in a
follow-up so batchers can also remove keys off-chain. It will share the
same nonce domain, expiry, and identity checks.

## Events

| Event                   | Topic                                | Data                                                 |
|-------------------------|--------------------------------------|------------------------------------------------------|
| MetadataSetEvent        | `("metadata_set", sub_id)`           | `{ subscription_id, key, authorizer }`               |
| MetadataDeletedEvent    | `("metadata_deleted", sub_id)`       | `{ subscription_id, key, authorizer }`               |
| MetadataSetSignedEvent  | `("metadata_set_signed", sub_id)`    | `{ subscription_id, key, signer, nonce, timestamp }` |

The `metadata_set` vs `metadata_set_signed` topic split lets indexers
and audit pipelines attribute auth (on-chain `require_auth` vs
off-chain ed25519) without ambiguity.

## Error Codes

| Error                    | Code | Condition                                    |
|--------------------------|------|----------------------------------------------|
| MetadataKeyLimitReached  | 1023 | Adding a new key would exceed the 10-key cap |
| MetadataKeyTooLong       | 1024 | Key is empty or exceeds 32 bytes             |
| MetadataValueTooLong     | 1025 | Value exceeds 256 bytes                      |
| Forbidden                | 403  | Caller is not subscriber or merchant         |
| NotActive                | 1002 | Subscription is cancelled (for set only)     |
| NotFound                 | 404  | Key or subscription does not exist           |

## Storage Schema

Metadata is stored in instance storage using composite keys:

- **Key list**: `(Symbol("mk"), subscription_id: u32)` -> `Vec<String>`
- **Values**: `(Symbol("mv"), subscription_id: u32, key: String)` -> `String`

Storage is bounded: at most 10 keys per subscription, with each key <= 32 bytes
and each value <= 256 bytes. Worst-case per subscription: ~3 KB.

## Schema recommendations (off-chain)

- Prefer short ASCII keys (e.g. `invoice_id`, `external_ref`) so they stay within the 32-byte key limit and remain easy to query in indexers.
- Values should be opaque identifiers or tags, not structured blobs; keep under 256 bytes so updates stay cheap.
- Treat keys as case-sensitive; normalize casing off-chain to avoid duplicate-looking keys (`INV` vs `inv`).
- After deleting optional keys, you may re-add up to the 10-key cap; updates to an existing key do not consume a new slot.

## Recommended Fields

Use metadata for lightweight off-chain references:

| Key              | Example Value         | Purpose                          |
|------------------|-----------------------|----------------------------------|
| `invoice_id`     | `INV-2025-001`        | Link to billing system invoice   |
| `customer_id`    | `cust_abc123`         | External customer reference      |
| `campaign_tag`   | `q1_promo`            | Marketing campaign attribution   |
| `plan_name`      | `Pro Monthly`         | Human-readable plan label        |
| `external_ref`   | `stripe_sub_xyz`      | Cross-system subscription ID     |

## Anti-Patterns (Do NOT Store)

- **PII**: Names, emails, phone numbers, addresses
- **Secrets**: API keys, tokens, passwords
- **Large blobs**: Base64 images, documents, JSON payloads
- **Financial data**: Credit card numbers, bank accounts
- **Mutable state**: Use on-chain fields for status/balance trackingMetadata is visible on-chain to anyone who can read ledger state. Treat all metadata values as **public and non-sensitive**.

## Security Model

The signed off-chain path (`set_metadata_signed`) preserves the same security guarantees as on-chain `set_metadata` while removing the per-key transaction overhead. Each attack vector and its defence:

| Attack vector | Defence | Rejection surface |
| --- | --- | --- |
| Forged ed25519 signature | `env.crypto().ed25519_verify` over canonical bytes | **Host panic** (no typed-error downgrade) |
| Wrong-key signature | Same — host panic | Host panic |
| Cross-chain replay | `chain_id` mixed into the canonical message bytes | Host panic |
| Same-chain replay | Nonce consumed on `(signer, DOMAIN_METADATA_SIGNED = 3)` via `nonce::check_and_advance` | `Error::NonceAlreadyUsed` (1005) |
| Out-of-order nonce | Strict `expected == stored` check — no skipping | `Error::NonceAlreadyUsed` |
| Expired payload (`now >= expires_at`) | Strict-rejection at signature check time | `Error::InvalidInput` (3002) |
| Cross-domain replay (signed → admin batch/rotate) | Domain tag `3` is part of the storage key; admin domains are 0/1 | `Error::NonceAlreadyUsed` |
| Signer is neither subscriber nor merchant | `Address::from_account(pubkey)` compared to `sub.subscriber` / `sub.merchant` | `Error::Forbidden` (1002) |
| Empty / whitespace key or value | `validation::reject_empty_string` ABI guard runs before crypto call | `Error::InvalidInput` |
| Key-cap overflow (> 10 keys per subscription) | Same per-subscription 10-key cap as the on-chain path | `Error::MetadataKeyLimitReached` (6005) |
| Counter overflow (`u64::MAX`) | `checked_add` in `nonce::check_and_advance` returns `Err` instead of wrapping | `Error::Overflow` (5005) |
| Unknown `subscription_id` | `get_subscription` returns `NotFound` before any crypto or storage write | `Error::NotFound` (2001) |

### Auth-path coupling soundness

The off-chain check `Address::from_account(env, &signer_pubkey) == sub.subscriber` only holds because Soroban Strkey-decodes the on-chain account address to the same underlying `AccountId` bytes. That is the **one implicit invariant** tying the two paths together; if Soroban ever introduces an Address variant whose on-chain and off-chain derivations disagree, this equality check must be revisited.

### Out-of-scope

- An all-zero ed25519 public key still verifies if the same all-zero key is the *signer*. We do not block this; doing so would require an explicit black-list and the contract already refuses to act on a signature the on-chain path wouldn't accept for the same identity pair.
- A compromised subscriber secret key can move metadata on every subscription the key is party to via either path. The environment cannot fix this in-protocol.


# Soulbound Credential System

## Overview

Each subscription automatically receives an on-chain credential badge upon creation. This credential acts as a permanent historical record of the subscription and is explicitly **soulbound** (non-transferable). There is no entrypoint in the ABI to transfer the credential to another subscriber.

## Lifecycle

- **Automatic Issuance**: When a subscription is successfully created via `do_create_subscription`, a `CredentialBadge` is automatically generated and stored on-chain. This credential has an initial `tier` of 1 and a `revoked` status of `false`. A `CredentialIssuedEvent` is emitted.
- **Automatic Revocation**: When a subscription is cancelled via `do_cancel_subscription`, the credential is automatically marked as `revoked = true`. The credential is not deleted from storage, preserving the historical record. A `CredentialRevokedEvent` is emitted.
- **Manual Revocation**: An authorized merchant or contract admin can manually revoke an active credential via the `revoke_credential` entrypoint. This sets `revoked = true` idempotently without deleting the record, emitting the `CredentialRevokedEvent`.

## Query Methods

- `get_credential(subscription_id)`: Returns the `CredentialBadge` struct if it exists. Returns `Error::NotFound` otherwise.
- `is_credential_active(subscription_id)`: Returns a boolean indicating whether the credential exists and is active (i.e. `revoked == false`).

## Storage Structure

Credentials are stored in persistent storage using a dedicated data key:
- **Key**: `DataKey::Credential(subscription_id: u32)` (Discriminant: 49)
- **Value**: `CredentialBadge { subscription_id: u32, tier: u32, issued_at: u64, revoked: bool }`

## Security Rationale

The credential system is designed to be a permanent, append-only record mapping subscriptions to badges. 
- **Non-transferability**: By explicitly omitting any `transfer_credential` method from the ABI, credentials are strictly soulbound to the original subscriber. 
- **Idempotency**: Revocation logic is idempotent, ensuring that cancelling a subscription with an already-revoked credential does not corrupt state. 
- **Authorization**: Manual revocation strictly enforces authorization, guaranteeing only the merchant who owns the subscription or the global admin can revoke the badge.
