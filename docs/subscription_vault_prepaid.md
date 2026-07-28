# Subscription Vault — Prepaid & Deposit Flow

## Deposit Flow Security & Atomicity

The `deposit_funds` flow is **atomic by construction**: if the token transfer fails for any
reason (insufficient balance, transfer error, etc.), Soroban's transactional semantics abort
the entire host call, reverting **both** the `prepaid_balance` update and the token transfer.
There is no intermediate state where the balance changed but tokens did not move.

### Invariants Guaranteed

| Invariant | Mechanism | Test Coverage |
|-----------|-----------|---------------|
| **No state change on insufficient balance** | `prepaid_balance` is written to storage _before_ `token.transfer`; Soroban rollback on transfer failure leaves state unchanged | `test_deposit_insufficient_token_balance_reverts`, `test_deposit_insufficient_partial_balance_reverts` |
| **Credit limit enforced before mutation** | `enforce_credit_limit_for_delta` scans aggregate subscriber exposure and rejects deposits that would exceed the configured limit before any state write | `test_deposit_rejected_when_credit_limit_exceeded`, `test_deposit_allowed_within_credit_limit`, `test_deposit_credit_limit_aggregate_two_subs` |
| **CEI pattern** | Checks → Effects (`prepaid_balance += amount`) → Interactions (`token.transfer`) | All deposit-flow tests in `test_insufficient_balance.rs` and `test_reentrancy_invariants.rs` |
| **Reentrancy guard** | `ReentrancyGuard` acquired at entry-point prevents recursive deposit calls | `test_reentrancy_guard_lock_is_released_after_operation` |
| **Lifetime cap gating** | `enforce_deposit_cap` rejects deposits that would lock funds beyond remaining chargeable capacity | `test_deposit_failure_leaves_state_unchanged` |
| **Overflow protection** | `safe_add_balance` enforces `checked_add` + non-negative validation | N/A (arithmetic property) |
| **Accounting mirror** | `add_total_accounted` incremented after each transfer = vault can reconcile `contract_balance == accounted + merchant_earnings` | N/A (reconciliation query) |

### Atomicity Proof (Sequence Diagram)

```
deposit_funds(id, subscriber, amount)
  │
  ├─ CHECK: subscriber.require_auth()
  ├─ CHECK: amount >= min_topup
  ├─ CHECK: subscription exists & active
  ├─ CHECK: credit_limit(enforce_credit_limit_for_delta)
  ├─ CHECK: enforce_deposit_cap (lifetime cap guard)
  │
  ├─ EFFECT: prepaid_balance += amount        ◄── persisted to storage
  ├─ EFFECT: write_subscription(state)        ◄── committed on-ledger
  │
  ├─ INTERACT: token.transfer(sub → vault)    ◄── if this FAILS → Soroban ROLLS BACK
  │                                              both the effect AND the interaction
  └─ INTERACT: add_total_accounted(token, amt)
```

**Key insight**: Because effects are written to storage _before_ the external call, a failure
during the token transfer causes the entire transaction to revert. The `prepaid_balance` is
never visible in a partially-updated state — it either committed together with the transfer
or not at all.

### Test Coverage

All deposit atomicity and credit-limit invariants are validated in:
- `test_insufficient_balance.rs` — 5 tests covering insufficient balance, credit limit enforcement, and aggregate exposure
- `test_reentrancy_invariants.rs` — CEI pattern, guard lifecycle, and emergency path tests

---

This contract manages prepaid balances for subscriptions. Funds enter the vault in two ways:

1. **`create_subscription`** — initial pull of the first interval amount during subscription creation.
2. **`deposit_funds`** — top-ups initiated by the subscriber to increase an existing subscription's `prepaid_balance`.

Both flows are built around **Checks-Effects-Interactions (CEI)**, **reentrancy guards**, and **safe math** to guarantee that the contract's internal accounting never diverges from actual token holdings.

---

## `deposit_funds` Flow

