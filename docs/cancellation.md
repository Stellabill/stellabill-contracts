# Subscription Cancellation and Refunds

## State Transitions

Subscriptions exist in one of the following states: `Active`, `Paused`, `Cancelled`, or `InsufficientBalance`.

When a subscription is cancelled via the `cancel_subscription` endpoint:

- The state transitions directly to `Cancelled`.
- This operation is idempotent: if the subscription is already `Cancelled`, the call succeeds without error and makes no changes.
- Cancellation guarantees that no further charges can be made against the subscription, as the billing engine will reject processing for non-Active states.

## Authorization

Cancellation can be triggered by either:

1. **The Subscriber** (the entity paying for the subscription)
2. **The Merchant** (the entity receiving the recurring payments)

Requiring authorization from either party ensures flexibility and protects both user autonomy and merchant management policies.

## Refund Model: Explicit Withdrawal

When a subscriber deposits funds into their `SubscriptionVault` for a specific subscription, those funds are credited to the `prepaid_balance`.

If the subscription is cancelled before these funds are exhausted, the remaining `prepaid_balance` belongs to the subscriber.

Stellarbill utilizes an **Explicit Withdrawal** model for refunds rather than issuing automatic, synchronous refunds during the cancellation call.

### Why Explicit Withdrawal?

1. **Merchant Independence**: If token transfers fail (e.g. the subscriber's Stellar account loses trustlines or becomes frozen), an automatic refund would fail the entire transaction. By divorcing cancellation from refunds, we guarantee a merchant can _always_ cancel a problematic subscription without being blocked by external token transfer constraints.
2. **Reentrancy Protection**: Explicit withdrawals are structurally safer and follow the recommended "pull over push" pattern for smart contract fund distribution.

### Getting a Refund

To retrieve their remaining funds, a subscriber performs the following steps:

1. `cancel_subscription` must be called to transition the status to `Cancelled`.
2. The subscriber calls `withdraw_subscriber_funds` authorizing the explicit withdrawal.
3. The vault transfers the remaining `prepaid_balance` (USDC or equivalent token) from the contract's balance to the subscriber's address.
4. The `prepaid_balance` in the contract state is reset to `0`.
