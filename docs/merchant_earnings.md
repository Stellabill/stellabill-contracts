# Merchant Earnings Accounting

`SubscriptionVault` tracks merchant earnings as an internal per-merchant, per-token ledger that
is independent from individual subscription records. Every fund movement is reflected
atomically so the ledger can be reconciled deterministically by off-chain indexers.

## Model

- Each successful charge (`charge_subscription`, `charge_usage`, `charge_one_off`) debits a
  subscription's `prepaid_balance` by its `amount`.
- The same amount (net of any protocol fee) is credited to
  `MerchantBalance[(merchant, token)]` in the same storage write — there is no window where
  a subscription has been debited but the merchant has not been credited.
- Merchant balances are tracked per `(merchant, token)` bucket so multi-token vaults stay
  cleanly separated.
- A `TokenEarnings` struct records the full accrual / withdrawal / refund history for each
  bucket, enabling deterministic reconciliation without re-reading event logs.

## Data Structures

```
TokenEarnings {
    accruals: AccruedTotals {
        interval: i128,   // sum of all interval-charge credits
        usage:    i128,   // sum of all usage-charge credits
        one_off:  i128,   // sum of all one-off-charge credits
    },
    withdrawals: i128,    // sum of all merchant withdrawals from this bucket
    refunds:     i128,    // sum of all merchant-initiated subscriber refunds
}

TokenReconciliationSnapshot {
    token:               Address,
    total_accruals:      i128,   // interval + usage + one_off
    total_withdrawals:   i128,
    total_refunds:       i128,
    computed_balance:    i128,   // total_accruals − total_withdrawals − total_refunds
}
```

## Canonical Invariant

For every `(merchant, token)` bucket the following must hold at all times:

```
MerchantBalance[(merchant, token)]
    == accruals.interval + accruals.usage + accruals.one_off
     − withdrawals
     − refunds
```

`get_reconciliation_snapshot(merchant)` returns a `TokenReconciliationSnapshot` per token
where `computed_balance` is exactly the right-hand side. Indexers can cross-check it against
`get_merchant_balance_by_token(merchant, token)` to detect any drift.

## Protocol Fee Handling

When a protocol fee is configured (via `set_protocol_fee`), the subscription charge is split:

- **Merchant receives** `charge_amount * (10_000 − fee_bps) / 10_000` (net amount)
- **Treasury receives** `charge_amount * fee_bps / 10_000` (fee amount)

Both amounts are credited via the same `credit_merchant_balance_for_token` path, so
both the merchant and the treasury address have fully reconciled `TokenEarnings` records.
The merchant's `accruals.interval` records the **net** amount credited to them, not the gross
charge amount.

## Charge Flow (Atomic Credit)

```
charge_one / charge_usage_one
  ├─ debit subscription.prepaid_balance  (write Sub record)
  ├─ credit merchant MerchantBalance     (set_merchant_balance)
  ├─ update TokenEarnings.accruals.*     (set_merchant_token_earnings)
  └─ emit `charged` / `usage_charged` event
```

All four operations happen in the same Soroban invocation frame. If any step fails the
entire transaction reverts atomically.

## Withdrawal Behavior

`withdraw_merchant_funds(merchant, amount)` / `withdraw_merchant_funds_for_token(merchant, token, amount)`:

1. Validates `merchant.require_auth()` and blocklist status.
2. Validates `amount > 0` and `MerchantBalance >= amount`.
3. Checks vault's actual token custody balance ≥ amount.
4. **EFFECTS** (before any external call):
   - Decrements `MerchantBalance[(merchant, token)]` by `amount`.
   - Increments `TokenEarnings.withdrawals` by `amount`.
   - Decrements `TotalAccounted[(token)]` by `amount`.
   - Emits `MerchantWithdrawalEvent` with topic `("withdrawn", merchant, token)`.
5. **INTERACTIONS**: Calls `token.transfer(contract → merchant, amount)`.

The topic contains the token address as the third element so indexers can efficiently
filter withdrawal events per token without decoding the payload.

