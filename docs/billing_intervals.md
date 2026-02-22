# Billing Interval Enforcement

How `charge_subscription` enforces timing between charges.

---

## Rule

A charge is allowed when:

```
env.ledger().timestamp() >= last_payment_timestamp + interval_seconds
```

The comparison is **inclusive** — a charge at exactly the boundary succeeds.

---

## Outcomes

| Condition | Result | Storage |
|-----------|--------|---------|
| `now < last_payment + interval` | `Error::IntervalNotElapsed` | Unchanged |
| `now >= last_payment + interval` | Ok | `last_payment_timestamp = now` |
| Subscription not Active | `Error::NotActive` | Unchanged |
| Subscription not found | `Error::NotFound` | Unchanged |

---

## Timestamp source

All timing uses the Soroban ledger timestamp (`env.ledger().timestamp()`), a Unix epoch value in seconds controlled by the Stellar validator network.

---

## Window reset

On success, `last_payment_timestamp` is set to the **current ledger timestamp**, not `last_payment_timestamp + interval_seconds`. This means late charges shift the next window forward rather than allowing a cascade of back-to-back catch-up charges.

### Example (30-day interval)

```
T0 = creation          → last_payment_timestamp = T0
T0 + 30d               → charge succeeds, last_payment_timestamp = T0 + 30d
T0 + 30d               → immediate retry rejected (IntervalNotElapsed)
T0 + 60d               → next charge succeeds
```

---

## First charge

`last_payment_timestamp` is initialised to `env.ledger().timestamp()` at subscription creation, so the first charge cannot occur until `interval_seconds` later.

---

## Ledger time monotonicity

Soroban ledger timestamps are set by Stellar validators and are expected to be **non-decreasing** across ledger closes (~5-6 s on mainnet). The contract does **not** assume strict monotonicity — it only checks `now >= last_payment_timestamp + interval_seconds`. Consequences:

* If two consecutive ledgers share the same timestamp (same second), a charge that just succeeded will simply be rejected on the next call because `0 < interval_seconds`.
* The contract never compares the current timestamp to a "previous ledger timestamp"; it only compares against its own stored `last_payment_timestamp`.
* Validators producing timestamps that move backward would violate the Stellar protocol; the contract does not defend against that scenario.

---

## Test coverage

| Test | Scenario |
|------|----------|
| `test_charge_rejected_before_interval` | 1 s before boundary — rejected, storage unchanged |
| `test_charge_succeeds_at_exact_interval` | Exact boundary — succeeds, timestamp updated |
| `test_charge_succeeds_after_interval` | Well past boundary — succeeds, timestamp updated |
| `test_immediate_retry_at_same_timestamp_rejected` | Same-timestamp retry after success — rejected |
| `test_repeated_charges_across_many_intervals` | 6 consecutive interval charges + trailing retry — all correct |
| `test_one_second_interval_boundary` | 1-second interval: creation time fails, T0+1 succeeds |
