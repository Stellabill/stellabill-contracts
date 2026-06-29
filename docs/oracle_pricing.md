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

---

## OracleAdapter Architecture (Issue #477)

### Overview

Oracle pricing is now pluggable via a **strategy pattern**. The `OracleConfig` struct carries an `OracleKind` field that selects which adapter resolves the price at charge time. The public contract ABI and storage remain backwards compatible — configs without an explicit `kind` default to `Spot`.

### OracleKind

```rust
pub enum OracleKind {
    Spot,      // latest single price sample (default)
    Twap,      // median across a configurable sliding window
    FixedRate, // deterministic ratio; no oracle reads
}
```

### Configuration

The `set_oracle_config` entrypoint now accepts additional fields:

```
set_oracle_config(
    admin,
    enabled,
    oracle,          // Option<Address> — required for Spot/Twap
    max_age_seconds, // staleness threshold
    kind,            // OracleKind::Spot | Twap | FixedRate
    window_secs,     // TWAP window (ignored for Spot/FixedRate)
    fixed_numerator, // FixedRate numerator (ignored otherwise)
    fixed_denominator // FixedRate denominator, must be != 0
)
```

The `oracle_config_updated` event now includes `kind`, `window_secs`, `fixed_numerator`, and `fixed_denominator` for full auditability.

---

### SpotAdapter

Reads the latest `OraclePrice` from `oracle.latest_price()` and validates it:
- Rejects non-positive prices (`OraclePriceInvalid`).
- Rejects prices whose age exceeds `max_age_seconds` (`OraclePriceStale`).

This is the default behaviour and preserves all existing charge logic.

---

### TwapAdapter

Reads a list of `OraclePrice` observations via `oracle.get_observations(since)` for the last `window_secs` seconds.

**Median calculation** — not arithmetic mean:

```
prices = [obs.price for obs in observations if obs.age <= max_age_seconds]
sort(prices)
median = prices[len(prices) / 2]  // middle element for odd-length
```

Using the median rather than the mean means an attacker must control **more than half** of the observations inside the window to shift the output meaningfully. This resists single-block (flash-loan) price manipulation.

**Edge cases:**
| Scenario | Result |
|---|---|
| 1 observation | That price (equivalent to spot) |
| Empty window | `OraclePriceUnavailable` |
| All observations stale after filtering | `OraclePriceStale` |

---

### FixedRateAdapter

Computes a deterministic price without any oracle contract calls:

```
price = (fixed_numerator × 10^7) / fixed_denominator
```

- `fixed_denominator == 0` is rejected at configuration time with `InvalidInput`.
- Staleness and oracle address are completely ignored.
- Suitable for pegged pairs, test environments, and fee structures expressed as ratios.

**Security:** configuration changes require admin auth, so unauthorized parties cannot alter the fixed rate.

---

### Dispatch Flow

`resolve_charge_amount` now delegates to `oracle_adapter::dispatch_price`:

```
match config.kind {
    Spot      → SpotAdapter::quote()
    Twap      → TwapAdapter::quote()
    FixedRate → FixedRateAdapter::quote()
}
```

All adapters share the same `OracleAdapter` trait and return a `u128` price scaled by `10^7`. The charge math that follows is unchanged.

---

### Security Rationale

| Property | Spot | TWAP | FixedRate |
|---|---|---|---|
| Oracle reads | Yes | Yes | No |
| Staleness enforced | Yes | Yes (per observation) | N/A |
| Manipulation resistance | Low | High (median) | Perfect (static) |
| Oracle dependency | Required | Required | None |
| Admin auth to change | Yes | Yes | Yes |
