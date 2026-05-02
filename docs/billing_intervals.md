# Billing Interval Enforcement

How `charge_subscription` validates and enforces timing between charges.

---

## Interval constraints

| Bound | Value | Constant |
|-------|-------|----------|
| Minimum | 60 s (1 minute) | `MIN_SUBSCRIPTION_INTERVAL_SECONDS` |
| Maximum | 31 536 000 s (365 days) | `MAX_SUBSCRIPTION_INTERVAL_SECONDS` |

`interval_seconds = 0` is implicitly rejected because zero is below the minimum.

Validation is performed by the single authoritative helper `validate_interval(interval_seconds)` at every entry point that persists an interval:

- `create_subscription` / `create_subscription_with_token`
- `create_plan_template` / `create_plan_template_with_token`
- `update_plan_template`

---

## Canonical time-math formula

The **next allowed charge timestamp** is computed by the single helper:

```
next_charge_time(last_payment, interval) = last_payment + interval
```

Implemented via checked addition; returns `Err(Overflow)` if the result
would exceed `u64::MAX`.  In practice this cannot happen for validated
intervals (≤ 365 days) and real Stellar ledger timestamps.

Both the **charge path** (`charge_core.rs`) and the **query path**
(`queries.rs`) import this same function from `subscription.rs` so that
the boundary each enforces or displays is identical.

---

## Charge rule

A charge is allowed when:

```
env.ledger().timestamp() >= last_payment_timestamp + interval_seconds
```

The comparison is **inclusive** — a charge at exactly the boundary succeeds.
The comparison at exactly `last_payment_timestamp + interval_seconds - 1` fails.

### Outcomes

| Condition | Result | Storage |
|-----------|--------|---------|
| `now < last_payment + interval` | `Error::IntervalNotElapsed` | Unchanged |
| `now >= last_payment + interval` | Ok | `last_payment_timestamp = now` |
| Subscription not Active/GracePeriod | `Error::NotActive` | Unchanged |
| Subscription not found | `Error::NotFound` | Unchanged |

---

## Timestamp source

All timing uses the Soroban ledger timestamp (`env.ledger().timestamp()`), a
Unix epoch value in seconds controlled by the Stellar validator network.

---

## Window reset

On success, `last_payment_timestamp` is set to the **current ledger
timestamp**, not `last_payment_timestamp + interval_seconds`.  This means
late charges shift the next window forward rather than allowing a cascade of
back-to-back catch-up charges.

### Example (30-day interval)

```
T0 = creation          → last_payment_timestamp = T0
T0 + 30d               → charge succeeds, last_payment_timestamp = T0 + 30d
T0 + 30d (immediate)   → retry rejected (IntervalNotElapsed)
T0 + 60d               → next charge succeeds
```

---

## First charge

`last_payment_timestamp` is initialised to `env.ledger().timestamp()` at
subscription creation, so the first charge cannot occur until
`interval_seconds` later.

---

## Ledger time monotonicity

Soroban ledger timestamps are set by Stellar validators and are expected to
be **non-decreasing** across ledger closes (~5-6 s on mainnet).  The contract
does **not** assume strict monotonicity — it only checks
`now >= last_payment_timestamp + interval_seconds`.  Consequences:

- If two consecutive ledgers share the same timestamp, a charge that just
  succeeded will simply be rejected on the next call because `0 < interval_seconds`.
- The contract never compares the current timestamp to a "previous ledger
  timestamp"; it only compares against its own stored `last_payment_timestamp`.
- Validators producing timestamps that move backward would violate the Stellar
  protocol; the contract does not defend against that scenario.

---

## Security notes

### Double-charge prevention

The boundary check `now >= last + interval` uses checked addition, so there
is no risk of wrap-around confusion.  The integer division-based replay guard
(`period_index = now / interval`) provides a second, independent barrier
against charging twice within the same period.

### Overflow

`next_charge_time` uses `u64::checked_add` and propagates `Error::Overflow`
rather than silently wrapping.  The maximum interval (365 days) means the
furthest future timestamp that can be stored is
`u64::MAX_REALISTIC_TIMESTAMP + 365 days`, which is far below `u64::MAX`
for any foreseeable ledger.

The query path (`compute_next_charge_info`) calls the same helper and clamps
to `u64::MAX` on the (unreachable) overflow path so that display code always
receives a valid timestamp without panicking.

---

## Test coverage

| Test | Scenario |
|------|----------|
| `test_interval_zero_rejected` | `interval = 0` — creation rejected |
| `test_interval_below_min_rejected` | `interval = MIN - 1` — creation rejected |
| `test_interval_at_min_accepted` | `interval = MIN (60 s)` — creation accepted |
| `test_interval_above_max_rejected` | `interval = MAX + 1` — creation rejected |
| `test_interval_at_max_accepted` | `interval = MAX (365 d)` — creation accepted |
| `test_plan_template_interval_below_min_rejected` | Plan template: `interval < MIN` — rejected |
| `test_plan_template_interval_at_min_accepted` | Plan template: `interval = MIN` — accepted |
| `test_plan_template_interval_above_max_rejected` | Plan template: `interval > MAX` — rejected |
| `test_plan_template_interval_at_max_accepted` | Plan template: `interval = MAX` — accepted |
| `test_charge_at_exact_boundary_succeeds` | `now = last + interval` — charge ok |
| `test_charge_one_second_before_boundary_rejected` | `now = last + interval - 1` — IntervalNotElapsed |
| `test_charge_past_boundary_succeeds` | `now >> last + interval` — charge ok |
| `test_window_resets_to_now_after_charge` | Window reset semantics verified |
| `test_max_interval_boundary` | `interval = MAX`: boundary and just-before verified |
| `test_compute_next_charge_info_max_interval_no_overflow` | MAX interval query: no overflow, correct value |
| `test_next_charge_info_matches_charge_enforcement` | Query timestamp == charge enforcement threshold |
| `test_consecutive_interval_charges_no_drift` | 6 consecutive charges at exact boundaries + trailing rejection |
