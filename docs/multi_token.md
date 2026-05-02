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
