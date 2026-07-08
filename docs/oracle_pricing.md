# Optional oracle pricing

The subscription vault supports optional cross-currency pricing through an external oracle contract.

## Oracle interface

When enabled, the vault calls oracle method:

- `latest_price() -> OraclePrice`

`OraclePrice` fields:

- `price`: quote units per 1 token (must be positive)
- `timestamp`: quote publication time

## Configuration

Admin-only:

- `set_oracle_config(admin, enabled, oracle, max_age_seconds)`

Read:

- `get_oracle_config()`

Safety checks:

- enabled requires oracle address
- enabled requires `max_age_seconds > 0` (zero disables staleness guard and is rejected)
- stale data rejected when quote age exceeds `max_age_seconds`
- zero/negative price rejected
- zero timestamp rejected as unavailable

## Charge conversion

With oracle disabled, `subscription.amount` is treated as token-denominated (existing behavior).

With oracle enabled, `subscription.amount` is interpreted as quote-denominated and converted:

`token_amount = ceil(quote_amount * 10^token_decimals / price)`

This preserves deterministic charging while allowing quote-currency plan pricing.

## Failure modes

- `OracleNotConfigured`
- `OraclePriceUnavailable`
- `OraclePriceStale`
- `OraclePriceInvalid`

These errors cause the charge to fail without mutating balances.

## Events

For off-chain verification and indexability, the following events are emitted:

- `oracle_config_updated`: Emitted when the admin updates oracle configuration. Includes enabled status, oracle address, max acceptable age, and timestamp.
- `oracle_charge_resolved`: Emitted when a charge resolves its token target via the oracle. Includes `quote_amount`, `token_amount`, `price`, `price_timestamp` from the oracle, and resolution `timestamp`.
- `oracle_liveness`: Emitted when `emit_oracle_liveness()` is called for monitoring. Includes `last_sample_ts`, `age`, `healthy` status, and check timestamp. Allows monitoring rigs to alert before charges start failing due to stale oracle data.

## Oracle Liveness Monitoring

The contract provides a view-only `emit_oracle_liveness()` entrypoint that enables monitoring systems to verify oracle health without requiring admin privileges.

### Usage

```rust,ignore
// Check oracle health before charging
match client.emit_oracle_liveness(&env) {
    Ok(event) => {
        if event.healthy {
            // Oracle is healthy, proceed with oracle-dependent charge
            println!("Oracle healthy: age={}s, threshold={}s", event.age, event.max_age_seconds / 2);
        } else {
            // Oracle is stale or approaching staleness
            // Use fallback pricing or alert operators
            eprintln!("WARNING: Oracle stale! Age={}s exceeds healthy threshold", event.age);
        }
    }
    Err(Error::OracleNotConfigured) => {
        // Oracle not enabled, use base pricing
        println!("Oracle not configured, using base subscription amounts");
    }
    Err(e) => panic!("Unexpected error: {:?}", e),
}
```

### OracleLivenessEvent Fields

| Field | Type | Description |
|-------|------|-------------|
| `last_sample_ts` | `u64` | Timestamp of the latest oracle price sample |
| `age` | `u64` | Age of the sample in seconds (`current_time - last_sample_ts`) |
| `healthy` | `bool` | `true` if `age <= max_age_seconds / 2`, indicating healthy oracle |
| `timestamp` | `u64` | Ledger timestamp when this liveness check was performed |

### Health Threshold

The `healthy` field is computed as:

```
healthy = (age <= max_age_seconds / 2)
```

This provides early warning when the oracle sample is approaching the staleness threshold. Monitoring systems can alert operators when `healthy = false`, allowing intervention before charges start failing with `OraclePriceStale` errors.

### Security Properties

- **No authentication required**: Any caller can invoke `emit_oracle_liveness()` to verify oracle health
- **View-only**: Does not modify contract state
- **Event emission**: Publishes `OracleLivenessEvent` for off-chain indexers and monitoring systems
- **Error handling**: Returns `OracleNotConfigured` if oracle is not enabled, preventing confusion

### Integration with Monitoring

Monitoring rigs can:

1. Call `emit_oracle_liveness()` on a schedule (e.g., every 60 seconds)
2. Track the `age` field to detect increasing staleness
3. Alert operators when `healthy = false` (age > max_age_seconds / 2)
4. Trigger fallback procedures before charges fail

This provides proactive oracle health monitoring, allowing operators to address issues before they impact subscription billing.
