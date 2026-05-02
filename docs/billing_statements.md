# Billing Statements (Past Periods)

## Purpose

`PeriodBillingStatement` persists a compact per-period billing record optimised for
Past Periods UX.  It is written at period close, on cancellation, and on final
settlement via the `finalize_billing_statement` admin entrypoint.

This complements the lower-level per-charge audit trail stored by `statements.rs`.
Both systems coexist: the per-charge rows give a detailed event log; period
statements give a denormalised, UI-friendly summary per billing interval.

---

## Schema — `PeriodBillingStatement`

| Field | Type | Description |
|---|---|---|
| `subscription_id` | `u32` | Subscription this period belongs to |
| `period_index` | `u32` | 0-indexed period counter for this subscription |
| `snapshot_period_index` | `u32` | Associated billing snapshot period (defaults to `period_index`) |
| `merchant` | `Address` | Merchant receiving payment |
| `subscriber` | `Address` | Subscriber who was charged |
| `token` | `Address` | Settlement token (resolved from contract storage) |
| `period_start_timestamp` | `u64` | Ledger timestamp of period start |
| `period_end_timestamp` | `u64` | Ledger timestamp of period end |
| `total_amount_charged` | `i128` | Sum of all charges (interval + usage + one-off) this period |
| `total_usage_units` | `i128` | Metered usage units billed (0 for non-usage subscriptions) |
| `protocol_fee_amount` | `i128` | Protocol fee withheld (0 if fee routing disabled) |
| `net_amount_to_merchant` | `i128` | `total_amount_charged − protocol_fee_amount` |
| `refund_amount` | `i128` | Total refunded to subscriber this period |
| `status_flags` | `u32` | Bit flags — see constants below |
| `subscription_status` | `SubscriptionStatus` | Subscription lifecycle state at finalization (resolved from storage) |
| `finalized_by` | `BillingStatementFinalization` | How this period was closed |
| `finalized_at` | `u64` | Ledger timestamp at which the statement was written |

### `status_flags` bit constants

| Constant | Value | Meaning |
|---|---|---|
| `STMT_FLAG_INTERVAL_CHARGED` | `0x01` | At least one interval charge this period |
| `STMT_FLAG_USAGE_CHARGED`    | `0x02` | At least one usage charge this period |
| `STMT_FLAG_ONEOFF_CHARGED`   | `0x04` | At least one one-off charge this period |
| `STMT_FLAG_CANCELLED`        | `0x08` | Subscription was cancelled during this period |
| `STMT_FLAG_SETTLED`          | `0x10` | Subscriber withdrew remaining balance; period fully settled |

### `BillingStatementFinalization` variants

| Variant | Meaning |
|---|---|
| `PeriodClosed` | Recurring period closed normally after a successful charge |
| `Cancellation` | Subscription was cancelled; covers the current partial period |
| `FinalSettlement` | Subscriber withdrew remaining prepaid balance; net settlement recorded |

---

## Storage layout

### Primary record

```
DataKey::BillingStatement(subscription_id: u32, period_index: u32)
  → PeriodBillingStatement
```

### Secondary indices

```
DataKey::BillingStatementsBySubscription(subscription_id: u32)
  → Vec<BillingStatementRef>

DataKey::BillingStatementsByMerchant(merchant: Address)
  → Vec<BillingStatementRef>
```

`BillingStatementRef` stores `(subscription_id, period_index, period_end_timestamp)`.
`period_end_timestamp` is kept in the ref so time-range merchant queries can filter
the index without loading each full statement record.

---

## Contract entrypoints

### Write

```
finalize_billing_statement(
    admin: Address,                     -- must be stored admin
    subscription_id: u32,
    period_index: u32,
    subscriber: Address,
    merchant: Address,
    period_start_timestamp: u64,
    period_end_timestamp: u64,
    amounts: PeriodStatementAmounts,    -- grouped financial fields
    status_flags: u32,
    finalized_by: BillingStatementFinalization,
) → Result<(), Error>
```

`PeriodStatementAmounts` groups the five financial fields to keep the contract
function within Soroban's 10-parameter limit:

```
PeriodStatementAmounts {
    total_amount_charged: i128,
    total_usage_units: i128,
    protocol_fee_amount: i128,
    net_amount_to_merchant: i128,
    refund_amount: i128,
}
```

`subscription_status` is resolved automatically from live subscription storage —
callers do not supply it.  The call fails with `Error::NotFound` if no subscription
exists for `subscription_id`.

Calling `finalize_billing_statement` with an existing `(subscription_id, period_index)`
pair **overwrites** the record and does not add duplicate index entries (idempotent).

### Read

```
get_billing_statement(subscription_id, period_index) → Result<PeriodBillingStatement, Error>
get_bill_stmts_by_sub(subscription_id, start, limit) → Vec<PeriodBillingStatement>
get_bill_stmts_by_merch_rng(merchant, start_ts, end_ts, start, limit) → Vec<PeriodBillingStatement>
```

- `get_bill_stmts_by_sub` returns statements in append order (oldest first).
  Combine with `start` / `limit` for cursor-style pagination.
- `get_bill_stmts_by_merch_rng` filters by `period_end_timestamp ∈ [start_ts, end_ts]`
  before applying `start` / `limit`.  Pass `start_ts = 0, end_ts = u64::MAX` for all periods.

---

## Lifecycle hooks

Period statements are written by off-chain tooling or indexers that call
`finalize_billing_statement` after processing on-chain charge events.  Typical
triggers:

- **Period close** — after a successful `charge_subscription` call; use `PeriodClosed`.
- **Cancellation** — after `cancel_subscription`; use `Cancellation`.
- **Final settlement** — after a subscriber withdraws remaining prepaid balance;
  use `FinalSettlement`, set `refund_amount` to the withdrawn amount, and
  set `STMT_FLAG_SETTLED`.

---

## Event coordination

Each `finalize_billing_statement` call emits:

```
topic:   "bill_stmt"
payload: BillingStatementPersistedEvent {
    subscription_id: u32,
    period_index: u32,
    merchant: Address,
    finalized_by: BillingStatementFinalization,
}
```

Off-chain systems can cross-check statements against:
- `charged` — per-charge debit events
- `fee` — protocol fee events
- `deposited` — top-up events
- `lifetime_cap_reached` — cap exhaustion events
- `migration_export` — snapshot exports

---

## Relationship to per-charge audit log

`statements.rs` maintains an **append-only per-charge row** for each
`charge_subscription`, `charge_usage`, and `charge_one_off` call. Those rows are
queryable via `get_sub_statements_offset` and `get_sub_statements_cursor`, and can
be compacted via `compact_billing_statements`.

`billing_statements.rs` maintains **one period summary per billing interval**, written
by the caller after one or more charges close a period.  Compaction of per-charge rows
does **not** affect period statements.

Both systems share `subscription_id` as the primary join key.

---

## Example query patterns

### Merchant Past Periods list

```
get_bill_stmts_by_merch_rng(merchant, from_ts, to_ts, 0, 20)
```

### Subscription drill-down (all periods)

```
get_bill_stmts_by_sub(subscription_id, 0, 100)
```

### Single period detail

```
get_billing_statement(subscription_id, period_index)
```

---

## Security notes

- `finalize_billing_statement` requires admin authentication.
- Statements cannot be forged without the admin key.
- An overwrite is allowed (idempotent) to support corrections by the admin.
- The contract does **not** automatically finalize statements; all writes are explicit.
- Upserts do not remove or modify per-charge audit rows.
