# Usage Rate Limits

Usage charge calls now support per-subscription rate limiting.

Configuration:

- `usage_rate_limit_max_calls: Option<u32>`
- `usage_rate_window_secs: u64`

Enforcement:

- a rolling fixed window counter is kept in contract storage
- when calls in the active window reach the configured maximum, additional usage charges are rejected
- counters reset automatically once the window expires

Storage footprint is bounded:

- one window start timestamp and one call counter per subscription
