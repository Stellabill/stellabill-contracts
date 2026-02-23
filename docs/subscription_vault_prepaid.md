# Subscription Vault prepaid initialization

This contract initializes prepaid balances during `create_subscription` by pulling token funds into the vault before persisting subscription state.

## Flow

1. `subscriber` authorizes `create_subscription`.
2. Contract validates input:
- `amount > 0`
- `interval_seconds > 0`
3. Contract loads configured token address from instance storage.
4. Contract checks token allowance:
- `allowance(subscriber, vault_contract) >= amount`
- returns `Error::InsufficientAllowance` when not satisfied.
5. Contract checks subscriber token balance:
- `balance(subscriber) >= amount`
- returns `Error::TransferFailed` when not satisfied.
6. Contract executes `transfer_from(vault_contract, subscriber, vault_contract, amount)`.
7. Contract writes subscription state:
- `prepaid_balance = amount`
- `last_payment_timestamp = ledger.timestamp`
- `status = Active`

## Safety assumptions

- The configured token contract follows Soroban token semantics for `allowance`, `balance`, and `transfer_from`.
- Subscriber must approve this contract address as spender before calling `create_subscription`.
- Pre-checks are used to convert common token transfer failures into explicit contract errors (`InsufficientAllowance`, `TransferFailed`) instead of opaque host failures.
- No partial state is written before transfer succeeds; subscription storage write happens after the transfer call.

## Storage compatibility

No changes were made to `Subscription` field order or storage keys. The implementation remains compatible with existing instance storage layout and subscription records.
