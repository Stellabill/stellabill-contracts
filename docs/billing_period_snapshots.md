# Billing Period Snapshots

Each subscription now stores a compact `BillingPeriodSnapshot` per closed period, keyed by:

- subscription id
- period index (starting at 0 from subscription creation)

Each snapshot records:

- period start and end timestamps
- total amount charged during that period
- total usage units charged during that period
- status flags (closed, interval charged, usage charged, empty)

Snapshots are written only when a successful charge closes one or more elapsed periods.
Failed charges do not create snapshots.

Retention strategy:

- snapshots are immutable once written
- old snapshots can be pruned or compacted by off-chain indexers after export
- period index ordering preserves historical continuity even if old records are archived
