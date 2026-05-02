# Batch charge

Admin-only entrypoint to charge multiple subscriptions in a single transaction.

## Function

`batch_charge(env, subscription_ids) -> Result<Vec<BatchChargeResult>, Error>`

- **subscription_ids**: List of subscription IDs to charge (order preserved in results).
- **Returns**: One `BatchChargeResult` per ID: `{ success: bool, error_code: u32 }`. Same admin auth as single `charge_subscription`.

## Limits

- **MAX_BATCH_SIZE**: 100 subscriptions per batch. This limit prevents excessive compute usage and ensures transactions stay within reasonable gas bounds. Callers should paginate larger lists.
- If the input list exceeds `MAX_BATCH_SIZE`, the entire call fails with `Error::InvalidInput` (no results Vec).

## Semantics

- **Empty list:** returns empty Vec.
- **Partial failures:** Each subscription is charged independently. A failure (e.g. IntervalNotElapsed, NotActive, InsufficientBalance) is recorded in that slot; other subscriptions are still charged. No rollback of successful charges.
- **Duplicate IDs:** Duplicate IDs are processed sequentially in-list. A later duplicate observes the state changes caused by the earlier occurrence, just like repeated `charge_subscription` calls.
- **Auth:** Single admin auth for the whole batch; internal charges do not consume auth again.

## Error handling

- Per-item errors are returned in the corresponding `BatchChargeResult` (`success: false`, `error_code` set from `Error::to_code()`).
- If the caller is not the stored admin, the entire call fails with `Error::Unauthorized` (no results Vec).
- If the input list exceeds `MAX_BATCH_SIZE`, the entire call fails with `Error::InvalidInput` (no results Vec).

## Trade-offs

- **Gas:** One transaction for N charges instead of N transactions; auth and contract call overhead paid once.
- **Determinism:** Order of processing is the order of the input Vec; results are deterministic.
- **Events:** Emit per-subscription events in the same order for indexing (if/when events are added).
- **Bounded compute:** MAX_BATCH_SIZE ensures predictable gas costs and prevents DoS via oversized batches.

## Consistency Guidance

- Batch callers should interpret `batch_charge` as "run `charge_subscription` for each ID in order and collect the outcomes."
- Failed items must be evaluated with the same expectations as a direct single call, including any documented status transitions.
- When reconciling results off-chain, compare by input index first, then by `success` and `error_code`.
- For lists larger than 100 items, paginate into multiple batch calls.
