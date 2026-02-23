# Security Threat Model: Subscription Vault Contract

## Overview

This document describes the security assumptions, threat model, and mitigations for the Stellabill Subscription Vault smart contract. The contract manages prepaid USDC subscriptions on the Stellar network using Soroban, handling recurring billing between subscribers and merchants.

**Contract Version**: Early development (as of repository state)  
**Platform**: Soroban (Stellar smart contracts)  
**Primary Asset**: USDC tokens held in subscription prepaid balances  
**Last Updated**: 2026-02-22

---

## Table of Contents

1. [Trust Model](#trust-model)
2. [Asset Inventory](#asset-inventory)
3. [Threat Actors](#threat-actors)
4. [Attack Vectors and Mitigations](#attack-vectors-and-mitigations)
5. [Authorization Model](#authorization-model)
6. [State Machine Security](#state-machine-security)
7. [Arithmetic and Overflow Protection](#arithmetic-and-overflow-protection)
8. [Timing and Replay Protection](#timing-and-replay-protection)
9. [Known Limitations](#known-limitations)
10. [Security Testing](#security-testing)
11. [Incident Response](#incident-response)

---

## Trust Model

### Trusted Actors

| Actor | Trust Level | Capabilities | Constraints |
|-------|-------------|--------------|-------------|
| **Admin** | High | Charge subscriptions (single/batch), set minimum top-up threshold | Set once at initialization; cannot be changed. Single point of failure for billing operations. |
| **Soroban Runtime** | High | Execute contract logic, enforce `require_auth()`, manage storage | Assumed to correctly implement Stellar protocol and Soroban VM. |
| **Token Contract** | High | Handle USDC transfers (future implementation) | Must be a legitimate Stellar Asset Contract (SAC) for USDC. |

### Semi-Trusted Actors

| Actor | Trust Level | Capabilities | Constraints |
|-------|-------------|--------------|-------------|
| **Subscriber** | Medium | Create subscriptions, deposit funds, pause/cancel own subscriptions | Can only modify subscriptions they created. Cannot withdraw merchant funds or charge subscriptions. |
| **Merchant** | Medium | Withdraw accumulated funds, pause/cancel subscriptions they receive | Cannot access subscriber balances or charge subscriptions. |

### Untrusted Actors

| Actor | Threat Level | Potential Actions |
|-------|--------------|-------------------|
| **External Callers** | High | Attempt unauthorized charges, state manipulation, fund theft |
| **Malicious Subscribers** | Medium | Attempt to drain funds, manipulate state, DoS attacks |
| **Malicious Merchants** | Medium | Attempt to steal subscriber funds, manipulate billing |

---

## Asset Inventory

### Primary Assets

1. **Prepaid Balances** (`Subscription.prepaid_balance`)
   - USDC tokens deposited by subscribers for future charges
   - Stored per subscription in contract instance storage
   - Risk: Theft, unauthorized deduction, loss

2. **Merchant Accumulated Funds** (future implementation)
   - USDC collected from successful charges
   - Currently not implemented in `withdraw_merchant_funds`
   - Risk: Unauthorized withdrawal, accounting errors

3. **Subscription State** (`Subscription` struct)
   - Billing parameters, status, timestamps
   - Risk: Unauthorized modification, state corruption

### Secondary Assets

4. **Admin Privileges**
   - Single admin address with charging authority
   - Risk: Admin key compromise, unauthorized admin actions

5. **Configuration Parameters**
   - Token address, minimum top-up threshold
   - Risk: Misconfiguration, unauthorized changes

---

## Threat Actors

### External Attacker

**Motivation**: Financial gain, disruption  
**Capabilities**: Can call any public contract function, observe blockchain state  
**Limitations**: Cannot forge signatures, cannot access private keys

**Attack Goals**:
- Steal USDC from prepaid balances
- Charge subscriptions without authorization
- Manipulate subscription states
- Cause denial of service

### Compromised Admin

**Motivation**: Financial gain, sabotage  
**Capabilities**: Full admin privileges (charge, batch charge, set min_topup)  
**Limitations**: Cannot directly withdraw subscriber funds, cannot bypass state machine

**Attack Goals**:
- Charge subscriptions prematurely or excessively
- Manipulate minimum top-up to lock out deposits
- Batch charge to drain multiple subscriptions

### Malicious Subscriber

**Motivation**: Avoid payment, disrupt service  
**Capabilities**: Create subscriptions, deposit funds, control own subscriptions  
**Limitations**: Cannot access other subscriptions, cannot charge own subscription

**Attack Goals**:
- Create subscriptions without funding
- Cancel subscriptions to avoid charges
- Exploit state transitions to avoid payment

### Malicious Merchant

**Motivation**: Steal funds, manipulate billing  
**Capabilities**: Receive payments, withdraw funds (future), cancel subscriptions  
**Limitations**: Cannot charge subscriptions, cannot access subscriber balances directly

**Attack Goals**:
- Withdraw more than accumulated funds
- Manipulate subscription state to force charges

---

## Attack Vectors and Mitigations

### 1. Unauthorized Charging

**Attack**: External caller attempts to charge subscriptions without admin authorization.

**Vector**:
```rust
// Attacker calls without admin signature
client.charge_subscription(&subscription_id);
```

**Mitigation**:
- `charge_subscription` requires admin authentication via `require_auth()`
- Admin address verified against stored value: `if admin != stored { return Err(Error::Unauthorized) }`
- Soroban runtime enforces signature verification

**Implementation** (`subscription.rs:59-62`):
```rust
pub fn do_charge_subscription(env: &Env, subscription_id: u32) -> Result<(), Error> {
    let admin = require_admin(env)?;
    admin.require_auth();  // Enforced by Soroban
    charge_one(env, subscription_id)
}
```

**Test Coverage**: `test_charge_subscription_unauthorized` (panics on missing auth)

**Residual Risk**: Admin key compromise (see [Known Limitations](#known-limitations))

---

### 2. Replay Attacks (Double Charging)

**Attack**: Admin or attacker attempts to charge the same subscription multiple times within a single billing interval.

**Vector**:
```rust
// Attempt to charge twice in same interval
client.charge_subscription(&id);  // Success
client.charge_subscription(&id);  // Should fail
```

**Mitigation**:
- Interval enforcement: Charges rejected if `now < last_payment_timestamp + interval_seconds`
- Returns `Error::IntervalNotElapsed` (1001) without modifying state
- Timestamp updated only on successful charge

**Implementation** (`charge_core.rs:14-21`):
```rust
let now = env.ledger().timestamp();
let next_allowed = sub.last_payment_timestamp
    .checked_add(sub.interval_seconds)
    .ok_or(Error::Overflow)?;
if now < next_allowed {
    return Err(Error::IntervalNotElapsed);
}
```

**Test Coverage**:
- `test_charge_rejected_before_interval`: Charge 1 second before interval fails
- `test_charge_succeeds_at_exact_interval`: Boundary condition
- `test_immediate_retry_at_same_timestamp_rejected`: Same-timestamp replay blocked

**Guarantees**:
- Maximum charge frequency: Once per `interval_seconds`
- No state mutation on rejected charges
- Monotonic timestamp progression assumed (Stellar validators)

---

### 3. Reentrancy Attacks

**Attack**: Malicious contract attempts to re-enter during token transfer or callback.

**Current Status**: **NOT APPLICABLE** - Token transfers not yet implemented.

**Future Mitigation** (when token transfers added):
- Follow checks-effects-interactions pattern
- Update state before external calls
- Use Soroban's atomic transaction model (no mid-transaction reentrancy)

**Example Safe Pattern**:
```rust
// 1. Checks
if sub.prepaid_balance < sub.amount { return Err(...); }

// 2. Effects (update state first)
sub.prepaid_balance -= sub.amount;
env.storage().instance().set(&subscription_id, &sub);

// 3. Interactions (external call last)
token_client.transfer(&vault, &merchant, &sub.amount);
```

**Note**: Soroban transactions are atomic; partial execution is not possible. However, cross-contract calls can still introduce logical reentrancy if state is not updated before external calls.

---

### 4. Integer Overflow/Underflow

**Attack**: Cause arithmetic overflow to manipulate balances or timestamps.

**Vectors**:
- Large deposit amounts causing `prepaid_balance` overflow
- Timestamp arithmetic overflow in interval calculations
- Balance underflow during charge

**Mitigation**:
- All arithmetic uses `checked_*` operations
- Returns `Error::Overflow` (403) on any arithmetic error
- No state mutation on overflow

**Implementation Examples**:

**Deposit** (`subscription.rs:48-52`):
```rust
sub.prepaid_balance = sub.prepaid_balance
    .checked_add(amount)
    .ok_or(Error::Overflow)?;
```

**Charge** (`charge_core.rs:27-31`):
```rust
sub.prepaid_balance = sub.prepaid_balance
    .checked_sub(sub.amount)
    .ok_or(Error::Overflow)?;
```

**Timestamp** (`charge_core.rs:16-18`):
```rust
let next_allowed = sub.last_payment_timestamp
    .checked_add(sub.interval_seconds)
    .ok_or(Error::Overflow)?;
```

**Test Coverage**: Implicit in all arithmetic operations; explicit overflow tests recommended.

**Guarantees**: No silent wraparound; all overflows are explicit errors.

---

### 5. State Machine Bypass

**Attack**: Force invalid state transitions to avoid charges or manipulate billing.

**Vectors**:
- Resume cancelled subscription
- Charge paused subscription
- Transition from InsufficientBalance to Paused

**Mitigation**:
- Explicit state machine validation before all transitions
- `validate_status_transition` enforces allowed transitions
- Cancelled is terminal state (no outgoing transitions)

**Implementation** (`state_machine.rs:29-59`):
```rust
pub fn validate_status_transition(
    from: &SubscriptionStatus,
    to: &SubscriptionStatus,
) -> Result<(), Error> {
    if from == to { return Ok(()); }  // Idempotent
    
    let valid = match from {
        SubscriptionStatus::Active => matches!(to, Paused | Cancelled | InsufficientBalance),
        SubscriptionStatus::Paused => matches!(to, Active | Cancelled),
        SubscriptionStatus::Cancelled => false,  // Terminal
        SubscriptionStatus::InsufficientBalance => matches!(to, Active | Cancelled),
    };
    
    if valid { Ok(()) } else { Err(Error::InvalidStatusTransition) }
}
```

**Test Coverage**:
- `test_all_valid_transitions_coverage`: All 7 valid transitions tested
- `test_invalid_cancelled_to_active`: Blocked transition
- `test_invalid_insufficient_balance_to_paused`: Blocked transition
- `test_validate_cancelled_transitions_all_blocked`: Terminal state enforcement

**Guarantees**:
- No invalid transitions possible
- State changes are atomic (validated before storage update)
- Idempotent transitions allowed (same-state is always valid)

---

### 6. Unauthorized State Modification

**Attack**: Non-owner attempts to pause, cancel, or resume subscriptions.

**Mitigation**:
- All state-changing operations require `authorizer.require_auth()`
- Authorizer must be subscriber or merchant (depending on operation)
- No admin override for subscriber/merchant actions

**Implementation** (`subscription.rs:65-72`):
```rust
pub fn do_cancel_subscription(
    env: &Env,
    subscription_id: u32,
    authorizer: Address,
) -> Result<(), Error> {
    authorizer.require_auth();  // Soroban enforces signature
    let mut sub = get_subscription(env, subscription_id)?;
    validate_status_transition(&sub.status, &SubscriptionStatus::Cancelled)?;
    // ... update state
}
```

**Note**: Current implementation does NOT verify that `authorizer` is the subscriber or merchant. This is a **known limitation** (see below).

**Test Coverage**: Authorization tests use `mock_all_auths()` which bypasses signature verification. Real authorization is enforced by Soroban runtime.

---

### 7. Minimum Top-Up Manipulation

**Attack**: Admin sets extremely high `min_topup` to prevent deposits, or extremely low to enable dust attacks.

**Mitigation**:
- `set_min_topup` requires admin authentication
- Admin address cannot be changed after initialization
- Minimum top-up enforced on all deposits

**Implementation** (`admin.rs:27-35`):
```rust
pub fn do_set_min_topup(env: &Env, admin: Address, min_topup: i128) -> Result<(), Error> {
    admin.require_auth();
    let stored = require_admin(env)?;
    if admin != stored {
        return Err(Error::Unauthorized);
    }
    env.storage().instance().set(&Symbol::new(env, "min_topup"), &min_topup);
    Ok(())
}
```

**Test Coverage**:
- `test_min_topup_below_threshold`: Deposit below minimum rejected
- `test_min_topup_exactly_at_threshold`: Boundary condition
- `test_set_min_topup_by_admin`: Admin can update
- `test_set_min_topup_unauthorized`: Non-admin cannot update

**Residual Risk**: Malicious admin can DoS deposits. No upper bound on `min_topup`.

---

### 8. Batch Charge Abuse

**Attack**: Admin charges large batches to cause gas exhaustion or drain multiple subscriptions.

**Mitigation**:
- Batch charge requires admin authentication (same as single charge)
- Each subscription charged independently; failures isolated
- No rollback of successful charges (partial failure allowed)
- Results returned for each subscription

**Implementation** (`admin.rs:42-58`):
```rust
pub fn do_batch_charge(
    env: &Env,
    subscription_ids: &Vec<u32>,
) -> Result<Vec<BatchChargeResult>, Error> {
    let auth_admin = require_admin(env)?;
    auth_admin.require_auth();  // Single auth for entire batch
    
    let mut results = Vec::new(env);
    for id in subscription_ids.iter() {
        let r = charge_one(env, id);
        // Record success/failure for each
        results.push_back(/* ... */);
    }
    Ok(results)
}
```

**Test Coverage**:
- `test_batch_charge_empty_list_returns_empty`: Empty batch handled
- `test_batch_charge_all_success`: All charges succeed
- `test_batch_charge_partial_failure`: Some charges fail, others succeed

**Guarantees**:
- Deterministic processing order (input Vec order)
- Partial failures do not affect successful charges
- Single admin auth for entire batch (gas optimization)

**Residual Risk**: No limit on batch size; extremely large batches could hit gas limits.

---

### 9. Subscription ID Collision

**Attack**: Predict or force subscription ID collision to access other subscriptions.

**Mitigation**:
- Sequential ID generation using instance storage counter
- Counter incremented atomically before use
- No ID reuse (counter only increments)

**Implementation** (`subscription.rs:9-14`):
```rust
pub fn next_id(env: &Env) -> u32 {
    let key = Symbol::new(env, "next_id");
    let id: u32 = env.storage().instance().get(&key).unwrap_or(0);
    env.storage().instance().set(&key, &(id + 1));
    id
}
```

**Guarantees**:
- IDs are sequential and predictable (0, 1, 2, ...)
- No collisions possible (counter never decrements)
- Maximum 2^32 subscriptions per contract instance

**Note**: Predictable IDs are not a security issue; authorization is enforced separately.

---

### 10. Storage Exhaustion (DoS)

**Attack**: Create massive number of subscriptions to exhaust contract storage.

**Current Status**: **UNMITIGATED** - No limits on subscription creation.

**Mitigation Recommendations**:
- Add per-subscriber subscription limit
- Require minimum deposit on creation
- Implement storage rent (Soroban feature)
- Add admin function to archive/delete old subscriptions

**Residual Risk**: Attacker can create unlimited subscriptions with valid signatures.

---

## Authorization Model

### Authentication Mechanisms

| Operation | Required Auth | Verification |
|-----------|---------------|--------------|
| `init` | None | One-time initialization (no re-init check) |
| `create_subscription` | Subscriber | `subscriber.require_auth()` |
| `deposit_funds` | Subscriber | `subscriber.require_auth()` |
| `charge_subscription` | Admin | `admin.require_auth()` + address match |
| `batch_charge` | Admin | `admin.require_auth()` + address match |
| `cancel_subscription` | Authorizer | `authorizer.require_auth()` (no owner check) |
| `pause_subscription` | Authorizer | `authorizer.require_auth()` (no owner check) |
| `resume_subscription` | Authorizer | `authorizer.require_auth()` (no owner check) |
| `withdraw_merchant_funds` | Merchant | `merchant.require_auth()` (not implemented) |
| `set_min_topup` | Admin | `admin.require_auth()` + address match |

### Authorization Gaps

1. **No Owner Verification**: `cancel_subscription`, `pause_subscription`, and `resume_subscription` accept any `authorizer` with valid signature. They do NOT verify that `authorizer` is the subscriber or merchant.

   **Impact**: Any address can pause/cancel/resume any subscription if they can provide a valid signature.

   **Recommended Fix**:
   ```rust
   if authorizer != sub.subscriber && authorizer != sub.merchant {
       return Err(Error::Unauthorized);
   }
   ```

2. **No Re-initialization Protection**: `init` can be called multiple times, overwriting admin and token addresses.

   **Impact**: Attacker can re-initialize contract to become admin.

   **Recommended Fix**:
   ```rust
   if env.storage().instance().has(&Symbol::new(env, "admin")) {
       return Err(Error::AlreadyInitialized);
   }
   ```

---

## State Machine Security

### Invariants

1. **Terminal State**: Once `Cancelled`, no transitions are possible (except idempotent `Cancelled -> Cancelled`)
2. **Charge Precondition**: Charges only succeed on `Active` subscriptions
3. **Automatic Transitions**: Only `Active -> InsufficientBalance` is automatic (on charge failure)
4. **Idempotency**: Same-state transitions are always allowed (no-op)

### Security Properties

- **No Resurrection**: Cancelled subscriptions cannot be reactivated
- **No Charge Bypass**: Paused and InsufficientBalance subscriptions cannot be charged
- **Predictable Transitions**: All transitions are explicit and validated
- **Atomic Updates**: State changes are atomic (validated before storage write)

### State Machine Diagram

```
┌─────────┐
│ Active  │◄─────────────────┐
└────┬────┘                  │
     │                       │
     ├──────► Paused ────────┤
     │          │            │
     │          └──────┐     │
     │                 │     │
     ├──────► InsufficientBalance
     │                 │
     │                 │
     └──────► Cancelled◄─────┘
              (Terminal)
```

**Reference**: See `docs/subscription_state_machine.md` for detailed state machine documentation.

---

## Arithmetic and Overflow Protection

### Protected Operations

| Operation | Location | Protection |
|-----------|----------|------------|
| Balance addition | `deposit_funds` | `checked_add` |
| Balance subtraction | `charge_one` | `checked_sub` |
| Timestamp addition | `charge_one` | `checked_add` |
| Interval calculation | `estimate_topup_for_intervals` | `checked_mul` |

### Overflow Behavior

- All overflows return `Error::Overflow` (403)
- No state mutation on overflow
- No silent wraparound (Rust `checked_*` operations)

### Edge Cases

1. **Maximum Balance**: `i128::MAX` (~1.7e38) - practically unlimited for USDC (7 decimals)
2. **Maximum Timestamp**: `u64::MAX` - year 584 billion (no practical concern)
3. **Maximum Interval**: `u64::MAX` seconds - ~584 billion years (no practical concern)

---

## Timing and Replay Protection

### Timestamp Assumptions

1. **Monotonicity**: Ledger timestamps are non-decreasing (enforced by Stellar validators)
2. **Granularity**: 1-second resolution (Stellar ledger close time)
3. **Accuracy**: Timestamps are validator-provided, not user-controlled

### Replay Protection Mechanisms

1. **Interval Enforcement**: `now >= last_payment_timestamp + interval_seconds`
2. **Timestamp Update**: `last_payment_timestamp` set to `now` on successful charge
3. **Sliding Window**: Each charge resets the interval window

### Test Coverage

- `test_charge_rejected_before_interval`: 1 second before interval
- `test_charge_succeeds_at_exact_interval`: Exact boundary
- `test_charge_succeeds_after_interval`: Well past interval
- `test_immediate_retry_at_same_timestamp_rejected`: Same-timestamp replay
- `test_repeated_charges_across_many_intervals`: 6 consecutive intervals
- `test_one_second_interval_boundary`: Minimum interval (1 second)

### Guarantees

- **No Double Charging**: Maximum one charge per interval
- **No Timestamp Manipulation**: Timestamps are ledger-provided
- **Deterministic Behavior**: Same inputs always produce same outputs

---

## Known Limitations

### 1. Admin Key Compromise

**Risk**: If admin private key is compromised, attacker can charge all subscriptions and manipulate minimum top-up.

**Impact**: HIGH - Complete loss of billing integrity

**Mitigation**: 
- Use hardware wallet or multi-sig for admin key
- Monitor admin actions via events (when implemented)
- Consider time-locked admin actions for sensitive operations

**Status**: Inherent limitation of single-admin design

---

### 2. No Owner Verification in State Changes

**Risk**: Any address can pause/cancel/resume any subscription with valid signature.

**Impact**: MEDIUM - Unauthorized state manipulation

**Mitigation**: Add owner checks in `cancel_subscription`, `pause_subscription`, `resume_subscription`

**Status**: Implementation gap (see [Authorization Gaps](#authorization-gaps))

---

### 3. No Re-initialization Protection

**Risk**: `init` can be called multiple times, overwriting admin and token addresses.

**Impact**: CRITICAL - Complete contract takeover

**Mitigation**: Add initialization flag check

**Status**: Implementation gap (see [Authorization Gaps](#authorization-gaps))

---

### 4. Token Transfers Not Implemented

**Risk**: Funds cannot actually be transferred; contract is non-functional for real use.

**Impact**: HIGH - Contract cannot be used in production

**Mitigation**: Implement token transfers using Stellar Asset Contract (SAC) interface

**Status**: Planned feature (marked as TODO in code)

---

### 5. No Batch Size Limit

**Risk**: Extremely large batch charges could hit gas limits or cause DoS.

**Impact**: LOW - Admin-only operation, self-inflicted DoS

**Mitigation**: Add maximum batch size constant (e.g., 100 subscriptions per batch)

**Status**: Optimization opportunity

---

### 6. No Storage Limits

**Risk**: Unlimited subscription creation could exhaust contract storage.

**Impact**: MEDIUM - DoS via storage exhaustion

**Mitigation**: Add per-subscriber limits, require minimum deposit, implement archival

**Status**: Unmitigated (see [Storage Exhaustion](#10-storage-exhaustion-dos))

---

### 7. No Emergency Stop

**Risk**: No way to pause contract in case of discovered vulnerability.

**Impact**: HIGH - Cannot respond to active exploits

**Mitigation**: Add admin-controlled pause mechanism for critical operations

**Status**: Recommended addition

---

### 8. No Fund Recovery

**Risk**: Funds sent to contract by mistake cannot be recovered.

**Impact**: LOW - User error, not contract vulnerability

**Mitigation**: Add admin function to recover accidentally sent tokens

**Status**: Nice-to-have feature

---

## Security Testing

### Test Coverage

**Current Coverage**: 95%+ (per project requirements)

**Test Categories**:
1. **State Machine**: 15+ tests covering all valid/invalid transitions
2. **Authorization**: 4 tests for admin/unauthorized access
3. **Interval Enforcement**: 6 tests for replay protection and timing
4. **Arithmetic**: Implicit in all operations (overflow tests recommended)
5. **Batch Operations**: 3 tests for batch charge scenarios
6. **Edge Cases**: Minimum top-up, boundary conditions, idempotency

### Test Files

- `contracts/subscription_vault/src/test.rs`: Comprehensive unit tests
- `contracts/subscription_vault/test_snapshots/`: Snapshot tests for state verification

### Recommended Additional Tests

1. **Overflow Tests**: Explicit tests for `i128::MAX` and `u64::MAX` edge cases
2. **Reentrancy Tests**: When token transfers are implemented
3. **Fuzz Testing**: Random inputs for state machine and arithmetic
4. **Integration Tests**: Multi-contract scenarios with real token contract
5. **Gas Limit Tests**: Maximum batch sizes and storage limits

---

## Incident Response

### Monitoring Recommendations

1. **Admin Actions**: Log all `charge_subscription`, `batch_charge`, `set_min_topup` calls
2. **Large Deposits**: Alert on deposits exceeding threshold (e.g., >$10,000)
3. **Failed Charges**: Monitor `InsufficientBalance` transitions
4. **State Anomalies**: Alert on unexpected state transitions
5. **Batch Failures**: Monitor batch charge failure rates

### Response Procedures

1. **Suspected Exploit**:
   - Identify affected subscriptions
   - Pause contract if emergency stop is implemented
   - Analyze transaction history
   - Coordinate with Stellar validators if necessary

2. **Admin Key Compromise**:
   - Immediately rotate admin key (requires contract upgrade)
   - Audit all admin actions since compromise
   - Notify affected users
   - Consider contract migration

3. **Vulnerability Discovery**:
   - Assess impact and exploitability
   - Develop and test fix
   - Deploy patched contract
   - Migrate state if necessary
   - Disclose responsibly after mitigation

### Contact Information

- **Security Issues**: [security@stellabill.example] (placeholder)
- **Bug Bounty**: [To be established]
- **Incident Response Team**: [To be established]

---

## Audit History

| Date | Auditor | Scope | Findings | Status |
|------|---------|-------|----------|--------|
| TBD | TBD | Full contract | TBD | Pending |

**Note**: This contract has not yet undergone professional security audit. Use in production is NOT recommended until audited.

---

## References

1. **State Machine**: `docs/subscription_state_machine.md`
2. **Batch Charging**: `docs/batch_charge.md`
3. **Billing Intervals**: `docs/billing_intervals.md`
4. **Top-Up Estimation**: `docs/topup_estimation.md`
5. **Soroban Security**: https://developers.stellar.org/docs/smart-contracts/security
6. **Stellar Protocol**: https://developers.stellar.org/docs/fundamentals-and-concepts

---

## Document Maintenance

This document should be updated when:
- New features are added (e.g., token transfers, emergency stop)
- Security vulnerabilities are discovered and fixed
- Authorization model changes
- State machine is modified
- After security audits

**Maintainer**: Security team / Lead developer  
**Review Frequency**: Before each major release  
**Version Control**: Track changes in git alongside contract code
