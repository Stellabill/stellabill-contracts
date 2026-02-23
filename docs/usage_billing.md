# Usage-Based Billing

## Overview

Usage-based billing allows merchants to charge subscribers for **metered
consumption** rather than (or in addition to) fixed recurring intervals.
The feature is opt-in per subscription via the `usage_enabled` flag set at
creation time.

When enabled, an off-chain **usage metering service** measures consumption and
calls the `charge_usage` contract entrypoint with the computed amount. The
contract debits the subscriber's prepaid vault accordingly.

## How It Works

```
Off-chain metering service
        │
        │  charge_usage(subscription_id, usage_amount)
        ▼
┌──────────────────────┐
│  SubscriptionVault   │
│                      │
│  1. Validate status  │  (must be Active)
│  2. Check usage_enabled
│  3. Validate amount  │  (> 0)
│  4. Check balance    │  (prepaid_balance ≥ usage_amount)
│  5. Debit balance    │
│  6. Transition if 0  │  → InsufficientBalance
└──────────────────────┘
```

### Entry Point

```rust
pub fn charge_usage(
    env: Env,
    subscription_id: u32,
    usage_amount: i128,
) -> Result<(), Error>;
```

| Parameter          | Type   | Description                                   |
|--------------------|--------|-----------------------------------------------|
| `subscription_id`  | `u32`  | ID returned by `create_subscription`.          |
| `usage_amount`     | `i128` | Amount (in token stroops) to debit.            |

### Pre-conditions

| Check                | Error Returned             | Description                                           |
|----------------------|----------------------------|-------------------------------------------------------|
| Subscription exists  | `NotFound`                 | The given ID must reference a stored subscription.     |
| Status is `Active`   | `NotActive`                | Paused, cancelled, or insufficient-balance subs are rejected. |
| `usage_enabled`      | `UsageNotEnabled`          | The subscription must have been created with usage enabled. |
| `usage_amount > 0`   | `InvalidAmount`            | Zero or negative amounts are rejected.                 |
| Balance sufficient   | `InsufficientPrepaidBalance` | `prepaid_balance` must be ≥ `usage_amount`.           |

### Post-conditions

* `prepaid_balance` is reduced by `usage_amount`.
* If `prepaid_balance` reaches **exactly zero**, the subscription transitions
  to `InsufficientBalance`. No further charges (interval **or** usage) can
  proceed until the subscriber calls `deposit_funds` to top up.

## Interaction with Interval-Based Charging

A subscription can use **both** interval and usage billing simultaneously:

* `charge_subscription` (interval-based) debits the fixed `amount` on each
  billing cycle.
* `charge_usage` debits an arbitrary metered amount at any time.

Both draw from the same `prepaid_balance`. If either charge drains the balance
to zero, the subscription moves to `InsufficientBalance`, blocking the other
charge type as well until the subscriber tops up.

## Integration Guide for Off-Chain Services

1. **Create a subscription** with `usage_enabled = true`.
2. **Top up** the vault via `deposit_funds` so there is sufficient
   `prepaid_balance`.
3. **Meter usage** off-chain (e.g. API calls, compute time, data transfer).
4. **Call `charge_usage`** periodically (e.g. every hour or daily) with the
   accumulated `usage_amount`.
5. **Monitor** the subscription status. When it transitions to
   `InsufficientBalance`, notify the subscriber to top up.
6. After the subscriber tops up and the status returns to `Active`, resume
   metering.

### Best Practices

* **Batch small charges**: accumulate usage off-chain and submit a single
  `charge_usage` call per period to minimise transaction fees.
* **Check balance first**: use `get_subscription` to read `prepaid_balance`
  before submitting a charge to avoid unnecessary failed transactions.
* **Use `estimate_topup_for_intervals`** alongside usage estimates to advise
  subscribers on how much to deposit.

## Error Codes

| Variant                    | Code  | Meaning                                      |
|----------------------------|-------|----------------------------------------------|
| `NotFound`                 | 404   | Subscription does not exist.                 |
| `NotActive`                | 1002  | Subscription is not in `Active` status.      |
| `UsageNotEnabled`          | 1004  | `usage_enabled` is `false` on subscription.  |
| `InvalidAmount`            | 1006  | `usage_amount` ≤ 0.                          |
| `InsufficientPrepaidBalance` | 1005 | Prepaid balance cannot cover the charge.     |
