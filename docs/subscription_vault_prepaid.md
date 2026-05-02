# Subscription Vault — Prepaid & Deposit Flow

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
