# Insufficient Balance Handling

This document describes the insufficient balance handling system in the Subscription Vault contract, including the state machine, failure guarantees, and recovery flows.

## Overview

The Subscription Vault implements robust insufficient balance protection to ensure that charges never execute when the prepaid balance is too low. This protects both the merchant (from failed charges) and the subscriber (from unexpected service interruptions).

## State Diagram

```
                    ┌─────────────────────────────────────────────────────────┐
                    │                                                         │
                    ▼                                                         │
              ┌──────────┐                                                  │
              │          │                                                  │
    ┌────────▶│  Active  │◀───────────────┐                               │
    │         │          │                 │                               │
    │         └──────────┘                 │                               │
    │               │                      │                               │
    │               │ Charge fails         │ Resume after                   │
    │               │ (insufficient        │ deposit                        │
    │               │ balance)             │                                │
    │               ▼                      │                               │
    │         ┌────────────────┐           │                               │
    │         │                │           │                               │
    └─────────│Insufficient    │           └───────────────────────────────┘
              │Balance         │
              │                │
              └────────────────┘
                    │
                    │
         Cancel from either state
                    │
                    ▼
              ┌───────────┐
              │ Cancelled │
              │ (terminal)│
              └───────────┘
```

## Status Values

| Status | Description |
|--------|-------------|
| `Active` | Subscription is active and ready for charging. |
| `Paused` | Subscription is temporarily paused. |
| `InsufficientBalance` | Subscription failed due to insufficient funds. |
| `Cancelled` | Subscription is permanently terminated. |

## Transition Rules

### Allowed Transitions

| From | To | Condition |
|------|-----|-----------|
| Active | InsufficientBalance | Automatic on failed charge |
| Active | Paused | Subscriber or merchant calls pause |
| Active | Cancelled | Subscriber or merchant calls cancel |
| InsufficientBalance | Active | After deposit + resume |
| InsufficientBalance | Cancelled | Subscriber calls cancel |
| Paused | Active | Subscriber calls resume |
| Paused | Cancelled | Subscriber calls cancel |

### Invalid Transitions (Return Error)

| From | To | Error |
|------|-----|-------|
| InsufficientBalance | Paused | InvalidStatusTransition |
| Cancelled | Any | InvalidStatusTransition |

## Failure Guarantees

### Non-Destructive Failure

When a charge fails due to insufficient balance:

1. **No Balance Deduction**: The subscriber's prepaid balance is NOT modified
2. **No Token Transfer**: No tokens are transferred to the merchant
3. **Minimal State Change**: Only the status changes to `InsufficientBalance`
4. **Atomic Operation**: No partial state updates occur

### Invariants Preserved

- `prepaid_balance` remains unchanged
- `last_payment_timestamp` remains unchanged
- `merchant` and `subscriber` addresses remain unchanged
- `amount` (charge amount) remains unchanged

## Charge Flow

### Successful Charge

```
1. Validate subscription exists
2. Validate status is Active
3. Validate interval has elapsed
4. Validate prepaid_balance >= amount
5. Deduct amount from prepaid_balance
6. Update last_payment_timestamp
7. Emit SubscriptionChargedEvent
8. Return success
```

### Failed Charge (Insufficient Balance)

```
1. Validate subscription exists
2. Validate status is Active
3. Validate interval has elapsed
4. Check: prepaid_balance >= amount?
   → NO: Transition to InsufficientBalance
   → Return InsufficientBalance error
   → DO NOT modify any balances
```

## Error Handling

### Error Codes

| Code | Error | Description |
|------|-------|-------------|
| 1003 | InsufficientBalance | Charge failed due to insufficient prepaid balance |
| 1002 | NotActive | Subscription is not Active (Paused, Cancelled, InsufficientBalance) |
| 1001 | IntervalNotElapsed | Not enough time since last charge |
| 1007 | Replay | This period has already been charged |

### Error Response Structure

