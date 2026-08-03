# Fee Routing — Walkthrough with Numeric Examples

## Overview

On every successful charge (interval, usage, or one-off), the vault splits the
gross charge amount into a **merchant net** and a **protocol fee** credited to
the treasury.

```
gross  =  net  +  fee
```

The fee is a percentage of the gross, expressed in **basis points** (bps).
One basis point = 0.01 %, so:

| Fee (bps) | Fee (%) |
|-----------|---------|
| 0         | 0 %     |
| 250       | 2.50 %  |
| 1 000     | 10 %    |
| 10 000    | 100 %   |

---

## The Formula

The fee is computed with a single integer operation:

```
fee  =  gross × fee_bps / 10 000          (floor division)
net  =  gross − fee
```

The division is **integer floor division** — the remainder stays with the
merchant.  This is the **deterministic rounding rule** and guarantees
conservation on every charge.

---

## Concrete Examples

### Example 1: 6-decimal token (USDC)

**Setup:** `fee_bps = 250` (2.50 %), charge of **100 USDC** (100 × 10⁶ = 100 000 000).

| Step | Calculation | Raw (i128) | Display |
|------|-------------|------------|---------|
| Gross | | 100 000 000 | 100.000 000 USDC |
| Fee | `100_000_000 × 250 / 10_000` | 2 500 000 | 2.500 000 USDC |
| Net | `100_000_000 − 2_500_000` | 97 500 000 | 97.500 000 USDC |

**Check:** `97 500 000 + 2 500 000 = 100 000 000` ✓

---

### Example 2: 2-decimal token

**Setup:** `fee_bps = 250` (2.50 %), charge of **100.00** tokens
(100 × 10² = 10 000).

| Step | Calculation | Raw (i128) | Display |
|------|-------------|------------|---------|
| Gross | | 10 000 | 100.00 |
| Fee | `10_000 × 250 / 10_000` | 250 | 2.50 |
| Net | `10_000 − 250` | 9 750 | 97.50 |

**Check:** `9 750 + 250 = 10 000` ✓

---

### Example 3: 7-decimal token

**Setup:** `fee_bps = 250` (2.50 %), charge of **100.000 000 0** tokens
(100 × 10⁷ = 1 000 000 000).

| Step | Calculation | Raw (i128) | Display |
|------|-------------|------------|---------|
| Gross | | 1 000 000 000 | 100.000 000 0 |
| Fee | `1_000_000_000 × 250 / 10_000` | 25 000 000 | 2.500 000 0 |
| Net | `1_000_000_000 − 25_000_000` | 975 000 000 | 97.500 000 0 |

**Check:** `975 000 000 + 25 000 000 = 1 000 000 000` ✓

---

## The Rounding Party (Edge Cases)

Because the fee uses integer floor division, not every amount divides evenly.
The **remainder always stays with the merchant**.

### Example 4: Non-divisible amount (6-decimal)

**Setup:** `fee_bps = 250`, charge of **1 USDC** (1 000 000 raw).

```
fee   = 1_000_000 × 250 / 10_000
      = 250_000_000 / 10_000
      = 25_000              (exact — 0.025 000 USDC)

net   = 1_000_000 − 25_000
      = 975_000             (0.975 000 USDC)
```

### Example 5: Non-divisible with remainder

**Setup:** `fee_bps = 333` (3.33 %), charge of **1 USDC** (1 000 000 raw).

```
fee   = 1_000_000 × 333 / 10_000
      = 333_000_000 / 10_000
      = 33_300              (floor — 0.033 300 USDC)

net   = 1_000_000 − 33_300
      = 966_700             (0.966 700 USDC)

check: 33_300 + 966_700 = 1_000_000 ✓
```

The exact mathematical result of 1 000 000 × 3.33 % would be **33 333.33**,
but integer arithmetic truncates the fractional .33 remainder.  The merchant
receives **one extra raw unit** — rounding always favors the merchant.

### Example 6: Very small charge with high fee

**Setup:** `fee_bps = 10 000` (100 %), charge of **1 unit** (1 raw).

```
fee   = 1 × 10_000 / 10_000
      = 10_000 / 10_000
      = 1

net   = 1 − 1
      = 0
```

The entire amount goes to the treasury; the merchant receives 0.

### Example 7: Sub-unit charge rounding to zero fee

