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

## Events

| Event              | Topic                           | Data                                        |
|--------------------|---------------------------------|---------------------------------------------|
| MetadataSetEvent   | `("metadata_set", sub_id)`      | `{ subscription_id, key, authorizer }`      |
| MetadataDeletedEvent | `("metadata_deleted", sub_id)` | `{ subscription_id, key, authorizer }`     |

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
- **Mutable state**: Use on-chain fields for status/balance tracking

Metadata is visible on-chain to anyone who can read ledger state.
Treat all metadata values as **public and non-sensitive**.

---

## Privacy Constraints

Metadata is stored **on-chain and is permanently public**. Every key-value pair
written to ledger state is visible to any node operator, indexer, or block explorer
for the lifetime of the ledger entry. There is no encryption, access control, or
expiry at the storage layer.

### What must never be stored on-chain

The following categories of data are **strictly prohibited** as metadata values:

| Category | Examples |
|----------|---------|
| Personally Identifiable Information (PII) | Full name, email address, phone number, postal address, date of birth, national ID |
| Authentication secrets | API keys, tokens, passwords, private keys, seed phrases |
| Financial account data | Credit card numbers, bank account numbers, IBAN, CVV |
| Health or biometric data | Medical records, biometric identifiers |
| Large structured blobs | Base64-encoded images, JSON payloads, XML documents |
| Mutable operational state | Subscription status, balance, charge timestamps (use on-chain fields) |

Storing any of the above violates user privacy, may breach data-protection
regulations (GDPR, CCPA, etc.), and cannot be undone once written to ledger.

### Allowlist of permitted metadata fields

Only use metadata for **opaque, non-sensitive off-chain references**. The following
fields are explicitly approved:

| Key | Max value length | Purpose |
|-----|-----------------|---------|
| `invoice_id` | 64 bytes | Link to billing system invoice (e.g. `INV-2025-001`) |
| `customer_id` | 64 bytes | External customer reference (opaque ID, not name/email) |
| `external_ref` | 64 bytes | Cross-system subscription ID (e.g. `stripe_sub_xyz`) |
| `campaign_tag` | 32 bytes | Marketing campaign attribution tag |
| `plan_name` | 64 bytes | Human-readable plan label (non-PII) |
| `merchant_ref` | 64 bytes | Merchant-side reference ID |
| `note` | 128 bytes | Short operational note (must not contain PII) |

All values must stay within the global 256-byte value limit. Keys must be ≤ 32 bytes.
Values outside this allowlist require explicit review before use.

### Encoding rules

- Values must be valid UTF-8 strings.
- Prefer ASCII-only identifiers for keys and reference IDs.
- Do not embed binary data, null bytes, or control characters in values.
- Do not use values as a side-channel for structured data (no JSON, no CSV).

### Enforcement

Size limits are enforced on-chain by `metadata.rs`:

- `key.len() > MAX_METADATA_KEY_LENGTH (32)` → `Error::MetadataKeyTooLong (#1024)`
- `value.len() > MAX_METADATA_VALUE_LENGTH (256)` → `Error::MetadataValueTooLong (#1025)`
- More than `MAX_METADATA_KEYS (10)` distinct keys → `Error::MetadataKeyLimitReached (#1023)`

Content-level privacy (e.g. detecting an email address in a value) cannot be
enforced on-chain. Enforcement of the allowlist and prohibited-content rules is
the responsibility of the **off-chain caller** and is verified during code review
using the checklist below.

---

## Reviewer Checklist

When reviewing any PR that touches metadata (reads, writes, or documentation):

- [ ] No PII (names, emails, phone numbers, addresses) appears in metadata values in tests or examples.
- [ ] No secrets (API keys, tokens, passwords) appear in metadata values in tests or examples.
- [ ] All metadata keys used in tests and examples are from the approved allowlist above, or are clearly labelled as synthetic test data.
- [ ] Value lengths in tests exercise the boundary (exactly 256 bytes) and over-boundary (257 bytes) cases.
- [ ] Key lengths in tests exercise the boundary (exactly 32 bytes) and over-boundary (33 bytes) cases.
- [ ] The 10-key limit is tested: filling to 10 keys, attempting an 11th, and verifying the error.
- [ ] Empty-key rejection is tested (`Error::MetadataKeyTooLong`).
- [ ] Unauthorized-caller rejection is tested (`Error::Forbidden`).
- [ ] Setting metadata on a cancelled subscription is tested (`Error::NotActive`).
- [ ] No metadata operation modifies financial state (balance, status, charge timestamps).
- [ ] Documentation updated if new fields are added to the allowlist.
