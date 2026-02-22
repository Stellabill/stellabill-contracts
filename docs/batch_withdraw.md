# Batch Merchant Withdrawal

## Overview

`batch_withdraw_merchant_funds` allows a merchant to withdraw funds for
multiple amounts in a single transaction, reducing fees and round trips
compared to calling `withdraw_merchant_funds` repeatedly.

## Function Signature
```rust
pub fn batch_withdraw_merchant_funds(
    env: Env,
    merchant: Address,
    amounts: Vec<i128>,
) -> Result<Vec<BatchWithdrawResult>, Error>
```

## Guarantees

- Merchant authorizes **once** for the entire batch.
- Each withdrawal is processed independently — a failure in one entry
  does not stop the rest of the batch.
- Results are returned in the **same order** as the input amounts.
- Invalid amounts (zero or negative) are rejected with `InsufficientBalance`.
- State is never modified for a failed entry — no double debits.

## BatchWithdrawResult

| Field | Type | Description |
|---|---|---|
| `success` | `bool` | True if withdrawal succeeded |
| `error_code` | `u32` | Error code if failed, 0 if success |

## Error Codes

| Code | Meaning |
|---|---|
| 1003 | InsufficientBalance — zero, negative, or insufficient funds |
| 401 | Unauthorized — merchant auth failed |

## Example
```rust
let amounts = vec![&env, 1_000_000i128, 2_000_000i128, 500_000i128];
let results = client.batch_withdraw_merchant_funds(&merchant, &amounts);

for i in 0..results.len() {
    let r = results.get(i).unwrap();
    if r.success {
        // withdrawal processed
    } else {
        // check r.error_code
    }
}
```

## Caveats

- Token transfer logic is currently a stub pending full vault integration.
- Batch size is unbounded — callers should keep batches reasonable to
  stay within Soroban resource limits.
- All amounts must be positive; zero and negative values fail safely.
