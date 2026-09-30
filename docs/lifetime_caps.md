# Lifetime Charge Caps

Lifetime charge caps let each subscription define a maximum total amount that may ever be charged over its entire lifespan. Once the cumulative charged amount reaches or would exceed the cap, no further charges are processed and the subscription is automatically cancelled.

---

## Overview

| Field | Type | Default | Description |
|---|---|---|---|
| `lifetime_cap` | `Option<i128>` | `None` | Maximum total chargeable amount in token base units. `None` = no cap. |
| `lifetime_charged` | `i128` | `0` | Running total of all successfully charged amounts. |

Units are **token base units** (same as `amount`). For USDC with 6 decimal places: `1 USDC = 1_000_000`.

---

## Cap Configuration Hierarchy

Caps are resolved using a three-level precedence at subscription creation time:

```
per-subscription explicit cap  ← highest priority
        │ (if None, falls through)
per-merchant default cap       ← set by merchant via set_merchant_cap_default
        │ (if None, falls through)
global default cap             ← set by admin via set_global_cap_default
        │ (if None, falls through)
None (no cap)                  ← lowest priority
```

### Global default cap (admin-controlled)

```rust
// Admin sets a contract-wide fallback cap
client.set_global_cap_default(&admin, &Some(120_000_000i128));

// Read it back
let cap: Option<i128> = client.get_global_cap_default();

// Remove it
client.set_global_cap_default(&admin, &None);
```

Emits `global_cap_set` event with `GlobalCapDefaultUpdatedEvent`.

### Per-merchant default cap (merchant-controlled)

```rust
// Merchant sets a default cap for all new subscriptions to them
client.set_merchant_cap_default(&merchant, &Some(60_000_000i128));

// Read it back
let cap: Option<i128> = client.get_merchant_cap_default(&merchant);

// Remove it (falls back to global default)
client.set_merchant_cap_default(&merchant, &None);
```

Emits `merchant_cap_set` event with `MerchantCapDefaultUpdatedEvent`.

### Per-subscription explicit cap

```rust
// Explicit cap overrides both defaults
client.create_subscription(
    &subscriber, &merchant, &10_000_000, &INTERVAL,
    &false, &Some(30_000_000i128), &None,
);
```

---

## Setting a Cap

### Direct subscription creation

```rust
// Cap at 120 USDC (12 monthly charges of 10 USDC each)
client.create_subscription(
    &subscriber,
    &merchant,
    &10_000_000,       // 10 USDC per interval
    &(30 * 24 * 3600), // monthly
    &false,
    &Some(120_000_000i128), // 120 USDC lifetime cap
    &None,             // no expiry
);
```

### Via plan template

```rust
let plan_id = client.create_plan_template(
    &merchant,
    &10_000_000,
    &(30 * 24 * 3600),
    &false,
    &Some(120_000_000i128), // inherited by all subscriptions from this template
);

// Subscription automatically inherits the template's cap
let sub_id = client.create_subscription_from_plan(&subscriber, &plan_id);
```

Pass `None` to create a subscription with no lifetime limit (unless a merchant or global default applies).

---

## Admin cap update

After creation, the admin can raise, lower, or remove a subscription's cap:

```rust
// Raise cap
client.update_subscription_cap(&admin, &sub_id, &Some(200_000_000i128));

// Remove cap entirely
client.update_subscription_cap(&admin, &sub_id, &None);
```

**Constraint**: `new_cap >= lifetime_charged`. Setting a cap below what has already been charged returns `Error::LifetimeCapReached` and leaves the subscription unchanged.

Emits `cap_updated` event with `LifetimeCapUpdatedEvent`.

---

## Enforcement Points

### 1 — Deposit (`deposit_funds`)

Deposits are capped to prevent locking funds that can never be charged:

```
depositable_remaining = lifetime_cap - lifetime_charged - prepaid_balance
```

If `amount > depositable_remaining` → `Error::LifetimeCapReached`.

This prevents bypassing the cap by pre-loading a large balance before the cap enforcement on charges.

### 2 — Interval charges (`charge_subscription`)

1. **Pre-check**: Before debiting, the contract computes `remaining = lifetime_cap - lifetime_charged`.
   - If `remaining == 0` or `amount > remaining` → the charge is **skipped**, the subscription is **cancelled**, and a `lifetime_cap_reached` event is emitted. Returns `Ok(())`.
2. **Post-charge**: After a successful debit, `lifetime_charged += amount`.
   - If `lifetime_charged >= lifetime_cap` → the subscription is **cancelled** and a `lifetime_cap_reached` event is emitted. Returns `Ok(())`.

### 3 — Usage charges (`charge_usage`)

1. **Pre-check**: Compute `pending = lifetime_charged + usage_amount`.
   - If `pending > lifetime_cap` → the usage charge is **blocked**, the subscription is
     **cancelled**, and `lifetime_cap_reached` is emitted. Returns `Ok(())`.
2. **Post-charge**: If `pending == lifetime_cap`, the usage charge is processed normally
   (prepaid debited, merchant credited), then the subscription is auto-cancelled and
   `lifetime_cap_reached` is emitted.

### 4 — One-off charges (`charge_one_off`)

One-off charges also count toward `lifetime_charged`:

- If `lifetime_charged + one_off_amount > lifetime_cap` → returns
  `Error::LifetimeCapReached` and **no state is changed**.
- If `lifetime_charged + one_off_amount == lifetime_cap` → charge is processed,
  subscription is auto-cancelled, and `lifetime_cap_reached` is emitted.

