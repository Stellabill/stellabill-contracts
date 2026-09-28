# Benchmark Methodology & Hardware Baseline

## 1. Overview

This document describes how gas and storage benchmarks are conducted for the
`subscription_vault` contract, the baseline hardware assumed by CI, and the
tolerance thresholds the CI pipeline enforces. External contributors can use
this document to reproduce benchmark results in their own environment.

---

## 2. Benchmark Types

### 2.1 Execution Cost (CPU instructions)

Soroban contracts consume **CPU instructions** tracked by the environment's
budget system. Cost is measured via `Env::budget()`:

- `env.budget().reset_unlimited()` — disables the instruction cap in tests,
  allowing the benchmark to measure raw consumption without artificial limits.
- The Soroban host at the end of a call reports the total CPU instructions
  consumed. This value is **not** directly exposed to contract code but is
  visible in Soroban RPC responses and CLI output (`soroban contract invoke`
  shows cost breakdowns).

**Measurement approach** (not yet automated in CI; manual / CLI-based):

```bash
soroban contract invoke \
  --id <CONTRACT_ID> \
  --fn charge_subscription \
  --arg <ID> \
  --cost
```

The `--cost` flag prints CPU instruction and memory byte totals.

**Relevant code:**
- Budget reset in tests: `contracts/subscription_vault/src/test_query_performance.rs:21`
  (`env.budget().reset_unlimited()`)
- Charge execution: `contracts/subscription_vault/src/charge_core.rs`

### 2.2 Storage footprint

