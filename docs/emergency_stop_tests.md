# Emergency Stop Tests — `subscription_vault`

## Overview

This document describes the emergency stop test suite added to
`contracts/subscription_vault/src/test.rs`. The tests verify that the
emergency stop mechanism correctly blocks critical operations, allows
safe read-only operations, and behaves consistently when toggled.

---

## Functions Under Test

| Function              | Guarded? | Notes                              |
|-----------------------|----------|------------------------------------|
| `create_subscription` | Yes      | Blocked when stopped               |
| `deposit_funds`       | Yes      | Blocked when stopped               |
| `charge_subscription` | Yes      | Blocked when stopped               |
| `cancel_subscription` | Yes      | Blocked when stopped               |
| `pause_subscription`  | Yes      | Blocked when stopped               |
| `resume_subscription` | Yes      | Blocked when stopped               |
| `batch_charge`        | Yes      | Blocked when stopped               |
| `get_subscription`    | No       | Read-only, always accessible       |
| `get_min_topup`       | No       | Read-only, always accessible       |
| `is_stopped`          | No       | Query function, always accessible  |

---

## Error Variant

All guarded functions return `Error::ContractStopped` when the emergency
stop is active. This maps to a distinct contract error code to allow
callers to distinguish a stop condition from other error types.

---

## Test Scenarios

### Toggling Behavior

| Test | Description |
|------|-------------|
| `test_emergency_stop_sets_stopped_state` | Contract starts in running state |
| `test_emergency_stop_flag_is_true_after_activation` | `is_stopped()` returns true after stop |
| `test_resume_contract_clears_stopped_flag` | `is_stopped()` returns false after resume |
| `test_toggle_stop_multiple_times_is_consistent` | Stop/resume cycles maintain correct state |

### Access Control

| Test | Description |
|------|-------------|
| `test_non_admin_cannot_trigger_emergency_stop` | Non-admin call to `emergency_stop` is rejected |
| `test_non_admin_cannot_resume_contract` | Non-admin call to `resume_contract` is rejected |

### Critical Operations Blocked

| Test | Description |
|------|-------------|
| `test_create_subscription_blocked_when_stopped` | Returns `ContractStopped` |
| `test_deposit_funds_blocked_when_stopped` | Returns `ContractStopped` |
| `test_charge_subscription_blocked_when_stopped` | Returns `ContractStopped` |
| `test_cancel_subscription_blocked_when_stopped` | Returns `ContractStopped` |
| `test_pause_subscription_blocked_when_stopped` | Returns `ContractStopped` |
| `test_resume_subscription_blocked_when_stopped` | Returns `ContractStopped` |
| `test_batch_charge_blocked_when_stopped` | Returns `ContractStopped` |

### Safe Operations Remain Allowed

| Test | Description |
|------|-------------|
| `test_get_subscription_allowed_when_stopped` | Query succeeds during stop |
| `test_get_min_topup_allowed_when_stopped` | Query succeeds during stop |
| `test_is_stopped_query_always_accessible` | Returns true during stop |

### Recovery After Resume

| Test | Description |
|------|-------------|
| `test_create_subscription_succeeds_after_resume` | Normal operation restored |
| `test_deposit_funds_succeeds_after_resume` | Normal operation restored |
| `test_charge_subscription_succeeds_after_resume` | Normal operation restored |

---

## Incident Handling Notes

- The emergency stop should be activated immediately upon detection of
  a critical vulnerability or unexpected contract behavior.
- Only the admin address registered during `init` can activate or
  deactivate the stop.
- All in-flight operations will fail with `ContractStopped` — callers
  should handle this error gracefully and retry after the stop is lifted.
- Read-only queries remain available so that the state of the contract
  can be inspected by the admin during an incident without modifying storage.
- After the root cause is resolved, call `resume_contract(admin)` to
  restore normal operation.

---

## Running the Tests

```bash
cargo test -p subscription_vault
```

To run only emergency stop tests:

```bash
cargo test -p subscription_vault emergency_stop
cargo test -p subscription_vault stopped
cargo test -p subscription_vault resume_contract
```
