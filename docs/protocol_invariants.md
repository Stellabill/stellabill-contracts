# Protocol Invariants: Subscription Vault

This document is the single source of truth for the formal invariants enforced by the
`subscription_vault` contract. Each invariant records:

- the **precise property** that must hold at all times,
- the **module** responsible for enforcing it,
- the **tests** that prove it, and
- any **known assumptions or limitations**.

Related background documentation: `docs/security.md`, `docs/reentrancy.md`,
`docs/subscription_state_machine.md`, `docs/safe_math.md`, `docs/replay_protection.md`.

---

## INV-1 — No Double-Charge per Interval

**Property:** A subscription can be charged at most once per billing interval. A second charge
attempt within the same interval is rejected without mutating state.

**Enforcement module:** `contracts/subscription_vault/src/lib.rs` — `charge_subscription` /
`charge_one`. The guard condition `now >= last_payment_timestamp + interval_seconds` must be
satisfied before any debit is applied. On failure the function returns `Error::IntervalNotElapsed`
(1001) and storage is not modified.

**Idempotency key path:** `batch_charge` deduplicates subscription IDs within a single call and
rejects retried charges via `Error::Replay` (1006 / 1007) when the same `idempotency_key` is
re-submitted before the next interval begins.

**Tests:**

| Test | File | What it proves |
|------|------|----------------|
| `test_charge_succeeds_at_exact_interval` | `test_security.rs` | Charge accepted when interval has elapsed |
| `test_immediate_retry_at_same_timestamp_rejected` | `test_security.rs` | Second charge at identical timestamp returns error |
| `test_replay_protection_same_timestamp_rejected` | `test_security.rs` | Replay protection fires with error code 1006 |
| `test_replay_protection_on_batch_charge` | `test_security.rs` | Duplicate IDs in a batch call are deduplicated |
| `test_charge_replay_rejected_no_state_mutation` | `test_reentrancy_invariants.rs` | Replay attempt leaves storage unchanged |

**Assumptions / Limitations:**
- Relies on the Soroban ledger timestamp (`env.ledger().timestamp()`). Clock skew between
  ledgers is not modelled; see `docs/replay_protection.md` for clock-skew resistance strategy.
- `batch_charge` authenticates against the stored admin without an explicit caller parameter
  (see `docs/security.md §4`).

---

## INV-2 — Accounted Balances Never Go Negative

**Property:** The prepaid balance of any subscription and the earned balance of any merchant are
non-negative integers at all times. No arithmetic operation may produce a value below zero or
silently wrap around.

**Enforcement module:** `contracts/subscription_vault/src/safe_math.rs` — `safe_add` and
`safe_sub` wrappers used in all balance mutations. Any addition that would exceed `i128::MAX`
returns `Error::Overflow` (403). Any subtraction that would produce a negative result returns
`Error::Underflow` (1004). Neither error path mutates storage.

**Tests:**

| Test | File | What it proves |
|------|------|----------------|
| `test_safe_add_overflow_returns_error` | `test_security.rs` | `safe_add` rejects `i128::MAX + 1` |
| `test_safe_sub_underflow_returns_error` | `test_security.rs` | `safe_sub` rejects any result below zero |
| `test_charge_amount_greater_than_balance_fails` | `test_security.rs` | Charge larger than balance returns `InsufficientBalance`, not a panic |
| `test_deposit_negative_amount_fails` | `test_security.rs` | Negative deposit amount returns error code 501 |
| `test_charge_insufficient_balance_no_partial_debit` | `test_reentrancy_invariants.rs` | No partial debit when balance is insufficient |
| `test_charge_lifetime_charged_monotonically_increases` | `test_reentrancy_invariants.rs` | `lifetime_charged` counter never decreases |

**Assumptions / Limitations:**
- Balance values are stored as `i128`. The protocol uses checked arithmetic throughout, but
  callers must not pass raw token amounts without prior validation against `min_topup`.
- `safe_math` regression tests are consolidated in `test_safe_math_regression.rs`.

---

## INV-3 — Withdrawals Are Limited to Owned Balances

**Property:** A merchant can only withdraw up to its own earned balance. A subscriber can only
reclaim its own prepaid balance, and only after the subscription is cancelled. Neither party can
access funds belonging to the other or to a different subscriber/merchant.

**Enforcement module:** `contracts/subscription_vault/src/lib.rs` —
`withdraw_merchant_funds` checks the stored merchant earning balance before transfer.
`do_withdraw_subscriber_funds` reads `prepaid_balance` of the specific subscription and requires
`SubscriptionStatus::Cancelled`. Both paths use `require_auth()` to bind the call to the
owner's signature.

**Tests:**

