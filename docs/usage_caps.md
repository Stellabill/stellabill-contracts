# Usage Caps

Subscriptions with `usage_enabled=true` can define an optional per-period hard cap:

- `usage_cap_units: Option<i128>`

Behavior:

- usage charges increment `current_period_usage_units`
- if a charge would exceed the cap, the transaction is rejected
- cap counters reset when the contract rolls into a new billing period on a successful charge

Notes:

- caps are configured by the merchant for each subscription
- rejection path is deterministic and storage-efficient
- cap-reached events are emitted for off-chain alerting
