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
