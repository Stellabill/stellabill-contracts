# Multi-token subscription support

The vault now supports multiple accepted settlement tokens with token-isolated accounting.

## Token registry

Admin can manage accepted tokens:

- `add_accepted_token(admin, token, decimals)`
- `remove_accepted_token(admin, token)` (default token cannot be removed)
- `list_accepted_tokens()`

`init` registers the initial token as the default accepted token.

## Subscription token pinning

- `create_subscription(...)` uses the default token.
- `create_subscription_with_token(...)` pins subscription to a chosen accepted token.
- `create_plan_template_with_token(...)` allows token-specific plan templates.

Each subscription stores its `token` and all future transfers/charges must use that token.

## Token-isolated merchant balances

Merchant earnings are now tracked by `(merchant, token)` bucket:

- `get_merchant_balance_by_token(merchant, token)`
- `withdraw_merchant_token_funds(merchant, token, amount)`

Withdrawals validate both the merchant's bucket balance and the contract's custody balance for
that token before transferring funds.

Legacy `get_merchant_balance` and `withdraw_merchant_funds` continue to target the default token bucket.

## Query helper

- `get_subscriptions_by_token(token, start, limit)` returns `Result<Vec<Subscription>, Error>` for paginated subscriptions; `limit` must be between `1` and `100` (same as merchant listing).
- `get_token_subscription_count(token)` returns the length of the token’s subscription id index for pagination metadata.

## Compatibility notes

- Existing single-token deployments continue to work unchanged.
- New multi-token flows are additive and opt-in.

## Security notes

- **Token confusion prevention**: each subscription stores its `token` address at creation time. All deposits, charges, and withdrawals use that stored token — it is never inferred from caller input after creation.
- **Allowlist enforced on every entrypoint**: `create_subscription_with_token`, `create_subscription_from_plan` (via plan template token), and `create_plan_template_with_token` all call `is_token_accepted` before proceeding. Unaccepted tokens return `Error::InvalidInput`.
- **Default token cannot be removed**: `remove_accepted_token` rejects the primary token with `Error::InvalidInput`, preventing accidental lockout of existing subscriptions.
- **Active subscriptions survive token removal**: removing a token from the allowlist does not affect existing subscriptions using that token. They remain readable and chargeable. Only *new* subscriptions with the removed token are blocked.
- **Per-token merchant buckets**: earnings are tracked by `(merchant, token)` key. `withdraw_merchant_token_funds` validates the correct bucket balance before transferring, preventing cross-token fund confusion.

## Amount normalization and migration path

### Decimal-aware amount normalization

To allow consistent tracking and auditing across multiple settlement tokens with differing decimals (e.g., 6-decimal EURC vs 7-decimal USDC/native assets), the contract defines standard decimal-aware normalization helpers:
- `normalize_amount(env, token, raw) -> Result<i128, Error>`: Normalizes a raw token amount to a standardized 9-decimal base (1e9 internal base).
- `denormalize_amount(env, token, normalized) -> Result<i128, Error>`: Converts a normalized 9-decimal amount back to the raw token's base units.

Reconciliation summaries (`get_token_reconciliation` and `get_recon_summary`) report both raw and 9-decimal normalized values for auditing consistency across diverse asset types.

### Migration path for existing balances

For deployments running prior to the introduction of multi-token support, existing subscriber prepaid balances and merchant liabilities were committed using the default token (assumed to be 7-decimal USDC).

When querying or migrating these balances:
1. **Identify default token configurations**: Subscriptions created before version 2 are implicitly pinned to the primary settlement token configuration.
2. **Calculate normalized values**: If migrating balances to another vault or verifying accounting off-chain, multiply 7-decimal balances by `10^2` (i.e. `10^(9 - 7)`) to normalize them to the 9-decimal base:
   $$\text{Normalized Balance} = \text{Raw Balance} \times 100$$
3. **Audit against normalized total**: Ensure the sum of normalized subscriber prepaid balances plus normalized merchant liabilities matches the normalized custody balance of the default token in the contract.

