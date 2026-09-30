# Replay Protection and Idempotency for Charges

This document describes how the subscription vault prevents double-charging and how off-chain billing engines should integrate with it.

## Overview

Charge operations (`charge_subscription` and, internally, each item in `batch_charge`) are protected against:

1. **Replay**: Charging the same billing period more than once.
2. **Idempotent retries**: Allowing the same logical charge to be submitted multiple times (e.g. network retry) without double-debiting.

Storage usage is kept bounded: one period index and one ring buffer of up to 32 idempotency key hashes per subscription.

## Mechanisms

### Period-based key (always on)

- For each subscription we record the **last charged billing period** as `period_index = now / interval_seconds` (integer division).
- Before charging we require that the current period has not already been charged. If it has, the contract returns `Error::Replay`.
- After a successful charge we store the current `period_index` for that subscription.
- **Storage**: One `u64` per subscription (key: `DataKey::ChargedPeriod(subscription_id)`).

### Optional idempotency key (caller-provided)

Three entrypoints accept an optional idempotency key:

| Entrypoint | Domain constant |
|---|---|
| `charge_subscription` | `DOMAIN_CHARGE_INTERVAL = 0` |
| `deposit_funds` | `DOMAIN_DEPOSIT_FUNDS = 1` |
| `charge_one_off` | `DOMAIN_CHARGE_ONEOFF = 2` |

- Each entrypoint accepts an `idem_key: Option<BytesN<32>>`.
- If the caller supplies a key and we have already recorded the **hash** of `(domain, subscription_id, key)` for this subscription, we return the success variant without modifying state (idempotent no-op).
- If the key is new, the normal checks run, the operation executes, and the hashed key is appended to the ring buffer.

#### Key hashing

The raw caller-supplied key is **never stored directly**. Instead, the contract computes:

```
hash = SHA256(domain || subscription_id || raw_key)
```

where `domain` is a 4-byte big-endian `u32`, `subscription_id` is a 4-byte big-endian `u32`, and `raw_key` is the 32-byte caller-supplied value. This ensures:

- The same raw key used on two different entrypoints produces different on-chain fingerprints.
- The same raw key used on two different subscriptions produces different fingerprints.
- No raw key material is visible in storage to indexers.

#### Ring buffer

- Hashes are stored in an `IdemRingBuffer` struct capped at `IDEM_HISTORY = 32` entries per subscription.
- A `cursor` field tracks where the next entry will be written. When the buffer is full, the oldest entry is silently overwritten (cursor wraps around).
- **Storage**: One `IdemRingBuffer` per subscription (key: `DataKey::IdemKey(subscription_id)`).

### Batch charge

- `batch_charge(subscription_ids)` does **not** take idempotency keys. Each subscription is charged with period-based replay protection only. Duplicate IDs in the list are processed independently (each may succeed or fail per period/balance/interval).

## Integrator responsibilities

1. **Use one idempotency key per billing event.** For a given subscription and billing period, use a single stable key (e.g. derived from `subscription_id` + period start or from your job id). Retries with the same key are safe; using a new key for the same period will be rejected as `Replay` once the period was already charged.

2. **Do not reuse keys across periods or entrypoints.** Use a new key for each new billing period so that the next charge is not mistaken for a replay of the previous period. The domain-separated hashing scheme prevents cross-entrypoint collisions, but best practice is still to use unique keys per event.

3. **Handle `Error::Replay`.** If you receive `Replay`, the charge for that period was already applied (by this or a previous request). Treat as success for reporting; do not retry with a different key for the same period.

4. **Handle idempotent no-op.** If you receive `Ok` but did not observe the corresponding on-chain event (e.g. your indexer missed it), the operation still succeeded. The contract does not re-emit events on idempotent matches; verify against the ring buffer off-chain if needed.

5. **Optional but recommended:** Persist idempotency keys in your billing engine (e.g. per subscription and period) so that retries use the same key.

6. **Retry window.** Because the ring buffer holds only the 32 most recent hashes, retries must complete within 32 operations for the same subscription. After 32 newer operations, the oldest hash is evicted and a retry with that key would be processed as a fresh operation.

## Required parameters and behavior (Rustdoc summary)

- **`charge_subscription(env, subscription_id, idem_key)`**
  - `idem_key`: `Option<BytesN<32>>`. Use `Some(key)` for safe retries; use `None` for period-only protection.
  - Returns `Ok(ChargeExecutionResult::Charged)` on success or idempotent match (same key already processed).
  - Returns `Err(Error::Replay)` if this billing period was already charged (and the call did not match a stored idempotency key).

