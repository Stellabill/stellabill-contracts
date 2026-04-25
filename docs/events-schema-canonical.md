# Canonical Event Schema Reference

This document defines all stable event schemas emitted by the subscription vault contract. Events are indexed and emitted exactly once per successful state transition or fund movement.

## Event Emission Strategy

All events follow this pattern:

```rust
env.events().publish(
    (Symbol::new(env, "event_name"), subscription_id_or_resource_id),
    EventStructWithAllFields {
        ...
    },
);
```

**Key Principles:**
- Topic tuple format: `(Symbol, optional_id)` — enables filtering by event type and resource
- Data struct: always a named type (never raw tuples) — provides stable schema for indexers
- Timestamp: included when useful for ordering/metrics
- No sensitive data: all event fields are customer-visible
- One per action: ensures deterministic batch operation ordering

## Lifecycle Events

### SubscriptionCreatedEvent

**Topic:** `("created", subscription_id)`

Emitted when a subscription is created via `create_subscription()` or `create_subscription_from_plan()`.

**Fields:**
- `subscription_id`: u32 — unique subscription identifier
- `subscriber`: Address — subscriber account
- `merchant`: Address — merchant account receiving payments
- `amount`: i128 — recurring charge amount per billing interval
- `interval_seconds`: u64 — billing interval in seconds
- `lifetime_cap`: Option<i128> — optional maximum total charge limit
- `expires_at`: Option<u64> — optional expiration timestamp

**Example Use Cases:**
- Build subscription graph in explorer
- Display user's active subscriptions
- Calculate subscription creation metrics

---

### FundsDepositedEvent

**Topic:** `("deposited", subscription_id)`

Emitted when a subscriber deposits funds into a subscription vault via `deposit_funds()`.

**Fields:**
- `subscription_id`: u32
- `subscriber`: Address
- `amount`: i128 — amount deposited (in token base units)
- `prepaid_balance`: i128 — total prepaid balance after deposit

**Example Use Cases:**
- Display deposit history in subscriber UI
- Monitor prepaid balance trends
- Trigger low-balance alerts

---

## Charging Events

### SubscriptionChargedEvent

**Topic:** `("charged",)` *(no subscription ID in topic)*

Emitted when an interval-based charge succeeds via `charge_subscription()` or `batch_charge()`.

**Fields:**
- `subscription_id`: u32
- `merchant`: Address — merchant receiving the payment
- `amount`: i128 — total charged amount
- `lifetime_charged`: i128 — cumulative total charged over subscription lifetime

**Example Use Cases:**
- Generate merchant revenue reports
- Track subscription payment history
- Aggregate merchant earnings by token

---

### SubscriptionChargeFailedEvent

**Topic:** `("charge_failed", subscription_id)`

Emitted when a charge attempt fails due to insufficient balance. Subscription transitions to `InsufficientBalance` or `GracePeriod` status.

**Fields:**
- `subscription_id`: u32
- `merchant`: Address
- `required_amount`: i128 — amount that was attempted to be charged
- `available_balance`: i128 — prepaid balance at time of charge
- `shortfall`: i128 — required_amount - available_balance
- `resulting_status`: SubscriptionStatus — status after failure (GracePeriod or InsufficientBalance)
- `timestamp`: u64

**Example Use Cases:**
- Send notifications when subscription enters non-active states
- Track recovery candidates for re-engagement
- Monitor subscription health metrics

---

### OneOffChargedEvent

**Topic:** `("one_off_charged", subscription_id)`

Emitted when a merchant initiates an off-interval charge via `charge_one_off()`.

**Fields:**
- `subscription_id`: u32
- `merchant`: Address
- `amount`: i128

---

### ProtocolFeeChargedEvent

**Topic:** `("protocol_fee_charged", subscription_id)`

Emitted when protocol fees are extracted and routed to treasury during `charge_subscription()` batch operations.

**Fields:**
- `subscription_id`: u32
- `treasury`: Address — fee recipient
- `fee_amount`: i128 — amount routed to treasury
- `merchant_amount`: i128 — amount retained by merchant after fee
- `timestamp`: u64

---

### LifetimeCapReachedEvent

**Topic:** `("lifetime_cap_reached", subscription_id)`

Emitted when a subscription's lifetime charge cap is exhausted. Subscription is automatically cancelled.

**Fields:**
- `subscription_id`: u32
- `lifetime_cap`: i128 — configured maximum total charges
- `lifetime_charged`: i128 — total charged at time of cap hit
- `timestamp`: u64

**Example Use Cases:**
- Trigger subscription renewal flows
- Track cap-reached metrics
- Archive completed subscriptions

---

### UsageStatementEvent

**Topic:** `("usage_statement", subscription_id)`

Emitted when usage-based charges are processed via `charge_usage()`.

**Fields:**
- `subscription_id`: u32
- `merchant`: Address
- `usage_amount`: i128 — metered charge amount
- `token`: Address — settlement token
- `timestamp`: u64
- `reference`: String — idempotency reference key

