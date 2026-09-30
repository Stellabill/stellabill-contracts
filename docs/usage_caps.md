# Usage Caps

Subscriptions with `usage_enabled = true` can define an optional per-period hard cap and a lifetime cap.

## Per-Period Caps

Configured via `configure_usage_limits`:
- `usage_cap_units: Option<i128>`

### Behavior

- `UsageState.current_period_usage_units` accumulates with each successful usage charge.
- Period index = `(now - sub.start_time) / sub.interval_seconds`. When the contract rolls into a new billing period, `current_period_usage_units` resets to `0` before applying the charge.
- If `current_period_usage_units + amount > usage_cap_units`, the call returns `UsageChargeResult::UsageCapExceeded` and emits `usage_charge_rejected`. No state is mutated.
- A charge that brings cumulative usage to exactly `usage_cap_units` is **allowed** (boundary-inclusive: `<=` not `<`).

### Boundary conditions

| Scenario | Result |
|---|---|
| `units + amount == cap` | `Charged` — exactly at cap is allowed |
| `units + amount > cap` | `UsageCapExceeded` |
| Period rollover before charge | Counter resets, cap re-applies fresh |
| `usage_cap_units = None` | Cap check skipped entirely |

### Notes

- Caps are stored in `DataKey::UsageLimits(subscription_id)` and counters in `DataKey::UsageState(subscription_id)`.
- The rejection path is deterministic and storage-efficient — no iteration over past statements.
- Cap enforcement fires after burst and rate-limit checks.

## Lifetime Caps

Configured at subscription creation (or inherited from plan templates):
- `lifetime_cap: Option<i128>`

### Behavior

- Both interval charges and usage charges increment `sub.lifetime_charged`.
- If a usage charge would exceed the remaining lifetime cap:
  - The subscription transitions to `Cancelled`.
  - No funds are debited; no merchant balance is credited.
  - A `lifetime_cap_reached` event is emitted.
  - The call returns `Ok(UsageChargeResult::Charged)` — the enforcement outcome is observed via the event, not the return code.
- If `sub.lifetime_charged >= cap` at charge time (already reached), an immediate `Error::LifetimeCapReached` is returned.

## Relationship Between Per-Period Cap and Lifetime Cap

The per-period cap resets every billing interval; the lifetime cap never resets. Both can be active simultaneously. The per-period cap check occurs first (in `charge_usage_one`'s limit-enforcement block). The lifetime cap check follows.
