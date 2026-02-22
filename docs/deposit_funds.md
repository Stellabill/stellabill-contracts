# deposit_funds

Entrypoint: `SubscriptionVault::deposit_funds(env, subscription_id, subscriber, amount)`  
Implementation: `subscription::do_deposit_funds`

---

## Overview

`deposit_funds` allows a subscriber to pre-load USDC into the vault against a
specific subscription. The vault holds the balance on the subscriber's behalf and
draws from it each time `charge_subscription` is successfully executed.

The operation is atomic: the USDC transfer and the internal balance increment
are part of the same Soroban transaction. A failure at any step rolls back the
entire operation.

---

## State Machine Changes

```
InsufficientBalance  --[deposit_funds]--> Active
Active               --[deposit_funds]--> Active  (balance increases, status unchanged)
Paused               --[deposit_funds]--> Paused  (balance increases, status unchanged)
Cancelled            --[deposit_funds]--> REJECTED (Error::InvalidStatusTransition)
```

A `Cancelled` subscription is a terminal state. No value can be deposited into it.

An `InsufficientBalance` subscription is automatically transitioned back to `Active`
upon a successful deposit, removing the need for a separate `resume_subscription`
call from the subscriber.

---

## Invariants

### Invariant 1 — Balance Integrity

> `prepaid_balance` must always equal the sum of all successful `deposit_funds`
> amounts minus the sum of all fees deducted by `charge_subscription`.

This is enforced by:
- Using `checked_add` for every deposit increment.
- Performing the USDC `transfer` before incrementing the balance. A failed transfer
  rolls back the transaction, so the balance is never credited without real USDC
  movement.

### Invariant 2 — Authorization

> No USDC moves without `require_auth()` from the `subscriber` address.

`subscriber.require_auth()` is the first statement in `do_deposit_funds`. It is
called before any I/O or state reads. The Soroban runtime enforces this at the
host level: if the authorization check fails, the transaction aborts immediately
with no side effects.

---

## Security Notes

### Overflow Prevention

`prepaid_balance` is typed as `i128` to match the Soroban token interface.  
The maximum representable value is `i128::MAX` (170,141,183,460,469,231,731,687,303,715,884,105,727).

All additions use `checked_add`, which returns `None` on overflow. The function
maps this to `Error::Overflow` and aborts, leaving `prepaid_balance` unchanged.

Plain arithmetic operators (`+`, `*`) are never used on financial values.

### Atomicity

Soroban smart contract execution is transactional. Within a single contract
invocation, if any operation panics (including a failed token transfer due to
insufficient allowance or balance), all storage mutations made during that
invocation are discarded. This guarantees that `prepaid_balance` cannot be
incremented without the corresponding USDC transfer completing successfully.

### USDC Decimal Convention

Stellar USDC uses 7 decimal places.  
All `amount` values in this contract are expressed in the smallest unit (stroops):

| Human-readable | Stroop value |
|---|---|
| 1 USDC | `10_000_000` |
| 0.1 USDC | `1_000_000` |
| 1 stroop | `1` |

The contract stores and operates on raw stroop values. No unit conversion is
performed internally. Callers are responsible for passing correctly scaled amounts.

### Minimum Top-up Enforcement

A configurable `min_topup` floor (set by the admin via `set_min_topup`) is
checked before any transfer occurs. Deposits below this threshold are rejected
with `Error::BelowMinimumTopup`, preventing micro-deposits that would not cover
even a single billing cycle.

---

## Execution Flow

```
1. subscriber.require_auth()
2. Check amount >= min_topup   → Error::BelowMinimumTopup if not
3. Load subscription from storage → Error::NotFound if missing
4. Check status != Cancelled   → Error::InvalidStatusTransition if cancelled
5. token_client.transfer(subscriber → contract, amount)  [atomic; may panic]
6. prepaid_balance = prepaid_balance.checked_add(amount) → Error::Overflow if saturated
7. If status == InsufficientBalance: transition → Active
8. Write updated subscription to storage
9. Return Ok(())
```
