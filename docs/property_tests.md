# Property-Based Tests for `charge_subscription`

## Overview

This document describes the property-based (fuzz-style) test strategy for the
`charge_subscription` endpoint and related logic in the `subscription_vault`
contract. The tests live in the `// Property-Based (Fuzz-Style) Tests` section
at the bottom of `contracts/subscription_vault/src/test.rs`.

---

## Why Not proptest or quickcheck?

`proptest` and `quickcheck` cannot be added as `[dev-dependencies]` to this
crate for the following reasons:

1. **`getrandom` feature conflict.** Both frameworks depend on `getrandom` for
   OS entropy. `soroban-sdk` already pulls in `getrandom` with specific feature
   flags for the `wasm32-unknown-unknown` target. Adding `proptest` causes Cargo
   feature unification to produce a conflicting `getrandom` configuration that
   breaks the WASM build.

2. **`#![no_std]` constraint.** The contract uses `#![no_std]`. Proptest's
   `no_std` mode still requires `rand_core` entropy which loops back to the same
   `getrandom` issue on `wasm32`.

3. **Separate `fuzz/` crate is the supported path.** The Stellar documentation
   recommends a dedicated `fuzz/` workspace crate (its own `Cargo.toml`) for
   `proptest` integration. That infrastructure is not in scope for this PR — the
   goal is augmenting the existing `cargo test` suite, not introducing a separate
   fuzzing binary.

---

## The Approach: Seeded XorShift64 PRNG

All 15 property tests use a self-contained `TestRng` struct implementing the
XorShift64 algorithm (Marsaglia 2003). Properties:

- **Deterministic**: same seed → same sequence on every run and every platform
- **No dependencies**: 6 lines of pure Rust, no `std`, no `rand`, no `getrandom`
- **Good statistical quality**: period 2⁶⁴−1, passes standard randomness tests
- **Zero WASM impact**: lives entirely inside `#[cfg(test)]`, excluded from the
  WASM build

### Seed Strategy

```
MASTER_SEED = 0x5EED_F00D_CAFE_BABE

Test P-N uses seed = MASTER_SEED.wrapping_add(N)
```

This gives each test an independent pseudo-random sequence with no correlation.
`MASTER_SEED` can be changed to explore a different region of the input space
while preserving full determinism.

---

## Running the Tests

```bash
# Run all tests (existing unit tests + 15 new property tests)
cargo test -p subscription_vault

# Verify the WASM build is unaffected (no new dependencies)
cargo build -p subscription_vault --target wasm32-unknown-unknown --release

# Lint
cargo clippy -p subscription_vault -- -D warnings

# Format check
cargo fmt --check
```

---

## Reproducing a Failing Iteration

When a property test fails, the panic message includes the iteration index `N`
and the exact parameter values that caused the failure. To isolate it:

1. Note the test name and iteration index `N` from the failure output.
2. Temporarily change `ITERATIONS` to `N + 1` in `test.rs`.
3. Re-run `cargo test -p subscription_vault <test_name>` — only the failing case
   will execute.
4. Restore `ITERATIONS` to 100 after fixing the underlying bug.

The seed for test P-N at iteration I is deterministic:
`TestRng::new(MASTER_SEED.wrapping_add(N))` with the I-th call sequence.

---

## Invariant Catalogue

