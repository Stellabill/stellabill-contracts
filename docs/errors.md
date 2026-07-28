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
- `7000-7099` Merchant config: fee, operations, and merchant configuration.
- `8000-8099` Token: token acceptance and decimals.
- `9000-9099` Subscription update: usage mode changes.
- `9100-9199` Schema migration: version compatibility.
- `10000-10099` Dispute/chargeback: dispute lifecycle errors.

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
| 10001 | `DisputeNotFound` | Not found | No dispute for the given ID. | Verify dispute ID. |
| 10002 | `DisputeAlreadyResolved` | State transition | Dispute has already been resolved. | Do not retry; inspect resolution. |
| 10003 | `DisputeNotResponded` | State transition | Cannot resolve an unresponded dispute before window elapses. | Retry after admin responds or window elapses. |
| 10004 | `DisputeWindowElapsed` | State transition | Dispute window has elapsed. | Check resolution rules. |
| 10005 | `DisputeAlreadyOpen` | State transition | A dispute is already open for this subscription. | Wait for resolution or inspect existing dispute. |
| 10006 | `DisputeAlreadyResponded` | State transition | Dispute is not in `Open` status. | Cannot respond twice. |

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

<!-- GENERATED:entrypoint-table:start -->
## Entrypoint Cross-Reference

This table is **generated** by `scripts/generate_error_table.py` and kept in sync
by CI (see `.github/workflows/docs.yml`). Do not edit the block between the
sentinel comments manually — run the script instead.

Column definitions:
- **Emitting entrypoints**: source modules that contain `Error::<Variant>`.
  The public entrypoint name as exposed in `lib.rs` is listed where it differs
  from the internal module name.
- **Recovery action**: recommended remediation for integrators.
- **Related event**: Soroban event type emitted alongside this error, where applicable.

