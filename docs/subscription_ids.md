# Subscription ID Semantics

This document describes how subscription IDs are generated, what guarantees they provide, and how overflow is prevented in the `subscription_vault` contract.

---

## Overview

Every subscription is identified by a `u32` integer assigned at creation time by the internal `_next_id` helper. IDs are **stable**, **unique**, and **monotonically increasing** for the lifetime of the contract.

---

## ID Properties

| Property | Value |
|---|---|
| **Type** | `u32` |
| **First ID** | `0` |
| **Maximum ID** | `u32::MAX` = 4 294 967 295 |
| **Allocation step** | `+1` per subscription |
| **Reuse** | Never — cancelled/expired subscriptions keep their ID forever |
| **Total capacity** | 4 294 967 296 unique subscriptions per contract instance |

---

## How IDs Are Allocated

Each call to `create_subscription` internally calls `_next_id`:

```rust
fn _next_id(env: &Env) -> Result<u32, Error> {
    let key = Symbol::new(env, "next_id");
    let current: u32 = env.storage().instance().get(&key).unwrap_or(0u32);

    if current == MAX_SUBSCRIPTION_ID {
        return Err(Error::SubscriptionLimitReached);
    }

    env.storage().instance().set(&key, &(current + 1));
    Ok(current)
}
```

Steps:
1. Read the current counter (defaults to `0` on first use).
2. **Overflow guard**: if `current == MAX_SUBSCRIPTION_ID`, return `SubscriptionLimitReached`.
3. Write `current + 1` back (safe — step 2 guarantees `current < u32::MAX`).
4. Return `current` as the newly allocated ID.

---

## Overflow Prevention

The original implementation used `id + 1` with no guard, which would:
- **Panic** in Rust debug builds at `u32::MAX + 1`.
- **Silently wrap to `0`** in release builds, overwriting the first subscription and breaking all uniqueness guarantees.

The hardened version **guards before incrementing**, so neither panic nor wrap can occur. Instead, callers receive a clean `Error::SubscriptionLimitReached` (code `429`).

---

## Named Limit Constant

```rust
pub const MAX_SUBSCRIPTION_ID: u32 = u32::MAX;
```

Using a named constant (rather than an inline `u32::MAX`) means:
- The limit is self-documenting.
- It is trivially changeable if a lower practical cap is desired.
- Tests can reference it directly instead of embedding magic numbers.

---

## Querying the Count

```rust
pub fn get_subscription_count(env: Env) -> u32
```

Returns the current value of the internal counter, which equals the total number of subscriptions ever created (including cancelled and expired ones). This is a read-only, zero-cost call.

```rust
let total = client.get_subscription_count();
println!("Total subscriptions created: {}", total);
```

---

## Storage Layout

| Storage key | Type | Description |
|---|---|---|
| `"next_id"` (Symbol) | `u32` | Next ID to be allocated; absent until first subscription |
| `<subscription_id>` (u32) | `Subscription` | Subscription data keyed by its ID |

Both live in instance storage and share the contract's storage budget.

---

## Error Reference

| Code | Name | When |
|---|---|---|
| `429` | `SubscriptionLimitReached` | Counter reached `MAX_SUBSCRIPTION_ID`; no more IDs available |
| `404` | `NotFound` | Subscription ID does not exist in storage |

---

## Test Coverage

| Test | Scenario |
|---|---|
| `test_id_starts_at_zero` | First subscription → ID 0 |
| `test_ids_are_monotonically_increasing` | IDs 0–9 are allocated in order |
| `test_ids_are_unique` | 100 allocations → 100 distinct IDs |
| `test_get_subscription_count` | Count equals number of subscriptions created |
| `test_id_at_max_minus_one_succeeds` | Counter at `MAX-1` → allocation returns `MAX-1` |
| `test_id_at_max_returns_limit_reached` | Counter at `MAX` → `SubscriptionLimitReached` |
| `test_no_id_reuse_after_limit` | Repeated calls after limit → always `SubscriptionLimitReached`, counter stable |
