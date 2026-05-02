# Usage Rate Limits & Burst Protection

Usage charge calls now support per-subscription rate limiting and abuse protection to ensure the billing system cannot be spammed or exploited.

## Configuration

These limits are configured by the merchant via the `configure_usage_limits` entry point:

- `rate_limit_max_calls: Option<u32>`: Maximum allowed usage charges within the rate window.
- `rate_window_secs: u64`: Duration of the rate limit window in seconds.
- `burst_min_interval_secs: u64`: Minimum required time (in seconds) between any two consecutive usage charges.
- `usage_cap_units: Option<i128>`: Maximum usage amount allowed per billing period.

## Enforcement Mechanisms

### 1. Burst Protection
- The contract records `last_usage_timestamp` for each subscription.
- If a new charge is attempted where `now - last_usage_timestamp < burst_min_interval_secs`, it is rejected with `UsageChargeResult::BurstLimitExceeded`.
- **Purpose**: Prevents rapid repeated calls in the same ledger or across very short intervals.

### 2. Time-Based Rate Limits
- A rolling fixed window counter (`window_call_count`) is kept in contract storage alongside `window_start_timestamp`.
- When calls in the active window reach `rate_limit_max_calls`, additional usage charges are rejected with `UsageChargeResult::RateLimitExceeded`.
- Counters reset automatically once the window expires (`now - window_start_timestamp >= rate_window_secs`).
  - A call at exactly `window_start_timestamp + rate_window_secs` is treated as the first call of a new window.

### 3. Replay Protection
- Every usage charge must include a unique `reference: String` parameter.
- The contract stores the last processed reference per subscription.
- If a subsequent call provides the exact same reference, it is rejected with `UsageChargeResult::Replay`.
- **Purpose**: Ensures the same off-chain usage event cannot be double-processed if the metering service retries a delayed transaction.

## Indexer-visible enforcement outcomes

Usage charge enforcement is designed to be observable by indexers without relying on reverted transactions.

- On success, `charge_usage_with_reference` completes normally and emits `usage_charged`.
- On enforcement outcomes (replay / burst / rate limit / per-period cap), `charge_usage_with_reference` completes normally and emits `usage_charge_rejected`.
- When a usage charge would exceed the subscription's `lifetime_cap`, the subscription is cancelled and `lifetime_cap_reached` is emitted (this outcome is not emitted as `usage_charge_rejected`).

`usage_charge_rejected` payload is `UsageChargeRejectedEvent` and includes:

- `subscription_id`
- `merchant`
- `token`
- `usage_amount`
- `timestamp`
- `reference`
- `result: UsageChargeResult`

`configure_usage_limits` emits `usage_limits_configured` with the final stored parameters.

## Storage Footprint
The state is highly bounded:
- **Limits**: Stored under `DataKey::UsageLimits(subscription_id)`
- **State**: Stored under `DataKey::UsageState(subscription_id)` (contains timestamps, counters, and period usage)
- **Reference**: Stored under `DataKey::UsageReference(subscription_id)`

## Example: Valid Usage vs Rejected Usage
- **Valid Usage**:
  - `charge_usage_with_reference(sub_id, 1_000_000, "txn_123")` succeeds.
  - 5 seconds later: `charge_usage_with_reference(sub_id, 2_000_000, "txn_124")` succeeds (burst interval > 2s).
- **Rejected Usage**:
  - `charge_usage_with_reference(sub_id, 1_000_000, "txn_123")` immediately fails (Replay).
  - Calling 10 times in 1 minute when max_calls=5 fails on the 6th call (RateLimitExceeded).