- **`deposit_funds(env, subscription_id, subscriber, amount, idem_key)`**
  - `idem_key`: `Option<BytesN<32>>`. Use `Some(key)` for safe retries.
  - Returns `Ok(())` on success or idempotent match.

- **`charge_one_off(env, subscription_id, merchant, amount, idem_key)`**
  - `idem_key`: `Option<BytesN<32>>`. Use `Some(key)` for safe retries.
  - Returns `Ok(())` on success or idempotent match.

## Residual risks and mitigations

- **Clock skew / timestamp manipulation:** Period is derived from ledger timestamp. Validators set ledger time; contract does not rely on caller-provided time. Mitigation: trust the network's ledger timestamp.
- **Unbounded growth:** Only one period index and one `IdemRingBuffer` (≤ 32 entries × 32 bytes = 1,024 bytes per subscription) are stored. No unbounded growth from replay protection.
- **Key collision:** If an integrator reuses the same 32-byte key for two different billing periods on the same entrypoint, the second period's charge would be treated as idempotent (return Ok without charging). Mitigation: derive keys from period (e.g. include period start or index in the key).
- **Ring buffer eviction:** A retry delayed by more than 32 operations will miss the ring buffer and execute as a fresh charge. Use `None` or choose a short retry window.
- **Cross-entrypoint safety:** Domain separation in the hash prevents the same raw key from replaying across `charge_subscription`, `deposit_funds`, and `charge_one_off`.

---

## Admin-operation nonce scheme

Privileged admin operations (`batch_charge` and `rotate_admin`) carry an additional layer of replay protection through an explicit, domain-separated, monotonic nonce scheme.

### Design

| Property | Value |
|---|---|
| Nonce type | `u64` (unsigned, monotonic) |
| Per-signer | One counter per `(signer: Address, domain: u32)` pair |
| Storage | Persistent storage, key `DataKey::AdminNonce(Address, u32)` |
| Initial value | `0` (absent key treated as `0`) |
| Enforcement | Caller provides the *current* stored value; contract checks equality, then atomically increments |
| Error on mismatch | `Error::NonceAlreadyUsed` (code `1038`) |

### Domain constants

```rust
pub const DOMAIN_BATCH_CHARGE: u32 = 0;   // label: "batch"
pub const DOMAIN_ADMIN_ROTATION: u32 = 1;  // label: "adm_rot"
```

Domain separation ensures that a nonce consumed in one operation cannot interfere with another. The labels appear in the emitted event topic so indexers can filter by domain.

### Nonce consumption flow

```
caller → batch_charge(ids, nonce)
  1. require_stored_admin_auth()   // auth check first – fails fast on wrong signer
  2. check_and_advance(admin, DOMAIN_BATCH_CHARGE, nonce)
        a. read stored nonce (default 0)
        b. assert provided == stored  → Error::NonceAlreadyUsed if not
        c. write stored + 1
        d. emit NonceConsumedEvent
  3. … rest of charge logic
```

### Emitted event

Every successful nonce consumption emits a `NonceConsumedEvent`:

```rust
pub struct NonceConsumedEvent {
    pub signer:    Address,  // the admin address that consumed the nonce
    pub domain:    u32,      // DOMAIN_BATCH_CHARGE or DOMAIN_ADMIN_ROTATION
    pub nonce:     u64,      // the consumed (previous) nonce value
    pub timestamp: u64,      // ledger timestamp at consumption
}
```

Event topic: `("nonce_consumed", signer, domain_label)` where `domain_label` is the human-readable symbol (`"batch"` or `"adm_rot"`).

### Off-chain integration

Use `get_admin_nonce(signer, domain) -> u64` to read the expected nonce before submitting a transaction:

```rust
// Pseudocode
let next_nonce = client.get_admin_nonce(&admin, DOMAIN_BATCH_CHARGE);
client.batch_charge(&subscription_ids, &next_nonce);
```

To prevent races, integrate this with a serialised job queue or use optimistic concurrency: if `NonceAlreadyUsed` is returned, re-read the nonce and retry.

### Security properties

| Threat | Mitigation |
|---|---|
| Cross-ledger replay | Nonce is monotonic; replaying any past transaction fails with `NonceAlreadyUsed` |
| Out-of-order submission | Only the exact stored value is accepted; skipping nonce values is rejected |
| Cross-domain replay | Domain tag is part of storage key; batch_charge nonce and rotate_admin nonce are fully independent |
| Cross-signer replay | Signer address is part of storage key; each admin has its own counter |
| Nonce overflow | `checked_add(1)` panics (transaction aborted) rather than wrapping to 0 |
| Auth bypass via nonce manipulation | Auth check (`require_admin_auth`) runs *before* nonce check; invalid signers are rejected without advancing any counter |

