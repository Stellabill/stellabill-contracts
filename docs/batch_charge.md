# batch_charge

Charges multiple subscriptions in one call. Admin-only.

## Signature
```rust
pub fn batch_charge(env: Env, ids: Vec<u64>) -> Result<Vec<ChargeResult>, Error>
```

## Behavior
- Admin is authorized once at the batch boundary
- Each subscription is processed via the shared `charge_one` helper
- Failed subscriptions are skipped, not aborted
- Returns one `ChargeResult` per input id

## ChargeResult
| Field | Type | Values |
|-------|------|--------|
| subscription_id | u64 | the input id |
| success | bool | true if charged |
| message | String | "charged", "not_found", "skipped" |

## Skip conditions
- Subscription not found
- Status is Paused, Cancelled, or InsufficientBalance
- Billing interval has not elapsed
- Insufficient prepaid balance (also marks status InsufficientBalance)