---

## Soroban State-Change Semantics

> **Important**: In Soroban, storage writes are rolled back when a contract call returns an error. This means cap-triggered cancellation returns `Ok(())` (not an error) so the `Cancelled` state persists on-chain.

Callers should:
1. Check whether `charge_subscription` / `charge_usage` returned `Ok`.
2. After `Ok`, read the subscription status. If `Cancelled`, the cap was reached.
3. Listen for the `lifetime_cap_reached` event for off-chain notifications.

```
cap pre-check fires
   └─► subscription.status = Cancelled
   └─► emit lifetime_cap_reached event
   └─► return Ok(())          ← state persists

cap hit exactly after charge
   └─► balance debited, merchant credited
   └─► lifetime_charged == lifetime_cap
   └─► subscription.status = Cancelled
   └─► emit charged event
   └─► emit lifetime_cap_reached event
   └─► return Ok(())
```

---

## Lifecycle Impact

```
Active ──[charge, cap hit exactly]──► Cancelled (terminal)
Active ──[charge, cap would be exceeded]──► Cancelled (terminal)
Cancelled ──[withdraw_subscriber_funds]──► subscriber recovers prepaid balance
```

A cap-cancelled subscription is **permanent** — it cannot be resumed. The subscriber can call `withdraw_subscriber_funds` to recover any remaining prepaid balance.

---

## Querying Cap Status

```rust
let info: CapInfo = client.get_cap_info(&subscription_id);

// info.lifetime_cap      → Some(120_000_000) or None
// info.lifetime_charged  → 80_000_000 (amount charged so far)
// info.remaining_cap     → Some(40_000_000) or None if uncapped
// info.cap_reached       → false (or true once exhausted)
```

`get_cap_info` is read-only and never mutates state.

---

## Events

### `lifetime_cap_reached`

Emitted whenever a cap prevents a charge or is hit exactly after a charge.

| Field | Type | Description |
|---|---|---|
| `subscription_id` | `u32` | Affected subscription |
| `lifetime_cap` | `i128` | The configured cap value |
| `lifetime_charged` | `i128` | Total charged at the point the cap was reached |
| `timestamp` | `u64` | Ledger timestamp when the event fired |

### `cap_updated` (`LifetimeCapUpdatedEvent`)

Emitted when admin calls `update_subscription_cap`.

| Field | Type | Description |
|---|---|---|
| `subscription_id` | `u32` | Affected subscription |
| `old_cap` | `Option<i128>` | Previous cap value |
| `new_cap` | `Option<i128>` | New cap value |
| `timestamp` | `u64` | Ledger timestamp |

### `global_cap_set` (`GlobalCapDefaultUpdatedEvent`)

Emitted when admin calls `set_global_cap_default`.

| Field | Type | Description |
|---|---|---|
| `admin` | `Address` | Admin address |
| `old_default` | `Option<i128>` | Previous global default |
| `new_default` | `Option<i128>` | New global default |
| `timestamp` | `u64` | Ledger timestamp |

### `merchant_cap_set` (`MerchantCapDefaultUpdatedEvent`)

Emitted when merchant calls `set_merchant_cap_default`.

| Field | Type | Description |
|---|---|---|
| `merchant` | `Address` | Merchant address |
| `old_default` | `Option<i128>` | Previous merchant default |
| `new_default` | `Option<i128>` | New merchant default |
| `timestamp` | `u64` | Ledger timestamp |

### `charged`

The standard `SubscriptionChargedEvent` includes:

| Field | Type | Description |
|---|---|---|
| `lifetime_charged` | `i128` | Running total after this charge |

---

## Example Cap Policies

| Policy | Cap Value | Effect |
|---|---|---|
| **12-month subscription** | `12 × monthly_amount` | Automatically expires after one year of billing |
| **Trial period** | `3 × monthly_amount` | Converts to "no charge" after 3 months (subscriber must renew) |
| **Budget cap** | `500_000_000` (500 USDC) | Hard spending limit regardless of usage |
| **No cap (default)** | `None` | Subscription runs indefinitely |

---

## Interaction with Other Features

### Refunds / prepaid balance withdrawals

Refunds reduce `prepaid_balance` only. They do **not** reduce `lifetime_charged`. The cap tracks money debited from the vault and credited to the merchant — not the subscriber's current vault balance.

### Grace period

The grace period applies to balance shortfalls, not to cap enforcement. If a charge is blocked by the cap, the grace period is not entered — the subscription is immediately cancelled.

### Replay protection

Cap enforcement runs after the replay check. A replayed charge (same billing period) is rejected before the cap is evaluated.

### Emergency stop

When the emergency stop is active, `charge_subscription` and `charge_usage` are blocked entirely. The cap is not evaluated.

---

## Storage

`lifetime_cap` and `lifetime_charged` are stored as fields on the `Subscription` struct (on-ledger as a `ScMap`). Adding these fields to existing subscriptions on upgrade requires a migration that sets `lifetime_charged = 0` and `lifetime_cap = None` for all pre-existing records.

---

## Validation

- `lifetime_cap` must be `> 0` if provided. Zero and negative values are rejected with `Error::InvalidAmount`.
- When creating a subscription directly, `lifetime_cap` (if provided) must be **at least** the recurring interval `amount`.
- There is no minimum cap value above zero.
- `lifetime_charged` is read-only from external callers — it is only incremented by the charge functions.
