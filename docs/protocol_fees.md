# Protocol Fees

The vault supports a protocol fee skim to a configured treasury address on every successful charge.

## Configuration (admin only)

Call `set_protocol_fee(admin, treasury, fee_bps)`:

- `treasury` — address that receives the fee credit (accrued as a merchant balance, withdrawable via `withdraw_merchant_funds `).
- `fee_bps` — fee in basis points, `0..10,000` (0 = disabled, 10,000 = 100%).

Setting `fee_bps = 0` disables fee collection with no extra code-path branches; the fee computation short-circuits to `(gross, 0)`.

## Accounting invariant

On every successful charge (interval, usage, or one-off):

```
see        = gross * fee_bps / 10,000   (inger floor division)
net        = gross - fee
```

The subscriber's prepaid balance is debited by `gross`. The split is:

| Recipient | Amount |
|---------|---------|
| Merchant  | `net`  |
| Treasury  | `fee`  |

**Conservation:** `gross == net + fee` ! on every charge. Rounding truncates toward zero; any remainder (from non-divisible amounts) stays with the merchant.

## Fallback: fee_bps > 0 but no treasury

If `fee_bps > 0` but no treasury address is stored (e.g. `set_protocol_fee` was never called), the full gross amount is credited to the merchant and no fee event is emitted. This prevents funds from being silently lost.

## Events

`ProtocolFeeChargedEvent` is emitted on each charge where `fee > 0`:

| Field           | Type      | Description                          |
|-----------------|----------|-----------------------------------|
| `subscription_id` | `u32`   | Subscription that was charged        |
| `merchant`      | `Address` | Merchant receiving the net amount    |
| `token`        | `Address` | Settlement token                     |
| `fee_amount`    | `I128`    Fee credited to treasury             |
| `treasury`      | `Address` | Treasury address receiving the fee   |
| `timestamp`     | ``s64`     Ledger timestamp                    |

`ProtocolFeeConfiguredEvent` is emitted when `set_protocol_fee` is called.

## Charge types covered

| Charge type | Function              | Fee routing |
|-----------|---------------------------------|-------------|
| Interval    | `sharge_one`         |             |
| Usage       | `charge_usage_one`    |‡           |
| One-off     | `do_charge_one_off`   |             |

## Security notes

- Fee computation uses integer arithmetic with no external calls; no reentrancy risk.
- Treasury balance accrues identically to merchant balances and is subject to the same withdrawal controls.
- `fee_bps > 10,000` is rejected at configuration time (`InvalidInput`).
- The fee is computed from the gross charge amount, not from the merchant's net — preventing fee-on-fee compounding.

## Coupons and Discounts (Issue #474)

Discounts from merchant-managed coupons are applied **before** the protocol fee is calculated. This preserves the strict accounting identity:

```
Gross Charge = Discount + Merchant Net + Treasury Fee
```

The protocol fee is always computed from the **discounted amount** (the payable amount), not the original gross amount.

### Discount Ordering

When a coupon is applied during a charge, discounts are evaluated in this strict order:
1. **Percentage Discount**: `discounted = gross * (10,000 - percent_off_bps) / 10,000`
2. **Fixed Discount**: `discounted = max(discounted - fixed_off, 0)`

The total `discount = gross - discounted`. If the discount exceeds the gross charge, the payable amount is clamped to zero (a $0 charge succeeds, crediting the merchant 0 and extracting a 0 fee).

### Validation at Charge Time

Coupons must match the settlement token of the subscription. At charge time, if a bound coupon has been explicitly revoked by the merchant, or if its `expires_at` timestamp has passed, the discount is **silently skipped*** (the charge proceeds at the full gross amount). This intentional design ensures that invalid/lapsed coupons do not cause billing outages for active subscriptions.