---

## Subscription Status Events

### SubscriptionPausedEvent

**Topic:** `("sub_paused", subscription_id)`

Emitted when a subscription is paused via `pause_subscription()`. No charges processed while paused.

**Fields:**
- `subscription_id`: u32
- `authorizer`: Address — subscriber or merchant who initiated pause

**Example Use Cases:**
- Display subscription pause status
- Analytics on pause/resume cycles
- Update merchant UI

---

### SubscriptionResumedEvent

**Topic:** `("sub_resumed", subscription_id)`

Emitted when a subscription is resumed from Paused/GracePeriod/InsufficientBalance via `resume_subscription()` OR automatically after a successful deposit rebalances an underfunded subscription.

**Fields:**
- `subscription_id`: u32
- `authorizer`: Address — subscriber or merchant who initiated resume

**Example Use Cases:**
- Update subscription status in UI
- Track recovery metrics
- Trigger billing resumption workflows

---

### SubscriptionCancelledEvent

**Topic:** `("subscription_cancelled", subscription_id)`

Emitted when a subscription is cancelled via `cancel_subscription()`. Terminal state.

**Fields:**
- `subscription_id`: u32
- `authorizer`: Address — subscriber or merchant who cancelled
- `refund_amount`: i128 — prepaid balance available for refund

**Example Use Cases:**
- Process refunds to subscribers
- Calculate churn metrics
- Archive cancelled subscriptions

---

### SubscriptionExpiredEvent

**Topic:** `("subscription_expired", subscription_id)`

Emitted when a subscription is automatically expired due to `expires_at` timestamp being reached.

**Fields:**
- `subscription_id`: u32
- `timestamp`: u64

---

### SubscriptionRecoveryReadyEvent

**Topic:** `("recovery_ready", subscription_id)`

Emitted after a deposit when a previously underfunded subscription (`InsufficientBalance` or `GracePeriod`) is restored to sufficient balance and automatically transitions to `Active`.

**Fields:**
- `subscription_id`: u32
- `subscriber`: Address
- `prepaid_balance`: i128
- `required_amount`: i128
- `timestamp`: u64

---

## Withdrawal Events

### MerchantWithdrawalEvent

**Topic:** `("withdrawn", merchant, token)`

Emitted when a merchant withdraws earned funds via `withdraw_merchant_funds()` or `withdraw_merchant_token_funds()`.

**Fields:**
- `merchant`: Address
- `token`: Address — token being withdrawn
- `amount`: i128 — amount withdrawn
- `remaining_balance`: i128 — balance after withdrawal

**Example Use Cases:**
- Track merchant payout history
- Display earnings ledger
- Reconcile merchant balances

---

### PartialRefundEvent

**Topic:** `("partial_refund", subscription_id)`

Emitted when a cancelled subscription has its refund amount transferred back to subscriber.

**Fields:**
- `subscription_id`: u32
- `subscriber`: Address
- `amount`: i128
- `timestamp`: u64

---

### MerchantRefundEvent

**Topic:** `("merchant_refund", merchant)`

Emitted when a merchant refunds a subscriber's balance via `merchant_refund()`.

**Fields:**
- `merchant`: Address
- `subscriber`: Address
- `token`: Address
- `amount`: i128

---

### SubscriberWithdrawalEvent

**Topic:** `("subscriber_withdrawal", subscription_id)`

Emitted when a subscriber withdraws funds from a cancelled subscription.

**Fields:**
- `subscription_id`: u32
- `subscriber`: Address
- `amount`: i128

---

## Admin & Config Events

### EmergencyStopEnabledEvent

**Topic:** `("emergency_stop_enabled",)`

Emitted when admin triggers emergency stop via `enable_emergency_stop()`.

**Fields:**
- `admin`: Address
- `timestamp`: u64

---

### EmergencyStopDisabledEvent

**Topic:** `("emergency_stop_disabled",)`

Emitted when admin disables emergency stop via `disable_emergency_stop()`.

**Fields:**
- `admin`: Address
- `timestamp`: u64

---

### AdminRotatedEvent

**Topic:** `("admin_rotated",)`

Emitted when admin is rotated to a new address via `rotate_admin()`.

**Fields:**
- `old_admin`: Address
- `new_admin`: Address
- `timestamp`: u64

---

### RecoveryEvent

**Topic:** `("recovery",)`

Emitted when admin recovers stranded funds via `recover_stranded_funds()`.

**Fields:**
- `admin`: Address
- `recipient`: Address
- `token`: Address
- `amount`: i128
- `reason`: RecoveryReason enum
- `timestamp`: u64

---

### BillingCompactedEvent

**Topic:** `("billing_compacted",)`

Emitted when billing statement compaction is run via `compact_statements()`.