**Setup:** `fee_bps = 250` (2.50 %), charge of **1 unit** (1 raw) — a sub-cent
amount in a 6-decimal token.

```
fee   = 1 × 250 / 10_000
      = 250 / 10_000
      = 0                  (floor — 0)

net   = 1 − 0
      = 1
```

When the calculated fee is less than one raw unit, it truncates to zero and
the **entire amount goes to the merchant**.  This is safe because the amounts
involved are economically negligible (e.g., 1 raw = 0.000 001 USDC).

---

## Edge Case: Fee at Zero

**Setup:** `fee_bps = 0`, any charge amount.

```
fee   = gross × 0 / 10_000 = 0
net   = gross
```

The full gross is credited to the merchant.  No `ProtocolFeeChargedEvent` is
emitted.  This is the default (disabled) state — see
[`docs/protocol_fees.md`](protocol_fees.md).

---

## Edge Case: Fee at MAX_FEE_BIPS (10 000)

**Setup:** `fee_bps = 10 000` (100 %), charge of **100 USDC**.

```
fee   = 100_000_000 × 10_000 / 10_000
      = 100_000_000 × 1
      = 100_000_000        (100 %)

net   = 0
```

The entire gross amount is credited to the treasury.  The merchant receives
nothing.  This extreme setting is useful only for on-chain treasury sweeps
or testing.

---

## Cross-Token Fee Routing & Fee-Token Override

By default every fee is routed in the **same token** as the charge — the
subscription's settlement token.  The admin may, however, configure a global
**fee-token override** (`set_fee_token`) so the treasury receives the protocol
fee in a different token from the settlement token.

```
gross (settlement token)      →  merchant receives gross − fee
fee (converted at oracle)     →  treasury receives fee value in fee_token
```

### When the override applies

Conversion is attempted **only** when all of the following hold:

1. A fee-token override is configured **and** differs from the settlement token.
2. The oracle is enabled and has an address configured.
3. The oracle price query for `settlement_token → fee_token` succeeds.
4. The converted amount is **greater than zero** (precision-loss guard).

Otherwise — no override, override equal to the settlement token, oracle
unavailable, or a failed/zero-rounding price query — the fee is credited in the
original settlement token and no conversion event is emitted.

### Conversion math

```
converted_fee = floor( fee_amount × price / PRICE_SCALE )
```

`price` is the oracle quote in **quote-per-base** scaled by `10^7`
(`PRICE_SCALE`).  The treasury is credited `converted_fee` of `fee_token`.

### Example 8: Cross-token conversion (fee-token override)

**Setup:** subscription charges **100 USDC** (6 decimals) with `fee_bps = 250`;
`set_fee_token(XLM)`; oracle price `1 USDC = 3.50 XLM` (`price = 35_000_000`
at `PRICE_SCALE = 10^7`).

| Step | Calculation | Result |
|------|-------------|--------|
| Gross (USDC raw) | | `100_000_000` |
| Fee (USDC raw) | `100_000_000 × 250 / 10_000` | `2_500_000` |
| Merchant net (USDC) | `100_000_000 − 2_500_000` | `97_500_000` |
| Converted fee (XLM raw) | `2_500_000 × 35_000_000 / 10_000_000` | `8_750_000` |
| Treasury credit | in fee token (XLM) | `8_750_000` |

The merchant net is **always** credited in the settlement token; only the
treasury's fee leg may be re-denominated.

### Example 9: Override disabled or oracle down

Same `100 USDC` / `fee_bps = 250` but with no fee-token override (or an
unavailable oracle).  The fee `2_500_000` is credited to the treasury in USDC
and **no** `FeeConvertedEvent` is emitted.

---

## Events

### `ProtocolFeeChargedEvent`

Emitted on every charge where `fee > 0`.  The event carries the exact
on-chain amounts so indexers can reconstruct the split.

| Field            | Type      | Description                          |
|------------------|-----------|--------------------------------------|
| `subscription_id` | `u32`    | Subscription that was charged        |
| `merchant`       | `Address` | Merchant receiving the net amount    |
| `token`          | `Address` | Settlement token                     |
| `fee_amount`     | `i128`    | Fee credited to treasury             |
| `treasury`       | `Address` | Treasury address receiving the fee   |
| `timestamp`      | `u64`     | Ledger timestamp                     |

### `FeeConvertedEvent`

