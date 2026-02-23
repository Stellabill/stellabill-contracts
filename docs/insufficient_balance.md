# Insufficient Balance

Overview

When a subscription's next scheduled charge cannot be processed because the
subscription's `prepaid_balance` is less than the subscription `amount`, the
contract records this condition by transitioning the subscription status to
`InsufficientBalance` and returning an `InsufficientBalance` error. This is a
non-destructive, recoverable state: the subscriber's existing funds are left
intact (no partial debits are applied).

Recovery path

- The subscriber should call `deposit_funds` to top up the subscription's
  `prepaid_balance`.
- If the deposit makes `prepaid_balance >= amount`, the contract will validate
  the state transition and set the status back to `Active` (allowing future
  `charge_subscription` attempts to succeed).
- After topping up, a subsequent scheduled or manual charge will debit the
  required `amount` and update `last_payment_timestamp`.

UI and backend guidance

- Display a clear payment required state for `InsufficientBalance` with the
  current `prepaid_balance` and required `amount` to resume normal charges.
- Offer a one-click top-up flow that calls `deposit_funds` (ensure the user has
  approved the transfer on the token contract). On successful deposit, re-query
  the subscription and resume charge attempts.
- Do not attempt to silently or partially debit the subscription when balance
  is insufficient; the contract preserves the prepaid balance when rejecting a
  charge.

Invariants

- Failed interval charges do not modify `prepaid_balance`.
- The status transition to `InsufficientBalance` is recorded so UIs can avoid
  retrying charges until the user tops up.
- Once `prepaid_balance >= amount`, `deposit_funds` will restore the status to
  `Active` enabling normal charging behavior.
