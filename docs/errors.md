# Error Codes and Handling

This document defines the canonical error taxonomy for `subscription_vault` and the stable numeric codes clients should use for UX, retry policy, alerting, and backend integration.

> [!IMPORTANT]
> Breaking changes
> Legacy mixed codes such as `400`, `401`, `404`, `429`, and `10xx` have been consolidated into stable category ranges. Clients must match against the new canonical codes from [`contracts/subscription_vault/src/types.rs`](../contracts/subscription_vault/src/types.rs).

## Taxonomy

- `1000-1099` Auth: caller identity or permission failure.
- `2000-2099` Not found: missing resource or missing initialization.
- `3000-3099` Invalid args: caller supplied invalid input.
- `4000-4099` State transition: lifecycle, replay, emergency-stop, or other state conflict.
- `5000-5099` Accounting: balance, arithmetic, and pricing failures.
- `6000-6099` Limits: caps, quotas, pagination limits, and throttles.

## Canonical Table

| Code | Variant | Category | Meaning | Recommended handling |
|---|---|---|---|---|
| 1001 | `Unauthorized` | Auth | Required signer or admin identity mismatch. | Do not retry unchanged. Rebuild request with the correct signer. |
| 1002 | `Forbidden` | Auth | Caller is authenticated but not allowed for this resource. | Do not retry unchanged. Surface permission error. |
| 1003 | `SubscriberBlocklisted` | Auth | Subscriber is blocklisted from protected flows. | Stop retrying. Escalate to support/admin flow. |
| 1004 | `SelfRotation` | Auth | Admin rotation target equals current admin. | Fix request payload. |
| 2001 | `NotFound` | Not found | Requested subscription, token metadata, blocklist entry, or similar record is missing. | Verify identifiers before retrying. |
| 2002 | `NotInitialized` | Not found | Contract or config has not been initialized. | Admin setup required before retrying. |
| 3001 | `InvalidAmount` | Invalid args | Amount is zero, negative, or otherwise structurally invalid. | Fix input. No automatic retry. |
| 3002 | `InvalidInput` | Invalid args | Generic caller input validation failure. | Fix request parameters. |
| 3003 | `InvalidRecoveryAmount` | Invalid args | Recovery amount is zero or negative. | Fix input. |
| 3004 | `InvalidNewAdmin` | Invalid args | Proposed admin address is invalid for rotation. | Fix request payload. |
| 3005 | `MetadataKeyTooLong` | Invalid args | Metadata key exceeds length limit. | Trim key and retry. |
| 3006 | `MetadataValueTooLong` | Invalid args | Metadata value exceeds length limit. | Trim value and retry. |
| 3007 | `OraclePriceInvalid` | Invalid args | Oracle returned a non-positive price. | Treat as terminal for this request; investigate oracle data. |
| 4001 | `InvalidStatusTransition` | State transition | Requested lifecycle transition is not legal from current status. | Refresh state before presenting next action. |
| 4002 | `NotActive` | State transition | Operation requires an active subscription state. | Refresh state; do not blindly retry. |
| 4003 | `SubscriptionExpired` | State transition | Subscription has expired. | Stop retrying mutating operations on this subscription. |
| 4004 | `IntervalNotElapsed` | State transition | Interval charge attempted too early. | Safe to retry after the next eligible timestamp only. |
| 4005 | `Replay` | State transition | Duplicate charge/recovery/reference was detected. | Treat as idempotent duplicate. Do not retry with a new key for the same action. |
| 4006 | `RecoveryNotAllowed` | State transition | Recovery flow is not allowed in the current context. | Stop and inspect state/policy. |
| 4007 | `EmergencyStopActive` | State transition | Emergency stop blocks critical mutations. | Pause writes until admin clears emergency stop. |
| 4008 | `AlreadyInitialized` | State transition | Contract init was called more than once. | Do not retry. |
| 4009 | `MerchantPaused` | State transition | Merchant-wide pause blocks this action. | Retry only after merchant pause is removed. |
| 4010 | `Reentrancy` | State transition | Reentrancy guard detected a nested call. | Treat as security failure and investigate immediately. |
| 5001 | `InsufficientBalance` | Accounting | Vault, merchant, or refundable balance is insufficient. | Safe to retry only after balances change. |
| 5002 | `InsufficientPrepaidBalance` | Accounting | Usage charge exceeds prepaid balance. | Top up first, then retry. |
| 5003 | `BelowMinimumTopup` | Accounting | Deposit/top-up is below configured threshold. | Increase amount and retry. |
| 5004 | `Underflow` | Accounting | Arithmetic underflow or negative-balance invariant violation. | Treat as terminal and investigate; not user-retriable. |
| 5005 | `Overflow` | Accounting | Arithmetic overflow or counter overflow. | Treat as terminal and investigate; not user-retriable. |
| 5006 | `OracleNotConfigured` | Accounting | Oracle pricing is enabled but no oracle address is configured. | Admin/configuration fix required. |
| 5007 | `OraclePriceUnavailable` | Accounting | Oracle payload is missing or malformed. | Retry only after oracle data recovers. |
| 5008 | `OraclePriceStale` | Accounting | Oracle quote is older than allowed max age. | Retry only after a fresh quote exists. |
| 6001 | `SubscriptionLimitReached` | Limits | Subscription ID space has been exhausted. | Treat as terminal capacity failure. |
| 6002 | `LifetimeCapReached` | Limits | Lifetime charge cap is exhausted or would be exceeded. | Stop charging; surface terminal state to user. |
| 6003 | `UsageNotEnabled` | Limits | Usage charge attempted on a non-usage subscription. | Fix request or subscription type. |
| 6004 | `InvalidExportLimit` | Limits | Export/list limit is outside allowed bounds. | Fix pagination limit. |
| 6005 | `MetadataKeyLimitReached` | Limits | Metadata key quota is exhausted. | Delete/update keys before retrying. |
| 6006 | `MaxConcurrentSubscriptionsReached` | Limits | Subscriber already has maximum active subscriptions for the plan. | Stop and surface quota state. |
| 6007 | `CreditLimitExceeded` | Limits | Requested liability exceeds subscriber credit limit. | Reduce exposure or raise limit before retrying. |
| 6008 | `RateLimitExceeded` | Limits | Usage rate limit exceeded in current window. | Retry after the rate window resets. |
| 6009 | `UsageCapExceeded` | Limits | Usage cap would be exceeded for the billing period. | Retry only after a new billing period or cap change. |
| 6010 | `BurstLimitExceeded` | Limits | Usage call arrived too soon after prior call. | Retry after the minimum interval elapses. |

