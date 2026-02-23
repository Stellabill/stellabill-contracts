# Merchant configuration

`SubscriptionVault` supports per-merchant configuration records to control subscription defaults and constraints without changing global contract settings.

## Fields

- `version` (`u32`): schema version for forward-compatible config upgrades.
- `min_subscription_amount` (`i128`): merchant-specific minimum subscription amount in token base units.
  - `0` means no merchant-specific minimum.
- `default_interval_seconds` (`u64`): default interval to use when `create_subscription` is called with `interval_seconds = 0`.
  - `0` means no default; caller must provide explicit interval.

## Storage key strategy

Configs are stored under `DataKey::MerchantConfig(Address)` in instance storage.
This keeps records isolated per merchant, scales across many subscriptions, and allows adding new key variants later without changing existing subscription storage.

## Entry points

- `set_merchant_config(actor, merchant, min_subscription_amount, default_interval_seconds)`
  - full overwrite; callable by contract admin or that merchant.
- `update_merchant_config(actor, merchant, min_subscription_amount?, default_interval_seconds?)`
  - partial update; `None` leaves a field unchanged.
- `get_merchant_config(merchant)`
  - returns stored config or default `{ version: 1, min_subscription_amount: 0, default_interval_seconds: 0 }`.

## Subscription creation behavior

When `create_subscription` runs:

1. If config `min_subscription_amount > 0`, `amount` must be at least that minimum.
2. If `interval_seconds == 0`, contract uses config `default_interval_seconds`.
3. If `interval_seconds == 0` and config default interval is also `0`, call fails with `InvalidAmount`.

## Recommended defaults

- `min_subscription_amount`: set to at least one billing unit (for USDC commonly `1_000000`).
- `default_interval_seconds`: set to your primary billing cadence (for example `2_592_000` for 30 days).
- Keep `version` at `1` until a migration requires new fields.

## Upgradeability note

The config struct includes `version` and is stored independently from `Subscription` records. New config fields can be added in later versions with migration logic while preserving existing subscription and merchant balance storage layout.
