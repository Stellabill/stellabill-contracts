# Emergency Stop (Circuit Breaker) - Incident Response Guide

## Overview

The Emergency Stop mechanism is a circuit breaker designed to halt critical financial operations in the Subscription Vault smart contract during incident response scenarios. When activated, it blocks new subscriptions, deposits, and charges while allowing safe operations like withdrawals and queries to continue.

## Semantics and Lifetime

The emergency stop has **no automatic expiry**. It remains active until an admin explicitly calls `disable_emergency_stop`. This is intentional: the stop should only be lifted after a human confirms the incident is resolved.

- Toggling is **idempotent**: enabling when already enabled (or disabling when already disabled) is a no-op and emits no event.
- Every real state transition emits an on-chain event (`EmergencyStopEnabledEvent` / `EmergencyStopDisabledEvent`) with the admin address and timestamp for audit purposes.
- The stop flag is stored in instance storage under the key `"emergency_stop"` and defaults to `false` (not stopped) when unset.

## Safe Recovery Behavior

The following operations remain available during an active emergency stop to ensure **no fund loss and no permanent lockout**:

| Operation | Reason allowed |
|-----------|---------------|
| `get_subscription`, `get_admin`, `get_emergency_stop_status`, all read queries | Read-only; no financial risk |
| `cancel_subscription` | Subscribers can exit at any time |
| `pause_subscription` / `resume_subscription` | State management; no fund movement |
| `withdraw_subscriber_funds` | Subscribers can reclaim prepaid balance from cancelled subscriptions |
| `withdraw_merchant_funds` / `withdraw_merchant_token_funds` | Merchants can collect already-earned balances |
| `partial_refund` | Admin can issue refunds during the stop |

This ensures that even under a prolonged emergency stop, no party is permanently locked out of their funds.

## Purpose

The emergency stop is intended for use when:

- **Security Incidents**: A vulnerability is discovered that could allow unauthorized fund extraction
- **Smart Contract Bugs**: A critical bug is found that could lead to financial loss
- **External Integrations Compromised**: An integrated system (e.g., payment processor, oracle) is compromised
- **Regulatory Requirements**: Legal or regulatory directives require halting operations
- **Mass Exit Scenarios**: Unusual subscription cancellation patterns suggest potential attack

## Who Is Authorized

Only the **contract admin** can trigger the emergency stop. This is enforced by:

1. Requiring `admin.require_auth()` - the admin must sign the transaction
2. Verifying the caller matches the stored admin address
3. Returning `Error::Unauthorized` (401) for any non-admin caller

## Activation Procedure

### Step 1: Verify Admin Identity

Ensure you are using the correct admin address. You can verify this by calling:

```rust
// Via RPC/client
let admin = subscription_vault.get_admin();
```

### Step 2: Enable Emergency Stop

Call the `enable_emergency_stop` entrypoint:

```rust
// Via RPC/client
subscription_vault.enable_emergency_stop(&admin_address);
```

This will:
- Set the `emergency_stop` flag to `true` in contract storage
- Emit an `EmergencyStopEnabledEvent` with the admin address and timestamp
- Immediately block all guarded operations

### Step 3: Verify Activation

Confirm the emergency stop is active:

```rust
let is_active = subscription_vault.get_emergency_stop_status();
// Returns: true
```

## Deactivation Procedure

**⚠️ IMPORTANT**: Only deactivate after the incident has been resolved and the contract is safe to operate.

### Step 1: Confirm Incident Resolution

Before deactivating, ensure:

- [ ] Vulnerability has been identified and patched
- [ ] No unauthorized access has occurred
- [ ] All affected subscriptions have been identified
- [ ] Funds are secure
- [ ] External integrations are verified safe

### Step 2: Disable Emergency Stop

```rust
// Via RPC/client
subscription_vault.disable_emergency_stop(&admin_address);
```

This will:
- Set the `emergency_stop` flag to `false`
- Emit an `EmergencyStopDisabledEvent` with the admin address and timestamp
- Restore normal contract operations

### Step 3: Verify Deactivation

```rust
let is_active = subscription_vault.get_emergency_stop_status();
// Returns: false
```

## Operations During Emergency Stop

### Blocked Operations (Financial Risk)

These operations will fail with `Error::EmergencyStopActive` (1009):

| Operation | Entry Point | Description |
|-----------|-------------|-------------|
| Create Subscription | `create_subscription` | New subscription agreements |
| Create Subscription (Token-specific) | `create_subscription_with_token` | New subscription agreements with explicit settlement token |
| Create Subscription (From Plan) | `create_subscription_from_plan` | New subscription agreements from plan templates |
| Deposit Funds | `deposit_funds` | Adding funds to existing subscriptions |
| Charge Subscription | `charge_subscription` | Interval-based billing charges |
| Charge Usage | `charge_usage` | Usage-based billing charges |
| Charge Usage (Reference) | `charge_usage_with_reference` | Usage-based billing with caller-supplied reference/idempotency key |
| One-off Charge | `charge_one_off` | Merchant-triggered ad-hoc prepaid debits |
| Batch Charge | `batch_charge` | Bulk subscription charging |