### Storage layout

```
Persistent storage:
  DataKey::AdminNonce(Address, 0) → u64   (batch_charge nonce for address)
  DataKey::AdminNonce(Address, 1) → u64   (rotate_admin nonce for address)
```

Nonce entries are stored in **persistent** storage so they survive ledger TTL extension and contract upgrades. Growth is bounded: one `u64` entry per `(signer, domain)` pair. In practice this means at most two entries per admin address (one per domain).

## Admin-operation nonce scheme

Privileged admin operations (`batch_charge` and `rotate_admin`) carry an additional layer of replay protection through an explicit, domain-separated, monotonic nonce scheme.

### Design

| Property | Value |
|---|---|
| Nonce type | `u64` (unsigned, monotonic) |
| Per-signer | One counter per `(signer: Address, domain: u32)` pair |
| Storage | Persistent storage, key `DataKey::AdminNonce(Address, u32)` |
| Initial value | `0` (absent key treated as `0`) |
| Enforcement | Caller provides the *current* stored value; contract checks equality, then atomically increments |
| Error on mismatch | `Error::NonceAlreadyUsed` (code `1038`) |

### Domain constants

```rust
pub const DOMAIN_BATCH_CHARGE: u32 = 0;   // label: "batch"
pub const DOMAIN_ADMIN_ROTATION: u32 = 1;  // label: "adm_rot"
```

Domain separation ensures that a nonce consumed in one operation cannot interfere with another. The labels appear in the emitted event topic so indexers can filter by domain.

### Nonce consumption flow

```
caller → batch_charge(ids, nonce)
  1. require_stored_admin_auth()   // auth check first – fails fast on wrong signer
  2. check_and_advance(admin, DOMAIN_BATCH_CHARGE, nonce)
       a. read stored nonce (default 0)
       b. assert provided == stored  → Error::NonceAlreadyUsed if not
       c. write stored + 1
       d. emit NonceConsumedEvent
  3. … rest of charge logic
```

### Emitted event

Every successful nonce consumption emits a `NonceConsumedEvent`:

```rust
pub struct NonceConsumedEvent {
    pub signer:    Address,  // the admin address that consumed the nonce
    pub domain:    u32,      // DOMAIN_BATCH_CHARGE or DOMAIN_ADMIN_ROTATION
    pub nonce:     u64,      // the consumed (previous) nonce value
    pub timestamp: u64,      // ledger timestamp at consumption
}
```

Event topic: `("nonce_consumed", signer, domain_label)` where `domain_label` is the human-readable symbol (`"batch"` or `"adm_rot"`).

### Off-chain integration

Use `get_admin_nonce(signer, domain) -> u64` to read the expected nonce before submitting a transaction:

```rust
// Pseudocode
let next_nonce = client.get_admin_nonce(&admin, DOMAIN_BATCH_CHARGE);
client.batch_charge(&subscription_ids, &next_nonce);
```

To prevent races, integrate this with a serialised job queue or use optimistic concurrency: if `NonceAlreadyUsed` is returned, re-read the nonce and retry.

### Security properties

| Threat | Mitigation |
|---|---|
| Cross-ledger replay | Nonce is monotonic; replaying any past transaction fails with `NonceAlreadyUsed` |
| Out-of-order submission | Only the exact stored value is accepted; skipping nonce values is rejected |
| Cross-domain replay | Domain tag is part of storage key; batch_charge nonce and rotate_admin nonce are fully independent |
| Cross-signer replay | Signer address is part of storage key; each admin has its own counter |
| Nonce overflow | `checked_add(1)` panics (transaction aborted) rather than wrapping to 0 |
| Auth bypass via nonce manipulation | Auth check (`require_admin_auth`) runs *before* nonce check; invalid signers are rejected without advancing any counter |

### Storage layout

```
Persistent storage:
  DataKey::AdminNonce(Address, 0) → u64   (batch_charge nonce for address)
  DataKey::AdminNonce(Address, 1) → u64   (rotate_admin nonce for address)
```

Nonce entries are stored in **persistent** storage so they survive ledger TTL extension and contract upgrades. Growth is bounded: one `u64` entry per `(signer, domain)` pair. In practice this means at most two entries per admin address (one per domain).