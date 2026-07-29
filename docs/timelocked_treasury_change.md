# Timelocked treasury change

Protocol treasury and fee-routing updates now follow a two-step flow:

1. Admin calls `queue_treasury_change` to store a pending change in persistent storage.
2. After 48 hours have elapsed, admin calls `execute_treasury_change` to apply the queued values.

The pending change is stored under `DataKey::PendingTreasuryChange` and can be cancelled before execution with `cancel_treasury_change`.

## Behavior

- Queueing rejects a second pending change while one is already active.
- Execution before the effective timestamp returns `Error::TimelockNotElapsed`.
- Successful execution updates the configured treasury and fee bps, and emits the corresponding events for downstream indexers.
