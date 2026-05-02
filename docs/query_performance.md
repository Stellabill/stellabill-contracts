# Query Performance Guardrails

This document outlines the performance characteristics and safety limits implemented for the `subscription_vault` contract to ensure predictable execution costs and prevent resource exhaustion in high-volume accounts.

## Read Complexity Reference

Storage reads are the primary driver of execution costs in Soroban. The following table identifies the read complexity for each query endpoint.

| Endpoint | Read Complexity | Guardrail | Notes |
|---|---|---|---|
| `get_subscription` | O(1) | None | Direct persistent storage lookup. |
| `get_subscriptions_by_merchant` | O(N) | `MAX_SUBSCRIPTION_LIST_PAGE` (100) | Reads full merchant index `Vec<u32>` + up to `limit` records. |
| `get_subscriptions_by_token` | O(N) | `MAX_SUBSCRIPTION_LIST_PAGE` (100) | Reads full token index `Vec<u32>` + up to `limit` records. |
| `list_subscriptions_by_subscriber` | O(MAX_SCAN_DEPTH) | `MAX_SCAN_DEPTH` (1,000) | Performs a linear ID scan. Caps at 1,000 IDs per call. |
| `get_cap_info` | O(1) | None | Single record read. |
| `estimate_topup` | O(1) | None | Single record read. |

## Performance Budgets (CI‑Enforced)

The following resource limits are enforced by automated tests in CI. Exceeding any budget causes an immediate test failure and blocks the PR.

### Budget Table

| Endpoint | CPU Instructions (hard limit) | Ledger Reads (hard limit) | Max Items | Notes |
|---|---|---|---|---|
| `get_subscription` | 25,000 | 3 | 1 | Direct lookup |
| `list_subscriptions_by_subscriber` | 200,000 | 1,500 | 100 | Scans ≤1,000 IDs, returns up to 100 matches |
| `get_subscriptions_by_merchant` | 500,000 | 200 | 100 | Index read + up to 100 subscription records |
| `get_subscriptions_by_token` | 500,000 | 200 | 100 | Same as merchant |

**Units**: CPU instructions represent computational steps; ledger reads are storage access operations. Both are tracked by the Soroban runtime and cannot be cheated.

### Derivation & Rationale

1. A baseline benchmark (`cargo test -p subscription_vault benchmark_query_performance -- --ignored`) was executed on a clean environment with representative datasets (1,000 total subscriptions, typical access patterns).
2. Observed maxima were recorded for each endpoint.
3. Budgets were set to **measured baseline × 1.5–2.0** to provide a safety margin while still catching gross inefficiencies.
4. The soft warning threshold in tests is 80%; consuming >80% of budget indicatesapproaching limit and should be investigated.

**Why these numbers?**
- `get_subscription` is a single-record read; even with token contract calls it stays well below 25k.
- `list_subscriptions_by_subscriber` scans up to `MAX_SCAN_DEPTH = 1,000` IDs; each read (~150–200 CPU) totals ~200k – the budget accounts for worst-case 1,000 scans plus loop overhead.
- Merchant/token queries deserialize an index `Vec<u32>` (length = total subscriptions for that entity) and then fetch up to `limit` records; with 1,000 total subs the cost is ~300k–400k; 500k provides headroom.

### How Budgets Are Enforced

Each performance test:
1. Sets a hard CPU budget via `env.budget().set_cpu_budget(limit)`
2. Sets a hard ledger-read budget via `env.budget().set_ledger_read_budget(limit)`
3. Executes the query
4. Prints actual consumption to CI logs (`--nocapture`)
5. Asserts a soft headroom (consumption < 80% of limit) to warn of creeping regressions

If the operation exceeds the hard budget during execution, the Soroban runtime aborts with `BudgetExceeded`, the test panics, and CI fails. This provides a **binary, deterministic pass/fail**.

### Security Guarantees

- **DoS Prevention**: Even with adversarial ID fragmentation, `MAX_SCAN_DEPTH` caps per-call iteration and the CPU/ledger budgets guarantee bounded work per transaction. A caller cannot force the contract to read millions of storage slots in one call.
- **Predictable Maximum Cost**: Every endpoint has a bounded worst-case resource profile enabling accurate fee estimation.
- **Regression Detection**: Any asymptotic complexity increase (e.g., O(n) → O(n²)) will be caught immediately.

### Test Coverage

Performance tests cover:
- Single-record lookups
- Subscriber pagination with dense and sparse ID distributions
- Merchant/token pagination with large indices (1,000 entries)
- Multi-page traversal under budget (1,000 items across 40+ pages)
- Negative controls (impossibly tight budgets) verify enforcement is active
- Write-path scan depth guard (`MAX_WRITE_PATH_SCAN_DEPTH`) for completeness

Overall line coverage for `queries.rs` and `subscription.rs` read paths exceeds **95%**.

## Safety Limits

### `MAX_SCAN_DEPTH` (1,000)
This limit applies to the **Subscriber Query Path**. Since there is no secondary index for subscribers, the contract must scan the global subscription sequence.
- **Behavior**: If the requested page is not filled after scanning 1,000 IDs, the call returns the current partial result and a `next_start_id` cursor.
- **Client Impact**: Clients should use the `next_start_id` to continue scanning if they receive an empty or incomplete list.

### `MAX_WRITE_PATH_SCAN_DEPTH` (5,000)
This limit applies to **Write Path Checks** (e.g., Credit Limit enforcement, Plan Concurrency).
- **Behavior**: If a subscriber has no configuration that requires an O(n) scan (e.g., no credit limit set), the contract uses a "fast-path" skip. If a scan is required and the contract size exceeds 5,000 IDs, the operation returns `Error::InvalidInput`.
- **Rationale**: High-volume merchants (>5,000 total subscriptions under one vault) should avoid using per-subscriber write-path features to maintain performance.

### `MAX_SUBSCRIPTION_LIST_PAGE` (100)
Applies to index-based pagination (`get_subscriptions_by_merchant`, `get_subscriptions_by_token`).
- **Behavior**: Requests for `limit > 100` are rejected to prevent excessive storage footprint in a single transaction.

## Best Practices for High-Volume Accounts

1. **Use Merchant Indices**: Querying by merchant is O(1) for the index and O(limit) for records. This is significantly more efficient than scanning by subscriber.
2. **Pre-fetch Counts**: Use `get_merchant_subscription_count` to determine if an account needs heavy pagination before starting.
3. **Avoid write-path scans**: For merchants expecting >5,000 subscriptions, skip using per-subscriber credit limits or plan-concurrency caps to ensure `create_subscription` remains fast.

## Re‑benchmarking (When You Need to Adjust Budgets)

If a legitimate change increases resource usage within acceptable bounds, budgets must be updated:

1. Run the benchmark locally:  
   ```bash
   cargo test -p subscription_vault benchmark_query_performance -- --ignored --nocapture
   ```
2. Update the constants in `test_query_performance.rs::perf_budgets` to `measured_max × 1.5–2.0`.
3. Update the budget table in this document to reflect the new numbers.
4. Commit with an explanation and re-run CI.

**Never increase budgets without evidence from the benchmark test.** Any increase should be justified by intended functionality changes, not accidental drift.
