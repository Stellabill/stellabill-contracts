## Global subscriber credit limits

Subscribers can hold multiple subscriptions in the vault, potentially across many plans and
merchants. To prevent overextension, the vault supports per-subscriber credit limits that
bound aggregate exposure across subscriptions for a given settlement token.

### Data model

Credit limits are configured per `(subscriber, token)` pair:

- `limit` – maximum allowed exposure in token base units (e.g. 1 USDC = 1_000_000 for 6 decimals).
- `limit = 0` – no limit is enforced for that subscriber/token.

Exposure is computed as:

- `sum(prepaid_balance)` for all subscriptions owned by the subscriber using the token, plus
- `sum(amount)` for subscriptions in the `Active` state (the next-interval liability).

This provides a conservative bound on how much value is either already locked or expected to be
charged in the near term.

### Configuration entrypoints

- `set_subscriber_credit_limit(admin, subscriber, token, limit)`  
  Sets or updates the credit limit for a `subscriber` and settlement `token`. Only the contract
  admin may call this. Passing `limit = 0` clears any effective cap (no limit).

- `get_subscriber_credit_limit(subscriber, token) -> i128`  
  Returns the configured limit, or `0` when none is set.

### Enforcement points

Credit limits are enforced before new liabilities are introduced:

- **Subscription creation**
  - `create_subscription`
  - `create_subscription_with_token`
  - `create_subscription_from_plan`
  For each of these, the vault checks that `current_exposure + amount <= limit`. If the
  limit would be exceeded, the operation returns `Error::CreditLimitExceeded` and no
  state changes occur.

- **Top-ups**
  - `deposit_funds`
  Deposits increase exposure by the deposit amount. The contract rejects deposits that would
  cause `current_exposure + deposit_amount` to exceed the configured limit.

Existing subscriptions continue to function (charges, cancellations, withdrawals) even when the
limit is reached; only new subscriptions and additional prepaid exposure are blocked.

### View helpers

- `get_subscriber_exposure(subscriber, token) -> i128`  
  Returns the current aggregate exposure used for limit checks. This is suitable for dashboards
  and pre-flight checks in frontends.

### UX and risk policy guidance

- **Choosing limits**
  - For conservative risk, set limits close to the expected total recurring liability across
    all plans (e.g. 1–3 billing intervals worth of charges).
  - For high-trust subscribers, either use a large limit or `0` (no limit).

- **Frontend behavior**
  - Before attempting subscription creation or deposit, frontends can fetch both
    `get_subscriber_credit_limit` and `get_subscriber_exposure` to give users clear guidance
    on how much additional exposure is allowed.
  - When `Error::CreditLimitExceeded` is returned, surface the limit and current exposure to
    explain why the operation failed and what adjustments are needed (e.g. lower amount, cancel
    other subscriptions, or raise the limit).

## Per-merchant maximum active subscriptions cap

To prevent storage exhaustion by buggy or malicious merchants, the contract admin can set a maximum active subscriptions limit per merchant.

### Data model

The limit is defined per merchant:
- `max_subs` – maximum allowed active (non-cancelled) subscriptions.
- `max_subs = u32::MAX` – no limit is enforced (default).

Active subscription count is the count of non-cancelled subscriptions for a given merchant.

### Configuration entrypoints

- `set_merchant_max_subs(admin, merchant, max_subs)`  
  Sets or updates the limit for a merchant. Only the contract admin may call this. Passing `u32::MAX` clears the cap (no limit).

- `get_merchant_max_subs(merchant) -> u32`  
  Returns the configured limit, or `u32::MAX` if none is set.

### Enforcement points

- **Subscription creation**
  Both direct subscription creation (`create_subscription`, `create_subscription_with_token`) and plan template-based creation (`create_subscription_from_plan`) check that the merchant's current active subscription count is less than `max_subs`.
  If the cap is reached, the transaction is rejected with `Error::MaxConcurrentSubscriptionsReached`.

- **Cancellations**
  When a subscription is cancelled, it is removed from the merchant index, freeing up a subscription slot for the merchant.

### Interaction with PlanMaxActive

When a subscription is created from a plan template, two caps are evaluated:
1. `PlanMaxActive`: The maximum active subscriptions a single subscriber is allowed to have on that plan template.
2. `MerchantMaxSubs`: The global active subscriptions a merchant is allowed to have across all plans.

These two limits interact additively, and **whichever cap is lower/stricter wins**:
- If `PlanMaxActive` is met for a subscriber, they cannot subscribe to that plan even if the merchant has global capacity remaining under `MerchantMaxSubs`.
- If the merchant has reached `MerchantMaxSubs` globally, no user can create any new subscriptions under that merchant, even if the subscriber has not reached their individual `PlanMaxActive` limit.

