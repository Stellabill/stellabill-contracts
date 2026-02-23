# Batch charge

Admin-only entrypoint to charge multiple subscriptions in a single transaction.

## Function

`batch_charge(env, subscription_ids) -> Result<Vec<BatchChargeResult>, Error>`

- **subscription_ids**: List of subscription IDs to charge (order preserved in results).
- **Returns**: One `BatchChargeResult` per ID: `{ success: bool, error_code: u32 }`. Same admin auth as single `charge_subscription`.

## Semantics

- **Empty list:** returns empty Vec.
- **Partial failures:** Each subscription is charged independently. A failure (e.g. IntervalNotElapsed, NotActive, InsufficientBalance) is recorded in that slot; other subscriptions are still charged. No rollback of successful charges.
- **Duplicate IDs:** Each ID is processed once; duplicates can succeed or fail independently.
- **Auth:** Single admin auth for the whole batch; internal charges do not consume auth again.

## Error handling

- Per-item errors are returned in the corresponding `BatchChargeResult` (`success: false`, `error_code` set from `Error::to_code()`).
- If the caller is not the stored admin, the entire call fails with `Error::Unauthorized` (no results Vec).

## Trade-offs

- **Gas:** One transaction for N charges instead of N transactions; auth and contract call overhead paid once.
- **Determinism:** Order of processing is the order of the input Vec; results are deterministic.
- **Events:** Emit per-subscription events in the same order for indexing (if/when events are added).