```text
Subscriber ──deposit_funds(subscription_id, amount)──► Vault
  │
  ├── Checks
  │   ├── subscriber.require_auth()
  │   ├── require_not_blocklisted(subscriber)
  │   ├── amount >= min_topup && amount >= 0
  │   ├── subscription exists & not expired
  │   └── credit_limit(enforce_credit_limit_for_delta)
  │
  ├── Effects
  │   ├── prepaid_balance += amount      (safe_add_balance)
  │   ├── persist subscription state
  │   └── if recovered status → emit recovery event
  │
  └── Interactions
      ├── token.transfer(subscriber → vault, amount)
      ├── accounting::add_total_accounted(token, amount)
      └── emit FundsDepositedEvent
```

### Security Properties

| Property | Implementation | Failure mode |
|---|---|---|
| **Only subscriber** | `subscriber.require_auth()` + `sub.subscriber` context from storage | `Error::Unauthorized` |
| **Atomic balance update** | Balance is incremented **before** `token.transfer`. If the transfer reverts, Soroban rolls back the entire transaction, leaving `prepaid_balance` unchanged. | Transaction reverts |
| **No partial state** | CEI pattern ensures no state mutation precedes a failing check. Reentrancy guard prevents recursive deposit calls. | `Error::Reentrancy` |
| **Overflow protection** | `safe_add_balance` wraps `checked_add` + `validate_non_negative` | `Error::Overflow` / `Error::Underflow` |
| **Credit limit** | `enforce_credit_limit_for_delta` scans active subscriptions to cap aggregate exposure | `Error::CreditLimitExceeded` |
| **Expiration guard** | Expired subscriptions are transitioned to `Expired` and reject new deposits | `Error::SubscriptionExpired` |
| **Accounting mirror** | `add_total_accounted(token, amount)` updates a global token-scoped counter after each transfer so vault token balance == sum(prepaid_balances) + merchant balances (always true unless accounting explicitly adjusted). | `Error::Overflow` |

---

## `create_subscription` Flow

1. `subscriber` authorizes `create_subscription`.
2. Contract validates input:
   - `amount > 0`
   - `interval_seconds > 0`
3. Contract loads configured token address from instance storage.
4. Contract checks token allowance:
   - `allowance(subscriber, vault_contract) >= amount`
   - returns `Error::InsufficientAllowance` when not satisfied.
5. Contract checks subscriber token balance:
   - `balance(subscriber) >= amount`
   - returns `Error::TransferFailed` when not satisfied.
6. Contract executes `transfer_from(vault_contract, subscriber, vault_contract, amount)`.
7. Contract writes subscription state:
   - `prepaid_balance = amount`
   - `last_payment_timestamp = ledger.timestamp`
   - `status = Active`

---

## Robustness Against Partial Failures

### Why “balance vs actual token holdings” can never diverge

1. **Effects-before-interactions** — The contract always updates `prepaid_balance` (or creates the subscription) **before** calling the external token contract. If the token transfer later fails, Soroban's transactional semantics abort the entire host call, reverting both the balance update and the transfer. There is no intermediate state where the balance changed but tokens did not move.

2. **Reentrancy guard** — `deposit_funds` acquires a `ReentrancyGuard` at the public entrypoint (`lib.rs`). Even if a malicious token callback attempted to re-enter `deposit_funds`, the guard returns `Error::Reentrancy`.

3. **Safe math** — All balance arithmetic uses `safe_add_balance` / `safe_sub_balance`, which enforce `checked_add`/`checked_sub` and reject negative amounts. This prevents overflow/underflow that could corrupt balances.

4. **Accounting reconciliation** — `accounting::add_total_accounted` / `sub_total_accounted` maintain a running ledger of tokens the contract believes it holds. This is updated in the same transaction as the transfer, giving an off-chain auditor a direct check: `vault_token_balance == total_accounted[token]` (plus any in-flight merchant earnings not yet withdrawn).

---

## Safety Assumptions

- The configured token contract follows Soroban token semantics for `allowance`, `balance`, and `transfer` / `transfer_from`.
- For `create_subscription`, the subscriber must approve this contract address as spender before calling.
- Pre-checks convert common token transfer failures into explicit contract errors (`InsufficientAllowance`, `TransferFailed`) instead of opaque host failures.
- No partial state is written before a transfer succeeds; subscription storage writes happen after transfer validations.

---

## Storage Compatibility

No changes were made to `Subscription` field order or storage keys. The implementation remains compatible with existing instance storage layout and subscription records.
