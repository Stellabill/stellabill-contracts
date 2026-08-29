# Usage Rate Limits & Burst Protection

Usage charge calls support per-subscription rate limiting and abuse protection to prevent billing spam or exploitation.

## Configuration

Configured by the merchant via `configure_usage_limits`:

- `rate_limit_max_calls: Option<u32>` — max usage charges within the rate window. `None` disables rate limiting.
- `rate_window_secs: u64` — duration of the rate-limit window in seconds.
- `burst_min_interval_secs: u64` — minimum seconds required between any two consecutive charges. `0` disables burst protection.
- `usage_cap_units: Option<i128>` — max usage amount per billing period. `None` disables the cap.

Limits are stored under `DataKey::UsageLimits(subscription_id)`. Runtime counters are stored under `DataKey::UsageState(subscription_id)`. All window math uses `env.ledger().timestamp()`.

## Enforcement in `charge_usage_one`

Enforcement runs **before** any state mutation or fund transfer, in this order:

### 1. Burst Protection

- Records `last_usage_timestamp` in `UsageState`.
- If `now - last_usage_timestamp < burst_min_interval_secs`, returns `UsageChargeResult::BurstLimitExceeded`.
- A call where `elapsed == burst_min_interval_secs` is **allowed** (boundary-inclusive).

### 2. Rate Limiting (Fixed Window)

- Tracks `window_call_count` and `window_start_timestamp` in `UsageState`.
- Window resets when `now >= window_start_timestamp + rate_window_secs`.
- If `window_call_count >= rate_limit_max_calls`, returns `UsageChargeResult::RateLimitExceeded`.
- A call at exactly `window_start_timestamp + rate_window_secs` starts a new window.

### 3. Per-Period Usage Cap

- Tracks `current_period_usage_units` and `period_index` in `UsageState`.
- Period index = `(now - sub.start_time) / sub.interval_seconds`.
- When a new period begins, `current_period_usage_units` resets to `0`.
- If `current_period_usage_units + usage_amount > usage_cap_units`, returns `UsageChargeResult::UsageCapExceeded`.
- A charge that brings usage to exactly `usage_cap_units` is **allowed**.

### State Update

On a successful pass through all checks, `UsageState` is updated atomically:
- `last_usage_timestamp = now`
- `window_call_count += 1`
- `current_period_usage_units += usage_amount`

This happens before the token transfer, preserving a safe Checks-Effects-Interactions order.

### Passthrough (No Limits Configured)

When no `UsageLimits` entry exists for a subscription, all three checks are skipped and the charge proceeds normally.

## Observability

All enforcement outcomes emit `usage_charge_rejected` with a `UsageChargeRejectedEvent` payload:

| Field | Description |
|---|---|
| `subscription_id` | Subscription that was rejected |
| `merchant` | Merchant address |
| `token` | Settlement token |
| `usage_amount` | Attempted charge amount |
| `timestamp` | Ledger timestamp |
| `reference` | Idempotency reference |
| `result` | `BurstLimitExceeded`, `RateLimitExceeded`, or `UsageCapExceeded` |

`configure_usage_limits` emits `usage_limits_configured` on every successful call.

## Storage

| Key | Tier | Contents |
|---|---|---|
| `DataKey::UsageLimits(id)` | Instance | Rate/burst/cap configuration |
| `DataKey::UsageState(id)` | Instance | Runtime counters (timestamps, window count, period units) |

## Security Notes

- All limit checks occur before any state mutation — a rejected charge never partially updates state.
- Replay protection (`DataKey::UsageReference`) runs before limit checks, so duplicate references are caught first.
- Limits are merchant-configurable per subscription; the merchant address stored in `UsageLimits` is verified against the subscription's merchant on creation.
