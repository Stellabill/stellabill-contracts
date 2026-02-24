# Event Testing Documentation

## Overview

This document describes the event emission implementation and testing strategy for the SubscriptionVault contract. All key contract actions emit events for indexing and monitoring.

## Event Schema

### Lifecycle Events

#### `initialized`
- **Topics**: `["initialized"]`
- **Data**: `(token: Address, admin: Address, min_topup: i128)`
- **Emitted by**: `init()`
- **When**: Contract initialization

#### `created`
- **Topics**: `["created", subscription_id: u32]`
- **Data**: `(subscriber: Address, merchant: Address, amount: i128, interval_seconds: u64)`
- **Emitted by**: `create_subscription()`
- **When**: New subscription created

#### `paused`
- **Topics**: `["paused", subscription_id: u32]`
- **Data**: `(authorizer: Address)`
- **Emitted by**: `pause_subscription()`
- **When**: Subscription paused successfully

#### `resumed`
- **Topics**: `["resumed", subscription_id: u32]`
- **Data**: `(authorizer: Address)`
- **Emitted by**: `resume_subscription()`
- **When**: Subscription resumed successfully

#### `cancelled`
- **Topics**: `["cancelled", subscription_id: u32]`
- **Data**: `(authorizer: Address)`
- **Emitted by**: `cancel_subscription()`
- **When**: Subscription cancelled successfully

### Billing Events

#### `deposited`
- **Topics**: `["deposited", subscription_id: u32]`
- **Data**: `(subscriber: Address, amount: i128, new_balance: i128)`
- **Emitted by**: `deposit_funds()`
- **When**: Funds deposited successfully

#### `charged`
- **Topics**: `["charged", subscription_id: u32]`
- **Data**: `(amount: i128, remaining_balance: i128, timestamp: u64)`
- **Emitted by**: `charge_subscription()` / `batch_charge()`
- **When**: Subscription charged successfully

### Withdrawal Events

#### `withdrawn`
- **Topics**: `["withdrawn", merchant: Address]`
- **Data**: `(amount: i128)`
- **Emitted by**: `withdraw_merchant_funds()`
- **When**: Merchant withdraws funds

### Config Events

#### `min_topup_updated`
- **Topics**: `["min_topup_updated"]`
- **Data**: `(new_min_topup: i128)`
- **Emitted by**: `set_min_topup()`
- **When**: Minimum topup threshold updated

## Testing Strategy

### Positive Path Tests

Each successful operation emits exactly one event. Tests verify event emission by:
1. Recording event count before operation
2. Executing operation
3. Verifying event count increased by 1

### Negative Path Tests

Failed operations must NOT emit events. Tests verify:
1. Event count before operation
2. Operation fails as expected
3. Event count unchanged

Failure cases tested:
- Below minimum topup
- Interval not elapsed
- Invalid state transitions
- Not found errors
- Unauthorized access

### Batch Operation Tests

`batch_charge()` tests verify:
- No events on empty batch
- One event per successful charge
- No events for failed charges in partial failure scenarios

### Sequence Tests

Tests verify correct number of events in multi-step workflows:
- Create → Deposit → Pause → Resume → Cancel emits 5 events
- Multiple deposits emit multiple events

## Test Coverage

### Lifecycle Events (9 tests)
- `test_init_emits_event`
- `test_create_subscription_emits_event`
- `test_pause_subscription_emits_event`
- `test_resume_subscription_emits_event`
- `test_cancel_subscription_emits_event`
- `test_set_min_topup_emits_event`
- `test_withdraw_merchant_funds_emits_event`
- `test_lifecycle_events_sequence`
- `test_multiple_deposits_emit_multiple_events`

### Billing Events (3 tests)
- `test_deposit_funds_emits_event`
- `test_charge_subscription_emits_event`
- `test_charge_event_data_accuracy`
- `test_deposit_event_cumulative_balance`

### Failure Cases (5 tests)
- `test_failed_deposit_no_event`
- `test_failed_charge_no_event`
- `test_failed_pause_no_event`
- `test_failed_resume_no_event`
- `test_failed_cancel_no_event`

### Batch Operations (3 tests)
- `test_batch_charge_no_events_on_empty`
- `test_batch_charge_emits_events_for_successes`
- `test_batch_charge_partial_failure_events`

## Implementation Notes

### Event Emission Pattern

Events are emitted immediately after successful storage updates:

```rust
env.storage().instance().set(&subscription_id, &sub);
env.events().publish(
    (Symbol::new(env, "event_name"), subscription_id),
    event_data,
);
Ok(())
```

### Error Handling

Events are only emitted on success. Early returns on validation failures prevent event emission:

```rust
if amount < min_topup {
    return Err(Error::BelowMinimumTopup); // No event
}
// ... success path with event
```

### Batch Charge

`batch_charge()` emits events per-subscription via `charge_one()`. Failed charges return errors without emitting events, while successful charges emit normally.

### Testing Approach

Due to Soroban SDK event handling in tests, we verify event emission by counting events before and after operations rather than inspecting event data directly. This approach:
- Confirms events are emitted
- Verifies correct number of events
- Ensures no events on failures
- Works reliably across SDK versions

## Indexer Integration

Events can be indexed by:
- **Topic[0]**: Event type for filtering
- **Topic[1]**: Subscription ID or merchant address for entity-specific queries
- **Data**: Detailed information for analytics

Example queries:
- All events for subscription #42: filter by topic[1] = 42
- All charges: filter by topic[0] = "charged"
- Merchant withdrawals: filter by topic[0] = "withdrawn" and topic[1] = merchant_address

## Maintenance

When adding new contract functions:
1. Add event emission after successful state change
2. Add positive path test verifying event count increases
3. Add negative path test verifying no event on failure
4. Update this document with event schema
5. Run full test suite: `cargo test`

## Known Limitations

- Event data validation in tests is limited by Soroban SDK's Val type not implementing PartialEq
- Tests verify event emission by counting rather than inspecting event data
- Event data correctness is verified indirectly through state assertions