## Refund Behavior

`merchant_refund(merchant, subscriber, token, amount)`:

1. Validates `merchant.require_auth()`.
2. Validates `amount > 0` and `MerchantBalance[(merchant, token)] >= amount`.
3. **EFFECTS** (before any external call):
   - Decrements `MerchantBalance[(merchant, token)]` by `amount`.
   - Increments `TokenEarnings.refunds` by `amount`.
   - Decrements `TotalAccounted[(token)]` by `amount`.
   - Emits `MerchantRefundEvent`.
4. **INTERACTIONS**: Calls `token.transfer(contract → subscriber, amount)`.

## Invariants

1. For each successful charge, `subscription.prepaid_balance` decreases by exactly
   `charge_amount` and `MerchantBalance[(merchant, token)]` increases by exactly
   `merchant_amount` (= `charge_amount − fee_amount`) in the same transaction.
2. For each successful withdrawal, `MerchantBalance[(merchant, token)]` decreases by exactly
   the withdrawn amount AND `TokenEarnings.withdrawals` increases by the same amount
   (reconciliation invariant is preserved).
3. For each successful refund, `MerchantBalance[(merchant, token)]` decreases by exactly
   the refunded amount AND `TokenEarnings.refunds` increases by the same amount.
4. Merchant balances are isolated by `(merchant, token)` — debiting one bucket never
   affects another.
5. `TotalAccounted[(token)]` tracks all tokens under custody. It increases on subscriber
   deposits and decreases on merchant withdrawals, merchant refunds, and subscriber fund
   withdrawals. The difference `token.balance(contract) − TotalAccounted[(token)]` is the
   recoverable stranded balance.
6. **Reconciliation**: `MerchantBalance = total_accruals − total_withdrawals − total_refunds`.
   This must always be true and is verified by `get_reconciliation_snapshot`.

## Storage Layout

| Key | Storage Tier | Value |
|-----|-------------|-------|
| `DataKey::MerchantBalance(merchant, token)` | instance | `i128` — spendable balance |
| `DataKey::MerchantEarnings(merchant, token)` | instance | `TokenEarnings` — accrual ledger |
| `DataKey::MerchantTokens(merchant)` | instance | `Vec<Address>` — known token list |
| `DataKey::TotalAccounted(token)` | instance | `i128` — total custody tracking |

## Reporting & Indexers

| Entrypoint | Returns |
|-----------|---------|
| `get_merchant_balance_by_token(merchant, token)` | `i128` — spendable balance |
| `get_merchant_token_earnings(merchant, token)` | `TokenEarnings` — full accrual record |
| `get_merchant_total_earnings(merchant)` | `Vec<(Address, TokenEarnings)>` — all tokens |
| `get_reconciliation_snapshot(merchant)` | `Vec<TokenReconciliationSnapshot>` — cross-check values |

Off-chain indexers should listen to:
- `charged` events — interval charge credit (`amount` = gross charge)
- `usage_charged` events — usage charge credit
- `one_off_charged` events — one-off charge credit
- `protocol_fee_charged` events — fee routing (contains `merchant_amount` and `fee_amount`)
- `withdrawn` events (topic: `("withdrawn", merchant, token)`) — withdrawal debit
- `merchant_refund` events — refund debit

## Security Notes

- All arithmetic uses checked operations (`checked_add`, `checked_sub`) returning
  `Error::Overflow` / `Error::Underflow` instead of panicking or wrapping.
- `validate_non_negative` rejects negative credit amounts before any state is written.
- Withdrawals and refunds follow the **Checks-Effects-Interactions** pattern: all state
  mutations (balance, earnings, accounting) are persisted before the external token transfer.
  If the external call reverts, the entire transaction reverts atomically.
- A blocklisted merchant address cannot call `withdraw_merchant_funds*`. Accumulated
  earnings are preserved and released upon admin unblock.
- Merchants can only withdraw their own `(merchant, token)` bucket; the balance lookup
  is keyed by the caller address, so cross-merchant withdrawal is structurally impossible.