Emitted on every charge where a cross-token fee-token override was applied,
recording the exact on-chain conversion.

| Field                    | Type      | Description                                  |
|--------------------------|-----------|----------------------------------------------|
| `subscription_id`        | `u32`     | Subscription that was charged                |
| `source_token`           | `Address` | Settlement token the fee was withheld in     |
| `target_token`           | `Address` | Fee token the treasury was credited with     |
| `original_fee_amount`    | `i128`    | Fee amount in the settlement token           |
| `converted_fee_amount`   | `i128`    | Fee amount credited in the fee token         |
| `rate`                   | `u128`    | Oracle quote-per-base price scaled by `10^7` |
| `timestamp`              | `u64`     | Ledger timestamp                             |

### `ProtocolFeeConfiguredEvent`

Emitted when the admin calls `set_protocol_fee`.

| Field       | Type          | Description              |
|-------------|---------------|--------------------------|
| `admin`     | `Address`     | Admin who changed the fee |
| `treasury`  | `Address`     | Fee recipient address    |
| `fee_bps`   | `u32`         | Fee in basis points      |
| `timestamp` | `u64`         | Ledger timestamp         |

### `MerchantFeeOverrideSetEvent`

Emitted when an admin calls `set_merchant_fee_override` or `clear_merchant_fee_override`,
per-merchant basis-point precedence over the global rate.

| Field       | Type      | Description                             |
|-------------|-----------|-----------------------------------------|
| `merchant`  | `Address` | Merchant whose rate was overridden      |
| `admin`     | `Address` | Admin who authorized the change         |
| `fee_bps`   | `u32`     | New per-merchant fee (`None` on clear)  |
| `timestamp` | `u64`     | Ledger timestamp                        |

The fee-token override is configured via `set_fee_token` (emitting on-chain
`fee_token_configured`) and read back with `get_fee_token`.

---

## Accounting Invariants

1. **Conservation:** `gross == net + fee` on every charge.  Verified in the
   code by subtracting the fee from gross, never by adding.

2. **Rounding favors the merchant:** Integer floor division means any
   remainder from the fee calculation stays with the merchant.  The treasury
   never receives more than `floor(gross × fee_bps / 10 000)`.

3. **Fee is computed from discounted amount:** When a coupon is applied,
   the protocol fee is computed from the **post-discount** payable amount,
   not the gross pre-discount amount.  See
   [`docs/protocol_fees.md`](protocol_fees.md) for details.

4. **Conservation holds in the settlement token:** `gross == net +
   fee_amount` is always true in the settlement token.  When a fee-token
   override is applied the treasury is credited in the fee token, but the
   computation that reconciles with `gross` always uses the original
   settlement-token fee amount.

5. **No treasury → no fee:** If `fee_bps > 0` but no treasury address is
   configured, the full gross is credited to the merchant.  This prevents
   silent fund loss.

---

## Security Notes

- Fee computation uses **pure integer arithmetic** with no external calls —
  no reentrancy risk.
- The treasury balance accrues identically to merchant balances and is
  subject to the same withdrawal controls.
- `fee_bps > 10_000` is rejected at configuration time (`InvalidInput`).
- The fee is computed from the **gross** charge amount, not from the
  merchant's net — preventing fee-on-fee compounding.
- Cross-token conversion rounds toward zero and **falls back to the
  settlement token** (crediting the trusted amount) whenever the oracle is
  down, the price query fails, or the converted value would be zero — the
  treasury is never credited a fabricated amount, and conversion never rounds
  a positive fee down to nothing.

---

## References

- [`docs/protocol_fees.md`](protocol_fees.md) — Full protocol fee specification
- [`docs/merchant_earnings.md`](merchant_earnings.md) — How merchant balances work
- [`docs/governance/authoring.md`](governance/authoring.md) — Governance proposals for changing fee parameters
- `contracts/subscription_vault/src/charge_core.rs` — Interval and usage charge fee logic
- `contracts/subscription_vault/src/subscription.rs` — One-off charge fee logic
- `contracts/subscription_vault/src/admin.rs` — Fee configuration (`set_protocol_fee`, `set_fee_token`)
- `contracts/subscription_vault/src/merchant.rs` — Per-merchant fee override (`set_merchant_fee_override`)
- `contracts/subscription_vault/src/types.rs` — Event structs, `MAX_FEE_BIPS`
