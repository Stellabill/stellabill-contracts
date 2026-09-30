# Billing Period Snapshots

Each subscription now stores a compact `BillingPeriodSnapshot` per closed period, keyed by:

- subscription id
- period index (starting at 0 from subscription creation)

Each snapshot records:

- period start and end timestamps
- total amount charged during that period
- total usage units charged during that period
- status flags (closed, interval charged, usage charged, empty)

Snapshots are written by two charge paths:

- Interval charges: write a snapshot with SNAPSHOT_FLAG_CLOSED and SNAPSHOT_FLAG_INTERVAL_CHARGED,
  closing the period so it may no longer be updated.
- Usage charges: write a snapshot with SNAPSHOT_FLAG_USAGE_CHARGED (no CLOSED flag), accumulating
  into the currently open period. Multiple usage charges in the same period are merged additively.

period_index = (timestamp - subscription.start_time) / subscription.interval_seconds,
starting at 0 from subscription creation.

Failed charges do not create snapshots.

Retention strategy:

- snapshots with SNAPSHOT_FLAG_CLOSED set are immutable and any attempt to overwrite them is
  rejected with InvalidStatusTransition
- open snapshots (without SNAPSHOT_FLAG_CLOSED) may be updated by subsequent charges in the
  same period via additive merge; period_start from the first write is preserved, period_end
  and finalized_at take the maximum across all writes, and status_flags are OR'd together
- old snapshots can be pruned or compacted by off-chain indexers after export
- period index ordering preserves historical continuity even if old records are archived

Closed-flag immutability invariant:

- Once a snapshot is stored with SNAPSHOT_FLAG_CLOSED set, write_period_snapshot refuses to
  overwrite it. This prevents interval or usage charges from modifying a finalized period record.

## Integrity Verification

Snapshots include built-in integrity checks enforced by `write_period_snapshot`:

- Period boundaries: period_start <= period_end (InvalidInput on violation)
- Interval charges require period_start < period_end (InvalidInput on violation)
- Amount validation: total_charged >= 0 for any non-EMPTY snapshot (InvalidInput on violation)
- Closed-flag guard: if an existing snapshot for (subscription_id, period_index) already has
  SNAPSHOT_FLAG_CLOSED, any further write to that key is rejected (InvalidStatusTransition)
- Merge arithmetic: total_charged and total_usage_units are accumulated via checked_add so
  overflow is detected and reported as Overflow
- Sequencing: monotonic sequence numbers across all charge kinds
- Compaction aggregates match pruned statement sums

These invariants ensure data consistency for reporting pipelines and prevent corruption from invalid inputs.

## Usage for Reporting Pipelines

Snapshots serve as the primary data source for billing reports:

- Each snapshot represents a complete billing period
- Compacted aggregates provide summary data for pruned periods
- Status flags indicate charge types processed in each period
- Timestamps enable temporal analysis and period alignment
- Immutable nature ensures audit trail integrity
