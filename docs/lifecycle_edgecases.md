# Subscription Lifecycle Edge Cases

This document describes the edge cases and complex scenarios tested in the subscription vault contract lifecycle operations (pause, resume, cancel).

## State Machine Overview

The subscription vault implements a strict state machine with four states:

```
┌─────────────────────────────────────────────────────────────┐
│                                                               │
│  ┌────────┐  pause   ┌────────┐  resume  ┌────────┐        │
│  │ Active │◄────────►│ Paused │          │  Insuf │        │
│  │        │          │        │          │ Balance│        │
│  └───┬────┘          └───┬────┘          └───┬────┘        │
│      │                   │                   │              │
│      │ charge fails      │                   │ resume       │
│      │ (no balance)      │                   │              │
│      │                   │                   │              │
│      ▼                   │                   │              │
│  ┌────────┐              │                   │              │
│  │  Insuf │──────────────┴───────────────────┘              │
│  │ Balance│                                                  │
│  └───┬────┘                                                  │
│      │                                                       │
│      │ cancel                                                │
│      ▼                                                       │
│  ┌────────────┐◄──────────────────────────────────────────┐ │
│  │ Cancelled  │                                            │ │
│  │ (terminal) │                                            │ │
│  └────────────┘                                            │ │
│                                                             │ │
│  All states can transition to Cancelled ───────────────────┘ │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

### Valid Transitions

| From                | To                  | Allowed | Notes                                    |
|---------------------|---------------------|---------|------------------------------------------|
| Active              | Paused              | ✅      | User or merchant initiated               |
| Active              | Cancelled           | ✅      | User or merchant initiated               |
| Active              | InsufficientBalance | ✅      | System triggered on failed charge        |
| Paused              | Active              | ✅      | Resume operation                         |
| Paused              | Cancelled           | ✅      | User or merchant initiated               |
| Paused              | InsufficientBalance | ❌      | Cannot enter grace period while paused   |
| InsufficientBalance | Active              | ✅      | Resume after topping up                  |
| InsufficientBalance | Cancelled           | ✅      | User or merchant initiated               |
| InsufficientBalance | Paused              | ❌      | Cannot pause during grace period         |
| Cancelled           | Any                 | ❌      | Terminal state, no outgoing transitions  |
| Any                 | Same                | ✅      | Idempotent operations allowed            |

## Edge Cases Covered

### 1. Terminal State Enforcement

**Scenario**: Once cancelled, no transitions are possible.

**Tests**:
- `test_pause_cancel_resume_blocked`: Active → Paused → Cancelled → Resume (fails)
- `test_cancel_then_pause_blocked`: Active → Cancelled → Pause (fails)
- `test_invalid_cancelled_to_active`: Cancelled → Active (fails)
- `test_cancelled_to_insufficient_balance_blocked`: Cancelled → InsufficientBalance (fails)

**Expected Outcome**: All attempts to transition from Cancelled state fail with `InvalidStatusTransition` error.

### 2. Idempotent Operations

**Scenario**: Calling pause/cancel on already paused/cancelled subscriptions should succeed without side effects.

**Tests**:
- `test_pause_subscription_from_paused_is_idempotent`: Paused → Paused (succeeds)
- `test_cancel_subscription_from_cancelled_is_idempotent`: Cancelled → Cancelled (succeeds)
- `test_idempotent_pause_preserves_all_fields`: Multiple pause calls preserve balance, timestamp, amount
- `test_idempotent_cancel_preserves_all_fields`: Multiple cancel calls preserve all fields

**Expected Outcome**: Operations succeed, no state changes, all fields preserved.

### 3. Grace Period (InsufficientBalance) Restrictions

**Scenario**: During grace period, only resume (with topup) or cancel are allowed.

**Tests**:
- `test_pause_from_insufficient_balance_blocked`: InsufficientBalance → Paused (fails)
- `test_pause_during_grace_period`: InsufficientBalance → Paused (fails)
- `test_resume_from_insufficient_balance_succeeds`: InsufficientBalance → Active (succeeds)
- `test_cancel_from_insufficient_balance_succeeds`: InsufficientBalance → Cancelled (succeeds)
- `test_cancel_during_grace_period`: InsufficientBalance → Cancelled (succeeds)

**Expected Outcome**: Pause blocked, resume and cancel allowed.

### 4. Multiple Pause/Resume Cycles

**Scenario**: Users should be able to pause and resume multiple times.

**Tests**:
- `test_multiple_pause_resume_cycles`: 5 consecutive pause/resume cycles
- `test_full_lifecycle_active_pause_resume`: Active → Paused → Active → Paused
- `test_rapid_state_transitions`: Active → Paused → Active → Paused → Active

**Expected Outcome**: All transitions succeed, state correctly reflects each operation.

### 5. Authorization Scenarios

**Scenario**: Both subscriber and merchant can pause, resume, and cancel.

**Tests**:
- `test_merchant_can_cancel_active`: Merchant cancels subscription
- `test_merchant_can_pause`: Merchant pauses subscription
- `test_merchant_can_resume`: Merchant resumes subscription

**Expected Outcome**: Both parties have equal control over lifecycle operations.

### 6. State Preservation

**Scenario**: Pause/resume/cancel should not affect balance or timestamps.

**Tests**:
- `test_timestamp_preserved_across_pause_resume`: Timestamp unchanged after pause/resume
- `test_balance_preserved_across_pause_resume`: Balance unchanged after pause/resume
- `test_balance_preserved_on_cancel`: Balance unchanged after cancel

**Expected Outcome**: Only status field changes, all other fields preserved.

### 7. Charging Restrictions

**Scenario**: Charges should be blocked in non-Active states.

**Tests**:
- `test_charge_blocked_while_paused`: Charge fails on Paused subscription
- `test_charge_blocked_after_cancel`: Charge fails on Cancelled subscription
- `test_resume_and_charge_immediately`: Resume → Charge succeeds if interval elapsed

**Expected Outcome**: Charges only succeed on Active subscriptions with elapsed intervals.

### 8. Multi-Subscription Scenarios

**Scenario**: Multiple subscriptions with shared merchants in different states.

**Tests**:
- `test_shared_merchant_multiple_states`: 3 subscriptions (Active, Paused, Cancelled) with same merchant
- `test_pause_with_varying_intervals`: Pause subscriptions with daily, weekly, monthly intervals
- `test_batch_charge_with_paused_and_cancelled`: Batch charge with mixed states

**Expected Outcome**: Each subscription maintains independent state, batch operations handle failures gracefully.

### 9. Invalid Transition Sequences

**Scenario**: Complex invalid sequences should be blocked.

**Tests**:
- `test_invalid_insufficient_balance_to_paused`: InsufficientBalance → Paused (fails)
- `test_resume_subscription_from_cancelled_should_fail`: Cancelled → Active (fails)
- `test_pause_subscription_from_cancelled_should_fail`: Cancelled → Paused (fails)

**Expected Outcome**: All invalid transitions fail with `InvalidStatusTransition` error.

### 10. Complete Transition Coverage

**Scenario**: Every valid transition should be tested at least once.

**Tests**:
- `test_all_valid_transitions_coverage`: Exercises all 7 valid transitions:
  1. Active → Paused
  2. Active → Cancelled
  3. Active → InsufficientBalance
  4. Paused → Active
  5. Paused → Cancelled
  6. InsufficientBalance → Active
  7. InsufficientBalance → Cancelled

**Expected Outcome**: All valid transitions succeed.

## Testing Strategy

### Unit Test Organization

Tests are organized in the following sections in `contracts/subscription_vault/src/test.rs`:

1. **State Machine Helper Tests**: Pure function tests for `validate_status_transition`, `can_transition`, `get_allowed_transitions`
2. **Contract Entrypoint Tests**: Basic pause/resume/cancel operations
3. **Complex State Transition Sequences**: Multi-step lifecycle scenarios
4. **Invalid Transition Tests**: `#[should_panic]` tests for blocked transitions
5. **Comprehensive Edge Case Tests (#39)**: New comprehensive suite covering all edge cases

### Coverage Goals

- **Minimum 95% coverage** for lifecycle-related code paths
- All valid transitions tested at least once
- All invalid transitions tested with failure assertions
- Idempotent operations verified
- State preservation verified across operations
- Multi-subscription scenarios covered
- Authorization scenarios covered

### Test Naming Convention

Tests follow the pattern: `test_<scenario>_<expected_outcome>`

Examples:
- `test_pause_from_insufficient_balance_blocked`
- `test_multiple_pause_resume_cycles`
- `test_balance_preserved_on_cancel`

## Implementation Notes

### State Machine Validation

All lifecycle operations (`pause_subscription`, `resume_subscription`, `cancel_subscription`) call `validate_status_transition` before applying changes. This ensures:

1. Invalid transitions are caught early
2. Consistent error handling across all operations
3. Idempotent operations are explicitly allowed
4. Terminal state (Cancelled) is enforced

### Timestamp Behavior

- `last_payment_timestamp` is set at subscription creation
- Pause/resume/cancel do NOT modify `last_payment_timestamp`
- Only successful charges update `last_payment_timestamp`
- Interval enforcement continues from last payment, even after pause/resume

### Balance Behavior

- `prepaid_balance` is preserved across all lifecycle operations
- Only charges and deposits modify balance
- Cancelled subscriptions retain balance for potential refunds
- Balance can be withdrawn after cancellation (future feature)

### Grace Period (InsufficientBalance)

- Automatically set when charge fails due to insufficient balance
- Subscriber can resume by topping up (transitions to Active)
- Subscriber or merchant can cancel
- Cannot pause during grace period (must resolve first)

## Future Considerations

### Potential Enhancements

1. **Grace Period Duration**: Add configurable grace period before auto-cancellation
2. **Pause Duration Limits**: Optional maximum pause duration
3. **Refund on Cancel**: Automatic refund of remaining balance
4. **Pause History**: Track pause/resume events for analytics
5. **Cancellation Reasons**: Enum for cancellation reasons (user, merchant, system)

### Breaking Changes to Avoid

- Do not allow transitions from Cancelled (terminal state)
- Do not allow pause during grace period
- Maintain idempotent operation behavior
- Preserve balance and timestamp across lifecycle operations

## Related Documentation

- [State Machine Implementation](../contracts/subscription_vault/src/state_machine.rs)
- [Subscription Types](../contracts/subscription_vault/src/types.rs)
- [Test Suite](../contracts/subscription_vault/src/test.rs)
