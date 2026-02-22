# Replay Protection and Idempotency for Charges

This document describes how the subscription vault prevents double-charging and how off-chain billing engines should integrate with it.

## Overview

Charge operations (`charge_subscription` and, internally, each item in `batch_charge`) are protected against:

1. **Replay**: Charging the same billing period more than once.
2. **Idempotent retries**: Allowing the same logical charge to be submitted multiple times (e.g. network retry) without double-debiting.

Storage usage is kept bounded: one period index and optionally one idempotency key per subscription.

## Mechanisms

### Period-based key (always on)

- For each subscription we record the **last charged billing period** as `period_index = now / interval_seconds` (integer division).
- Before charging we require that the current period has not already been charged. If it has, the contract returns `Error::Replay`.
- After a successful charge we store the current `period_index` for that subscription.
- **Storage**: One `u64` per subscription (key: `("cp", subscription_id)`).

### Optional idempotency key (caller-provided)

- `charge_subscription(subscription_id, idempotency_key)` accepts an optional `Option<BytesN<32>>`.
- If the caller supplies a key and we have already processed a charge for this subscription with the **same** key, we return `Ok(())` without changing state (idempotent success).
- If the caller supplies a key and we have not seen it for this subscription, we perform the normal checks (period replay, interval, balance), then charge and store the key.
- **Storage**: At most one idempotency key per subscription (key: `("idem", subscription_id)`). Supplying a new key for a new period overwrites the previous one.

### Batch charge

- `batch_charge(subscription_ids)` does **not** take idempotency keys. Each subscription is charged with period-based replay protection only. Duplicate IDs in the list are processed independently (each may succeed or fail per period/balance/interval).

## Integrator responsibilities

1. **Use one idempotency key per billing event.** For a given subscription and billing period, use a single stable key (e.g. derived from `subscription_id` + period start or from your job id). Retries with the same key are safe; using a new key for the same period will be rejected as `Replay` once the period was already charged.

2. **Do not reuse keys across periods.** Use a new key for each new billing period so that the next charge is not mistaken for a replay of the previous period.

3. **Handle `Error::Replay`.** If you receive `Replay`, the charge for that period was already applied (by this or a previous request). Treat as success for reporting; do not retry with a different key for the same period.

4. **Optional but recommended:** Persist idempotency keys in your billing engine (e.g. per subscription and period) so that retries use the same key.

## Required parameters and behavior (Rustdoc summary)

- **`charge_subscription(env, subscription_id, idempotency_key)`**
  - `idempotency_key`: `Option<BytesN<32>>`. Use `Some(key)` for safe retries; use `None` for period-only protection.
  - Returns `Ok(())` on success or idempotent match (same key already processed).
  - Returns `Err(Error::Replay)` if this billing period was already charged (and the call did not match a stored idempotency key).

## Residual risks and mitigations

- **Clock skew / timestamp manipulation:** Period is derived from ledger timestamp. Validators set ledger time; contract does not rely on caller-provided time. Mitigation: trust the network’s ledger timestamp.
- **Unbounded growth:** Only one period index and one idempotency key per subscription are stored. No unbounded growth from replay protection.
- **Key collision:** If an integrator reuses the same 32-byte key for two different billing periods, the second period’s charge would be treated as idempotent (return Ok without charging). Mitigation: derive keys from period (e.g. include period start or index in the key).
