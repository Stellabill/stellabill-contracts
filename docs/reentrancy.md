code docs\reentrancy_hardening.md# Reentrancy Protection

## Overview

Reentrancy is a critical security vulnerability where a contract becomes vulnerable to unexpected behavior when an external contract (particularly the token contract) calls back into the original contract during the execution of a function.

This document describes the reentrancy protection measures implemented in the Subscription Vault contract, design decisions, and residual risks.

## Threat Model

### Attack Scenario

1. **Subscriber** calls `deposit_funds()` with an amount to prepay for their subscription
2. Inside `deposit_funds()`, the contract calls `token.transfer()` to receive tokens from the subscriber
3. **Malicious USDC Contract**: During the transfer, the USDC token contract calls a callback hook (e.g., `onTransferReceived`) back into our contract
4. **Reentrancy**: The attacker exploits this callback to:
   - Withdraw more funds than deposited
   - Charge a subscription multiple times
   - Drain merchant balances
   - Create inconsistent contract state

## Defense Strategy: Checks-Effects-Interactions (CEI)

The primary defense against reentrancy is the **Checks-Effects-Interactions** (CEI) pattern, also known as the "Guards and Effects" pattern.

### Pattern Definition

```text
1. CHECKS:      Verify all preconditions and inputs
2. EFFECTS:     Update internal contract state
3. INTERACTIONS: Call external contracts (token transfers)
```

### Why CEI Protects Against Reentrancy

When internal state is updated **before** external calls:
- If a callback occurs, the contract state is already consistent
- The callback sees the updated balances and cannot exploit inconsistency
- Even if the callback attempts to withdraw twice, the balance reflects only one deposit

## Applied Protections

### 1. Deposit Funds (`do_deposit_funds`)

**Location**: `contracts/subscription_vault/src/subscription.rs`

**Vulnerability Fixed**:
- **Before**: Updated in-memory balance → called token transfer → stored state
- **After**: Validated inputs → updated balance in memory AND persisted to storage → token transfer

**Implementation**:
```rust
// CHECKS: Validate all preconditions
let min_topup = crate::admin::get_min_topup(env)?;
if amount < min_topup { return Err(...); }
validate_non_negative(amount)?;

// EFFECTS: Update internal state BEFORE external calls
sub.prepaid_balance = safe_add_balance(sub.prepaid_balance, amount)?;
env.storage().instance().set(&subscription_id, &sub);  // ← STATE PERSISTED

// INTERACTIONS: Only AFTER state is consistent
let token_client = soroban_sdk::token::Client::new(env, &token_addr);
token_client.transfer(&subscriber, &env.current_contract_address(), &amount);
```

**Guarantee**: If the token contract calls back (e.g., through a callback), it will see the updated `prepaid_balance` in storage. The balance cannot be double-deposited because the storage reflects the increased amount.

### 2. Withdraw Merchant Funds (`withdraw_merchant_funds`)

**Location**: `contracts/subscription_vault/src/merchant.rs`

**Vulnerability Fixed**:
- **Before**: Checked balance → called token transfer → updated balance
- **After**: Checked balance → updated balance in storage → token transfer

**Implementation**:
```rust
// CHECKS: Validate all preconditions
let current = get_merchant_balance(env, &merchant);
if amount > current { return Err(Error::InsufficientBalance); }

let new_balance = current.checked_sub(amount)?;

// EFFECTS: Update internal state BEFORE external calls
set_merchant_balance(env, &merchant, &new_balance);  // ← STATE PERSISTED
env.events().publish(...);

// INTERACTIONS: Only AFTER state is consistent
let token_client = token::Client::new(env, &token_addr);
token_client.transfer(&env.current_contract_address(), &merchant, &amount);
```

**Guarantee**: The merchant's balance is already reduced in storage. Even if the token contract calls back during the transfer, a second withdrawal would fail because the balance is insufficient.

### 3. Withdraw Subscriber Funds (`do_withdraw_subscriber_funds`)

**Location**: `contracts/subscription_vault/src/subscription.rs`

**Status**: ✓ Already implemented correctly

**Implementation**:
```rust
let amount_to_refund = sub.prepaid_balance;
if amount_to_refund > 0 {
    sub.prepaid_balance = 0;
    env.storage().instance().set(&subscription_id, &sub);  // ← STATE PERSISTED FIRST
    
    // Token transfer happens AFTER balance is zeroed
    let token_client = soroban_sdk::token::Client::new(env, &token_addr);
    token_client.transfer(...);
}
```

## Secondary Protection: Reentrancy Guards

While CEI is the primary defense, the contract includes an optional **reentrancy guard** module for additional protection on critical paths.

### Reentrancy Guard Pattern

**Location**: `contracts/subscription_vault/src/reentrancy.rs`

The guard uses a locking mechanism to detect if a function is already executing:

```rust
pub fn some_protected_function(env: &Env) -> Result<(), Error> {
    let _guard = crate::reentrancy::ReentrancyGuard::lock(env, "some_protected_function")?;
    
    // Critical operations here
    // Even if callback occurs, lock prevents reentry
    
    // Guard automatically unlocks when dropped
}
```

**Implementation Details**:
- Each critical section has a unique lock key stored in contract storage
- On entry, the function checks if lock exists (Err(Reentrancy) if it does)
- Lock is acquired by setting a flag in storage
- When the function exits, the guard's Drop implementation removes the lock

**Current Usage**: Guards are available for use but not currently applied to the critical paths because CEI pattern provides sufficient protection.

## External Calls Summary

