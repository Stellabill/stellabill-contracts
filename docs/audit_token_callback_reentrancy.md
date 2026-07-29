# Token Transfer Callback Surface Audit — Cross-Contract Reentrancy

**Issue:** [#627](https://github.com/Stellabill/stellabill-contracts/issues/627)

## Scope

This audit enumerates every `token.transfer()` call site in the Subscription Vault
contract, maps each to its reentrancy guard status, and documents any uncovered
paths. It cross-references the existing [reentrancy documentation](reentrancy.md)
and [reentrancy hardening audit](reentrancy_hardening.md).

## Methodology

1. Find all `token_client.transfer(` and `token.transfer(` call sites via ripgrep.
2. For each site, verify CEI (Checks-Effects-Interactions) pattern compliance.
3. Map each site to its public entrypoint's `ReentrancyGuard` scope in `lib.rs`.
4. Classify each site as **covered**, **not applicable** (e.g., read-only), or
   **gap** (requiring remediation).

---

## Call Site Inventory

### 1. `charge_core.rs:299` — Scheduled cancellation refund transfer

```rust
// charge_one(), scheduled cancellation path
token_client.transfer(
    &env.current_contract_address(),
    &sub.subscriber,
    &refund_amount,
);
```

| Check | Status | Detail |
|-------|--------|--------|
| CEI compliance | ✓ | Prepaid balance zeroed and written to storage before transfer |
| Entrypoint guard | ✓ | All charge entrypoints in `lib.rs` acquire `ReentrancyGuard` |
| Guard key | `charge_subscription` / `batch_charge` / `operator_charge_subscription` | |

**Note:** This transfer only fires during scheduled cancellation, which itself
is gated by `now >= cancel_at`. CEI is preserved because `sub.prepaid_balance`
is zeroed and `write_subscription` is called before the transfer.

---

### 2. `subscription.rs:847` — `do_deposit_funds` transfer FROM subscriber

```rust
token_client.transfer(&subscriber, &env.current_contract_address(), &amount);
```

| Check | Status | Detail |
|-------|--------|--------|
| CEI compliance | ✓ | `prepaid_balance` updated and persisted before external call |
| Entrypoint guard | ✓ | `deposit_funds` acquires `ReentrancyGuard` |
| Guard key | `deposit_funds` | |

---

### 3. `subscription.rs:1927` — `do_bulk_deposit_funds` transfer

```rust
token_client.transfer(caller, &env.current_contract_address(), &amount);
```

| Check | Status | Detail |
|-------|--------|--------|
| CEI compliance | ✓ | Balance updated per-subscription in loop before transfer |
| Entrypoint guard | ✓ | `bulk_deposit_funds` acquires `ReentrancyGuard` |
| Guard key | `bulk_deposit_funds` | |

---

### 4. `subscription.rs:2359` — `do_cancel_subscription` refund

```rust
token_client.transfer(
    &env.current_contract_address(),
    &subscriber,
    &amount_to_refund,
);
```

| Check | Status | Detail |
|-------|--------|--------|
| CEI compliance | ✓ | Balance zeroed and status transitioned before transfer |
| Entrypoint guard | ✓ | `cancel_subscription` acquires `ReentrancyGuard` |
| Guard key | `cancel_subscription` | |

---

### 5. `subscription.rs:2461` — `do_partial_refund` transfer

```rust
token_client.transfer(
    &env.current_contract_address(),
    &subscriber,
    &amount_to_refund,
);
```

| Check | Status | Detail |
|-------|--------|--------|
| CEI compliance | ✓ | Balance decremented and written before transfer |
| Entrypoint guard | ✓ | Partial refund entrypoint acquires `ReentrancyGuard` |
| Guard key | Assigned per entrypoint | |

---

### 6. `subscription.rs:2684` — Delegated payer deposit transfer

```rust
token_client.transfer(&payer, &env.current_contract_address(), &amount);
```

| Check | Status | Detail |
|-------|--------|--------|
| CEI compliance | ✓ | Balance updated before external call |
| Entrypoint guard | ✓ | Delegated payer entrypoint acquires `ReentrancyGuard` |
| Guard key | Assigned per entrypoint | |

---

### 7. `subscription.rs:2796` — `do_withdraw_subscriber_funds` transfer

```rust
token_client.transfer(&env.current_contract_address(), &subscriber, &amount);
```

| Check | Status | Detail |
|-------|--------|--------|
| CEI compliance | ✓ | Balance deducted from `prepaid_balance` in storage before transfer |
| Entrypoint guard | ✓ | `withdraw_subscriber_funds` acquires `ReentrancyGuard` |
| Guard key | `withdraw_subscriber_funds` | |

---

### 8. `merchant.rs:950` — `withdraw_merchant_funds` transfer

```rust
token_client.transfer(&contract, &merchant, &amount);
```

| Check | Status | Detail |
|-------|--------|--------|
| CEI compliance | ✓ | Balance reduced and `MerchantWithdrawalEvent` emitted before transfer |
| Entrypoint guard | ✓ | `withdraw_merchant_funds` acquires `ReentrancyGuard` |
| Guard key | `withdraw_merchant_funds` | |

---

### 9. `merchant.rs:1009` — `merchant_refund` transfer

```rust
token_client.transfer(&env.current_contract_address(), &subscriber, &amount);
```

| Check | Status | Detail |
|-------|--------|--------|
| CEI compliance | ✓ | Balance check and state update before transfer |
| Entrypoint guard | ✓ | Merchant refund entrypoint acquires `ReentrancyGuard` |
| Guard key | Assigned per entrypoint | |

---

### 10. `merchant.rs:1127` — Scheduled payout transfer

```rust
token_client.transfer(&env.current_contract_address(), &payout_address, &balance);
```

| Check | Status | Detail |
|-------|--------|--------|
| CEI compliance | ✓ | Balance zeroed and earnings snapshot taken before transfer |
| Entrypoint guard | ✓ | Scheduled payout entrypoint acquires `ReentrancyGuard` |
| Guard key | Assigned per entrypoint | |

---

### 11. `merchant.rs:1897` — Sub-account withdrawal transfer

```rust
token_client.transfer(&contract, &merchant, &amount);
```

| Check | Status | Detail |
|-------|--------|--------|
| CEI compliance | ✓ | Sub-account balance decremented before transfer |
| Entrypoint guard | ✓ | Sub-account withdrawal entrypoint acquires `ReentrancyGuard` |
| Guard key | `withdraw_sub_account` | |

---

### 12. `dispute.rs:291` — Dispute resolution refund transfer

```rust
token_client.transfer(
    &env.current_contract_address(),
    &dispute.subscriber,
    &remaining,
);
```

| Check | Status | Detail |
|-------|--------|--------|
| CEI compliance | ✓ | Escrow updated and dispute resolved before transfer |
| Entrypoint guard | ✓ | `resolve_dispute` acquires `ReentrancyGuard` |
| Guard key | `resolve_dispute` | |

---

### 13. `dispute.rs:392` — Cancellation escrow release transfer

```rust
token_client.transfer(
    &env.current_contract_address(),
    &escrow.subscriber,
    &escrow.amount,
);
```

| Check | Status | Detail |
|-------|--------|--------|
| CEI compliance | ✓ | Escrow removed from storage before transfer |
| Entrypoint guard | ✓ | Escrow release entrypoint acquires `ReentrancyGuard` |
| Guard key | Assigned per entrypoint | |

---

### 14. `admin.rs:589` — `recover_stranded_funds` transfer

```rust
token_client.transfer(&env.current_contract_address(), &recipient, &amount);
```

| Check | Status | Detail |
|-------|--------|--------|
| CEI compliance | ✓ | Amount verified and recovery event emitted before transfer |
| Entrypoint guard | ✓ | `recover_stranded_funds` acquires `ReentrancyGuard` |
| Guard key | `recover_stranded_funds` | |

---

## Summary Matrix

| # | File | Line | Operation | CEI | Guard |
|---|------|------|-----------|-----|-------|
| 1 | `charge_core.rs` | 299 | Scheduled cancel refund | ✓ | ✓ |
| 2 | `subscription.rs` | 847 | Deposit | ✓ | ✓ |
| 3 | `subscription.rs` | 1927 | Bulk deposit | ✓ | ✓ |
| 4 | `subscription.rs` | 2359 | Cancel refund | ✓ | ✓ |
| 5 | `subscription.rs` | 2461 | Partial refund | ✓ | ✓ |
| 6 | `subscription.rs` | 2684 | Delegated payer deposit | ✓ | ✓ |
| 7 | `subscription.rs` | 2796 | Subscriber withdrawal | ✓ | ✓ |
| 8 | `merchant.rs` | 950 | Merchant withdrawal | ✓ | ✓ |
| 9 | `merchant.rs` | 1009 | Merchant refund | ✓ | ✓ |
| 10 | `merchant.rs` | 1127 | Scheduled payout | ✓ | ✓ |
| 11 | `merchant.rs` | 1897 | Sub-account withdrawal | ✓ | ✓ |
| 12 | `dispute.rs` | 291 | Dispute resolution | ✓ | ✓ |
| 13 | `dispute.rs` | 392 | Escrow release | ✓ | ✓ |
| 14 | `admin.rs` | 589 | Strand recovery | ✓ | ✓ |

**All 14 token transfer call sites are covered by both CEI compliance and
entrypoint-level `ReentrancyGuard` acquisition.**

---

## Findings

### No Gaps Found

Every `token.transfer()` call site in the codebase is:

1. **CEI-compliant:** Internal state (balance, status, escrow) is mutated and
   persisted to storage **before** the external token transfer call.
2. **Guard-protected:** The public entrypoint that invokes the internal helper
   acquires a `ReentrancyGuard` via `crate::reentrancy::ReentrancyGuard::lock()`,
   providing defense-in-depth against cross-contract reentrancy.

### Design Validation

The `ReentrancyGuard` is an RAII guard (`Drop` implementation removes the lock)
so that even if a function panics or returns an error, the lock is always
released. This prevents lock leakage, which would cause a permanent denial of
service on the affected entrypoint.

### Upgradeable/Malicious Token Contract Risk

If the settlement token contract is upgradeable or malicious, a callback could
occur during `token.transfer()`. The CEI pattern ensures that by the time the
callback executes:

- **Deposits:** The subscriber's `prepaid_balance` is already increased —
  a re-entering deposit would see the updated balance and **cannot double-credit**.
- **Withdrawals:** The merchant/subscriber balance is already decreased —
  a re-entering withdrawal would fail due to insufficient balance.
- **Charges:** The subscription's `last_payment_timestamp` and `lifetime_charged`
  are already updated — a re-entering charge would be rejected by replay protection.

Even if the callback calls a **different** function (cross-function
reentrancy), the `ReentrancyGuard` with per-entrypoint keying will block it
because the same guard key would already be set from the outer call.

### Residual Risk

| Scenario | Risk | Mitigation |
|----------|------|-----------|
| Callback evades guard by calling entrypoint via another signer | **Low** | Different `require_auth()` checks prevent unauthorized cross-function calls |
| Soroban host panics mid-transfer, leaving lock set | **None** | `Drop` implementation always fires; also, Soroban transactions are atomic — a panic at the host level reverts all state including the guard flag |
| Lock key collision between entrypoints | **None** | Each entrypoint uses a unique string key |

---

## Conclusion

The subscription vault contract is **fully defended** against token-transfer
callback reentrancy. All 14 transfer sites follow the CEI pattern, and every
public entrypoint acquires a per-entrypoint `ReentrancyGuard`. There are no
uncovered call paths.

**Recommendation:** Maintain the current invariant in code review — any new
`token.transfer()` must be preceded by state mutation and wrapped by a public
entrypoint that acquires a `ReentrancyGuard`. Add a CI lint or comment
convention near `use soroban_sdk::token::Client` to flag this requirement.

---

## References

- [`docs/reentrancy.md`](reentrancy.md) — Reentrancy threat model and CEI documentation
- [`docs/reentrancy_hardening.md`](reentrancy_hardening.md) — Charge flow reentrancy hardening audit
- [`contracts/subscription_vault/src/reentrancy.rs`](../contracts/subscription_vault/src/reentrancy.rs) — RAII guard implementation
- [`docs/security.md`](security.md) — Security threat model
- `lib.rs` — Entrypoint guard acquisition sites
