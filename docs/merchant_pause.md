# Merchant Pause

## Overview

Merchant-wide emergency pause provides merchants with a scoped circuit breaker that halts all charges for their subscriptions without affecting other merchants. This is useful during incidents specific to a merchant's service (e.g., service outage, security incident, maintenance).

## Semantics

### Pause State

- **Storage**: Per-merchant boolean flag stored at `DataKey::MerchantPaused(merchant_address)`
- **Default**: `false` (not paused)
- **Scope**: Affects all subscriptions where the merchant is the recipient

### Authorization

- **Pause**: Only the merchant can pause their own subscriptions (`merchant.require_auth()`)
- **Unpause**: Only the merchant can unpause their own subscriptions (`merchant.require_auth()`)

### Effects When Paused

**Blocked Operations:**
- `charge_subscription` - Returns `Error::MerchantPaused`
- `charge_usage` / `charge_usage_with_reference` - Returns `Error::MerchantPaused`
- `charge_one_off` - Returns `Error::MerchantPaused`
- `batch_charge` - Individual entries for paused merchants produce `success: false` with `error_code = MerchantPaused.to_code()`; other merchants in the same batch are unaffected

**Allowed Operations:**
- `withdraw_merchant_funds` - Merchants can still withdraw accumulated balances
- `withdraw_subscriber_funds` - Subscribers can withdraw refunds after cancellation
- `cancel_subscription` - Subscriptions can be cancelled
- `pause_subscription` / `resume_subscription` - Individual subscription pause state can be modified
- `deposit_funds` - Subscribers can still top up their prepaid balance

### Interaction with Other Pause Mechanisms

The contract has three levels of pause controls:

1. **Global Emergency Stop** (admin-controlled)
   - Checked first in entrypoints
   - Blocks all charges globally
   - Takes precedence over merchant pause

2. **Merchant Pause** (merchant-controlled)
   - Checked in charge functions after emergency stop
   - Blocks charges for specific merchant's subscriptions
   - Independent per merchant

3. **Subscription Pause** (subscriber or merchant-controlled)
   - Checked via subscription status
   - Blocks charges for individual subscription
   - Managed through subscription lifecycle

**Precedence Order:**
```
Emergency Stop → Merchant Pause → Subscription Status → Charge Logic
```

When a charge is attempted:
1. If emergency stop is active → `Error::EmergencyStopActive`
2. If merchant is paused → `Error::MerchantPaused`
3. If subscription status is not Active/GracePeriod → `Error::NotActive`
4. Otherwise → proceed with charge

## Conflict Resolution and Cross-Actor Rules

Merchant-wide pause is orthogonal to individual subscription status. This leads to the following rules:

- **Independent State**: If a merchant pauses their service, individual subscriptions retain their `Active`, `Paused`, or `InsufficientBalance` status.
- **Preference Preservation**: If a subscriber manually pauses their subscription while the merchant is already paused, and the merchant later unpauses, the subscription remains `Paused`. Individual subscriber preference is always preserved.
- **Terminal State**: Subscribers and merchants can always `cancel_subscription` regardless of the blanket merchant pause state.
- **Authorization**: All lifecycle changes (`pause_subscription`, `resume_subscription`, `cancel_subscription`) require authorization from either the subscriber or the merchant (actor attribution).
## Events

### MerchantPausedEvent
```rust
pub struct MerchantPausedEvent {
    pub merchant: Address,
    pub timestamp: u64,
}
```
Emitted when a merchant enables their pause. **Not re-emitted** if the merchant is already paused (idempotent calls produce no event).

### MerchantUnpausedEvent
```rust
pub struct MerchantUnpausedEvent {
    pub merchant: Address,
    pub timestamp: u64,
}
```
Emitted when a merchant disables their pause. **Not re-emitted** if the merchant was not paused (idempotent calls produce no event).

## API

### Query Pause Status
```rust
pub fn get_merchant_paused(env: Env, merchant: Address) -> bool
```
Returns `true` if the merchant is currently paused, `false` otherwise.

### Enable Pause
```rust
pub fn pause_merchant(env: Env, merchant: Address) -> Result<(), Error>
```
Pauses all subscriptions for the merchant. Requires merchant authorization.
Idempotent: calling multiple times has no additional effect.

### Disable Pause
```rust
pub fn unpause_merchant(env: Env, merchant: Address) -> Result<(), Error>
```
Unpauses all subscriptions for the merchant. Requires merchant authorization.
Idempotent: calling multiple times has no additional effect.

## Use Cases

### Service Outage
When a merchant's service is down, they can pause all subscriptions to prevent charges during the outage:
```rust
// Merchant pauses during incident
client.pause_merchant(&merchant);

// ... resolve incident ...

// Merchant resumes after recovery
client.unpause_merchant(&merchant);
```

### Scheduled Maintenance
Merchants can pause before maintenance windows and resume after:
```rust
// Before maintenance
client.pause_merchant(&merchant);

// Perform maintenance

// After maintenance
client.unpause_merchant(&merchant);
```

### Security Incident
If a merchant detects unauthorized access or security issues, they can immediately halt all charges:
```rust
// Immediate pause on security alert
client.pause_merchant(&merchant);

// Investigate and remediate

// Resume when safe
client.unpause_merchant(&merchant);
```

## Design Rationale

### Why Merchant-Scoped?

- **Isolation**: Issues with one merchant shouldn't affect others
- **Autonomy**: Merchants control their own service availability
- **Granularity**: More precise than global emergency stop

### Why Allow Withdrawals During Pause?

- **Liquidity**: Merchants may need funds during incidents
- **Subscriber Rights**: Subscribers should be able to cancel and get refunds
- **Flexibility**: Pause is about preventing new charges, not freezing all operations

### Why Check After Emergency Stop?

- **Admin Override**: Global emergency stop takes precedence for contract-wide incidents
- **Layered Defense**: Multiple independent circuit breakers
- **Clear Hierarchy**: Simpler reasoning about pause state

## Storage Impact

- **Per-merchant overhead**: 1 boolean flag per merchant who has ever toggled pause
- **Bounded growth**: At most one flag per unique merchant address
- **Cleanup**: Flags persist even after unpause (minimal storage cost)

## Testing

See `contracts/subscription_vault/src/test_merchant_pause.rs` for comprehensive test coverage:
- Toggle pause on/off, idempotency of pause/unpause
- Interval, usage, usage-with-reference, and one-off charges blocked when paused
- Balance not mutated on a blocked charge
- Charges succeed after unpause
- Batch charge reports per-entry error code; mixed paused/active batch
- Withdrawal, deposit, and cancellation allowed when paused
- Subscription status preserved when merchant pauses; subscriber pause not cleared on merchant unpause
- Emergency stop takes precedence over merchant pause
- Pause flag is isolated per merchant — pausing one merchant does not affect another