### Allowed Operations (No Financial Risk)

These operations remain functional:

| Operation | Entry Point | Description |
|-----------|-------------|-------------|
| Query Subscription | `get_subscription` | Read subscription details |
| Query Admin | `get_admin` | Read admin address |
| Query Min Topup | `get_min_topup` | Read minimum top-up threshold |
| Query Status | `get_emergency_stop_status` | Read emergency stop state |
| Merchant Withdraw | `withdraw_merchant_funds` | Merchant withdrawals |
| Cancel Subscription | `cancel_subscription` | Subscriber cancellation |
| Pause Subscription | `pause_subscription` | Pause charges |
| Resume Subscription | `resume_subscription` | Resume charges |
| Withdraw Subscriber Funds | `withdraw_subscriber_funds` | Subscriber fund withdrawal |

## Error Codes

| Code | Error Variant | Description |
|------|---------------|-------------|
| 1009 | `EmergencyStopActive` | Operation blocked due to active emergency stop |

## Risks of Misuse

### Potential Harms

1. **Service Disruption**: Activating emergency stop blocks all new subscriptions and deposits
2. **User Impact**: Subscribers cannot add funds or create new subscriptions
3. **Revenue Loss**: Merchants cannot receive new subscription payments
4. **Reputation Damage**: Emergency stop signals a serious issue to users

### Mitigation

- **Idempotent Toggling**: Enabling when already enabled or disabling when already disabled is safe (no-op)
- **Deterministic Eventing**: `EmergencyStopEnabledEvent` / `EmergencyStopDisabledEvent` are emitted only on real state transitions
- **Comprehensive Logging**: All state changes emit events for audit trails
- **Read-Only Access**: Queries remain functional for transparency

## Security Considerations

### Access Control

- Only the admin can toggle emergency stop
- Non-admin attempts return `Error::Unauthorized` (401)
- Admin rotation is independent of emergency stop state

### Audit Trail

Every state change emits an event:

```rust
// When enabled
EmergencyStopEnabledEvent {
    admin: Address,
    timestamp: u64,
}

// When disabled
EmergencyStopDisabledEvent {
    admin: Address,
    timestamp: u64,
}
```

### Testing Coverage

The emergency stop mechanism includes comprehensive test coverage:

- Normal operation (disabled)
- Emergency stop enabled
- Emergency stop disabled
- Repeated toggling
- Edge cases (paused/cancelled subscriptions)
- All error variants explicitly tested

## Production Incident Checklist

Use this checklist when responding to an incident:

### Initial Response

- [ ] Identify the nature of the incident
- [ ] Assess immediate risk to funds
- [ ] Document initial findings
- [ ] Notify relevant stakeholders

### Activation Decision

- [ ] Confirm admin identity
- [ ] Verify no unauthorized access has occurred
- [ ] Assess if emergency stop will mitigate the issue
- [ ] Get approval from security team (if applicable)

### Activation

- [ ] Enable emergency stop
- [ ] Verify activation via `get_emergency_stop_status`
- [ ] Document timestamp of activation

### During Emergency Stop

- [ ] Monitor blocked operations (should fail)
- [ ] Verify allowed operations still work
- [ ] Investigate root cause
- [ ] Identify affected subscriptions

### Resolution

- [ ] Patch vulnerability
- [ ] Verify fix
- [ ] Get approval to deactivate
- [ ] Disable emergency stop
- [ ] Verify normal operations resume

### Post-Incident

- [ ] Document incident timeline
- [ ] Review response effectiveness
- [ ] Update procedures if needed

## Example Integration

### Detecting Emergency Stop in Application Code

```rust
// Check before operations that might fail
let is_emergency_stopped = client.get_emergency_stop_status();
if is_emergency_stopped {
    // Show appropriate message to user
    // Redirect to status page
}

// Handle the error gracefully
match client.try_create_subscription(...) {
    Ok(id) => { /* success */ }
    Err(Error::EmergencyStopActive) => {
        // Show "Service temporarily paused" message
    }
    Err(e) => { /* Handle other errors */ }
}
```

### Monitoring Emergency Stop Events

```rust
// Listen for emergency stop events
env.events()
    .publish((Symbol::new("emergency_stop_enabled"),), event);
env.events()
    .publish((Symbol::new("emergency_stop_disabled"),), event);
```

## Contract References

- **Error Code**: `Error::EmergencyStopActive` = 1009
- **Storage Key**: `DataKey::EmergencyStop`
- **Event Types**: `EmergencyStopEnabledEvent`, `EmergencyStopDisabledEvent`
