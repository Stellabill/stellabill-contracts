git status# Reentrancy Hardening: Charge Flow Audit

**Date**: April 23, 2026  
**Scope**: Subscription vault charge path, state mutations, and token interactions

## Executive Summary

This document audits the charge flow (`charge_subscription`, `batch_charge`, `charge_one`) for reentrancy vulnerabilities. The vault is hardened using two complementary defenses:

1. **Checks-Effects-Interactions (CEI) Pattern**: Primary mitigation across all fund-moving operations
2. **ReentrancyGuard**: Secondary layer on public entrypoints to prevent recursive calls

## Key Findings

### ✅ Existing CEI Compliance

| Function                       | Module          | Status | Notes                                                          |
| ------------------------------ | --------------- | ------ | -------------------------------------------------------------- |
| `do_deposit_funds`             | subscription.rs | ✓ CEI  | Updates prepaid_balance in storage BEFORE token.transfer()     |
| `do_withdraw_subscriber_funds` | subscription.rs | ✓ CEI  | Zeros balance in storage BEFORE token.transfer() to subscriber |
| `do_partial_refund`            | subscription.rs | ✓ CEI  | Debits balance in storage BEFORE token.transfer()              |
| `withdraw_merchant_funds`      | merchant.rs     | ✓ CEI  | Updates merchant_balance in storage BEFORE token.transfer()    |
| `merchant_refund`              | merchant.rs     | ✓ CEI  | Debits merchant balance in storage BEFORE token.transfer()     |

### 📋 Charge Path Analysis

#### Entry Points (lib.rs)

- `charge_subscription(subscription_id)` → calls `charge_core::charge_one()`
- `batch_charge(subscription_ids)` → calls `admin::do_batch_charge()` → calls `charge_one()` for each
- `charge_usage(subscription_id, amount)` → calls `charge_core::charge_usage_one()`

#### External Call Sites in charge_core

1. **No direct token transfers** in `charge_one()` itself
2. **Merchant crediting happens via `merchant::credit_merchant_balance_for_token()`**
   - This is an **internal call** (only updates storage, no external interactions)
   - Happens AFTER all subscription state is updated

#### Mutation Order in `charge_one()`

```
1. CHECKS: expiration, status, balance, replay protection, lifetime cap
2. EFFECTS:
   - sub.prepaid_balance ← new_balance
   - sub.last_payment_timestamp ← now
   - sub.lifetime_charged ← updated
   - sub.status ← updated (e.g., Active after grace period)
   - merchant earnings (storage update only)
   - storage.set(&subscription_id, &sub)  ← ALL state committed
3. INTERACTIONS: None! (charge_one doesn't call token contract)
```

**Implication**: `charge_one` is naturally safe from external reentrancy because it doesn't call any external contracts. However, the public entrypoint `charge_subscription` should still be guarded to prevent re-entry attempts through orchestrated callback attacks (e.g., if a malicious token contract tries to call back during a concurrent charge).

## Reentrancy Guard Strategy

### When Guards Are Needed

Per Soroban's synchronous execution model, reentrancy requires:

1. An external call (token transfer or host call)
2. A callback from that external contract
3. Entry back into our contract during the callback

**Public fund-moving operations guard coverage**:

- ✅ `deposit_funds`: Will add reentrancy guard
- ✅ `withdraw_subscriber_funds`: Will add reentrancy guard
- ✅ `charge_subscription`: Will add reentrancy guard (covers batch_charge indirectly)
- ✅ `charge_usage`: Will add reentrancy guard

**No guard needed for**:

- Read-only queries (never call external functions)
- Internal helpers (private functions, called after guard acquired)

### Guard Placement Rationale

Guards are placed at **PUBLIC entry-points** (lib.rs layer), not internal functions, because:

1. **Single choke point**: One guard per user-initiated action
2. **Efficiency**: Avoid redundant guards for internal call chains
3. **Clarity**: Readers know exactly which functions have protection
4. **Maintenance**: Easier to audit and update

## Assumptions About Soroban Token Contracts

**Documented in code and tests:**

- Soroban token contracts (USDC stellar asset) **do NOT implement ERC777-style callbacks**
- Token transfers are **atomic** and **synchronous**
- No reentry occurs during `token.transfer()` in normal operation
- Guards therefore serve as a **defense-in-depth** mechanism for malicious/non-standard token implementations

See `test_reentrancy_invariants.rs` for validation of these assumptions.

## Implementation Checklist

- [x] Audit all external call sites (done in this document)
- [x] Confirm CEI pattern in place (all fund-moving operations compliant)
- [ ] Add reentrancy guards to public entrypoints:
  - [ ] `lib.rs::charge_subscription`
  - [ ] `lib.rs::charge_usage`
  - [ ] `lib.rs::charge_usage_with_reference`
  - [ ] `lib.rs::charge_one_off`
  - [ ] `lib.rs::deposit_funds`
  - [ ] `lib.rs::withdraw_subscriber_funds`
  - [ ] `lib.rs::partial_refund`
- [ ] Update guard cleanup on error paths (guard.drop() automatic)
- [ ] Document guard usage in module headers
- [ ] Update test_reentrancy_invariants.rs to cover edge cases
- [ ] Add cross-function reentrancy test (if feasible in Soroban)
- [ ] Document assumptions about token contract behavior

## Testing Strategy

### Invariants to Verify

1. **State Consistency**: After charge attempt, prepaid_balance equals expected value
2. **Merchant Earnings Consistency**: Earnings exactly equal amount deducted from subscription
3. **Replay Protection**: Same idempotency key returns same result
4. **Guard Cleanup**: Guard is released even on error (no permanent lock)
5. **Lifetime Cap Enforcement**: Lifetime_charged never exceeds cap
6. **Status Transitions**: All transitions respect state machine rules

### Test Cases

- `test_charge_with_reentrancy_guard_lock()`: Verify guard prevents double-entry
- `test_guard_cleanup_on_error()`: Confirm guard is released on charge failure
- `test_deposit_then_charge_state_consistency()`: Verify ordering between deposit and charge
- Existing `test_reentrancy_invariants.rs` suite ensures CEI compliance

## Soroban-Specific Notes

### Synchronous Execution Model

- All contract calls in Soroban execute sequentially within a single transaction
- Deep call stacks are possible but expensive in resources
- Reentrancy requires explicit callback during an external call
- Stellar token contract does NOT support callbacks

### Guard Implementation Details

- Uses `ReentrancyGuard` from `reentrancy.rs`
- Lock stored as a per-function `Symbol` in contract instance storage
- Automatically released via `Drop` trait (exception-safe)
- Minimal overhead: single storage read/write pair

### Future Considerations

- If support for ERC777-style tokens is added, all guards remain effective
- If cross-contract calls are enabled, review guard scope (per-function is likely insufficient)
- Multi-signature operations should maintain separate guard scopes

## References

- `docs/reentrancy.md` - User-facing reentrancy documentation
- `contracts/subscription_vault/src/reentrancy.rs` - Guard implementation
- `contracts/subscription_vault/src/test_reentrancy_invariants.rs` - CEI validation tests
- `contracts/subscription_vault/src/subscription.rs` - Deposit/withdraw with CEI
- `contracts/subscription_vault/src/merchant.rs` - Merchant fund operations with CEI
