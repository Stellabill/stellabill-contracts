# batch_charge

Charges multiple subscriptions in one call. **Admin-only.**

## Signature
```rust
pub fn batch_charge(env: Env, subscription_ids: Vec<u32>, nonce: u64) -> Result<Vec<BatchChargeResult>, Error>
```

## Authorization
- Stored admin is loaded from `DataKey::Admin` and must sign the transaction
- Non-admin callers are rejected with a host auth failure (no signature) or
  `Error::Unauthorized` (stale admin after rotation)
- Nonce-based replay protection via `DOMAIN_BATCH_CHARGE` (domain 0)
- Emergency-stop gated: blocked when circuit breaker is active

## Behavior
- Admin is authorized once at the batch boundary
- Each subscription is processed via the shared `charge_one` helper
- Failed subscriptions are skipped, not aborted
- Returns one `BatchChargeResult` per input id

## BatchChargeResult
| Field | Type | Values |
|-------|------|--------|
| success | bool | true if charged or scheduled cancellation |
| error_code | u32 | 0 on success, error code on failure |

## Skip conditions
- Subscription not found
- Status is Paused, Cancelled, or InsufficientBalance
- Billing interval has not elapsed
- Insufficient prepaid balance (also marks status InsufficientBalance)
- Auto-renew disabled and interval elapsed (silently skipped)