The contract makes external calls **only** to the USDC token contract:

| Function | External Call | CEI Applied |
|----------|---------------|------------|
| `do_deposit_funds` | `token.transfer(subscriber → contract)` | ✓ Yes |
| `do_withdraw_subscriber_funds` | `token.transfer(contract → subscriber)` | ✓ Yes |
| `withdraw_merchant_funds` | `token.transfer(contract → merchant)` | ✓ Yes |
| `charge_subscription` | `credit_merchant_balance` (internal) | ✓ Yes |
| `charge_usage` | Internal balance update only | ✓ Yes |

## Residual Risks and Assumptions

### 1. Token Contract Callback Hooks

**Assumption**: The USDC token contract may call back into our contract during `transfer()`.

**Mitigation**: CEI pattern ensures internal state is consistent before callback occurs.

**Residual Risk**: 
- **Low**: If USDC token implements ERC777-style receive hooks, our state is already updated, preventing exploitation
- The token contract could theoretically implement arbitrary logic, but as long as our state is committed first, the damage is limited

### 2. Single-Function Reentrancy (Recursive Calls)

**Risk**: A callback could call `deposit_funds` again recursively.

**Mitigation**: 
- CEI pattern limits damage from recursive calls
- Reentrancy guard module can be enabled if needed (currently unused)

**Residual Risk**:
- **Very Low**: In Soroban, cross-contract calls are expensive and require explicit authorization, making recursive calls unlikely in practice

### 3. Cross-Function Reentrancy

**Risk**: A callback could call a different function (e.g., `deposit_funds` calls token, callback calls `charge_subscription`).

**Mitigation**: 
- Each function independently follows CEI
- Charge operations require authorization and time checks
- Merchant withdrawal requires merchant authorization

**Residual Risk**:
- **Very Low**: Different functions have different authorization requirements

### 4. Soroban-Specific Considerations

Unlike Ethereum, Soroban has some natural reentrancy protections:

- **No Implicit Callbacks**: The Soroban SDK does not implement automatic callback hooks (like ERC777)
- **Explicit Invocation**: Cross-contract calls must be explicit `invoke()` calls
- **Atomic Transactions**: Each contract invocation is atomic at the ledger level
- **Synchronous Execution**: Call stacks are synchronous, making deep reentrancy chains less likely

**Assessment**: The natural properties of Soroban significantly reduce reentrancy risk compared to Ethereum.

## Testing

Reentrancy protection is verified by:

1. **CEI Pattern Enforcement**: Code review ensures state updates precede external calls
2. **State Consistency Tests**: Tests verify that balances are correctly updated even after multiple operations
3. **Integration Tests**: Tests verify end-to-end scenarios with deposits, charges, and withdrawals

**Test Coverage**: See `contracts/subscription_vault/src/test.rs`:
- `test_deposit_funds_state_committed_before_transfer`
- `test_withdraw_merchant_funds_state_committed_before_transfer`
- `test_withdraw_subscriber_funds_state_committed_before_transfer`
- `test_multiple_deposits_maintain_consistent_state`
- `test_charge_and_withdrawal_atomic_sequence`

## Future Hardening

Potential enhancements (not currently implemented):

1. **Enable Reentrancy Guards** on all critical paths for defense-in-depth
2. **Pull-Over-Push Pattern**: Migrate to pull-based withdrawals only (no automatic transfers)
3. **Separate Withdrawal Contract**: Use a separate contract for USDC redemptions
4. **Multi-Signature Locks**: Require multiple authorizations for large withdrawals

## Recommendations for Auditors

When reviewing this contract:

1. **Verify CEI Pattern**:
   - In `do_deposit_funds`: Check that state is persisted before `token_client.transfer()`
   - In `withdraw_merchant_funds`: Check that balance is updated before `token_client.transfer()`
   - In `do_withdraw_subscriber_funds`: Check that balance is zeroed before `token_client.transfer()`

2. **Review Token Assumptions**: Verify assumptions about USDC token behavior:
   - Whether it implements callbacks
   - Whether it allows re-entry during transfers
   - Whether it respects spending limits properly

3. **Test Reentrancy Scenarios**: While not possible to fully simulate in Soroban, test:
   - Multiple concurrent deposits
   - Deposits and withdrawals in rapid succession
   - Balance consistency across operations

4. **Monitor Token Contract Updates**: If USDC token contract changes behavior, reevaluate protection.

## References

- **CEI Pattern**: [OpenZeppelin Study on Reentrancy](https://docs.openzeppelin.com/contracts/4.x/securing-contracts#reentrancy)
- **ERC-777 Callback Attacks**: [Example reentrancy vulnerability](https://github.com/OpenZeppelin/openzeppelin-contracts/security/advisories/GHSA-4h98-2769-gh97)
- **Soroban Documentation**: [Soroban Smart Contracts Guide](https://developers.stellar.org/docs/build/smart-contracts)
- **Secure Patterns**: [Trail of Bits Reentrancy Guide](https://blog.trailofbits.com/2017/06/14/protecting-against-reentrancy-attacks/)

## Summary

The Subscription Vault contract implements **Checks-Effects-Interactions (CEI) pattern** for all external calls, ensuring that internal state is consistent before any callback can occur. Combined with Soroban's natural protections against reentrancy, the contract is hardened against callback-based attacks.

**Risk Level: VERY LOW**

The contract is designed to be safe even if the USDC token contract or other external contracts implement aggressive callback mechanisms. The primary defense (CEI pattern) and Soroban's synchronous execution model together provide strong reentrancy protection.
