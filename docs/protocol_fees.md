# Protocol Fees

The vault supports a protocol fee skim to a configured treasury.

Configuration (admin only):

- treasury address
- `protocol_fee_bps` in basis points (0..=10_000)

On each successful interval or usage charge:

- subscriber is debited the gross amount
- fee is computed as `gross * protocol_fee_bps / 10_000`
- merchant receives net amount
- treasury balance receives fee amount

This preserves conservation of value:

`gross debit == merchant credit + treasury credit`
