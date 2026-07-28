# Contributing to Stellabill Contracts

## Event Emit Ordering Convention

**CRITICAL SECURITY REQUIREMENT**: All events MUST be emitted AFTER state has been written and is durable. This prevents indexer double-counting under partial-failure scenarios.

### Rule

Always follow this pattern:
1. **Checks**: Validate all preconditions
2. **Effects**: Write all state mutations (storage writes, balance updates, etc.)
3. **Events**: Emit events only after state is durable

### Correct Pattern

```rust
// ✅ CORRECT: State written first, then event emitted
write_subscription(env, subscription_id, &sub);
env.storage().instance().set(&key, &value);

env.events().publish(
    (Symbol::new(env, "event_name"), subscription_id),
    Event { /* ... */ },
);
```

### Incorrect Pattern

```rust
// ❌ INCORRECT: Event emitted before state is written
env.events().publish(
    (Symbol::new(env, "event_name"), subscription_id),
    Event { /* ... */ },
);
write_subscription(env, subscription_id, &sub);
```

### Why This Matters

If an event is emitted before state is written and the transaction fails after event emission but before state persistence:
- The event is visible to indexers
- The state change never occurred
- Indexers will double-count or have inconsistent state
- This can lead to incorrect accounting and financial discrepancies

### Implementation Guidelines

1. **Batch operations**: Ensure all state writes complete before any events in the batch
2. **Cross-module events**: When multiple modules are involved, ensure the final state write happens before any cross-module events
3. **Panicking write paths**: If a write panics, no event should have been emitted yet (follows naturally from this convention)
4. **Temporary storage**: If you need to emit an event based on data that would be overwritten, store the event data in a temporary variable before the state write, then emit after

### Examples

#### Storing Event Data for Later Emission

```rust
let should_emit_fee_event = if fee_amount > 0 {
    if let Some(ref treasury) = treasury_opt {
        credit_merchant_balance_for_token(env, treasury, &token, fee_amount)?;
        Some((treasury.clone(), fee_amount))
    } else {
        None
    }
} else {
    None
};

write_subscription(env, subscription_id, &sub);

// Emit after state is written
if let Some((treasury, fee)) = should_emit_fee_event {
    env.events().publish(
        (Symbol::new(env, "protocol_fee_charged"), subscription_id),
        ProtocolFeeChargedEvent {
            subscription_id,
            fee_amount: fee,
            treasury,
            // ...
        },
    );
}
```

### Enforcement

This convention is enforced through:
- Code review
- Automated linting (see `contracts/subscription_vault/tests/emit_ordering_test.rs`)
- Integration tests that verify event ordering

When adding new code that emits events, always ensure events are emitted after all state mutations are complete.
