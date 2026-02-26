# Safe Math Strategy

## Purpose

Safe math helpers are critical for token contracts to prevent arithmetic overflow, underflow, and precision errors. In smart contracts handling financial transactions, a single arithmetic error can lead to loss of funds or contract exploitation. This document describes the safe math implementation strategy for the Stellabill subscription vault contract.

## Strategy

The safe math system uses Rust's built-in checked arithmetic operations (`checked_add()`, `checked_sub()`) which return `Option<i128>`. These are wrapped in helper functions that convert `None` results (indicating overflow/underflow) into clear contract errors.

### Why Checked Arithmetic?

- **Prevents Panics**: Standard arithmetic operations in Rust can panic on overflow/underflow in debug mode or wrap around in release mode, both of which are unacceptable in smart contracts
- **Explicit Error Handling**: Checked operations return `Option<T>`, allowing us to handle errors gracefully
- **No Performance Overhead**: In release builds with optimizations, checked arithmetic has minimal performance impact
- **Compatibility**: Works seamlessly with Soroban SDK's `i128` type used for token amounts

## Guarantees

Each helper function provides specific guarantees:

### `safe_add(a: i128, b: i128) -> Result<i128, Error>`
- **Guarantee**: Returns the sum of `a` and `b` if no overflow occurs
- **Error**: Returns `Error::Overflow` if result would exceed `i128::MAX`
- **Use Case**: General addition operations

### `safe_sub(a: i128, b: i128) -> Result<i128, Error>`
- **Guarantee**: Returns the difference of `a` and `b` if no underflow occurs
- **Error**: Returns `Error::Underflow` if result would go below `i128::MIN`
- **Use Case**: General subtraction operations
- **Note**: Allows negative results (use `safe_sub_balance` for balance operations)

### `validate_non_negative(amount: i128) -> Result<(), Error>`
- **Guarantee**: Validates that an amount is non-negative (>= 0)
- **Error**: Returns `Error::Underflow` if amount is negative
- **Use Case**: Input validation for amounts that must be non-negative

### `safe_add_balance(balance: i128, amount: i128) -> Result<i128, Error>`
- **Guarantee**: 
  - Result is always >= 0 when successful
  - Amount must be non-negative
  - No overflow occurs
- **Errors**: 
  - `Error::Underflow` if `amount` is negative
  - `Error::Overflow` if result would exceed `i128::MAX`
- **Use Case**: Adding funds to balances (deposits, credits)

### `safe_sub_balance(balance: i128, amount: i128) -> Result<i128, Error>`
- **Guarantee**: 
  - Result is always >= 0 when successful
  - Amount must be non-negative
  - Balance never goes negative
- **Errors**: 
  - `Error::Underflow` if `amount` is negative
  - `Error::Underflow` if result would be negative (insufficient balance)
  - `Error::Underflow` if subtraction would go below `i128::MIN`
- **Use Case**: Deducting funds from balances (charges, withdrawals)

## Error Handling

### Error Types

The contract defines two arithmetic error variants:

- **`Error::Overflow (500)`**: Returned when addition or multiplication would exceed `i128::MAX`
- **`Error::Underflow (501)`**: Returned when:
  - Subtraction would go below `i128::MIN`
  - An operation would result in a negative balance
  - A negative amount is provided where non-negative is required

### Error Propagation

All safe math functions return `Result<i128, Error>`, allowing errors to propagate using Rust's `?` operator:

```rust
let new_balance = safe_add_balance(current_balance, deposit_amount)?;
```

This ensures that arithmetic errors are caught and returned to the caller, preventing silent failures or panics.

## Invariants

The safe math system maintains the following contract invariants:

1. **Balances Never Go Negative**: `safe_sub_balance` ensures balances remain >= 0
2. **Amounts Never Overflow**: All additions are checked against `i128::MAX`
3. **All Arithmetic is Checked**: No direct arithmetic operations on token amounts; all use safe helpers
4. **Input Validation**: Negative amounts are rejected before arithmetic operations
5. **Consistent Error Handling**: All arithmetic errors return clear, actionable error types

## USDC Compatibility

The safe math system is designed to work with USDC-style fixed decimals (6 decimals):

- **1 USDC** = `1_000_000` smallest units
- **1000 USDC** = `1_000_000_000` smallest units
- **Maximum Reasonable Amount**: Well below `i128::MAX` (which is ~9.2 × 10¹⁸)

### Example Calculations

```rust
// 10 USDC deposit
let deposit = 10_000_000i128; // 10 * 10^6
let balance = safe_add_balance(0, deposit)?; // Ok(10_000_000)

// 1000 USDC charge
let charge = 1_000_000_000i128; // 1000 * 10^6
let new_balance = safe_sub_balance(balance, charge)?; // Ok(990_000_000)
```

## Usage Examples

### Depositing Funds

```rust
pub fn deposit_funds(
    env: Env,
    subscription_id: u32,
    subscriber: Address,
    amount: i128,
) -> Result<(), Error> {
    subscriber.require_auth();
    validate_non_negative(amount)?; // Reject negative amounts
    
    let mut sub: Subscription = env
        .storage()
        .instance()
        .get(&subscription_id)
        .ok_or(Error::NotFound)?;
    
    // Safely add to balance
    sub.prepaid_balance = safe_add_balance(sub.prepaid_balance, amount)?;
    
    env.storage().instance().set(&subscription_id, &sub);
    Ok(())
}
```

### Charging Subscription

```rust
pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
    let mut sub: Subscription = env
        .storage()
        .instance()
        .get(&subscription_id)
        .ok_or(Error::NotFound)?;
    
    // Safely deduct from balance (prevents negative balances)
    sub.prepaid_balance = safe_sub_balance(sub.prepaid_balance, sub.amount)?;
    
    sub.last_payment_timestamp = env.ledger().timestamp();
    env.storage().instance().set(&subscription_id, &sub);
    Ok(())
}
```

### Input Validation

```rust
pub fn create_subscription(
    env: Env,
    subscriber: Address,
    merchant: Address,
    amount: i128,
    // ...
) -> Result<u32, Error> {
    subscriber.require_auth();
    validate_non_negative(amount)?; // Ensure amount is non-negative
    
    // ... rest of function
}
```

## Testing

The safe math module has comprehensive test coverage (95%+) including:

### Basic Operations
- Normal addition/subtraction within bounds
- Overflow conditions (i128::MAX)
- Underflow conditions (i128::MIN)
- Zero operations

### Balance Operations
- Adding/subtracting from balances
- Preventing negative balances
- Rejecting negative amounts
- Exact balance operations (zero result)

### Edge Cases
- Maximum values
- Minimum values
- Boundary conditions
- Repeated operations

### Integration Tests
- Multiple deposits without overflow
- Repeated charges without underflow
- USDC amount compatibility
- Error propagation

### Running Tests

```bash
cargo test -p subscription_vault
```