| Code | Variant | Category | Emitting entrypoints (modules) | Recovery action | Related event |
|---:|:---|:---|:---|:---|:---|
| 1001 | `Unauthorized` | Auth | `admin.rs`, `coupon.rs`, `dispute.rs`, `governance.rs`, `lib.rs`, `subscription.rs`, `test.rs`, `test_admin_transfer_auth.rs`, `test_bulk_admin_ops.rs`, `test_coupon.rs`, `test_governance.rs`, `test_merchant_whitelist.rs`, `test_recovery.rs`, `test_require_auth.rs`, `test_security.rs`, `test_subscriber_active_cap.rs` | Rebuild request with correct signer; do not retry unchanged. | AdminRotatedEvent (if admin changed) |
| 1002 | `Forbidden` | Auth | `lib.rs`, `metadata.rs`, `subscription.rs`, `test.rs`, `test_require_auth.rs`, `test_scheduled_cancel.rs` | Surface permission error; caller authenticated but not authorised for resource. | — |
| 1003 | `SubscriberBlocklisted` | Auth | `blocklist.rs`, `test.rs` | Escalate to admin/support flow; stop retrying. | BlocklistAddedEvent |
| 1004 | `SelfRotation` | Auth | `admin.rs`, `test.rs`, `test_admin_transfer_auth.rs`, `test_governance.rs` | Fix request payload — new_admin must differ from current_admin. | — |
| 1005 | `NonceAlreadyUsed` | Auth | `lib.rs`, `metadata.rs`, `nonce.rs`, `test.rs`, `test_bulk_admin_ops.rs`, `test_governance.rs`, `test_metadata_signed.rs`, `test_nonce_domains.rs`, `test_operator.rs` | Re-fetch nonce via get_admin_nonce / get_operator_nonce, then retry. | NonceConsumedEvent |
| 1006 | `BatchTooLarge` | Auth | `lib.rs`, `subscription.rs`, `test_bulk_admin_ops.rs` | Reduce batch size and retry; check BATCH_MAX_SIZE. | — |
| 2001 | `NotFound` | Not Found | `admin.rs`, `blocklist.rs`, `governance.rs`, `lib.rs`, `merchant.rs`, `metadata.rs`, `queries.rs`, `subscription.rs`, `test.rs`, `test_bulk_admin_ops.rs`, `test_governance.rs`, `test_metadata_signed.rs`, `test_reentrancy_invariants.rs`, `test_require_auth.rs` | Verify identifiers before retrying. | — |
| 2002 | `NotInitialized` | Not Found | `admin.rs` | Admin must call init before any other operation. | — |
| 3001 | `InvalidAmount` | Invalid Args | `accounting.rs`, `admin.rs`, `charge_core.rs`, `dispute.rs`, `lib.rs`, `merchant.rs`, `subscription.rs`, `test.rs`, `test_reentrancy_invariants.rs`, `test_require_auth.rs` | Fix input; amount must be > 0. | — |
| 3002 | `InvalidInput` | Invalid Args | `admin.rs`, `coupon.rs`, `governance.rs`, `lib.rs`, `merchant.rs`, `metadata.rs`, `oracle_adapter.rs`, `period_snapshots.rs`, `queries.rs`, `subscription.rs`, `test.rs`, `test_abi_validators_integration.rs`, `test_billing_period_snapshots.rs`, `test_coupon.rs`, `test_decimal_normalization.rs`, `test_metadata_signed.rs`, `test_operator.rs`, `test_scheduled_cancel.rs`, `test_validation.rs`, `validation.rs` | Fix request parameters. | — |
| 3003 | `InvalidRecoveryAmount` | Invalid Args | `admin.rs`, `test_recovery.rs` | Fix amount; must be > 0. | — |
| 3004 | `InvalidNewAdmin` | Invalid Args | `admin.rs`, `test.rs`, `test_admin_transfer_auth.rs`, `test_governance.rs` | Fix payload; new_admin must not equal contract address. | — |
| 3005 | `MetadataKeyTooLong` | Invalid Args | `lib.rs`, `metadata.rs`, `test_metadata_signed.rs` | Trim key to ≤ MAX_METADATA_KEY_LENGTH bytes and retry. | — |
| 3006 | `MetadataValueTooLong` | Invalid Args | `lib.rs`, `metadata.rs`, `test_metadata_signed.rs` | Trim value to ≤ MAX_METADATA_VALUE_LENGTH bytes and retry. | — |
| 3007 | `OraclePriceInvalid` | Invalid Args | `oracle_adapter.rs`, `test.rs` | Treat as terminal for this request; investigate oracle data feed. | OracleConfigUpdatedEvent |
| 3008 | `InvalidExpiration` | Invalid Args | `subscription.rs`, `test_expiration.rs` | Fix expiration timestamp; must be strictly in the future. | — |
| 4001 | `InvalidStatusTransition` | State Transition | `lib.rs`, `period_snapshots.rs`, `state_machine.rs`, `subscription.rs`, `test.rs`, `test_billing_period_snapshots.rs` | Refresh subscription state before presenting the next action. | — |
| 4002 | `NotActive` | State Transition | `charge_core.rs`, `subscription.rs`, `test.rs`, `test_operator.rs`, `test_reentrancy_invariants.rs`, `test_subscription_status_transitions.rs` | Refresh state; do not blindly retry. | — |
| 4003 | `SubscriptionExpired` | State Transition | `charge_core.rs`, `subscription.rs`, `test_bulk_admin_ops.rs`, `test_expiration.rs` | Stop retrying mutating operations on this subscription. | SubscriptionExpiredEvent |
| 4004 | `IntervalNotElapsed` | State Transition | `charge_core.rs`, `merchant.rs`, `test.rs`, `test_interval_boundary.rs`, `test_payout_schedule.rs` | Retry only after next_charge_timestamp reported by get_next_charge_info. | — |
| 4005 | `Replay` | State Transition | `admin.rs`, `charge_core.rs`, `test.rs`, `test_recovery.rs`, `test_reentrancy_invariants.rs` | Treat as idempotent duplicate; do not retry with a new key for the same action. | — |
| 4006 | `RecoveryNotAllowed` | State Transition | `lib.rs` | Stop and inspect subscription state or policy before retrying. | RecoveryEvent |
| 4007 | `EmergencyStopActive` | State Transition | `lib.rs`, `test.rs`, `test_emergency_stop_lifetime_caps.rs`, `test_emergency_stop_matrix.rs`, `test_operator.rs`, `test_reentrancy_invariants.rs` | Pause writes; poll get_emergency_stop_status and retry after admin clears stop. | EmergencyStopDisabledEvent |
| 4008 | `AlreadyInitialized` | State Transition | `admin.rs`, `test.rs` | Do not retry; contract is already set up. | — |
| 4009 | `MerchantPaused` | State Transition | `charge_core.rs`, `subscription.rs` | Retry only after merchant pause is removed (unpause_merchant). | MerchantUnpausedEvent |
| 4010 | `Reentrancy` | State Transition | `reentrancy.rs` | Treat as a security failure; investigate calling path immediately. | — |
| 4011 | `NotInGracePeriod` | State Transition | `subscription.rs`, `test_grace_buyout.rs` | Only subscriptions in GracePeriod can use this operation; resume or wait for grace. | SubscriptionResumedEvent |
| 5001 | `InsufficientBalance` | Accounting | `admin.rs`, `dispute.rs`, `lib.rs`, `merchant.rs`, `subscription.rs`, `test.rs`, `test_grace_buyout.rs`, `test_recovery.rs`, `test_reentrancy_invariants.rs` | Retry only after subscriber deposits funds via deposit_funds. | FundsDepositedEvent |
| 5002 | `InsufficientPrepaidBalance` | Accounting | `charge_core.rs`, `subscription.rs`, `test.rs` | Top up subscription via deposit_funds, then retry. | FundsDepositedEvent |
| 5003 | `BelowMinimumTopup` | Accounting | `subscription.rs`, `test.rs` | Increase deposit amount above get_min_topup() threshold and retry. | — |
| 5004 | `Underflow` | Accounting | `accounting.rs`, `admin.rs`, `dispute.rs`, `merchant.rs`, `safe_math.rs`, `test_security.rs` | Treat as terminal; investigate accounting invariant violation; not user-retriable. | — |
| 5005 | `Overflow` | Accounting | `accounting.rs`, `charge_core.rs`, `dispute.rs`, `governance.rs`, `lib.rs`, `merchant.rs`, `metadata.rs`, `nonce.rs`, `period_snapshots.rs`, `safe_math.rs`, `subscription.rs`, `test.rs`, `test_decimal_normalization.rs`, `test_grace_buyout.rs`, `test_metadata_signed.rs`, `test_nonce_domains.rs`, `test_security.rs` | Treat as terminal; investigate arithmetic overflow; not user-retriable. | — |
| 5006 | `OracleNotConfigured` | Accounting | `lib.rs`, `oracle_adapter.rs`, `test.rs`, `test_oracle_liveness.rs` | Admin must call set_oracle_config with a valid oracle address. | OracleConfigUpdatedEvent |
| 5007 | `OraclePriceUnavailable` | Accounting | `oracle_adapter.rs`, `test.rs` | Retry only after oracle data feed recovers. | OracleChargeResolvedEvent |
| 5008 | `OraclePriceStale` | Accounting | `oracle_adapter.rs`, `test.rs` | Retry only after a fresh oracle quote is published. | OracleChargeResolvedEvent |
| 6001 | `SubscriptionLimitReached` | Limits | `lib.rs`, `subscription.rs`, `test.rs` | Treat as terminal capacity failure; no new subscriptions can be created. | — |
| 6002 | `LifetimeCapReached` | Limits | `admin.rs`, `charge_core.rs`, `subscription.rs`, `test.rs`, `test_emergency_stop_lifetime_caps.rs` | Stop charging; surface terminal state to user. | LifetimeCapReachedEvent |
| 6003 | `UsageNotEnabled` | Limits | `charge_core.rs`, `subscription.rs`, `test.rs` | Fix request — subscription was created with usage_enabled=false. | — |
| 6004 | `InvalidExportLimit` | Limits | `lib.rs` | Fix pagination limit to [1, 100]. | — |
| 6005 | `MetadataKeyLimitReached` | Limits | `lib.rs`, `metadata.rs`, `test_metadata_signed.rs` | Delete or update existing keys (up to MAX_METADATA_KEYS) before retrying. | MetadataDeletedEvent |
| 6006 | `MaxConcurrentSubscriptionsReached` | Limits | `subscription.rs`, `test.rs`, `test_subscriber_active_cap.rs` | Subscriber already at plan concurrency limit; cancel an existing subscription first. | SubscriptionCancelledEvent |
| 6007 | `CreditLimitExceeded` | Limits | `subscription.rs`, `test.rs`, `test_insufficient_balance.rs` | Reduce deposit / subscription amount or raise limit via set_subscriber_credit_limit. | — |
| 6008 | `RateLimitExceeded` | Limits | — | Retry after the rate window resets (see configure_usage_limits). | UsageLimitsConfiguredEvent |
| 6009 | `UsageCapExceeded` | Limits | — | Retry only after new billing period begins or cap is raised. | UsageLimitsConfiguredEvent |
| 6010 | `BurstLimitExceeded` | Limits | — | Retry after burst_min_interval_secs elapses. | UsageLimitsConfiguredEvent |
| 6011 | `CouponNotFound` | Limits | `coupon.rs` | Verify the coupon code before retrying. | — |
| 6012 | `CouponExpired` | Limits | `coupon.rs`, `test_coupon.rs` | Coupon has expired; request a new coupon from the merchant. | CouponCreatedEvent |
| 6013 | `CouponRedemptionLimitReached` | Limits | `coupon.rs`, `test_coupon.rs` | Coupon has reached its global redemption limit; merchant may create a new coupon. | CouponCreatedEvent |
| 6014 | `CouponRevoked` | Limits | `coupon.rs`, `test_coupon.rs` | Coupon was revoked by the merchant; choose a different coupon. | CouponRevokedEvent |
| 6015 | `CouponAlreadyExists` | Limits | `coupon.rs`, `test_coupon.rs` | Choose a different coupon code and retry. | CouponCreatedEvent |
| 6016 | `CouponAlreadyApplied` | Limits | `coupon.rs`, `test_coupon.rs` | Subscription already has a coupon bound; only one coupon per subscription. | CouponAppliedEvent |
| 6017 | `CouponTokenMismatch` | Limits | `coupon.rs`, `test_coupon.rs` | Use a coupon whose token matches the subscription's settlement token. | — |
| 6019 | `SubscriberRateLimited` | Limits | `subscription.rs`, `test.rs` | Subscriber exceeded 24h subscription creation limit; retry after window resets. | RateLimitTrippedEvent |
| 6020 | `UsageLimitsRequired` | Limits | `subscription.rs`, `test_usage_limits_required.rs` | Configure usage limits via configure_usage_limits before creating usage-enabled subscriptions. | UsageLimitsConfiguredEvent |
| 7001 | `InvalidFeeBips` | Merchant Config | `merchant.rs` | Fix fee_bips to be in range [0, 10000]. | MerchantConfigUpdatedEvent |
| 7002 | `InvalidOperations` | Merchant Config | `merchant.rs` | Fix allowed_operations bitmap to use only valid OP_* bits. | MerchantConfigUpdatedEvent |
| 7003 | `MustAllowChargeOperation` | Merchant Config | `merchant.rs` | Set OP_CHARGE bit in allowed_operations; merchants must accept charges. | MerchantConfigUpdatedEvent |
| 7004 | `MerchantNotApproved` | Merchant Config | `merchant.rs`, `test_merchant_whitelist.rs` | Merchant is not whitelisted; admin must approve via merchant config. | MerchantConfigInitializedEvent |
| 8001 | `InvalidTokenDecimals` | Token | `admin.rs`, `test_decimal_normalization.rs` | Fix token_decimals; must be in [1, 19]. | — |
| 8002 | `InvalidToken` | Token | `admin.rs`, `test_decimal_normalization.rs` | Provide an accepted token address from list_accepted_tokens. | — |
| 9001 | `CannotChangeUsageMode` | Subscription Update | `subscription.rs` | Cannot toggle usage_enabled on an existing subscription; create a new one. | — |
| 9101 | `SchemaMigrationDowngrade` | Schema Migration | `admin.rs`, `test.rs`, `test_config_migration.rs` | Downgrade rejected; deploy the correct binary version. | SchemaMigratedEvent |
| 10001 | `DisputeNotFound` | Dispute | `dispute.rs`, `lib.rs`, `test.rs`, `test_dispute_matrix.rs` | Verify dispute ID before retrying. | — |
| 10002 | `DisputeAlreadyResolved` | Dispute | `dispute.rs`, `lib.rs`, `test.rs`, `test_dispute_matrix.rs` | Inspect existing resolution; do not retry. | DisputeResolvedEvent |
| 10003 | `DisputeNotResponded` | Dispute | `dispute.rs`, `lib.rs`, `test.rs`, `test_dispute_matrix.rs` | Wait for admin response or dispute window to elapse. | DisputeRespondedEvent |
| 10004 | `DisputeWindowElapsed` | Dispute | — | Check auto-resolution rules; dispute can now be resolved. | — |
| 10005 | `DisputeAlreadyOpen` | Dispute | `dispute.rs`, `lib.rs`, `test.rs` | A dispute is already open for this subscription; wait for resolution. | DisputeOpenedEvent |
| 10006 | `DisputeAlreadyResponded` | Dispute | `dispute.rs`, `lib.rs`, `test.rs`, `test_dispute_matrix.rs` | Dispute is not in `Open` status; cannot respond twice. | DisputeRespondedEvent |
| 11001 | `TransferIntentNotFound` | Unknown | `subscription.rs`, `test_subscription_transfer.rs` | Verify transfer initiation or expiry before retrying. | — |
| 11002 | `TransferIntentExpired` | Unknown | `subscription.rs`, `test_subscription_transfer.rs` | Transfer intent has expired; initiate a new transfer. | — |
| 11003 | `InvalidTransferTarget` | Unknown | `subscription.rs`, `test_subscription_transfer.rs` | Provide a valid target address (not self). | — |
<!-- GENERATED:entrypoint-table:end -->