| Test | File | What it proves |
|------|------|----------------|
| `test_withdraw_merchant_overdraw_rejected_no_state_change` | `test_reentrancy_invariants.rs` | Overdraw attempt rejected with no state change |
| `test_withdraw_merchant_sequential_correct_accounting` | `test_reentrancy_invariants.rs` | Sequential withdrawals correctly drain balance to zero |
| `test_merchant_cannot_withdraw_other_merchant` | `test_reentrancy_invariants.rs` | Merchant A cannot access merchant B's balance |
| `test_withdraw_subscriber_double_withdrawal_rejected` | `test_reentrancy_invariants.rs` | Second subscriber withdrawal rejected after balance is zero |
| `test_withdraw_subscriber_requires_cancelled_status` | `test_reentrancy_invariants.rs` | Active/paused subscriptions block subscriber withdrawal |
| `test_withdraw_subscriber_exact_amount_transferred` | `test_reentrancy_invariants.rs` | Token transfer amount equals stored prepaid balance exactly |
| `test_chained_charge_and_cancel_preserves_balance` | `test_security.rs` | Balance accounting is consistent across charge → cancel → withdraw sequence |
| `test_refund_cumulative_cannot_exceed_deposit` | `test_reentrancy_invariants.rs` | Cumulative partial refunds cannot surpass original deposit |

**Assumptions / Limitations:**
- Ownership verification relies on Soroban's `require_auth()`. A compromised subscriber or
  merchant key grants full access to that party's own funds; see `docs/security.md §4`.
- There is no cross-subscription aggregation guard — a merchant with earnings across multiple
  subscriptions draws from a single pooled balance, not per-subscription ledger entries.

---

## INV-4 — Emergency Stop Halts All State-Changing Operations

**Property:** When the emergency stop flag is active, all state-changing entrypoints
(`charge_subscription`, `batch_charge`, `deposit_funds`, and withdrawal functions) are blocked
before any storage mutation. Read-only view functions remain available.

**Enforcement module:** `contracts/subscription_vault/src/lib.rs` — each mutating entrypoint
checks the stored emergency-stop flag at the start of execution and returns
`Error::EmergencyStopActive` if it is set. The check occurs before any balance read or write.

**Tests:**

| Test | File | What it proves |
|------|------|----------------|
| `test_charge_blocked_by_emergency_stop_no_mutation` | `test_reentrancy_invariants.rs` | Charge returns error and storage is unchanged when stop is active |
| `test_deposit_blocked_by_emergency_stop_no_mutation` | `test_reentrancy_invariants.rs` | Deposit returns error and storage is unchanged when stop is active |
| `test_withdraw_merchant_blocked_by_emergency_stop_no_mutation` | `test_reentrancy_invariants.rs` | Merchant withdrawal blocked when stop is active |
| `test_withdraw_subscriber_blocked_by_emergency_stop_no_mutation` | `test_reentrancy_invariants.rs` | Subscriber withdrawal blocked when stop is active |

**Assumptions / Limitations:**
- Emergency stop can only be toggled by the current admin. A compromised admin key can both
  activate and deactivate it (see `docs/security.md §4` — future hardening: multi-sig controls).
- Lifetime caps and emergency stop interact; see `test_emergency_stop_lifetime_caps.rs` for
  combined scenarios.

---

## INV-5 — Reentrancy: State Is Committed Before External Token Transfers

**Property:** For every function that calls an external token contract (`do_deposit_funds`,
`withdraw_merchant_funds`, `do_withdraw_subscriber_funds`, `partial_refund`), all internal
balance mutations in contract storage are applied **before** the `token.transfer()` invocation.
A reentrant callback therefore observes the already-updated state and cannot exploit an
inconsistency to double-withdraw or double-deposit.

**Enforcement module:** `contracts/subscription_vault/src/lib.rs` — enforced structurally by
the Checks-Effects-Interactions (CEI) pattern. An optional secondary guard is available in
`contracts/subscription_vault/src/reentrancy.rs` but is not currently enabled, as CEI is
sufficient given Soroban's synchronous execution model.

**Tests:**

| Test | File | What it proves |
|------|------|----------------|
| `test_deposit_funds_state_committed_before_transfer` | `test_security.rs` | Prepaid balance updated in storage before token transfer |
| `test_withdraw_merchant_funds_state_committed_before_transfer` | `test_security.rs` | Merchant earning balance reduced before transfer |
| `test_reentrancy_lock_prevents_recursive_calls` | `test_security.rs` | Optional reentrancy guard locks/unlocks correctly |
| `test_deposit_state_committed_before_transfer` | `test_reentrancy_invariants.rs` | CEI ordering verified for deposit path |
| `test_withdraw_subscriber_state_committed_before_transfer` | `test_reentrancy_invariants.rs` | Balance zeroed before subscriber refund transfer |
| `test_withdraw_merchant_state_committed_before_transfer` | `test_reentrancy_invariants.rs` | Merchant ledger updated before merchant transfer |
| `test_refund_state_committed_before_transfer` | `test_reentrancy_invariants.rs` | Prepaid balance debited before token return |
| `test_reentrancy_guard_lock_is_released_after_operation` | `test_reentrancy_invariants.rs` | Guard lock does not remain set after normal operation |
| `test_reentrancy_guard_released_after_merchant_withdrawal` | `test_reentrancy_invariants.rs` | Lock released after merchant withdrawal path |
| `test_reentrancy_guard_not_stuck_after_rejection` | `test_reentrancy_invariants.rs` | Rejected operations do not leave a dangling lock |

