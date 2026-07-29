# Merchant-Initiated One-Off Charges

This document describes the one-off charge feature as **implemented** in
`contracts/subscription_vault/src/subscription.rs` (`do_charge_one_off`) and
exposed via `SubscriptionVault::charge_one_off` in `lib.rs`.

---

## Overview

`charge_one_off(subscription_id, merchant, amount)` lets the **merchant** debit a
one-time `amount` from the subscription's prepaid balance. It is distinct from:

- **Interval-based charges** (`charge_subscription` / `batch_charge`): triggered
  by the billing engine on a schedule; require admin auth.
- **Usage charges** (`charge_usage` / `charge_usage_with_reference`): metered;
  require `usage_enabled = true`.
- **Subscription lifecycle actions** (pause, resume, cancel): do not move funds.

One-off charges are intended for ad-hoc fees — overages, one-time add-ons — that
the merchant is authorized to collect from the subscriber's existing prepaid balance.

---

## Entrypoint

```rust
pub fn charge_one_off(
    env: Env,
    subscription_id: u32,
    merchant: Address,
    amount: i128,
) -> Result<(), Error>
```

Disabled when the **emergency stop** is active (`Error::EmergencyStopActive`).
A **reentrancy guard** is acquired for the duration of the call.

---

## Semantics

### Authorization
- The caller must be the subscription's **merchant** and must authorize the call
  (`merchant.require_auth()`).
- Any other address receives `Error::Unauthorized`.

### Amount validation
- `amount` must be **strictly positive** (`> 0`).
- Zero or negative values return `Error::InvalidAmount`.

### Status guard
- The subscription must be **`Active`** or **`Paused`**.
- `Cancelled`, `InsufficientBalance`, and `GracePeriod` return `Error::NotActive`.

### Balance check
- `amount` must not exceed `prepaid_balance`. No overdraft.
- Violations return `Error::InsufficientPrepaidBalance`.

### Lifetime cap
- When a `lifetime_cap` is configured, `lifetime_charged + amount` must not exceed
  the cap. Violations return `Error::LifetimeCapReached`.
- On success, `lifetime_charged` is incremented by `amount`.

### Effects on subscription state
| Field                    | Change                      |
|--------------------------|-----------------------------|
| `prepaid_balance`        | Decremented by `amount`     |
| `lifetime_charged`       | Incremented by `amount`     |
| `last_payment_timestamp` | **Unchanged**               |
| `status`                 | **Unchanged**               |
| `interval_seconds`       | **Unchanged**               |

One-off charges deliberately do **not** update `last_payment_timestamp` so that
the recurring billing clock is unaffected.

### Merchant payout
Funds are credited to the merchant's accumulated balance via
`merchant::credit_merchant` — the same accounting path as interval charges.
The merchant withdraws via `withdraw_merchant_funds` /
`withdraw_merchant_token_funds` as normal.

---

## Billing Statement

A statement row is appended with:

| Field          | Value                          |
|----------------|-------------------------------|
| `kind`         | `BillingChargeKind::OneOff`   |
| `amount`       | `amount`                       |
| `period_start` | `env.ledger().timestamp()`    |
| `period_end`   | `env.ledger().timestamp()`    |
| `merchant`     | subscription's merchant        |

`period_start == period_end` signals a point-in-time charge rather than a
billing-window charge.

---

## Event

**Topic:** `("oneoff_ch", subscription_id)`

**Payload:**

```rust
OneOffChargedEvent {
    subscription_id: u32,
    merchant: Address,
    amount: i128,
}
```

Indexers can distinguish one-off revenue from recurring revenue by filtering on
the `oneoff_ch` topic (vs. `charged` for interval charges).

---

## Error reference

| Error                        | Condition                                         |
|------------------------------|---------------------------------------------------|
| `EmergencyStopActive`        | Emergency stop is enabled                         |
| `Unauthorized`               | Caller is not this subscription's merchant        |
| `InvalidAmount`              | `amount ≤ 0`                                      |
| `NotFound`                   | Subscription does not exist                       |
| `NotActive`                  | Status is `Cancelled`, `InsufficientBalance`, or `GracePeriod` |
| `LifetimeCapReached`         | `lifetime_charged + amount > lifetime_cap`        |
| `InsufficientPrepaidBalance` | `amount > prepaid_balance`                        |

---

## Interaction with other features

### Pause / Resume
One-off charges work on **Paused** subscriptions. This allows merchants to collect
ad-hoc fees even while recurring billing is suspended, without forcing a resume.

### Cancellation
Once `Cancelled`, no further charges (interval, usage, or one-off) are possible.
Use `withdraw_subscriber_funds` to return the remaining prepaid balance to the subscriber.

### Compaction & aggregation
The aggregate compaction totals (`get_stmt_compacted_aggregate`) track one-off
revenue separately in `AccruedTotals::one_off`, so historical one-off totals
are preserved even after statement rows are pruned.

### Coexistence with interval charges
One-off and interval charges both debit from the same `prepaid_balance`. Merchants
should ensure sufficient balance exists for upcoming interval charges before issuing
large one-off charges. Use `estimate_topup_for_intervals` to advise subscribers.

### Emergency stop
When the emergency stop is active, `charge_one_off` returns `Error::EmergencyStopActive`
immediately. No state is modified.

---

## Security notes

1. **Only the subscription's merchant** can call `charge_one_off` for that
   subscription; otherwise `Unauthorized` is returned.
2. **Amount and balance checks prevent overdraft**; safe math is used throughout.
3. **No bypass of lifetime caps**: one-off charges count towards `lifetime_charged`
   identically to interval charges.
4. **No replay risk**: one-off charges have no interval window or deduplication key —
   each call is an independent debit. Merchants must implement their own idempotency
   at the application layer (e.g., checking statement history before issuing a charge).
5. **Reentrancy guard**: the entry-point in `lib.rs` acquires a reentrancy lock
   before any state mutation.
6. **CEI pattern**: state is updated (`prepaid_balance`, `lifetime_charged`,
   merchant credit) before the event is emitted, so partial failure leaves no
   inconsistent state.

---

## When to use

- One-time add-ons or overages within the same billing relationship.
- Fees the merchant is authorized to collect from the existing prepaid balance.
- Any ad-hoc debit that does not fit the recurring interval model.

## When NOT to use

- Recurring billing → use `charge_subscription` or `batch_charge`.
- Subscriber refunds → use `partial_refund` (admin) or `merchant_refund` (merchant).
- Subscription cancellation → use `cancel_subscription`.
- Scenarios where the subscriber has insufficient prepaid balance — the call will
  fail with `InsufficientPrepaidBalance`; ask the subscriber to top up first.