# Merchant earnings accounting

`SubscriptionVault` tracks merchant earnings as an internal per-merchant ledger that is independent from individual subscription records.

## Model

- Each successful `charge_subscription(subscription_id)` debits one subscription's `prepaid_balance` by its `amount`.
- The same amount is credited to `merchant_balance[subscription.merchant]`.
- Merchant balances are stored under `DataKey::MerchantBalance(Address)` in instance storage.
- Merchant balances aggregate earnings across any number of subscriptions and subscribers.

## Withdrawal behavior

- `withdraw_merchant_funds(merchant, amount)` requires merchant auth.
- It validates `amount > 0` and `merchant_balance >= amount`.
- On success it debits internal merchant balance, then transfers tokens from vault custody to the merchant wallet.
- Repeated withdraw attempts cannot exceed internally recorded earnings, preventing double spending.

## Invariants

1. For each successful charge, `subscription.prepaid_balance` decreases by exactly `subscription.amount`.
2. For each successful charge, `merchant_balance[merchant]` increases by exactly `subscription.amount`.
3. For each successful merchant withdrawal, `merchant_balance[merchant]` decreases by exactly withdrawn amount.
4. Merchant balances are isolated by merchant address and must not leak across merchants.
5. Contract state updates and token transfer happen in one transaction; if token transfer fails, the transaction aborts and state is reverted.

## Security notes

- Charge logic rejects non-`Active` subscriptions and returns `InsufficientBalance` for underfunded subscriptions.
- Internal accounting uses checked arithmetic (`checked_add`, `checked_sub`) to prevent silent overflow/underflow.
- Earnings are accrued internally before payout; funds remain in contract custody until explicit merchant withdrawal.
