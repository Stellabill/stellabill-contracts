# Subscription Lifecycle Events

This document describes the event schemas emitted by the `subscription_vault` contract for indexing and monitoring subscription lifecycle actions.

## Event Overview

All events are emitted using Soroban's native event system and can be consumed by indexers, backends, and monitoring tools. Events are emitted exactly once per action with minimal redundancy.

## Event Schemas

### SubscriptionCreatedEvent

**Topic:** `sub_new`

Emitted when a new subscription is created.

**Fields:**
- `subscription_id` (u32): Unique identifier for the subscription
- `subscriber` (Address): Address of the subscriber
- `merchant` (Address): Address of the merchant receiving payments
- `amount` (i128): Payment amount per billing interval (in token base units)
- `interval_seconds` (u64): Billing interval in seconds

**Indexing Strategy:**
- Index by `subscription_id` for lookup
- Index by `subscriber` and `merchant` for filtering user/merchant subscriptions
- Track creation timestamp from ledger metadata

**Example Use Cases:**
- Build subscriber dashboard showing all subscriptions
- Merchant analytics on new subscriptions
- Monitoring subscription creation rate

---

### FundsDepositedEvent

**Topic:** `deposit`

Emitted when a subscriber deposits funds to their subscription vault.

**Fields:**
- `subscription_id` (u32): Subscription receiving the deposit
- `subscriber` (Address): Address making the deposit
- `amount` (i128): Amount deposited (in token base units)
- `new_balance` (i128): Total prepaid balance after deposit

**Indexing Strategy:**
- Index by `subscription_id` to track balance history
- Aggregate deposits per subscriber for analytics
- Monitor `new_balance` for low-balance alerts

**Example Use Cases:**
- Display deposit history in subscriber UI
- Alert subscribers when balance is low
- Track total value locked in the contract

---

### SubscriptionChargedEvent

**Topic:** `charged`

Emitted when a subscription is charged for a billing interval.

**Fields:**
- `subscription_id` (u32): Subscription that was charged
- `merchant` (Address): Merchant receiving the payment
- `amount` (i128): Amount charged (in token base units)
- `remaining_balance` (i128): Prepaid balance remaining after charge

**Indexing Strategy:**
- Index by `subscription_id` for payment history
- Index by `merchant` to track merchant revenue
- Monitor `remaining_balance` for insufficient balance warnings

**Example Use Cases:**
- Generate merchant revenue reports
- Track subscription payment history
- Trigger notifications when balance is insufficient for next charge

---

### SubscriptionPausedEvent

**Topic:** `paused`

Emitted when a subscription is paused (no charges until resumed).

**Fields:**
- `subscription_id` (u32): Subscription that was paused
- `authorizer` (Address): Address that authorized the pause (subscriber or merchant)

**Indexing Strategy:**
- Index by `subscription_id` to track status changes
- Track pause duration by comparing with resume events

**Example Use Cases:**
- Display paused status in UI
- Analytics on pause frequency and duration
- Notify relevant parties of status change

---

### SubscriptionResumedEvent

**Topic:** `resumed`

Emitted when a paused subscription is resumed.

**Fields:**
- `subscription_id` (u32): Subscription that was resumed
- `authorizer` (Address): Address that authorized the resume (subscriber or merchant)

**Indexing Strategy:**
- Index by `subscription_id` to track status changes
- Calculate pause duration by comparing with pause events

**Example Use Cases:**
- Update subscription status in UI
- Track subscription lifecycle metrics
- Resume billing operations

---

### SubscriptionCancelledEvent

**Topic:** `cancelled`

Emitted when a subscription is cancelled by subscriber or merchant.

**Fields:**
- `subscription_id` (u32): Subscription that was cancelled
- `authorizer` (Address): Address that authorized the cancellation
- `refund_amount` (i128): Remaining prepaid balance available for refund

**Indexing Strategy:**
- Index by `subscription_id` for final status
- Track cancellation rate by subscriber/merchant
- Monitor `refund_amount` for refund processing

**Example Use Cases:**
- Process refunds to subscribers
- Calculate churn rate and cancellation analytics
- Archive cancelled subscriptions

---

### MerchantWithdrawalEvent

**Topic:** `withdraw`

Emitted when a merchant withdraws accumulated funds.

**Fields:**
- `merchant` (Address): Merchant withdrawing funds
- `amount` (i128): Amount withdrawn (in token base units)
- `remaining_balance` (i128): Merchant's accumulated balance remaining after withdrawal

**Indexing Strategy:**
- Index by `merchant` to track withdrawal history
- Aggregate total withdrawals per merchant
- Monitor withdrawal frequency

**Example Use Cases:**
- Display merchant withdrawal history
- Track merchant payout schedules
- Reconcile merchant balances

---

## General Indexing Recommendations

### Event Consumption

1. **Subscribe to contract events** using Stellar RPC or Horizon API
2. **Filter by contract address** to get only subscription vault events
3. **Parse event topics** to identify event type
4. **Decode event data** using the schemas above

### Storage Strategy

- Store events in time-series database for historical analysis
- Maintain current state in relational database for fast queries
- Index by `subscription_id`, `subscriber`, and `merchant` addresses

### Error Handling

- Events are emitted after state changes succeed
- If a transaction fails, no event is emitted
- Monitor transaction status alongside events

### Privacy Considerations

- Events contain only addresses and amounts (no personal data)
- Addresses are pseudonymous but publicly visible on-chain
- Off-chain systems should implement additional privacy controls

---

## Example Event Flow

**Typical subscription lifecycle:**

1. `SubscriptionCreatedEvent` - Subscriber creates subscription
2. `FundsDepositedEvent` - Subscriber deposits initial funds
3. `SubscriptionChargedEvent` (recurring) - Billing engine charges subscription
4. `FundsDepositedEvent` (as needed) - Subscriber tops up balance
5. `SubscriptionPausedEvent` (optional) - Subscriber pauses temporarily
6. `SubscriptionResumedEvent` (optional) - Subscriber resumes
7. `SubscriptionCancelledEvent` - Subscriber or merchant cancels
8. `MerchantWithdrawalEvent` (periodic) - Merchant withdraws earnings

---

## Integration Examples

### Indexer Pseudocode

```rust
// Listen for events
for event in contract_events {
    match event.topic {
        "sub_new" => {
            let data: SubscriptionCreatedEvent = decode(event.data);
            db.insert_subscription(data);
        }
        "deposit" => {
            let data: FundsDepositedEvent = decode(event.data);
            db.update_balance(data.subscription_id, data.new_balance);
        }
        "charged" => {
            let data: SubscriptionChargedEvent = decode(event.data);
            db.record_payment(data);
        }
        // ... handle other events
    }
}
```

### Backend Monitoring

```javascript
// Monitor for low balance
events.on('charged', (event) => {
  if (event.remaining_balance < event.amount * 2) {
    notifySubscriber(event.subscription_id, 'Low balance warning');
  }
});

// Track merchant revenue
events.on('charged', (event) => {
  analytics.recordRevenue(event.merchant, event.amount);
});
```

---

## Version History

- **v1.0** (2026-02-20): Initial event schema definitions for all lifecycle actions