| Test | Function name | Invariant | Parameters varied |
|------|---------------|-----------|-------------------|
| P-01 | `prop_balance_conservation_after_charge` | `new_balance == old_balance - amount` exactly | amount [1, 100B], balance [amount, 200B], interval [1s, 1yr] |
| P-02 | `prop_no_double_charge_same_timestamp` | 2nd charge at same `now` → `IntervalNotElapsed` | amount, balance, interval, timestamp |
| P-03 | `prop_status_becomes_insufficient_when_balance_low` | `balance < amount` → returns `InsufficientBalance` error | amount, deficit [1, amount] |
| P-04 | `prop_status_stays_active_after_successful_charge` | Successful charge leaves status=`Active` | amount, balance ≥ amount |
| P-05 | `prop_timestamp_set_to_current_after_charge` | `last_payment_timestamp == now` (sliding window, not `t0+interval`) | t0, interval, extra delay [0, interval] |
| P-06 | `prop_non_active_status_always_rejects_charge` | Non-Active → always `NotActive`, storage unchanged | status ∈ {Paused, Cancelled, InsufficientBalance} |
| P-07 | `prop_interval_guard_rejects_early_charge` | `now < t0 + interval` → `IntervalNotElapsed`, storage unchanged | t0, interval [2s, 1yr], now < boundary |
| P-08 | `prop_overflow_protection_timestamp_addition` | `t0 + interval` overflows u64 → `Overflow` (no panic/wrap) | t0 near `u64::MAX`, large interval |
| P-09 | `prop_balance_never_goes_negative_after_charge` | `prepaid_balance >= 0` always after successful charge | amount, balance ≥ amount |
| P-10 | `prop_state_machine_only_makes_valid_transitions` | After any operation, new status ∈ `get_allowed_transitions(old)` ∪ {old} | random op ∈ {charge, pause, resume, cancel} |
| P-11 | `prop_batch_isolation_failure_does_not_contaminate` | Failure at index K doesn't affect other results or storage | batch_size [2, 8], fail_idx random |
| P-12 | `prop_estimate_topup_always_non_negative` | `estimate_topup_for_intervals` always returns ≥ 0 | amount, balance [0, 200B], num_intervals [0, 50] |
| P-13 | `prop_estimate_topup_zero_when_balance_covers` | `balance >= amount×n` → topup == 0 | amount, n [1, 20], extra balance |
| P-14 | `prop_estimate_topup_equals_shortfall` | `balance < amount×n` → topup == `amount×n − balance` exactly | amount, n [1, 50], balance < required |
| P-15 | `prop_multi_charge_cumulative_balance_reduction` | After N charges: balance == initial − N×amount; timestamp and status correct | amount, N [2, 8], initial balance |

---

## Key Design Decisions

### Soroban storage rollback on contract errors

Soroban rolls back all storage writes made during a contract invocation that
returns an error (including `contracterror` variants). When using `try_*` client
methods, a failed invocation leaves storage in the pre-call state. This means
post-call storage assertions (e.g. checking that status changed to
`InsufficientBalance`) are not appropriate for the `try_*` error path — only the
error return value itself can be asserted. Tests that check storage after a
failed call verify the *unchanged* pre-call values (testing that no spurious
mutations occurred), not the intended new value.

### Direct storage injection via `setup_property_env`

The helper `setup_property_env` in `test.rs` bypasses the normal
`create_subscription` + `deposit_funds` flow (which enforces `min_topup` and
starts balance at 0) by writing a fully constructed `Subscription` struct
directly into instance storage using `env.as_contract(...)`. This gives tests
complete control over `amount`, `prepaid_balance`, `interval_seconds`, and
`status` without fighting the contract's validation logic.

### Parameter ranges

Ranges are chosen to stress boundaries while keeping the full suite under ~10
seconds:

| Parameter | Range | Rationale |
|-----------|-------|-----------|
| `amount` | [1, 100_000_000_000] | 1 stroop to ~100,000 USDC |
| `prepaid_balance` | [0, 200_000_000_000] | Zero through large surplus |
| `interval_seconds` | [1, 31_536_000] | 1 second to 1 year |
| `last_payment_timestamp` | [1, u64::MAX/4] | Avoids overflow in `t0 + interval` |
| Charge timestamp offset | [0, interval] | Tests at, slightly past, and far past boundary |
| `num_intervals` | [0, 50] | Zero (special case) through reasonable planning horizon |
| Batch size | [2, 8] | Multi-item with guaranteed at least one neighbour |

### Iteration count

100 iterations per test (50 for batch/multi-charge tests that create multiple
subscriptions). This provides good coverage of the input space while keeping
CI runtime bounded. The total number of contract invocations across all 15 tests
is approximately 1,700.

---

## Extending the Test Suite

To add a new property test:

1. Define the invariant as a precise mathematical or logical statement.
2. Choose a unique seed offset: `MASTER_SEED.wrapping_add(NEW_N)` where `NEW_N`
   is the next integer after 15.
3. Select parameter ranges that stress the boundaries of the invariant (include
   zero, minimum, exact-boundary, and large values).
4. Use `setup_property_env` for tests that need arbitrary subscription state, or
   build the environment manually for tests involving multiple subscriptions.
5. Keep `ITERATIONS` at 100 unless the test creates many subscriptions (use
   30–50 in that case).
6. Add the new invariant to the table above in this document.
7. Add a comment inside the test explaining which code path the invariant targets.