When a charge fails due to insufficient balance, the error includes:
- Error code: `1003` (InsufficientBalance)
- Status transition: `Active` → `InsufficientBalance`

## Recovery Flow

### Scenario: Subscriber's Balance Runs Out

1. **Initial State**: Subscription is Active with some balance
2. **Charge Fails**: Balance falls below charge amount
3. **Status Changes**: Subscription → InsufficientBalance
4. **Service Impact**: No further charges processed

### Recovery Steps

1. **Deposit Funds**: Subscriber calls `deposit_funds` to add more funds
2. **Resume Subscription**: Subscriber calls `resume_subscription` (or it's automatic)
3. **Charge Again**: Next charge attempt will succeed if balance is sufficient

### Recovery via Resume

```rust
// After deposit, subscriber must resume
client.resume_subscription(&subscription_id, &subscriber);

// Now status is Active again
// Next charge will succeed if balance >= amount
```

### Auto-Recovery Pattern

Some implementations may choose to auto-recover:

1. Detect InsufficientBalance status
2. Poll for deposit (via events or queries)
3. When balance >= amount, auto-resume
4. Next scheduled charge succeeds

## Integration Guidance

### For UI Applications

1. **Display Balance**: Show subscriber their current prepaid balance
2. **Low Balance Warning**: Alert when balance < 2x monthly charge
3. **Payment Prompt**: When status is InsufficientBalance, show clear payment CTA
4. **Status Indicator**: Visual indicator for InsufficientBalance state

### For Backend Services

1. **Monitor Events**: Listen for `SubscriptionChargedEvent` and status changes
2. **Handle Failures**: When `InsufficientBalance` error occurs:
   - Don't retry immediately (will fail again)
   - Notify subscriber to add funds
   - Retry after deposit confirmed
3. **Batch Operations**: Check status before including in batch charge

### Example: Handling Failed Charge

```rust
match client.try_charge_subscription(&subscription_id) {
    Ok(()) => {
        // Charge succeeded
    }
    Err(Error::InsufficientBalance) => {
        // Get subscription to check balance
        let sub = client.get_subscription(&subscription_id);
        // Notify subscriber: need ${sub.amount - sub.prepaid_balance} more
    }
    Err(e) => {
        // Handle other errors
    }
}
```

## Test Scenarios

### Core Tests

| Test | Description |
|------|-------------|
| `test_charge_succeeds_when_balance_equals_amount` | Edge case: exact balance |
| `test_charge_fails_when_balance_less_than_amount` | Basic insufficient case |
| `test_charge_fails_when_balance_zero` | Zero balance |
| `test_charge_fails_just_below_amount` | Off-by-one |
| `test_multiple_failed_charges_no_state_corruption` | No state drift |

### Recovery Tests

| Test | Description |
|------|-------------|
| `test_deposit_after_failure_enables_charging` | Full recovery flow |
| `test_rapid_deposit_charge_sequence` | Quick recovery |
| `test_status_transition_insufficient_balance_to_active` | Resume works |

### Invariant Tests

| Test | Description |
|------|-------------|
| `test_failed_charge_preserves_state` | Balance unchanged |
| `test_no_double_charges` | No over-charging |
| `test_no_tokens_lost_after_failed_charge` | Token conservation |

## Security Considerations

1. **No Partial Updates**: Charge either fully succeeds or fully fails
2. **Status Validation**: Only Active subscriptions can be charged
3. **Replay Protection**: Same period cannot be charged twice
4. **Deterministic Execution**: Same inputs always produce same outputs
5. **Overflow Safety**: All arithmetic uses checked operations

## Gas Efficiency

The charge function uses early validation to avoid unnecessary state modifications:

1. Status check first (cheap)
2. Interval check second (cheap)
3. Balance check before any state write (prevents revert costs)
4. Only write state on success

This ensures that failed charges (insufficient balance) are cheap operations that only update status, while successful charges do the full state updates.