**Fields:**
- `admin`: Address
- `subscription_id`: u32
- `pruned_count`: u32
- `kept_count`: u32
- `total_pruned_amount`: i128
- `timestamp`: u64
- `aggregate_pruned_count`: u32 — total pruned across all historical compactions
- `aggregate_total_amount`: i128 — total pruned amount
- `aggregate_oldest_period_start`: Option<u64>
- `aggregate_newest_period_end`: Option<u64>

---

## Merchant Configuration Events

### MerchantPausedEvent

**Topic:** `("merchant_paused", merchant)`

Emitted when a merchant enables blanket pause via `pause_merchant()`.

**Fields:**
- `merchant`: Address
- `timestamp`: u64

**Effect:** All subscriptions for this merchant become non-chargeable.

---

### MerchantUnpausedEvent

**Topic:** `("merchant_unpaused", merchant)`

Emitted when a merchant disables blanket pause via `unpause_merchant()`.

**Fields:**
- `merchant`: Address
- `timestamp`: u64

---

### PlanTemplateUpdatedEvent

**Topic:** `("plan_updated",)`

Emitted when a plan template is updated to a new version.

**Fields:**
- `template_key`: u32 — logical template group shared by all versions
- `old_plan_id`: u32
- `new_plan_id`: u32
- `version`: u32
- `merchant`: Address
- `timestamp`: u64

---

### PlanMaxActiveUpdatedEvent

**Topic:** `("plan_max_active_updated",)`

Emitted when a plan's maximum active subscriptions limit is configured.

**Fields:**
- `plan_template_id`: u32
- `merchant`: Address
- `max_active`: u32 — (`0` = no limit)
- `timestamp`: u64

---

### SubscriptionMigratedEvent

**Topic:** `("subscription_migrated", subscription_id)`

Emitted when a subscription is migrated from one plan template version to another.

**Fields:**
- `subscription_id`: u32
- `template_key`: u32
- `from_plan_id`: u32
- `to_plan_id`: u32
- `merchant`: Address
- `subscriber`: Address
- `timestamp`: u64

---

## Metadata Events

### MetadataSetEvent

**Topic:** `("metadata_set", subscription_id)`

Emitted when subscription metadata is set or updated.

**Fields:**
- `subscription_id`: u32
- `key`: String
- `authorizer`: Address

---

### MetadataDeletedEvent

**Topic:** `("metadata_deleted", subscription_id)`

Emitted when subscription metadata is deleted.

**Fields:**
- `subscription_id`: u32
- `key`: String
- `authorizer`: Address

---

## Initialization & Migration Events

### MigrationExportEvent

**Topic:** `("export_subscriptions",)`

Emitted when subscriptions are exported via `export_subscription_summaries()`.

**Fields:**
- `admin`: Address
- `start_id`: u32
- `limit`: u32
- `exported`: u32 — actual count exported
- `timestamp`: u64

---

## Security Considerations

### No Sensitive Data in Events

Events are **public blockchain data**. The following are **intentionally NOT included**:

- ✗ Private keys or account secrets
- ✗ Optional metadata values (only keys are emitted)
- ✗ Subscriber email or PII (only Address)
- ✗ Merchant fee percentages or routing details (only transactions recorded)
- ✗ Grace period configuration (only status transition events)

### Failure Events Are Explicit

Subscriptions that fail charging emit `SubscriptionChargeFailedEvent` — never emit a "success" event. This ensures indexers can distinguish:

- ✓ Successful charge → `SubscriptionChargedEvent`
- ✗ Failed charge → `SubscriptionChargeFailedEvent`
- ✗ Charge not attempted (wrong status) → no event

### Deterministic Batch Ordering

`batch_charge()` emits exactly one event per successfully charged subscription, in the order subscriptions were processed. Failed charges do not emit events. This ensures indexers can reconstruct execution order without re-querying the contract.

### Authorization Events Include Authorizer

When an action requires explicit authorization (`pause`, `resume`, `cancel`, `refund`), the `authorizer` field in the event identifies who approved it:

- Subscriber initiated
- Merchant initiated
- Admin initiated (for recovery)

This enables audit trails for compliance.

---

## Indexing Recommendations

| Event | Index By | Aggregate By |
|-------|----------|--------------|
| SubscriptionCreatedEvent | subscription_id, subscriber, merchant | time (metrics) |
| FundsDepositedEvent | subscription_id, subscriber | balance progression |
| SubscriptionChargedEvent | subscription_id, merchant, token | merchant earnings, pvt revenue |
| SubscriptionChargeFailedEvent | subscription_id, subscriber | recovery metrics |
| MerchantWithdrawalEvent | merchant, token, timestamp | payout history |
| LifetimeCapReachedEvent | subscription_id, merchant, timestamp | completion metrics |
| SubscriptionCancelledEvent | subscription_id, subscriber, timestamp | churn metrics |
| MerchantPausedEvent | merchant, timestamp | merchant downtime |

---

## Timestamp Precision

All timestamps are **ledger timestamps in seconds** (Unix epoch). Resolution is not nanosecond-accurate; use for relative ordering and metrics, not high-frequency reconciliation.
