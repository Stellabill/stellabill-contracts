# Auto-Renewal Disable Flag and Renewal Window

Implements issue [#562](https://github.com/Stellabill/stellabill-contracts/issues/562).

This document describes the `auto_renew` flag on `Subscription`, the renewal
window mechanism, and the `set_auto_renew` entrypoint.

---

## Overview

By default every subscription is created with `auto_renew = true`, meaning the
billing engine will continue charging it at each interval boundary indefinitely.

A subscriber or merchant may call `set_auto_renew(false)` to opt out. Once
disabled, billing halts at the **next interval boundary** — the current period
completes normally but no further automatic charges are taken. Prepaid balance
and all history are preserved in place.

A **renewal window** of one full `interval_seconds` after the flag was first
disabled allows the subscriber or merchant to re-enable auto-renewal without
having to cancel and recreate the subscription. If the window elapses without
re-enabling, the subscription can no longer be reactivated via
`set_auto_renew(true)` — it must be cancelled and a new one created.

---

## Data Model

Two fields are added to `Subscription`:

| Field | Type | Default | Description |
|---|---|---|---|
| `auto_renew` | `bool` | `true` | When `false`, the billing engine skips charges once the interval elapses. |
| `auto_renew_disabled_at` | `Option<u64>` | `None` | Ledger timestamp of the **first** `set_auto_renew(false)` call. Used to calculate the renewal window deadline. Reset to `None` when auto-renewal is re-enabled. |

### Renewal window boundary

```
window_end = auto_renew_disabled_at + interval_seconds
```

Re-enabling is permitted while `now < window_end`. At or after `window_end`
the call returns `Error::RenewalWindowClosed` (12001).

---

## Entrypoint: `set_auto_renew`

```
set_auto_renew(env: Env, subscription_id: u32, authorizer: Address, enabled: bool)
  -> Result<(), Error>
```

### Authorization

`authorizer` must be the subscription's `subscriber` **or** `merchant`.
Any other caller receives `Error::Forbidden` (1002).

### Guards

| Condition | Error returned |
|---|---|
| Subscription does not exist | `NotFound` (2001) |
| Caller is neither subscriber nor merchant | `Forbidden` (1002) |
| Subscription is `Cancelled` | `InvalidStatusTransition` (4001) |
| Subscription has passed its `expires_at` | `SubscriptionExpired` (4003) |
| `enabled = true` but `auto_renew` is already `false` and window has closed | `RenewalWindowClosed` (12001) |

### Idempotency

- Calling `set_auto_renew(false)` when already `false` **preserves the
  original `auto_renew_disabled_at` timestamp**. The window continues to count
  from the first disable, not the most recent call.
- Calling `set_auto_renew(true)` when already `true` is a no-op (succeeds,
  no state change).

### Event emitted

`AutoRenewToggledEvent` with topic `("auto_renew_toggled", subscription_id)`:

| Field | Type | Description |
|---|---|---|
| `subscription_id` | `u32` | Affected subscription. |
| `subscriber` | `Address` | Subscription subscriber. |
| `merchant` | `Address` | Subscription merchant. |
| `enabled` | `bool` | New value of `auto_renew`. |
| `authorizer` | `Address` | Caller who made the change. |
| `timestamp` | `u64` | Ledger timestamp at time of call. |
| `schema_version` | `u32` | Event schema version. |

---

## Billing Engine Integration

In `charge_core.rs`, after the interval guard (which still fires normally), an
additional check short-circuits charges when `auto_renew = false`:

```
if !sub.auto_renew && now >= next_allowed_charge_time {
    return Ok(ChargeExecutionResult::Skipped);
}
```

- The charge returns `Skipped` (not an error) so that `batch_charge` can
  continue past non-renewing subscriptions without aborting the batch.
- `IntervalNotElapsed` still fires normally if the interval has not yet elapsed,
  regardless of `auto_renew`.

---

## State Machine Interaction

`auto_renew` is orthogonal to `SubscriptionStatus`. A subscription may be
`Paused` and have `auto_renew = false` simultaneously. Toggling `auto_renew` is
permitted in all non-terminal, non-expired states:

| Status | Can toggle `auto_renew`? |
|---|---|
| `Active` | ✅ Yes |
| `Paused` | ✅ Yes |
| `GracePeriod` | ✅ Yes |
| `InsufficientBalance` | ✅ Yes |
| `Cancelled` | ❌ No — `InvalidStatusTransition` |
| Expired (`now >= expires_at`) | ❌ No — `SubscriptionExpired` |

---

## Storage Migration

Subscriptions loaded from pre-562 snapshots (via `restore_subscriptions`) are
assigned `auto_renew = true` and `auto_renew_disabled_at = None`, which matches
the creation default and preserves existing billing behaviour transparently.

---

## Error Code

| Code | Variant | When |
|---|---|---|
| `12001` | `RenewalWindowClosed` | Re-enabling auto-renewal after the one-interval window has elapsed. |

---

## Test Coverage

`contracts/subscription_vault/src/test_auto_renew.rs` covers 20 scenarios:

| # | Scenario |
|---|---|
| 1 | Default `auto_renew = true` on creation |
| 2 | Subscriber can disable |
| 3 | Merchant can disable |
| 4 | Third party receives `Forbidden` |
| 5 | Charge skipped when disabled and interval elapsed |
| 6 | Charge proceeds normally when enabled |
| 7 | Re-enable within window succeeds |
| 8 | Re-enable after window closed → `RenewalWindowClosed` |
| 9 | Toggle mid-interval (interval guard still fires first) |
| 10 | Toggle then cancel |
| 11 | Renewal after long dormancy → `RenewalWindowClosed` |
| 12 | Double disable preserves original timestamp |
| 13 | Double enable is a no-op |
| 14 | Cancelled subscription rejects toggle |
| 15 | Expired subscription rejects toggle |
| 16 | Non-existent subscription → `NotFound` |
| 17 | `AutoRenewToggledEvent` emitted on disable |
| 18 | `AutoRenewToggledEvent` emitted on re-enable |
| 19 | Batch charge skips non-renewing subscription cleanly |
| 20 | Paused subscription can toggle `auto_renew` |
