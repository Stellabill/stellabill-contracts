# Grace Period support

The Subscription Vault supports configurable grace periods to provide subscribers an allowance window when their prepaid balance falls below the required amount for their recurring charge. This allows users to retain their active integration without sudden cancellations, while temporarily marking the subscription in a `GracePeriod` status.

## Configuration

A global `grace_period` duration (in seconds) can be set during initialization by the `admin`.

```rust
pub fn init(env: Env, token: Address, admin: Address, min_topup: i128, grace_period_duration: u64)
```

The admin can modify this duration explicitly:
```rust
pub fn set_grace_period(env: Env, admin: Address, grace_period: u64)
```

## Behavior and Status Transitions

1. **Failure during `Active` state**
   If a successful `charge_subscription` attempt (either single or batched) encounters `prepaid_balance < amount`, the contract normally sets the status to `InsufficientBalance` (Suspended).
   
   However, if `grace_period` is > 0, the contract identifies the **expiration window** (`last_payment_timestamp + interval_seconds + grace_period`). If the current ledger time is within this window, the subscription falls into a `GracePeriod` status instead.
   
2. **Charges in `GracePeriod`**
   During the grace windows, the vault allows merchants/CRON engines to continually retry `charge_subscription`. Repeated failures within the grace window bounds will safely maintain the status as `GracePeriod` and return an `InsufficientBalance` error flag without canceling the subscription.

3. **Recovery**
   A subscriber can deposit funds anytime using `deposit_funds`. This process does not alter the status explicitly, but on the *subsequent retry* of `charge_subscription`, the process will successfully deduct the balance, update the `last_payment_timestamp` to the current ledger time, and transition the user back to the `Active` status seamlessly!

4. **Expiration (Suspension)**
   If repeated failures or `batch_charge` cron invocations attempt to charge the subscription pass the expiration window, the contract will firmly transition the subscription to `InsufficientBalance`, blocking access to any linked `usage_enabled` properties dependent on `GracePeriod` or `Active`.

## Integrator/Merchant Advice
Integrators and merchants should evaluate `SubscriptionStatus::GracePeriod` as a yellow-flag status. UX properties could potentially read:
- Displaying a warning "Payment failed! Please top-up within X days to retain your service."
- Restricting premium functions or adjusting the quality of service while in the grace parameter. 
- Using Soroban Events/Webhook indexing to notify subscribers prior to full suspension.
