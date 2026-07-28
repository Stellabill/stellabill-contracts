# Stellabill: Billing & Indexing Integration Guide

This guide describes how backend billing engines, indexers, analytics services,
and merchant tooling should interact with the Stellabill subscription vault smart
contract on the Stellar network.

## Table of Contents

1. [Overview](#overview)
2. [Actor Model & Authorization](#actor-model--authorization)
3. [Contract Entrypoints Reference](#contract-entrypoints-reference)
4. [Lifecycle State Semantics](#lifecycle-state-semantics)
5. [Common Workflow Invocation Sequences](#common-workflow-invocation-sequences)
6. [Indexing & Event Sourcing](#indexing--event-sourcing)
7. [Error Codes, Failure Modes & Retry Behavior](#error-codes-failure-modes--retry-behavior)
8. [Security Notes](#security-notes)
9. [Testing Auth with Channel Accounts](#testing-auth-with-channel-accounts)

---

## Testing Auth with Channel Accounts

Testing smart contracts using `env.mock_all_auths()` is a convenient way to bypass authorization checks during early development. However, relying on it in integration tests hides critical real-world auth bugs. For instance:
- Missing token transfer sub-invocations in the `InvokerContractAuthEntry`.
- Passing the wrong signer to `require_auth()`.
- Incorrect parameter ordering or values signed in the authorization payload.

To ensure robust security, the `channel_account_auth` tests use a real `ChannelAccountContract` mock to drive entrypoints through the real Soroban auth stack. This contract uses `env.authorize_as_current_contract` to provide explicit authorization entries. By testing with this mock, we verify that BOTH subscriber and merchant auth stacks function correctly, preventing authorization replay attacks and mismatched signature payloads.

To run the channel account auth tests along with the rest of the test suite:
```bash
cargo test --all
```

---

## Overview

The Stellabill subscription vault is a Soroban smart contract that manages
prepaid token-denominated subscriptions. It acts as an escrow: subscriber funds
are held in the vault and the authorized billing engine (admin) transfers them to
merchants on a defined cadence.

Key design properties:

- **Deterministic charging**: given the same ledger state, the same charge
  produces the same result. Replay protection prevents double-charging within a
  billing period.
- **Emergency circuit breaker**: the admin can halt all financial writes
  (`create_subscription`, `deposit_funds`, `charge_*`, `batch_charge`) while
  leaving queries and withdrawals operational.
- **Multi-token support**: subscriptions may settle in the default token or any
  admin-accepted token.
- **Usage billing**: subscriptions created with `usage_enabled = true` accept
  metered `charge_usage` calls in addition to interval charges.
- **Lifetime caps**: subscriptions may carry an optional cumulative charge cap;
  when reached the subscription is auto-cancelled.
- **Plan templates**: merchants publish reusable plan definitions; subscribers
  instantiate them via `create_subscription_from_plan`.

---

## Actor Model & Authorization

| Actor | Description | Authorized Entrypoints |
|-------|-------------|------------------------|
| **Admin** | Singleton address set at `init`. Rotatable via `rotate_admin`. | `init`, `set_min_topup`, `rotate_admin`, `recover_stranded_funds`, `batch_charge`, `charge_subscription`, `charge_usage`, `charge_usage_with_reference`, `charge_one_off`, `enable_emergency_stop`, `disable_emergency_stop`, `export_contract_snapshot`, `export_subscription_summary`, `export_subscription_summaries`, `add_accepted_token`, `remove_accepted_token`, `set_billing_retention`, `partial_refund`, `set_oracle_config`, `set_subscriber_credit_limit`, `add_to_blocklist` (global), `remove_from_blocklist` |
| **Subscriber** | Owner of a subscription; identified by their Stellar address. Must sign. | `create_subscription`, `create_subscription_with_token`, `create_subscription_from_plan`, `deposit_funds`, `pause_subscription`, `resume_subscription`, `cancel_subscription`, `withdraw_subscriber_funds`, `migrate_subscription_to_plan` |
| **Merchant** | Recipient of subscription charges. Must sign for merchant actions. | `create_plan_template`, `create_plan_template_with_token`, `update_plan_template`, `set_plan_max_active_subs`, `pause_subscription` (own), `resume_subscription` (own), `cancel_subscription` (own), `withdraw_merchant_funds`, `withdraw_merchant_token_funds`, `merchant_refund`, `pause_merchant`, `unpause_merchant`, `configure_usage_limits`, `add_to_blocklist` (scoped to own subscribers) |
| **Anyone** | No auth required. | `get_subscription`, `get_subscription_count`, `get_merchant_balance`, `get_merchant_balance_by_token`, `get_plan_template`, `get_plan_max_active_subs`, `get_next_charge_info`, `estimate_topup_for_intervals`, `get_subscriptions_by_merchant`, `get_subscriptions_by_token`, `list_subscriptions_by_subscriber`, `get_cap_info`, `get_sub_statements_offset`, `get_sub_statements_cursor`, `get_admin`, `get_min_topup`, `get_emergency_stop_status`, `list_accepted_tokens`, `is_blocklisted`, `get_blocklist_entry`, `get_subscriber_credit_limit`, `get_subscriber_exposure`, `get_merchant_subscription_count`, `get_merchant_total_earnings`, `get_reconciliation_snapshot` |

> **Note on `pause_subscription` / `resume_subscription` / `cancel_subscription`:**
> These accept an `authorizer: Address` parameter. The caller must be either the
> subscriber or the merchant of that specific subscription; any other address
> returns `Error::Unauthorized` (401).

---

## Contract Entrypoints Reference

### Admin / Config

| Entrypoint | Signature | Auth | Notes |
|-----------|-----------|------|-------|
| `init` | `(token, token_decimals, admin, min_topup, grace_period) -> ()` | None (one-time) | Must be called once before any other operation. Returns `AlreadyInitialized` (1301) on repeat. |
| `set_min_topup` | `(admin, min_topup) -> ()` | Admin | Rejects deposits below this threshold. |
| `get_min_topup` | `() -> i128` | None | |
| `get_admin` | `() -> Address` | None | |
| `rotate_admin` | `(current_admin, new_admin) -> ()` | Admin | Emits `AdminRotatedEvent`. Existing subscriptions are unaffected. |
| `recover_stranded_funds` | `(admin, recipient, amount, reason) -> ()` | Admin | `reason` is `RecoveryReason`: `AccidentalTransfer`=0, `DeprecatedFlow`=1, `UnreachableSubscriber`=2. |
| `set_oracle_config` | `(admin, enabled, oracle, max_age_seconds) -> ()` | Admin | When enabled, subscription `amount` is interpreted as quote-currency and converted via oracle price. |
| `get_oracle_config` | `() -> OracleConfig` | None | |
| `set_billing_retention` | `(admin, keep_recent) -> ()` | Admin | Configures how many detailed statement rows are retained per subscription. |
| `add_accepted_token` | `(admin, token, decimals) -> ()` | Admin | Adds a token to the multi-token registry. |
| `remove_accepted_token` | `(admin, token) -> ()` | Admin | Removes a non-default token. |
| `list_accepted_tokens` | `() -> Vec<AcceptedToken>` | None | |

### Emergency Stop

| Entrypoint | Signature | Auth | Notes |
|-----------|-----------|------|-------|
| `enable_emergency_stop` | `(admin) -> ()` | Admin | Idempotent. Blocks `create_subscription`, `deposit_funds`, `charge_*`, `batch_charge`. Emits `EmergencyStopEnabledEvent`. |
| `disable_emergency_stop` | `(admin) -> ()` | Admin | Idempotent. Restores normal operations. Emits `EmergencyStopDisabledEvent`. |
| `get_emergency_stop_status` | `() -> bool` | None | Returns `true` when the circuit breaker is active. |

Blocked operations return `Error::EmergencyStopActive` (1009). Queries,
withdrawals, pause/resume/cancel, and export calls are **not** blocked.

### Subscription Lifecycle

| Entrypoint | Signature | Auth | Notes |
|-----------|-----------|------|-------|
| `create_subscription` | `(subscriber, merchant, amount, interval_seconds, usage_enabled, lifetime_cap) -> u32` | Subscriber | Returns the new `subscription_id`. Blocked by emergency stop and blocklist. |
| `create_subscription_with_token` | `(subscriber, merchant, token, amount, interval_seconds, usage_enabled, lifetime_cap) -> u32` | Subscriber | Same as above, settles in the specified accepted token. |
| `create_subscription_from_plan` | `(subscriber, plan_template_id) -> u32` | Subscriber | Instantiates from a plan template. Respects `max_active_subs` limit if set. |
| `deposit_funds` | `(subscription_id, subscriber, amount) -> ()` | Subscriber | Must be ≥ `min_topup`. Blocked by emergency stop and blocklist. Emits `FundsDepositedEvent`. Emits `RecoveryReadyEvent` when balance becomes sufficient. |
| `pause_subscription` | `(subscription_id, authorizer) -> ()` | Subscriber or Merchant | Allowed from `Active` only. Emits `SubscriptionPausedEvent`. |
| `resume_subscription` | `(subscription_id, authorizer) -> ()` | Subscriber or Merchant | From `Paused`: no balance check. From `GracePeriod` or `InsufficientBalance`: requires `prepaid_balance >= amount`. Emits `SubscriptionResumedEvent`. |
| `cancel_subscription` | `(subscription_id, authorizer) -> ()` | Subscriber or Merchant | Terminal. Allowed from any non-`Cancelled` state. Emits `SubscriptionCancelledEvent`. |
| `withdraw_subscriber_funds` | `(subscription_id, subscriber) -> ()` | Subscriber | Returns remaining `prepaid_balance` to subscriber. Subscription must be `Cancelled`. |
| `partial_refund` | `(admin, subscription_id, subscriber, amount) -> ()` | Admin | Debits `prepaid_balance` and transfers `amount` back to subscriber. |
| `migrate_subscription_to_plan` | `(subscriber, subscription_id, new_plan_template_id) -> ()` | Subscriber | Migrates subscription to a newer version of the same plan template. Token cannot change. |

### Plan Templates

| Entrypoint | Signature | Auth | Notes |
|-----------|-----------|------|-------|
| `create_plan_template` | `(merchant, amount, interval_seconds, usage_enabled, lifetime_cap) -> u32` | Merchant | Returns `plan_template_id`. |
| `create_plan_template_with_token` | `(merchant, token, amount, interval_seconds, usage_enabled, lifetime_cap) -> u32` | Merchant | Token-specific plan. |
| `update_plan_template` | `(merchant, plan_template_id, amount, interval_seconds, usage_enabled, lifetime_cap) -> u32` | Merchant | Creates a new version; never mutates the existing template in-place. Returns new `plan_template_id`. Existing subscriptions are unaffected until migrated. |
| `get_plan_template` | `(plan_template_id) -> PlanTemplate` | None | |
| `set_plan_max_active_subs` | `(merchant, plan_template_id, max_active) -> ()` | Merchant | `0` means no limit. Checked at `create_subscription_from_plan`. |
| `get_plan_max_active_subs` | `(plan_template_id) -> u32` | None | |

### Charging (Admin / Billing Engine)

| Entrypoint | Signature | Auth | Notes |
|-----------|-----------|------|-------|
| `charge_subscription` | `(subscription_id) -> ChargeExecutionResult` | Admin | Single-subscription interval charge. Blocked by emergency stop. |
| `batch_charge` | `(subscription_ids: Vec<u32>) -> Vec<BatchChargeResult>` | Admin | Preferred for billing runs. Per-item results; never reverts the whole transaction for individual failures. Blocked by emergency stop. |
| `charge_usage` | `(subscription_id, usage_amount) -> ()` | Admin | Metered debit. Requires `usage_enabled = true`. Blocked by emergency stop. |
| `charge_usage_with_reference` | `(subscription_id, usage_amount, reference) -> ()` | Admin | Same as above with explicit replay-protection reference string. |
| `charge_one_off` | `(subscription_id, merchant, amount) -> ()` | Merchant | Merchant-initiated ad-hoc debit. Blocked by emergency stop. |

`BatchChargeResult` fields: `{ success: bool, error_code: u32 }`. When
`success` is `false`, `error_code` identifies why that item failed (see [Error
Codes](#error-codes-failure-modes--retry-behavior)).

### Merchant

| Entrypoint | Signature | Auth | Notes |
|-----------|-----------|------|-------|
| `withdraw_merchant_funds` | `(merchant, amount) -> ()` | Merchant | Withdraws from the default-token bucket. |
| `withdraw_merchant_token_funds` | `(merchant, token, amount) -> ()` | Merchant | Withdraws from a specific token bucket. |
| `get_merchant_balance` | `(merchant) -> i128` | None | Default-token accumulated balance. |
| `get_merchant_balance_by_token` | `(merchant, token) -> i128` | None | |
| `pause_merchant` | `(merchant) -> ()` | Merchant | Blanket pause: all of the merchant's subscriptions are treated as paused. |
| `unpause_merchant` | `(merchant) -> ()` | Merchant | Lifts blanket pause. |
| `get_merchant_paused` | `(merchant) -> bool` | None | |
| `merchant_refund` | `(merchant, subscriber, token, amount) -> ()` | Merchant | Debits merchant balance and transfers `amount` to subscriber. |
| `get_reconciliation_snapshot` | `(merchant) -> Vec<TokenReconciliationSnapshot>` | None | Per-token reconciliation view. |
| `get_merchant_total_earnings` | `(merchant) -> Vec<(Address, TokenEarnings)>` | None | Lifetime accrued earnings per token. |

### Queries

| Entrypoint | Signature | Auth | Notes |
|-----------|-----------|------|-------|
| `get_subscription` | `(subscription_id) -> Subscription` | None | Returns full `Subscription` struct. |
| `get_subscription_count` | `() -> u32` | None | Total ever created. |
| `get_merchant_subscription_count` | `(merchant) -> u32` | None | |
| `get_subscriptions_by_merchant` | `(merchant, start, limit) -> Vec<Subscription>` | None | Offset pagination. |
| `get_subscriptions_by_token` | `(token, start, limit) -> Vec<Subscription>` | None | Offset pagination. |
| `list_subscriptions_by_subscriber` | `(subscriber, start_from_id, limit) -> SubscriptionsPage` | None | Cursor-style pagination by subscriber. |
| `estimate_topup_for_intervals` | `(subscription_id, num_intervals) -> i128` | None | Calculates how much the subscriber needs to deposit to cover the next N intervals. |
| `get_next_charge_info` | `(subscription_id) -> NextChargeInfo` | None | Returns `{ next_charge_at, is_due, status }`. |
| `get_cap_info` | `(subscription_id) -> CapInfo` | None | Lifetime cap summary: cap value, amount charged, remaining, reached flag. |
| `get_sub_statements_offset` | `(subscription_id, offset, limit, newest_first) -> BillingStatementsPage` | None | Offset pagination for billing statements. `newest_first = true` recommended for infinite scroll. |
| `get_sub_statements_cursor` | `(subscription_id, cursor, limit, newest_first) -> BillingStatementsPage` | None | Cursor pagination; pass `cursor = None` for first page. |
| `get_subscriber_credit_limit` | `(subscriber, token) -> i128` | None | `0` means no limit. |
| `get_subscriber_exposure` | `(subscriber, token) -> i128` | None | Sum of prepaid balances + next-interval amounts for active subscriptions. |

### Blocklist

| Entrypoint | Signature | Auth | Notes |
|-----------|-----------|------|-------|
| `add_to_blocklist` | `(authorizer, subscriber, reason) -> ()` | Admin (global) or Merchant (own subscribers only) | Prevents new subscriptions and deposits. Existing subscriptions and balances are preserved. |
| `remove_from_blocklist` | `(admin, subscriber) -> ()` | Admin only | Returns `NotFound` (404) if subscriber is not blocklisted. |
| `is_blocklisted` | `(subscriber) -> bool` | None | |
| `get_blocklist_entry` | `(subscriber) -> BlocklistEntry` | None | Returns `NotFound` (404) if not blocklisted. |

### Migration / Export

| Entrypoint | Signature | Auth | Notes |
|-----------|-----------|------|-------|
| `export_contract_snapshot` | `(admin) -> ContractSnapshot` | Admin | Returns `{ admin, token, min_topup, next_id, storage_version, timestamp }`. |
| `export_subscription_summary` | `(admin, subscription_id) -> SubscriptionSummary` | Admin | |
| `export_subscription_summaries` | `(admin, start_id, limit) -> Vec<SubscriptionSummary>` | Admin | Max 100 per call (`InvalidExportLimit` if exceeded). |

---

## Lifecycle State Semantics

### Status Variants

| Status | Charges Allowed | Deposits Allowed | Entered Via |
|--------|----------------|-----------------|-------------|
| `Active` | Yes | Yes | `create_subscription`; `resume_subscription` from Paused/GracePeriod/InsufficientBalance; successful charge from GracePeriod |
| `GracePeriod` | Yes (retry allowed) | Yes | Failed charge while `now < last_payment_timestamp + interval_seconds + grace_period` |
| `Paused` | No | Yes | `pause_subscription` from Active |
| `InsufficientBalance` | No | Yes | Failed charge after grace period expires; `charge_usage` drains balance to zero |
| `Cancelled` | No | No | `cancel_subscription` (terminal — no exit) |

### State Transition Table

| From | To | Trigger |
|------|-----|---------|
| Active | GracePeriod | Charge fails, balance insufficient, grace still active |
| Active | InsufficientBalance | Charge fails, balance insufficient, grace elapsed |
| Active | Paused | `pause_subscription` |
| Active | Cancelled | `cancel_subscription` |
| GracePeriod | Active | Successful charge or `resume_subscription` (balance ≥ amount) |
| GracePeriod | InsufficientBalance | Charge fails after grace expiry |
| GracePeriod | Cancelled | `cancel_subscription` |
| Paused | Active | `resume_subscription` |
| Paused | Cancelled | `cancel_subscription` |
| InsufficientBalance | Active | `resume_subscription` (requires `prepaid_balance >= amount`) |
| InsufficientBalance | Cancelled | `cancel_subscription` |
| Cancelled | — | Terminal; all transition attempts return `InvalidStatusTransition` (400) |
| *any* | Same | Idempotent; always allowed |

> **Important:** `deposit_funds` does **not** change status. After topping up
> from `InsufficientBalance` or `GracePeriod`, the subscriber (or merchant) must
> explicitly call `resume_subscription`.

### Subscription Struct Fields

```
subscriber            Address   — owner; must sign create/deposit
merchant              Address   — recipient of charges
amount                i128      — charge per interval (token base units, or quote units if oracle enabled)
interval_seconds      u64       — minimum seconds between interval charges
last_payment_timestamp u64      — ledger timestamp of last successful interval charge
status                SubscriptionStatus
prepaid_balance       i128      — current vault balance for this subscription
usage_enabled         bool      — whether charge_usage calls are accepted
token                 Address   — settlement token
lifetime_cap          Option<i128> — max cumulative charged amount; None = no cap
lifetime_charged      i128      — cumulative amount charged to date
```

---

## Common Workflow Invocation Sequences

### 1. Basic Subscription Onboarding (Subscriber Flow)

```
Subscriber → create_subscription(subscriber, merchant, amount, interval_seconds,
                                 usage_enabled=false, lifetime_cap=None)
             → returns subscription_id (u32)

Subscriber → deposit_funds(subscription_id, subscriber, deposit_amount)
             → emits FundsDepositedEvent
             → prepaid_balance increases; status stays Active
```

Pre-conditions:
- Emergency stop must not be active.
- `deposit_amount >= min_topup`.
- Subscriber must not be blocklisted.

### 2. Recurring Billing Cycle (Admin / Billing Engine)

```
1. Query DB for subscription_ids where:
     status IN (Active, GracePeriod)
     AND now >= last_payment_timestamp + interval_seconds

2. Billing engine → batch_charge([id_1, id_2, ..., id_N])
     N ≤ ~50–100 (network-dependent gas limit)
   → returns Vec<BatchChargeResult>

3. For each result:
   success=true  → update DB: prepaid_balance, last_payment_timestamp, status
   error_code=1101 (IntervalNotElapsed)  → safe to skip; clock skew or early retry
   error_code=1102 (Replay)              → already charged this period; remove from queue
   error_code=1001 (InsufficientBalance) → notify subscriber; watch for RecoveryReadyEvent
   error_code=1103 (NotActive)           → subscription paused or cancelled; suspend billing
   error_code=404  (NotFound)            → remove from billing queue
```

### 3. Recovery from Insufficient Balance

```
Admin: batch_charge([sub_id])
  → BatchChargeResult { success: false, error_code: 1001 }
  → contract emits SubscriptionChargeFailedEvent
  → status transitions to GracePeriod or InsufficientBalance

Subscriber: deposit_funds(sub_id, subscriber, topup_amount)
  → emits FundsDepositedEvent
  → if prepaid_balance >= amount: emits RecoveryReadyEvent (signal to subscriber/merchant)

Subscriber or Merchant: resume_subscription(sub_id, authorizer)
  → validates prepaid_balance >= amount
  → status transitions to Active
  → emits SubscriptionResumedEvent

Admin: next billing cycle → batch_charge([sub_id]) → success
```

### 4. Merchant Withdrawal

```
Merchant → get_merchant_balance(merchant)          // check available balance
Merchant → withdraw_merchant_funds(merchant, amount)
           → emits MerchantWithdrawalEvent
           → token transferred to merchant wallet
```

For multi-token:
```
Merchant → get_merchant_balance_by_token(merchant, token)
Merchant → withdraw_merchant_token_funds(merchant, token, amount)
```

### 5. Subscriber Cancellation and Refund

```
Subscriber or Merchant → cancel_subscription(sub_id, authorizer)
                         → status = Cancelled (terminal)
                         → emits SubscriptionCancelledEvent (includes refund_amount)

Subscriber → withdraw_subscriber_funds(sub_id, subscriber)
             → remaining prepaid_balance returned to subscriber
```

### 6. Usage-Based Billing

```
// Subscription created with usage_enabled = true
Subscriber → create_subscription(..., usage_enabled=true, ...)
Subscriber → deposit_funds(sub_id, subscriber, deposit_amount)

// Off-chain metering service aggregates usage

// Billing engine submits metered debit
Admin → charge_usage_with_reference(sub_id, usage_amount, reference_string)
        — reference_string must be unique per subscription (e.g. "2026-03-usage-batch-001")
        → replay protection: duplicate reference returns Error::Replay (1102)
        → on success: prepaid_balance reduced; UsageStatementEvent emitted
        → if balance hits 0: status → InsufficientBalance

// Recovery
Subscriber → deposit_funds(...)
Subscriber or Merchant → resume_subscription(...)
```

Best practices:
- Accumulate usage off-chain; submit one `charge_usage_with_reference` per period.
- Call `get_subscription` first to verify `prepaid_balance > 0` before submitting.
- Use a stable, deterministic reference string (e.g. `"{sub_id}-{period_epoch}"`).

### 7. Plan Template Lifecycle

```
// Merchant publishes plan
Merchant → create_plan_template(merchant, amount, interval_seconds, usage_enabled, lifetime_cap)
           → returns plan_template_id

// Optional: limit concurrent active subscriptions per subscriber
Merchant → set_plan_max_active_subs(merchant, plan_template_id, max_active=1)

// Subscriber subscribes
Subscriber → create_subscription_from_plan(subscriber, plan_template_id)
             → returns subscription_id

// Merchant updates plan (non-destructive versioning)
Merchant → update_plan_template(merchant, plan_template_id, new_amount, ...)
           → returns new_plan_template_id (existing subs unaffected)

// Subscriber migrates to new plan version
Subscriber → migrate_subscription_to_plan(subscriber, sub_id, new_plan_template_id)
```

### 8. Admin Rotation

```
current_admin → rotate_admin(current_admin, new_admin)
                → emits AdminRotatedEvent { current_admin, new_admin, timestamp }
                → current_admin loses privileges immediately
                → new_admin gains privileges immediately
                → existing subscriptions are unaffected
```

Update off-chain billing engine config with `new_admin` immediately after.

### 9. Emergency Stop Sequence

```
// Incident: activate circuit breaker
Admin → enable_emergency_stop(admin)
        → emits EmergencyStopEnabledEvent
        → blocks: create_subscription, deposit_funds, charge_subscription,
                  batch_charge, charge_usage, charge_usage_with_reference

// Verify
Admin → get_emergency_stop_status() → true

// Allowed during stop: queries, withdrawals, pause/resume/cancel, export

// After incident resolution
Admin → disable_emergency_stop(admin)
        → emits EmergencyStopDisabledEvent
        → normal operations resume
```

---

## Indexing & Event Sourcing

### Event Topics and Schemas

| Topic | Event Struct | Key Fields |
|-------|-------------|-----------|
| `sub_new` | `SubscriptionCreatedEvent` | `subscription_id`, `subscriber`, `merchant`, `amount`, `interval_seconds` |
| `deposit` | `FundsDepositedEvent` | `subscription_id`, `subscriber`, `amount`, `new_balance` |
| `charged` | `SubscriptionChargedEvent` | `subscription_id`, `merchant`, `amount`, `remaining_balance` |
| `charge_failed` | `SubscriptionChargeFailedEvent` | `subscription_id`, `error_code`, `status` |
| `paused` | `SubscriptionPausedEvent` | `subscription_id`, `authorizer` |
| `resumed` | `SubscriptionResumedEvent` | `subscription_id`, `authorizer` |
| `cancelled` | `SubscriptionCancelledEvent` | `subscription_id`, `authorizer`, `refund_amount` |
| `withdrawn` | `MerchantWithdrawalEvent` | `merchant`, `token`, `amount`, `remaining_balance` |
| `admin_rotation` | `AdminRotatedEvent` | `current_admin`, `new_admin`, `timestamp` |
| `recovery` | `RecoveryEvent` | `admin`, `recipient`, `amount`, `reason`, `timestamp` |
| `emergency_stop_enabled` | `EmergencyStopEnabledEvent` | `admin`, `timestamp` |
| `emergency_stop_disabled` | `EmergencyStopDisabledEvent` | `admin`, `timestamp` |
| `blocklist_added` | `BlocklistAddedEvent` | `subscriber`, `added_by`, `timestamp`, `reason` |
| `blocklist_removed` | `BlocklistRemovedEvent` | `subscriber`, `removed_by`, `timestamp` |
| `usage_statement` | `UsageStatementEvent` | `subscription_id`, `usage_amount`, `reference` |
| `one_off_charged` | `OneOffChargedEvent` | `subscription_id`, `merchant`, `amount` |
| `migration_export` | `MigrationExportEvent` | `admin`, `start_id`, `limit`, `exported`, `timestamp` |

### Recommended Indexer Pseudocode

```rust
for event in contract_events {
    match event.topic {
        "sub_new" => {
            let e: SubscriptionCreatedEvent = decode(event.data);
            db.upsert_subscription(e.subscription_id, status=Active, ...);
        }
        "deposit" => {
            let e: FundsDepositedEvent = decode(event.data);
            db.update_balance(e.subscription_id, e.new_balance);
        }
        "charged" => {
            let e: SubscriptionChargedEvent = decode(event.data);
            db.record_payment(e.subscription_id, e.amount, e.remaining_balance);
            db.increment_merchant_revenue(e.merchant, e.amount);
        }
        "charge_failed" => {
            let e: SubscriptionChargeFailedEvent = decode(event.data);
            db.update_status(e.subscription_id, e.status);
            notify_subscriber(e.subscription_id);
        }
        "paused" | "resumed" | "cancelled" => {
            // Update subscription status in local DB
        }
        "admin_rotation" => {
            let (current, new, ts) = decode(event.data);
            config.update_admin(new);
        }
        _ => { /* log and ignore unknown topics */ }
    }
}
```

### Key Metrics

| Metric | Calculation |
|--------|-------------|
| **MRR** | Sum `amount` for all `Active` subscriptions per merchant, normalized to 30-day interval: `amount / interval_seconds * 2_592_000` |
| **Churn risk** | `prepaid_balance < amount` → call `estimate_topup_for_intervals(id, 1)` for low-balance alert |
| **TVL** | Sum of all `prepaid_balance` across active subscriptions |
| **Lifetime cap proximity** | `(lifetime_cap - lifetime_charged) <= amount` → subscription will auto-cancel on next charge |

---

## Error Codes, Failure Modes & Retry Behavior

### Complete Error Code Table

| Code | Variant | Category | Meaning | Retry? |
|------|---------|----------|---------|--------|
| 400 | `InvalidStatusTransition` | State | Requested transition not allowed (e.g. Cancelled → Active). | No |
| 401 | `Unauthorized` | Auth | Caller is not admin or not the subscriber/merchant for this subscription. | No (fix signing key) |
| 402 | `BelowMinimumTopup` | Input | Deposit amount below `min_topup`. | No (increase amount) |
| 403 | `Forbidden` | Auth | Authorized caller lacks permission for this specific action (e.g. merchant blocklisting unrelated subscriber). | No |
| 404 | `NotFound` | Input | Subscription ID or resource not found. | No (verify ID) |
| 405 | `InvalidAmount` | Input | Amount is zero or negative. | No |
| 406 | `InvalidRecoveryAmount` | Input | Recovery amount is zero or negative. | No |
| 407 | `UsageNotEnabled` | Input | `charge_usage` on a subscription without `usage_enabled`. | No |
| 408 | `InvalidInput` | Input | Invalid parameters (e.g. `limit=0` on export). | No |
| 1001 | `InsufficientBalance` | Funds | Interval charge failed; balance too low. Status → GracePeriod or InsufficientBalance. | After top-up + resume |
| 1002 | `InsufficientPrepaidBalance` | Funds | Usage charge exceeds balance. | After top-up |
| 1003 | `NotActive` | Lifecycle | Charge on Paused or Cancelled subscription. | After resume |
| 1004 | `UsageNotEnabled` | Input | (same as 407 in usage context) | No |
| 1005 | `InsufficientPrepaidBalance` | Funds | (same as 1002 in charge_usage context) | After top-up |
| 1009 | `EmergencyStopActive` | System | Operation blocked by circuit breaker. | After stop disabled |
| 1101 | `IntervalNotElapsed` | Timing | Charge attempted before interval elapsed. | Yes — wait until due |
| 1102 | `Replay` | Timing | Charge already processed this period (replay protection). | No (already charged) |
| 1103 | `NotActive` | Lifecycle | (alias used in `BatchChargeResult` for not-active items) | After resume |
| 1201 | `Overflow` | Math | Arithmetic overflow. | No (check amounts) |
| 1202 | `Underflow` | Math | Arithmetic underflow. | No (check balances) |
| 1301 | `AlreadyInitialized` | Config | Contract already initialized. | No |
| 1302 | `NotInitialized` | Config | Contract not yet initialized. | No (call `init` first) |

### Retry Behavior for the Billing Engine

**Safe to retry unconditionally:**
- `IntervalNotElapsed` (1101): The original transaction may have timed out before inclusion. If it did succeed, a retry returns 1101 — no double-charge possible because the contract checks `period_index`. Always safe to retry.
- `Replay` (1102): Already charged; remove from queue.

**Retry after state change:**
- `InsufficientBalance` (1001): Keep in queue. Wait for `RecoveryReadyEvent` or `SubscriptionResumedEvent` before retrying.
- `NotActive` (1003/1103): Wait for `SubscriptionResumedEvent`.

**Do not retry:**
- `Unauthorized` (401), `NotFound` (404), `Forbidden` (403), `EmergencyStopActive` (1009), `InvalidStatusTransition` (400): Indicate a structural issue. Fix the root cause first.

### Batch Charge Partial Failures

`batch_charge` never reverts the whole transaction when individual items fail.
Each item in `Vec<BatchChargeResult>` is independent:

```
result[i].success == true   → item charged successfully
result[i].success == false  → item failed; result[i].error_code indicates why
```

Parse every result. A single `Unauthorized` (401) at the batch level means
**no items** were attempted (the admin auth check precedes the loop).

### Usage Charge Replay Protection

`charge_usage_with_reference` rejects duplicate `reference` strings per
subscription with `Error::Replay` (1102). Design reference strings to be:
- Globally unique per subscription per period (e.g. `"{sub_id}-{epoch_day}"`).
- Idempotent: the same reference from a retried network call is safe and returns 1102 rather than double-charging.

---

## Security Notes

1. **Admin key management**: The admin address controls billing execution, emergency stop, fund recovery, and admin rotation. Compromise of the admin key is critical. Rotate immediately on suspicion via `rotate_admin`; monitor `AdminRotatedEvent` on-chain.

2. **Replay protection**: Interval charges use `period_index = now / interval_seconds` stored per subscription. Usage charges use a caller-supplied reference string. Both prevent double-charges across retries.

3. **Emergency stop is not a key revocation**: Activating the circuit breaker blocks financial writes but does not revoke admin authority. Ensure admin key security is resolved before disabling the stop.

4. **Blocklist is global**: A merchant-added blocklist entry prevents the subscriber from creating subscriptions with **any** merchant on this contract, not just the merchant who added the entry. Use with appropriate governance.

5. **Cancelled is irreversible**: The `Cancelled` state is terminal. There is no `uncancel` entrypoint. Ensure cancellation flows are deliberate and communicated to subscribers.

6. **Oracle staleness**: When oracle pricing is enabled, charges fail with `OraclePriceStale` rather than charging at a wrong rate. Billing engines must handle this and retry after the oracle is refreshed.

7. **Lifetime cap auto-cancel**: When `lifetime_charged` reaches `lifetime_cap`, the subscription is automatically cancelled by the charge operation. Indexers should watch for `SubscriptionCancelledEvent` on subscriptions that are near their cap.

8. **CEI pattern**: All token transfer operations in the contract follow Checks-Effects-Interactions ordering. State is updated before the token transfer, preventing reentrancy via the token callback path.

9. **Credit limits**: `set_subscriber_credit_limit` caps the aggregate prepaid + exposure per subscriber per token. Exceeding this limit rejects new subscriptions and top-ups with `CreditLimitExceeded`. Monitor `get_subscriber_exposure` to anticipate rejections.
