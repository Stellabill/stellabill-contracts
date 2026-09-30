# Delegated Payer Authorization Audit

## Issue: #629

## Audit Findings

### Status: **FEATURE NOT IMPLEMENTED**

After comprehensive audit of the `contracts/subscription_vault/src/subscription.rs` and related modules, **no delegated payer feature currently exists** in the codebase.

### Search Results

The following searches were performed:
- `grep` for "delegated" - No matches found in source code
- `grep` for "payer" - No matches found in source code  
- `grep` for "grant" - Only found in test files and unrelated contexts
- Manual review of `subscription.rs::do_deposit_funds` - No delegated payer logic

### Current Deposit Flow

The current `do_deposit_funds` function in `subscription.rs` (lines 596-710) implements a simple deposit flow:

```rust
pub fn do_deposit_funds(
    env: &Env,
    subscription_id: u32,
    subscriber: Address,
    amount: i128,
    idem_key: Option<soroban_sdk::BytesN<32>>,
) -> Result<(), Error> {
    subscriber.require_auth();  // Only subscriber can deposit
    // ... validation checks ...
    if subscriber != sub.subscriber {
        return Err(Error::Unauthorized);
    }
    // ... deposit logic ...
}
```

**Authorization**: Only the subscription's subscriber can deposit funds. There is no mechanism for a delegated payer to deposit on behalf of a subscriber.

## Security Guidelines for Future Implementation

If a delegated payer feature is implemented in the future, it MUST follow these security requirements:

### 1. Dual Authorization Requirement

Both parties MUST authorize:
- **Subscriber (grantor)**: Must authorize the delegation grant
- **Payer (delegate)**: Must authorize each deposit action

### 2. Grant Validation

The delegation grant MUST include:
- `max_amount`: Maximum total amount the payer can deposit
- `expiration`: Timestamp after which the grant is invalid
- `subscriber`: The subscriber authorizing the delegation
- `payer`: The authorized payer address

### 3. Deposit Path Authorization

When a delegated payer deposits funds, the contract MUST:

1. **Require payer authorization**: `payer.require_auth()`
2. **Validate grant exists**: Check that a valid grant exists for `(subscriber, payer)`
3. **Enforce max_amount**: Track cumulative deposits and ensure `total_deposited <= max_amount`
4. **Check expiration**: Ensure `current_timestamp < grant.expiration`
5. **Require subscriber authorization on grant creation**: `subscriber.require_auth()` when creating the grant

### 4. Prevention of Bypass Attacks

The implementation MUST prevent:
- **Self-redirection**: Payer cannot redirect withdrawals to themselves
- **Grant exhaustion**: Multiple deposits cannot exceed `max_amount`
- **Expired grant usage**: Deposits must fail after expiration
- **Unauthorized payer**: Only explicitly authorized payers can deposit

### 5. Required Tests

When implementing this feature, include these negative tests:
- Payer attempts to deposit without valid grant → FAIL
- Payer attempts to deposit exceeding `max_amount` → FAIL
- Payer attempts to deposit after expiration → FAIL
- Unauthorized address attempts to deposit → FAIL
- Payer attempts to withdraw funds (should be impossible) → FAIL
- Grant with `max_amount = i128::MAX` → Should work correctly
- Grant expired mid-transaction → Should fail

### 6. Trust Model Documentation

The trust model MUST be documented:
- **Subscriber trusts**: The payer to deposit up to `max_amount`
- **Contract trusts**: Both parties to authorize correctly
- **No trust assumption**: Payer cannot access subscriber's withdrawal rights

## Current Authorization Matrix

Based on existing code (see `test_require_auth.rs`):

| Entrypoint | Authorizer | Notes |
|------------|-----------|-------|
| `deposit_funds` | subscriber only | No delegated payer support |
| `create_subscription` | subscriber | N/A |
| `cancel_subscription` | subscriber OR merchant | N/A |
| `pause_subscription` | subscriber OR merchant | N/A |
| `withdraw_merchant_funds` | merchant | N/A |

## Recommendation

**DO NOT implement delegated payer without comprehensive security review.**

If this feature is needed, it should be designed as a separate, auditable module with:
- Clear separation of concerns
- Extensive negative test coverage
- Formal verification of authorization invariants
- External security audit before deployment

## Conclusion

The delegated payer feature referenced in issue #629 does not exist in the current codebase. This audit serves as:
1. Documentation of current state
2. Security guidelines for future implementation
3. Prevention of insecure implementation

No code changes are required at this time.