**Assumptions / Limitations:**
- CEI sufficiency depends on Soroban's synchronous, non-reentrant execution model. This
  invariant must be re-evaluated if the platform introduces async cross-contract calls.
- The USDC token contract (Stellar Asset Contract) is trusted not to issue malicious callbacks;
  see `docs/reentrancy.md` and `docs/security.md §1`.

---

## INV-6 — Subscription Lifecycle: Cancelled Is a Terminal State

**Property:** Once a subscription reaches `Cancelled` status, no further state transition is
possible. Attempts to resume, pause, or charge a cancelled subscription are rejected before
any storage mutation.

**Enforcement module:** `contracts/subscription_vault/src/lib.rs` —
`validate_status_transition` is called at the start of every lifecycle entrypoint
(`pause_subscription`, `resume_subscription`, `cancel_subscription`, `charge_subscription`).
An invalid transition returns `Error::InvalidStatusTransition` (400) without modifying storage.

**Tests:**

| Test | File | What it proves |
|------|------|----------------|
| `test_invalid_cancelled_to_active` | `test.rs` | Resume on cancelled returns `InvalidStatusTransition` |
| `test_validate_cancelled_transitions_all_blocked` | `test.rs` | All six invalid `Cancelled →` transitions are rejected |
| `test_all_valid_transitions_coverage` | `test.rs` | All seven valid transitions accepted |
| `test_charge_on_paused_no_state_change` | `test_reentrancy_invariants.rs` | Charge on a non-active subscription leaves storage unchanged |
| `test_deposit_on_cancelled_subscription_rejected_cleanly` | `test_reentrancy_invariants.rs` | Deposit to cancelled subscription is rejected with no state change |

**Assumptions / Limitations:**
- The state machine does not model time-based auto-cancellation; cancellation is always
  triggered explicitly by the subscriber, merchant, or admin.
- Grace period transitions (`Active → GracePeriod → InsufficientBalance / Active`) are managed
  exclusively within the `charge_one` / `batch_charge` flow and do not pass through the public
  `pause`/`resume`/`cancel` entrypoints.

---

## INV-7 — Authorization: Only the Admin May Execute Privileged Operations

**Property:** Functions designated as admin-only (`set_min_topup`, `rotate_admin`,
`recover_stranded_funds`, `batch_charge`, emergency controls, oracle/billing maintenance) reject
any caller that is not the current stored admin. After an admin rotation, the old admin key is
immediately invalidated.

**Enforcement module:** `contracts/subscription_vault/src/lib.rs` — shared admin guard using
`require_auth()` against the stored admin address. `rotate_admin` replaces the stored address atomically.

**Tests:**

| Test | File | What it proves |
|------|------|----------------|
| `test_admin_authorization_matrix_rejects_non_admin_across_protected_entrypoints` | `test_security.rs` | All admin-only routes return `Unauthorized` for non-admin callers |
| `test_admin_authorization_matrix_rejects_stale_admin_after_rotation` | `test_security.rs` | Old admin key rejected immediately after rotation |
| `test_rotate_admin_unauthorized` | `test_security.rs` | Non-admin cannot rotate the admin key |
| `test_pause_subscription_unauthorized_stranger` | `test_security.rs` | Third-party address cannot pause a subscription |

**Assumptions / Limitations:**
- The system depends on a single admin key (see `docs/security.md §4`). Future hardening:
  multi-signature rotation and time-locked upgrades.
- `batch_charge` does not accept an explicit admin argument; authentication is against the
  stored admin at the Soroban host auth layer.

---

## Audit Notes

All invariants above map directly to tests in `contracts/subscription_vault/src/`. The test suite
can be exercised with:

```bash
cargo test -p subscription_vault
```

**Pre-existing compilation issues:**

The project has pre-existing compilation errors in multiple test files (`test_refactor_check.rs`,
`test_auth_fuzz.rs`, `test_emergency_stop_lifetime_caps.rs`, `test_deterministic_charging.rs`,
`test_events_snapshot.rs`, `test_expiration.rs`, `test_governance.rs`, `test_insufficient_balance.rs`,
`test_multi_actor.rs`, `test_query_performance.rs`, `test_recovery.rs`, `test_usage_limits.rs`) and
in the main library (`types.rs` has duplicate `TokenReconciliationSnapshot` definitions).

These issues are **unrelated to this invariants document** and exist on `main`. The invariant tests
listed here are designed to work with the correctly-compiling test files:
- `test_security.rs` (most invariant tests)
- `test_reentrancy_invariants.rs` (CEI and reentrancy tests)
- `test.rs` (lifecycle state machine tests)