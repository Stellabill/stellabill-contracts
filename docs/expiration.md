# Subscription Expiration

This document describes how subscription expiration works in the `subscription_vault` contract, how to configure it, and how it is enforced.

---

## Overview

Every `Subscription` has an optional `expiration` field of type `Option<u64>`.

| Value | Meaning |
|---|---|
| `None` | The subscription has **no fixed end date** and runs indefinitely. |
| `Some(ts)` | The subscription expires at Unix timestamp `ts` (seconds since epoch). |

When `expiration` is `Some(ts)` and the current ledger timestamp satisfies `ledger.timestamp() >= ts`, any call to `charge_subscription` is **rejected** with `Error::SubscriptionExpired` (code `410`). No funds are moved.

---

## Configuration

### Setting expiration at creation

Pass the desired expiration as the last argument to `create_subscription`:

```rust
// Expires in exactly 90 days from the Unix epoch start of the ledger
let exp_ts: u64 = 90 * 24 * 60 * 60;

let id = client.create_subscription(
    &subscriber,
    &merchant,
    &amount,
    &interval_seconds,
    &usage_enabled,
    &Some(exp_ts),   // ← expiration
);
```

For an open-ended subscription (no expiration):

```rust
let id = client.create_subscription(
    &subscriber,
    &merchant,
    &amount,
    &interval_seconds,
    &usage_enabled,
    &None,           // ← no expiration
);
```

### Reading expiration

```rust
let sub = client.get_subscription(&id);
match sub.expiration {
    Some(ts) => println!("Expires at ledger timestamp {}", ts),
    None     => println!("No expiration (open-ended)"),
}
```

---

## Enforcement Logic

The enforcement happens inside `charge_subscription` **before** any funds are moved:

```rust
pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
    let sub: Subscription = env
        .storage()
        .instance()
        .get(&subscription_id)
        .ok_or(Error::NotFound)?;

    // Expiration guard
    if let Some(exp_ts) = sub.expiration {
        if env.ledger().timestamp() >= exp_ts {
            return Err(Error::SubscriptionExpired);
        }
    }

    // ... charge logic ...
    Ok(())
}
```

### Boundary semantics

| Condition | Result |
|---|---|
| `ledger.timestamp() < expiration` | ✅ Charge allowed |
| `ledger.timestamp() == expiration` | ❌ Rejected (`SubscriptionExpired`) |
| `ledger.timestamp() > expiration` | ❌ Rejected (`SubscriptionExpired`) |
| `expiration == None` | ✅ Always allowed |

The boundary is **inclusive** on expiration: the moment the ledger reaches the expiration timestamp, the subscription is considered expired.

---

## Storage Compatibility

The `expiration` field uses Rust's `Option<u64>` type, which Soroban serializes as an optional XDR value. This means:

- **Existing subscriptions** stored before this field was introduced will deserialize with `expiration = None` (no fixed end), preserving their original open-ended behavior. No migration is required.
- **New subscriptions** carry the field explicitly.

This is a **non-breaking, additive change**.

---

## Error Reference

| Code | Name | When returned |
|---|---|---|
| `410` | `SubscriptionExpired` | `charge_subscription` called at or after `expiration`. |
| `404` | `NotFound` | Subscription ID does not exist in storage. |

---

## Test Coverage

The following tests in `contracts/subscription_vault/src/test.rs` cover expiration behavior:

| Test | Scenario |
|---|---|
| `test_create_subscription_no_expiration` | `None` stored correctly |
| `test_create_subscription_with_expiration` | `Some(ts)` stored correctly |
| `test_charge_expired_subscription` | Ledger past expiration → `SubscriptionExpired` |
| `test_charge_at_exact_expiration_boundary` | Ledger == expiration → `SubscriptionExpired` |
| `test_charge_one_second_before_expiration` | Ledger one second before → `Ok` |
| `test_charge_no_expiration_always_allowed` | No expiration, large timestamp → `Ok` |
| `test_charge_nonexistent_subscription` | Missing ID → `NotFound` |
| `test_long_running_no_expiration` | 60 monthly charges, no expiration → all `Ok` |
