# Merchant Blocklist

## Overview
The Merchant Blocklist feature allows merchants to prevent specific subscribers from creating new subscriptions to their plans or depositing funds into existing subscriptions. This helps merchants manage malicious users or manage compliance requirements. The blocklist is enforced on a per-merchant basis, meaning a subscriber blocked by Merchant A can still freely subscribe to Merchant B.

## Storage
The blocklist status is stored in the contract's instance storage using the `DataKey::MerchantBlocklist(Address, Address)` enum variant. The value stored is a boolean (`true` if blocked, `false` or absent if not blocked).

## Enforcements
The blocklist is strictly enforced during key lifecycle events:
1. **Subscription Creation:** `create_subscription` and `create_subscription_from_plan` check the blocklist. If the subscriber is blocked, the contract returns `Error::SubscriberBlocked` (1022).
2. **Fund Deposits:** `deposit_funds` ensures that blocked subscribers cannot artificially prolong their existing subscriptions by topping them up.

*Note: Existing active subscriptions belonging to a newly blocked user are not automatically cancelled. They will naturally expire when they run out of funds.*

## Interfaces
The following public functions are exposed in the contract:
- `block_subscriber(env: Env, merchant: Address, subscriber: Address)`: Blocks a subscriber. Requires `merchant` authorization.
- `unblock_subscriber(env: Env, merchant: Address, subscriber: Address)`: Unblocks a subscriber. Requires `merchant` authorization.
- `is_subscriber_blocked(env: Env, merchant: Address, subscriber: Address) -> bool`: Query endpoint to check if a subscriber is currently blocked by the specified merchant.

## Events
When a subscriber's blocklist status changes (either blocked or unblocked), the contract emits a `MerchantBlocklistUpdatedEvent`.
```rust
pub struct MerchantBlocklistUpdatedEvent {
    pub merchant: Address,
    pub subscriber: Address,
    pub is_blocked: bool,
    pub timestamp: u64,
}
```
