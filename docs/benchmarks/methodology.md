# Benchmark Methodology & Performance Baseline

## 1. Overview & Purpose

This document defines the canonical benchmarking methodology, execution hardware baseline, budget regression policy, and reproducibility instructions for gas and storage benchmarks in the `subscription_vault` contract workspace.

### Core Objectives
- **Resource Measurement**: Quantify CPU instruction consumption, storage footprint (read/write entries and byte volume), and memory usage across contract entrypoints and state access patterns.
- **Regression Detection**: Catch accidental performance degradations or unintended storage growth during pull request CI checks before deployment to Stellar testnet/mainnet.
- **Scale Verification**: Validate O(1) performance guarantees for hot paths (e.g., subscription charges, idempotency checks, merchant withdrawals) and enforce bounded linear scaling limits for query scans and governance quorum tallies.
- **Reproducibility**: Provide external contributors with exact instructions and hardware baselines to replicate, verify, and interpret benchmark measurements.

### Why Reproducible Measurements Matter
Soroban smart contracts operate under strict transaction budget caps imposed by the Stellar network protocol. Unintended increases in CPU instructions or storage entries increase transaction fees, risk budget exhaustion errors (`HostEvent::BudgetExceeded`), and can degrade protocol throughput. Standardized, reproducible benchmarking ensures all optimizations preserve safety while adhering to network execution limits.

---

## 2. Benchmark Suite Architecture & File Inventory

The benchmark suite consists of three distinct tiers:
1. **Hot-Path Microbenchmarks** (`contracts/subscription_vault/benches/`): Focused benchmarks isolating specific execution paths, storage access patterns, and scaling curves.
2. **Gas & Storage Budget Regression Tests** (`contracts/subscription_vault/tests/gas_budget.rs` & `src/test_query_performance.rs`): CI-integrated tests enforcing strict upper budget limits on mutating and query entrypoints.
3. **Scale & Soak Benchmarks** (`contracts/subscription_vault/tests/soak_100k.rs`): Stress tests evaluating query bounds under massive state volume (100,000 active subscriptions).

### Comprehensive Benchmark File Inventory

