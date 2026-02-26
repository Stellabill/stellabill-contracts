# Merchant Withdrawals

Merchants can withdraw their accumulated USDC balance from the Subscription Vault using the `withdraw_merchant_funds` entrypoint. 
Funds accumulate to a merchant's balance each time a subscription for that merchant is successfully charged.

## Process and Requirements

1. **Authorization**: The merchant must authorize the withdrawal transaction. The contract enforces this using `merchant.require_auth()`.
2. **Valid Amounts**: The `amount` to withdraw must be strictly positive (`> 0`). An attempt to withdraw `0` or a negative amount will result in `Error::InvalidAmount` (`405`).
3. **No Overdrafts**: A merchant cannot withdraw more than their currently accumulated balance. Overdraft attempts are rejected with `Error::InsufficientBalance` (`1003`).
4. **Zero Balance**: If a merchant has no recorded accumulated balance (e.g., no subscriptions have been charged yet), withdrawal attempts will return `Error::NotFound` (`404`).

## Security Guarantees

- **Transfer First**: To prevent double-spending or re-entrancy issues, the contract transfers the tokens from the vault to the merchant *before* committing the updated (subtracted) balance to the ledger. If the token transfer fails, the contract execution aborts, and the original merchant balance is retained.
- **Arithmetic Safety**: Internal checks comprehensively prevent overflows using checked arithmetic (`checked_add`, `checked_sub`).
- **No Side Effects**: A failed withdrawal (due to overdraft or mismatched auth) has no side-effects on the ledger state or other subscriptions.

## Interaction Flow
1. An admin charges a subscription using `charge_subscription`.
2. The `SubscriptionVault` increments the `merchant_balance` by the subscription's `amount`.
3. The merchant triggers `withdraw_merchant_funds` specifying the `amount` of USDC to withdraw.
4. The requested USDC amount is transferred to the merchant's Stellar account.
5. The `merchant_balance` is permanently debited.
