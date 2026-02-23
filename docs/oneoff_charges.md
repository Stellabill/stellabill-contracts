# Merchant-Initiated One-Off Charges

This document describes the one-off charge feature: merchant-authorized debits from a subscriber's prepaid balance, independent of the subscription's billing interval.

## Overview

`charge_one_off(subscription_id, merchant, amount)` lets the **merchant** debit a one-time `amount` from the subscription's prepaid balance. It is distinct from:

- **Interval-based charges** (`charge_subscription`): triggered by the billing engine on a schedule; require admin auth.
- **Subscription cancellation or modifications**: lifecycle actions by subscriber or merchant (pause, resume, cancel).

One-off charges are intended for ad-hoc fees (e.g. overage, one-time add-ons) that the merchant is authorized to collect from the subscriber's existing prepaid balance.

## Semantics

- **Authorization**: The caller must be the subscription's **merchant** and must authorize the call (Soroban auth).
- **Balance**: `amount` must be positive and must not exceed the subscription's `prepaid_balance`. No overdraft.
- **Status**: The subscription must be **Active** or **Paused**. One-off charges are not allowed on Cancelled or InsufficientBalance.
- **Effect**: `prepaid_balance` is decreased by `amount`. No change to `last_payment_timestamp` or interval logic. Funds are considered collected by the merchant (payout semantics are the same as for recurring charges; see merchant withdrawal).

## Event

**Topic:** `oneoff_ch`

**Payload:** `OneOffChargedEvent { subscription_id, merchant, amount }`

Indexers can use this to track one-off revenue and balance history alongside recurring `charged` events.

## When to Use

- One-time add-ons or overages within the same billing relationship.
- Fees that both parties have agreed to debit from the existing prepaid balance.
- When the merchant is trusted to initiate the debit (authorization is enforced on-chain).

## When Not to Use

- Do not use for recurring billing; use `charge_subscription` (or batch) with interval enforcement.
- Do not use for subscription cancellation or refunds; use the lifecycle entrypoints (cancel, etc.).
- Do not use when the subscriber has not prepaid enough; the call will fail with `InsufficientBalance`.

## Security Notes

- Only the subscription's merchant can call `charge_one_off` for that subscription; otherwise `Unauthorized` is returned.
- Amount and balance checks prevent overdraft; safe math is used.
- One-off and interval-based charges coexist: both debit from the same `prepaid_balance`. Ensure sufficient balance for both recurring and one-off usage.
