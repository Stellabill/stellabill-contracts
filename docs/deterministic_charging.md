# Deterministic Charging & Replay Protection

This document outlines the deterministic guarantees and replay protection mechanisms implemented in `charge_core.rs`.

## Deterministic Behavior

The charging logic is designed to be deterministic based on the following inputs:
- **Ledger Timestamp**: Used to calculate the current billing period and validate interval elapsing.
- **Subscription State**: Current status, last payment timestamp, and prepaid balance.
- **Reference String (for usage)**: Unique identifier for metered usage charges.

For a given subscription and ledger state, the output of a charging operation (Success, Replay, or Failure) is stable and reproducible.

## Replay Protection

### Interval Charges (`charge_one`)
Replay protection for interval charges is enforced using a tracking variable `charged_period_key(subscription_id)` in the contract's instance storage.
- The `period_index` is calculated as `now / interval_seconds`.
- If the calculated `period_index` is less than or equal to the `stored_period`, the request is rejected with `Error::Replay`.
- This ensures that a subscription can only be charged once per billing interval, even if multiple calls are made within the same interval.

### Usage Charges (`charge_usage_one`)
Replay protection for usage charges is enforced using a unique reference string.
- Each usage charge requires a `reference` string.
- The contract stores a mapping `(Symbol("usage_ref"), subscription_id, reference) -> true`.
- If a reference has already been used for a given subscription, subsequent attempts with the same reference are rejected with `Error::Replay`.

## Idempotency

- All replay attempts for both interval and usage charges return the same error code: `Error::Replay`.
- This allows external systems to reliably retry failed or timed-out transactions without risk of double-charging.

## Admin-Only Authorization

All charge entrypoints that move funds from subscriber vaults to merchant earnings
are **admin-only**. The stored admin is loaded from `DataKey::Admin` and must
sign the transaction. This applies to:

- `charge_subscription` — single interval charge
- `charge_usage` — metered usage charge
- `charge_usage_with_reference` — metered usage charge with reference string
- `batch_charge` — batch interval charge (with nonce replay protection)

Operator-based charging (`operator_charge_subscription`, `operator_charge_usage`,
`operator_batch_charge`) delegates to the same `charge_core` functions but
requires the stored operator to authenticate instead.

Non-admin callers are rejected before any state mutation occurs, ensuring that
only the billing engine (or designated operators) can trigger charges.

## Security Considerations

- Replay protection state is stored in `instance` storage to ensure it persists across ledger boundaries.
- The use of `period_index` prevents "skipping" intervals while ensuring exactly-once charging for those that have elapsed.
- All charge entrypoints enforce admin-only authorization to prevent unauthorized fund movement.