## Retry Guidance

- Safe to retry later: `IntervalNotElapsed`, `EmergencyStopActive`, `OraclePriceStale`, `OraclePriceUnavailable`, `RateLimitExceeded`, `BurstLimitExceeded`, `InsufficientBalance`, `InsufficientPrepaidBalance`.
- Safe only after request changes: `Unauthorized`, `Forbidden`, `InvalidAmount`, `InvalidInput`, `InvalidExportLimit`, `UsageNotEnabled`, `CreditLimitExceeded`, `MaxConcurrentSubscriptionsReached`.
- Treat as idempotent duplicate, not a fresh retry: `Replay`.
- Treat as terminal and operator-visible: `Overflow`, `Underflow`, `Reentrancy`, `SubscriptionLimitReached`, `LifetimeCapReached`.

## Security Notes

- Errors are intentionally coarse and must not leak sensitive internal balances beyond already-public business state.
- Charging paths avoid ambiguous reverted errors when a lifetime-cap overrun must persist a cancellation. In those cases the contract may return a semantic success/result while batch interfaces still map the condition to stable code `6002`.
- Never auto-retry a charge after `Replay`, `LifetimeCapReached`, `NotActive`, or `SubscriptionExpired`.
- Client payment UX should distinguish �insufficient balance� from �request rejected� to avoid duplicate funding or duplicate charge attempts.

## Source of Truth

- Enum and numeric assignments: [`contracts/subscription_vault/src/types.rs`](../contracts/subscription_vault/src/types.rs)
- Batch charge error-code mapping: [`contracts/subscription_vault/src/admin.rs`](../contracts/subscription_vault/src/admin.rs)
- Core charge semantics: [`contracts/subscription_vault/src/charge_core.rs`](../contracts/subscription_vault/src/charge_core.rs)
