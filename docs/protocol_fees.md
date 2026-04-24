# Protocol Fees

The StellaBill Protocol supports a protocol fee mechanism that skims a percentage of merchant earnings to a configured treasury address.

## Configuration

Protocol fees are managed by the contract admin.

- **Treasury**: The address where protocol fees are credited.
- **Fee Rate**: Defined in basis points (BPS), where 1 BPS = 0.01%.
- **Maximum Fee**: The protocol enforces a hard cap of **5000 BPS (50%)** to prevent misconfiguration.

## Fee Calculation & Rounding

Fees are calculated on every successful charge (Interval, Usage, or One-Off).

### Rounding Rule
The protocol uses **floor rounding** (integer division truncation) for fee calculations:
`fee_amount = (gross_amount * fee_bps) / 10,000`

This ensures that the fee never exceeds the mathematical expectation, slightly favoring the merchant/subscriber in cases of non-clean division.

### Conservation of Value
The protocol strictly enforces:
`gross_debit == merchant_credit + treasury_credit`

## Event Reporting

Every charge event includes transparent fee accounting:

- `SubscriptionChargedEvent`: Includes `amount` (gross), `fee`, and `net_amount`.
- `UsageStatementEvent`: Includes `usage_amount` (gross), `fee`, and `net_amount`.
- `OneOffChargedEvent`: Includes `amount` (gross), `fee`, and `net_amount`.

Additionally, a dedicated `ProtocolFeeChargedEvent` is emitted whenever a non-zero fee is collected, linking the charge to the treasury address.

## Reconciliation

Protocol fees are treated as internal balance credits to the treasury address, using the same ledger accounting as merchant earnings. This ensures they are fully covered by the contract's total accounted balance and compatible with all reconciliation tooling.