Storage costs on Stellar are a function of **entry count** and **entry size**
(see [CAP-46](https://github.com/stellar/stellar-protocol/blob/master/core/cap-0046.md)).
The contract uses Soroban's instance storage (`env.storage().instance()`) for:

- **Subscription records**: one key-value pair per subscription ID (`u32` key →
  `Subscription` struct as XDR).
- **Merchant balances**: compound key `(Symbol("merchant_balance"), Address,
  Address)` → `i128`.
- **Index entries**: merchant and token indices as `Vec<u32>`.
- **Billing statements**: per-subscription statement rows.
- **Replay protection**: charged-period index and optional idempotency key per
  subscription.
- **Metadata**: per-subscription key-value store (bounded by
  `MAX_METADATA_KEYS`, `MAX_METADATA_KEY_LENGTH`,
  `MAX_METADATA_VALUE_LENGTH`).

**Storage limits enforced by the contract:**

| Constant | Value | Location |
|---|---|---|
| `MAX_SCAN_DEPTH` | 1,000 | `contracts/subscription_vault/src/queries.rs` |
| `MAX_WRITE_PATH_SCAN_DEPTH` | 5,000 | `contracts/subscription_vault/src/subscription.rs` |
| `MAX_SUBSCRIPTION_LIST_PAGE` | 100 | `contracts/subscription_vault/src/queries.rs` |
| `MAX_SUBSCRIPTION_ID` | `u32::MAX` | `contracts/subscription_vault/src/lib.rs` |
| `MAX_EXPORT_LIMIT` | 100 | `contracts/subscription_vault/src/lib.rs` |
| `MAX_METADATA_KEYS` | 50 | `contracts/subscription_vault/src/types.rs` |
| `MAX_METADATA_KEY_LENGTH` | 64 | `contracts/subscription_vault/src/types.rs` |
| `MAX_METADATA_VALUE_LENGTH` | 256 | `contracts/subscription_vault/src/types.rs` |

### 2.3 Query performance guardrails

The contract implements scan-depth budgets for subscriber listing
(`list_subscriptions_by_subscriber`):

- Each call scans at most `MAX_SCAN_DEPTH` IDs (1,000). If the budget is
  exhausted before the page is full, the response includes a `next_start_id`
  cursor for continuation.
- **Rationale**: No secondary index exists for subscribers; a linear scan is
  required. The depth limit bounds worst-case CPU cost per call.

See `docs/query_performance.md` for the full read-complexity reference table.

---

## 3. Baseline Hardware (CI)

All benchmarks and CI tests run on **GitHub-hosted runners** with the following
specification:

| Property | Value |
|---|---|
| Runner image | `ubuntu-latest` (GitHub Actions) |
| CPU | x86_64, 2 vCPUs (Intel Xeon Platinum 8370C or equivalent) |
| RAM | 7 GB |
| Storage | SSD (~14 GB ephemeral) |
| Network | 10 Gbps (shared) |

**Important caveats:**
- GitHub Actions runners are **shared** and may exhibit **noise** from
  co-tenanted workloads. Observed CPU instruction counts for the same operation
  can vary by ±5–10% across runs.
- Benchmarks should **not** be used for absolute performance claims. They are
  intended for **regression detection** — a statistically significant increase
  (+15% or more) in CPU instructions or storage writes after a change indicates
  a potential performance regression.
- To reproduce results locally, ensure `cargo test` passes on the **same Rust
  toolchain** used by CI (`stable`, pinned in
  `.github/workflows/ci.yml`):

```yaml
- name: Install Rust
  uses: dtolnay/rust-toolchain@stable
```

---

## 4. Tolerance & Regression Policy

### 4.1 Pass / Fail criteria

CI (`cargo test --all`) validates **correctness**, not raw performance. There
is **no automated gas-limit or instruction-count threshold** in CI today.
Performance regression detection is a **manual review step**.

The following heuristics apply during code review:

| Signal | Threshold | Action |
|---|---|---|
| New storage writes per call | > 0 (unless a new feature explicitly requires it) | Reviewer flags; author must justify |
| New storage reads per call | > 0 (unless justified by new query functionality) | Reviewer flags; author must justify |
| Estimated CPU instructions | ≥ 15% increase vs. baseline | Author must run `soroban contract invoke --cost` and report diff |
| Scan depth increase | Any change to `MAX_SCAN_DEPTH` or `MAX_WRITE_PATH_SCAN_DEPTH` | Requires team lead approval |
| Budget reset in test | Any new `env.budget().reset_unlimited()` call | Must be accompanied by a comment explaining why unlimited budget is needed |

### 4.2 Hardware upgrade / re-baseline

When CI runner hardware changes (e.g., GitHub updates `ubuntu-latest` to a new
instance type):
1. A committer runs the full test suite and records the output of a cost
   snapshot (see §5).
2. The snapshot is saved to `docs/benchmarks/snapshots/` as
   `baseline-<YYYY-MM-DD>.json`.
3. Previous baselines are retained for reference but marked as superseded.

### 4.3 Noisy runs

If a CI run fails due to what appears to be an infrastructure timeout (not a
test logic failure):
1. Re-run the failed job via the GitHub Actions "Re-run jobs" button.
2. If the failure is consistently reproducible, investigate for a regression.
3. Known-flaky tests (see §4.4) are excluded from pass/fail gate.

### 4.4 Known-flaky benchmarks

No benchmarks are currently flagged as flaky. Any test that exhibits
non-deterministic failures across ≥ 3 CI runs on unrelated commits will be:
1. Moved to a separate `#[ignore]` test annotated with `// FLAKY: <reason>`.
2. Tracked in a GitHub issue labeled `flaky-test`.
3. Fixed before the next release.

---

## 5. Cost Snapshot Format

Committers should produce a cost snapshot when making changes that affect
gas/storage. The snapshot is a JSON file with this structure:

```json
{
  "date": "2026-07-28",
  "commit": "abc123def...",
  "ci_run": "https://github.com/stellabill/contracts/actions/runs/12345",
  "toolchain": "stable-2026-07-01",
  "operations": {
    "create_subscription": {
      "cpu_instructions": 450000,
      "storage_writes": 4,
      "storage_reads": 3
    },
    "charge_subscription": {
      "cpu_instructions": 320000,
      "storage_writes": 3,
      "storage_reads": 4
    },
    "deposit_funds": {
      "cpu_instructions": 280000,
      "storage_writes": 2,
      "storage_reads": 2
    },
    "withdraw_merchant_funds": {
      "cpu_instructions": 250000,
      "storage_writes": 1,
      "storage_reads": 2
    }
  },
  "hardware": {
    "runner": "ubuntu-latest",
    "cpu": "x86_64, 2 vCPUs",
    "ram_gb": 7
  }
}
```

Snapshots are produced by running each operation via the Soroban CLI with
`--cost` and recording the instruction / storage output.

---

## 6. Reproducing Benchmarks Locally

```bash
# 1. Build the WASM
cargo build -p subscription_vault --target wasm32-unknown-unknown --release

# 2. Deploy to a local testnet or standalone network
soroban contract deploy \
  --wasm target/wasm32-unknown-unknown/release/subscription_vault.wasm \
  --network standalone

# 3. Initialize the contract
soroban contract invoke \
  --id <CONTRACT_ID> \
  --fn init \
  --arg <TOKEN_ADDR> --arg 7 --arg <ADMIN_ADDR> --arg 1000000 --arg 604800

# 4. Run operations with cost output
soroban contract invoke \
  --id <CONTRACT_ID> \
  --fn create_subscription \
  --arg <SUBSCRIBER> --arg <MERCHANT> --arg 10000 --arg 2592000 --arg false \
  --cost

# 5. Run unit tests (no network needed)
cargo test --all
```

---

## 7. Cross-Reference: All Bench Files

| File | Description |
|---|---|
| `contracts/subscription_vault/src/test_query_performance.rs` | Tests query scan-depth limits, pagination, and budget reset |
| `contracts/subscription_vault/src/test_deterministic_charging.rs` | Verifies charge determinism across intervals |
| `contracts/subscription_vault/src/test_multi_actor.rs` | Multi-subscriber, multi-merchant concurrent scenarios |
| `contracts/subscription_vault/src/test_reentrancy_invariants.rs` | CEI-pattern verification for all token flows |
| `contracts/subscription_vault/src/test_security.rs` | Security regression pack (auth, replay, overflow) |
| `contracts/subscription_vault/src/test_safe_math_regression.rs` | Arithmetic bounds and overflow regression |
| `contracts/subscription_vault/src/test_expiration.rs` | Subscription expiration and grace-period logic |
| `contracts/subscription_vault/src/test_usage_limits.rs` | Usage rate-limit and cap enforcement |
| `contracts/subscription_vault/src/test_governance.rs` | Admin rotation and protocol-fee governance |
| `contracts/subscription_vault/src/test_recovery.rs` | Fund recovery and emergency-stop paths |
| `contracts/subscription_vault/src/test_emergency_stop_lifetime_caps.rs` | Emergency stop + lifetime cap interaction |
| `docs/query_performance.md` | Query performance guardrail documentation |

---

## 8. Security and Correctness Assumptions

Benchmarks assume the following invariants hold (verified by test suite):

| Assumption | Verified By | Doc Reference |
|---|---|---|
| CEI pattern: state updated before token transfer | `test_reentrancy_invariants.rs` | `docs/reentrancy.md` |
| Replay protection: period index prevents double-charge | `test_reentrancy_invariants.rs:test_charge_replay_rejected_no_state_mutation` | `docs/replay_protection.md` |
| Safe math: checked arithmetic prevents overflow/underflow | `test_safe_math_regression.rs` | `docs/safe_math.md` |
| State machine: invalid transitions are rejected | `test_security.rs` + `state_machine.rs` | `docs/subscription_state_machine.md` |
| Emergency stop: all mutating entry-points blocked | `test_emergency_stop_lifetime_caps.rs` | `docs/security.md` |
| Lifetime cap: auto-cancel on cap exhaustion | `test_emergency_stop_lifetime_caps.rs` | `docs/lifetime_caps.md` |
| Scan-depth limits: subscriber query bounded by `MAX_SCAN_DEPTH` | `test_query_performance.rs` | `docs/query_performance.md` |
| Budget reset: only `test_query_performance.rs` uses `env.budget().reset_unlimited()` | `test_query_performance.rs:21` | §2.1 above |

---

## 9. Document Maintenance

- **Update frequency**: Every time a new storage-write or read-introducing
  feature is added.
- **Author**: The PR author is responsible for updating the relevant cost
  snapshot and this document.
- **Review**: A maintainer must verify that new storage operations are
  justified and that the `MAX_*` constants remain appropriate.