| File Path | Target / Module | Primary Purpose & Scope |
|---|---|---|
| [`contracts/subscription_vault/benches/batch_charge_scaling.rs`](file:///c:/Users/ICT%20LASIEC/stellabill-contracts/contracts/subscription_vault/benches/batch_charge_scaling.rs) | `batch_charge_scaling` | Measures CPU instruction scaling when charging batches of subscriptions. |
| [`contracts/subscription_vault/benches/charge_cold_warm.rs`](file:///c:/Users/ICT%20LASIEC/stellabill-contracts/contracts/subscription_vault/benches/charge_cold_warm.rs) | `charge_cold_warm` | Evaluates cold storage path (persistent read miss) vs warm storage path (cached entry) for `charge_subscription`. |
| [`contracts/subscription_vault/benches/dispute_lifecycle.rs`](file:///c:/Users/ICT%20LASIEC/stellabill-contracts/contracts/subscription_vault/benches/dispute_lifecycle.rs) | `dispute_lifecycle` | Measures CPU and storage resources across the dispute workflow: open → respond → resolve. |
| [`contracts/subscription_vault/benches/gov_tally.rs`](file:///c:/Users/ICT%20LASIEC/stellabill-contracts/contracts/subscription_vault/benches/gov_tally.rs) | `gov_tally` | Quantifies voter-count scaling for `calculate_quorum` at 10, 100, and 1,000 voters. |
| [`contracts/subscription_vault/benches/idem_lookup.rs`](file:///c:/Users/ICT%20LASIEC/stellabill-contracts/contracts/subscription_vault/benches/idem_lookup.rs) | `idem_lookup` | Asserts constant-time O(1) ring-buffer lookup cost regardless of query position. |
| [`contracts/subscription_vault/benches/ttl_extension.rs`](file:///c:/Users/ICT%20LASIEC/stellabill-contracts/contracts/subscription_vault/benches/ttl_extension.rs) | `ttl_extension` | Benchmarks storage TTL extension overhead (`extend_subscription_ttl`). |
| [`contracts/subscription_vault/benches/withdraw_fixed_cost.rs`](file:///c:/Users/ICT%20LASIEC/stellabill-contracts/contracts/subscription_vault/benches/withdraw_fixed_cost.rs) | `withdraw_fixed_cost` | Pins fixed CPU cost for `withdraw_merchant_funds` across single-token, multi-token, and zero-balance scenarios. |
| [`contracts/subscription_vault/tests/gas_budget.rs`](file:///c:/Users/ICT%20LASIEC/stellabill-contracts/contracts/subscription_vault/tests/gas_budget.rs) | `gas_budget` | Enforces conservative 2× upper budget caps for `create_subscription`, `deposit_funds`, `charge_subscription`, and `withdraw_merchant_funds`. |
| [`contracts/subscription_vault/src/test_query_performance.rs`](file:///c:/Users/ICT%20LASIEC/stellabill-contracts/contracts/subscription_vault/src/test_query_performance.rs) | `query_performance` | Validates query scan-depth limits (`MAX_SCAN_DEPTH`), pagination cursors, and query execution budgets. |
| [`contracts/subscription_vault/tests/soak_100k.rs`](file:///c:/Users/ICT%20LASIEC/stellabill-contracts/contracts/subscription_vault/tests/soak_100k.rs) | `soak_100k` | Long-running soak test validating query budgets under 100,000 subscriptions. |

---

## 3. Metrics & Measurement Methodology

Soroban contracts execute within an environment budget managed by the Soroban Host. Benchmarks capture five core resource metrics:

1. **CPU Instructions (`instructions`)**: Total host instructions consumed during contract invocation.
2. **Ledger Read Entries (`read_entries`)**: Number of distinct ledger key-value pairs read from persistent or instance storage.
3. **Ledger Write Entries (`write_entries`)**: Number of distinct ledger key-value pairs written or updated in storage.
4. **Ledger Read Bytes (`read_bytes`)**: Total byte size of all key-value entries read from storage.
5. **Ledger Write Bytes (`write_bytes`)**: Total byte size of all key-value entries written to storage.

### How Results Should Be Interpreted

- **Host Instruction & Storage Determinism**: CPU instruction counts and ledger read/write entry counts generated by `soroban-sdk` testutils are **deterministic** for a given contract byte-code and state input. They reflect exact host invocation mechanics.
- **Wall-Clock Time Variance**: Execution wall-clock time (`std::time::Instant`) is **non-deterministic** and sensitive to CPU throttling, runner co-tenancy, and OS scheduling. Benchmarks focus primarily on CPU instruction and storage metrics.
- **Headroom & Conservative Budgets**: In `gas_budget.rs`, limits are set to 2× measured baselines to accommodate host SDK upgrades while remaining strict enough to detect order-of-magnitude architectural regressions.

---

## 4. `env.cost_estimate` Usage & Mechanics

Soroban provides the `env.cost_estimate()` interface within the `testutils` framework to inspect environment resource usage during testing.

### API Mechanics

```rust
// 1. Reset budget before calling target function to isolate setup overhead
env.cost_estimate().budget().reset_unlimited();

// 2. Execute target contract function
vault.charge_subscription(&sub_id, &None);

// 3. Inspect reported resources
let resources = env.cost_estimate().resources();
let cpu_instructions = resources.instructions.max(0) as u64;
let read_entries = resources.read_entries as u64;
let write_entries = resources.write_entries as u64;
let read_bytes = resources.read_bytes as u64;
let write_bytes = resources.write_bytes as u64;
```

### Purpose of `reset_unlimited()`
By default, the Soroban test environment enforces network transaction limits. Calling `env.cost_estimate().budget().reset_unlimited()` resets the CPU and memory instruction accumulators to zero and removes the artificial instruction cap. This allows test harnesses to perform arbitrary setup (e.g. minting tokens, initializing merchant configurations, setting up 10,000 subscriptions) without hitting host budget errors, and ensures that `resources()` reflects *only* the specific contract function under test.

### Assumptions & Limitations of `env.cost_estimate()`
- **Mock Host Context**: `env.cost_estimate()` runs inside the in-memory Rust test harness (`soroban-sdk` testutils). While instruction tracking and ledger access counts mirror on-chain behavior, disk I/O latency, network latency, and fee metering are not part of the local test harness.
- **Budget Reset Isolation**: `reset_unlimited()` must be called immediately prior to invoking the target function; otherwise, preceding setup operations will pollute measured instruction counts.
- **Host SDK Versioning**: Upgrades to the `soroban-sdk` or Soroban host runtime may shift baseline instruction counts slightly (e.g. host function overhead adjustments).

### `env.cost_estimate()` vs. Functional Tests

| Dimension | Functional Tests (`cargo test --all`) | Cost Estimate Benchmarks |
|---|---|---|
| **Primary Goal** | Verify business logic, authorization, error conditions, and state mutations. | Quantify CPU, storage read/write, and memory resource consumption. |
| **Pass/Fail Criteria** | Boolean assertions, expected return values, `panic!` / `Error` matching. | Enforce numerical bounds (`cpu <= CPU_LIMIT`, `reads <= READ_LIMIT`, budget fixture delta <= tolerance). |
| **Scope** | All contract functions and edge cases. | Hot-path entrypoints, queries, scale-sensitive loops. |

---

## 5. CI Hardware Baseline

CI benchmark checks run on GitHub-hosted runners to ensure consistent execution environments for PR verification.

### Baseline Hardware Specifications

| Specification | GitHub-Hosted Runner Assumption |
|---|---|
| **Runner Image** | `ubuntu-latest` (Ubuntu 22.04 LTS) |
| **CPU Architecture** | `x86_64` |
| **vCPUs** | 2 vCPUs (Intel Xeon Platinum 8370C / AMD EPYC 7763 or equivalent) |
| **RAM** | 7 GB |
| **Ephemeral Storage** | SSD (~14 GB available) |
| **Rust Toolchain** | Pinned `stable` via `dtolnay/rust-toolchain@stable` (Edition 2021) |

### CI Workflow Integration

Benchmarks and performance budgets are executed automatically via GitHub Actions workflows:

1. **`performance-budgets` Job** in [`.github/workflows/ci.yml`](file:///.github/workflows/ci.yml):
   - Runs on every `push` and `pull_request` to `main`.
   - Executes `cargo test -p subscription_vault --test query_performance -- --nocapture`.
   - Executes `cargo test -p subscription_vault --test gas_budget -- --nocapture`.
   - Uses `--nocapture` to print `[Budget]` and `[Perf]` lines to CI logs for graphing and review.
2. **`max-subscriptions-soak` Job** in [`.github/workflows/soak.yml`](file:///.github/workflows/soak.yml):
   - Runs on daily schedule (`0 8 * * *`) or manual dispatch.
   - Executes `cargo test --release -p subscription_vault --test soak_100k soak_100k -- --ignored --nocapture`.

---

## 6. Regression Policy & Evaluation Standards

### Tolerance Thresholds

| Metric / Benchmark | Tolerance / Threshold | Action on Violation |
|---|---|---|
| **Hard Budget Limits** (`gas_budget.rs`, `query_performance.rs`) | `cpu <= LIMIT`, `reads <= LIMIT`, `writes <= LIMIT` | CI test failure; PR merge **blocked**. |
| **Soft Warning Threshold** (`WARN_THRESHOLD` in `gas_budget.rs`) | `80.0%` of budget limit | Prints `[Warn]` line in CI output for maintainer visibility. |
| **Fixture Baseline Delta** (`benches/fixtures/*.json`) | `max_delta_tolerance_pct` (typically `10.0%`) | Test assertion failure; PR merge **blocked**. |
| **Instruction Count Regression** | Sustained `≥ 15%` increase vs baseline without logic change | Reviewer flag; author must run local benchmark & explain diff. |
| **Storage Operations** | Unexpected `> 0` new read/write entries per call | Requires explicit architecture approval in PR review. |

### Evaluation & Enforcement Workflow

1. **Automated CI Check**: The `performance-budgets` job runs on every PR. Any failure in `gas_budget.rs` or benchmark integration tests fails the job and prevents PR approval.
2. **Log Review**: Reviewers check `[Budget]` and `[Perf]` output in CI run logs to verify delta trends.
3. **Intentional Baseline Updates**: If a new feature or security fix legitimately increases CPU usage or storage footprint:
   - The author must run the benchmark suite locally with `-- --nocapture`.
   - The author updates the budget constants in `tests/gas_budget.rs`, `src/test_query_performance.rs`, or fixture JSON files in `benches/fixtures/`.
   - The PR description must explicitly document before/after instruction counts and provide technical justification for the increase.

---

## 7. Edge Cases, Variance & Troubleshooting

### 7.1 Noisy Benchmark Runs
GitHub Actions runners run on shared physical hardware. While CPU instruction counts in Soroban host simulations are deterministic, wall-clock timing or VM context switching can occasionally introduce minor runner noise.
- **Handling**: If a CI job fails due to an infrastructure timeout or transient environment glitch, maintainers should re-trigger the job using GitHub Actions "Re-run failed jobs".

### 7.2 Hardware / Toolchain Upgrades & Re-Baselining
When updating the Rust toolchain version, `soroban-sdk` dependency version, or GitHub runner image:
1. Run the benchmark suite across all targets.
2. Review fixture outputs in `benches/fixtures/*.json` (`charge_cold_warm_budget.json`, `dispute_lifecycle_budget.json`, `withdraw_fixed_cost_budget.json`).
3. Update fixture baselines and budget constants if host instruction accounting has shifted across the board.
4. Commit updated fixture JSON files with a commit message referencing the SDK/toolchain version update.

### 7.3 Known-Flaky Benchmarks
- No benchmarks are currently classified as flaky.
- **Policy**: If a benchmark exhibits non-deterministic failure across multiple CI runs on unchanged code:
  1. It must be annotated with `#[ignore]` and tagged with `// FLAKY: <issue-number> <description>`.
  2. A tracking issue labeled `flaky-test` must be opened immediately.
  3. The root cause must be resolved before the next release.

### 7.4 Interpreting Unexpected Variance
- **Instruction count changed, read/write entries unchanged**: Indicates code-level logic changes (e.g. added loops, new serialization, extra helper calls).
- **Read/write entries changed**: Indicates storage model changes (e.g. new DataKey created, extra instance storage lookups).
- **Instruction count unchanged, wall-clock duration spiked**: Indicates runner CPU throttling / OS scheduling noise, not a smart contract regression.

---

## 8. Reproducibility Guide for Contributors

Contributors can run all benchmark and performance budget tests locally.

### Running Performance & Budget Tests Locally

```bash
# 1. Run mutating entrypoint gas budget tests (with printed output)
cargo test -p subscription_vault --test gas_budget -- --nocapture

# 2. Run query performance budget tests
cargo test -p subscription_vault --test query_performance -- --nocapture

# 3. Run individual hot-path benchmarks
cargo test -p subscription_vault --test charge_cold_warm -- --nocapture
cargo test -p subscription_vault --test dispute_lifecycle -- --nocapture
cargo test -p subscription_vault --test withdraw_fixed_cost -- --nocapture
cargo test -p subscription_vault --test idem_lookup -- --nocapture
cargo test -p subscription_vault --test gov_tally -- --nocapture
cargo test -p subscription_vault --test ttl_extension -- --nocapture

# 4. Run full test suite across workspace
cargo test --all
```

### Measuring On-Chain Invocation Costs via Soroban CLI

To measure contract costs in a live local network or standalone environment:

```bash
# Build WASM binary
cargo build -p subscription_vault --target wasm32-unknown-unknown --release

# Deploy contract to standalone testnet
soroban contract deploy \
  --wasm target/wasm32-unknown-unknown/release/subscription_vault.wasm \
  --network standalone

# Invoke function with --cost flag to print CPU and memory metrics
soroban contract invoke \
  --id <CONTRACT_ID> \
  --fn charge_subscription \
  --arg <SUB_ID> \
  --cost
```

---

## 9. Security & Correctness Invariants

Performance optimization MUST NOT compromise contract security. All benchmark implementations and performance improvements must adhere to the following invariants:

1. **CEI Pattern Preservation**: State updates (effects) MUST occur before external token transfers or external contract calls (interactions). Gas optimization must never reorder state writes after token transfers.
2. **Safe Arithmetic Enforcement**: All numerical calculations MUST use checked arithmetic (`safe_math.rs`). Optimizations MUST NOT replace checked operations with unchecked or raw `as` casts to save instructions.
3. **Reentrancy Protection**: Reentrancy guards MUST remain active on all fund-mutating entrypoints (`deposit_funds`, `withdraw_merchant_funds`, `cancel_subscription`). Removing guards for gas savings is strictly forbidden.
4. **Complete Authorization Verification**: Authorization checks (`address.require_auth()`) MUST NOT be bypassed, deferred, or skipped under any circumstances.
5. **No Production Test Hooks**: Methods like `env.cost_estimate().budget().reset_unlimited()` and test authorization mocks are strictly confined to test modules (`#[cfg(test)]`). Production code must never alter environment budget settings.
6. **No Validation Bypassing**: Benchmark results can NEVER be used as justification for removing bounds checks, parameter validation, or security guardrails.