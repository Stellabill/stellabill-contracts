# Subscription Vault Events

This is a reference guide to all events emitted by the subscription vault contract.

**For the complete canonical schema with all fields, examples, and detailed security notes, see:**
→ [Canonical Event Schema Reference](./events-schema-canonical.md)

## Quick Event Reference

All events are emitted through Soroban's native event system exactly once per successful state transition or fund movement. Events enable indexers and off-chain systems to reconstruct subscription state without querying the contract.

## Event Categories by Type

## Event Categories by Type

### Subscription Lifecycle (6 events)
| Event | Topic | When | Key Fields |
|-------|-------|------|-----------|
| **SubscriptionCreatedEvent** | `("created", id)` | Subscription created | subscription_id, subscriber, merchant, amount, interval_seconds, lifetime_cap, expires_at |
| **SubscriptionPausedEvent** | `("sub_paused", id)` | Subscription paused | subscription_id, authorizer |
| **SubscriptionResumedEvent** | `("sub_resumed", id)` | Subscription resumed | subscription_id, authorizer |
| **SubscriptionCancelledEvent** | `("subscription_cancelled", id)` | Subscription cancelled | subscription_id, authorizer, refund_amount |
| **SubscriptionExpiredEvent** | `("subscription_expired", id)` | Auto-expired by expires_at | subscription_id, timestamp |
| **SubscriptionRecoveryReadyEvent** | `("recovery_ready", id)` | Underfunded → Active | subscription_id, subscriber, prepaid_balance, required_amount, timestamp |

### Charging Events (6 events)
| Event | Topic | When | Key Fields |
|-------|-------|------|-----------|
| **SubscriptionChargedEvent** | `("charged",)` | Interval charge succeeds | subscription_id, merchant, amount, lifetime_charged |
| **SubscriptionChargeFailedEvent** | `("charge_failed", id)` | Charge fails (insufficient balance) | subscription_id, merchant, required_amount, available_balance, shortfall, resulting_status, timestamp |
| **OneOffChargedEvent** | `("one_off_charged", id)` | Off-interval charge | subscription_id, merchant, amount |
| **ProtocolFeeChargedEvent** | `("protocol_fee_charged", id)` | Fees routed to treasury | subscription_id, treasury, fee_amount, merchant_amount, timestamp |
| **UsageStatementEvent** | `("usage_statement", id)` | Usage-based charge | subscription_id, merchant, usage_amount, token, timestamp, reference |
| **LifetimeCapReachedEvent** | `("lifetime_cap_reached", id)` | Lifetime cap exhausted | subscription_id, lifetime_cap, lifetime_charged, timestamp |

### Fund Movement (5 events)
| Event | Topic | When | Key Fields |
|-------|-------|------|-----------|
| **FundsDepositedEvent** | `("deposited", id)` | Subscriber deposits | subscription_id, subscriber, amount, prepaid_balance |
| **MerchantWithdrawalEvent** | `("withdrawn", merchant, token)` | Merchant withdraws | merchant, token, amount, remaining_balance |
| **PartialRefundEvent** | `("partial_refund", id)` | Refund processed | subscription_id, subscriber, amount, timestamp |
| **MerchantRefundEvent** | `("merchant_refund", merchant)` | Merchant refunds subscriber | merchant, subscriber, token, amount |
| **SubscriberWithdrawalEvent** | `("subscriber_withdrawal", id)` | Subscriber withdraws | subscription_id, subscriber, amount |

### Admin & Configuration (7 events)
| Event | Topic | When | Key Fields |
|-------|-------|------|-----------|
| **EmergencyStopEnabledEvent** | `("emergency_stop_enabled",)` | Admin activates emergency stop | admin, timestamp |
| **EmergencyStopDisabledEvent** | `("emergency_stop_disabled",)` | Admin deactivates emergency stop | admin, timestamp |
| **AdminRotatedEvent** | `("admin_rotated",)` | Admin address changed | old_admin, new_admin, timestamp |
| **RecoveryEvent** | `("recovery",)` | Admin recovers stranded funds | admin, recipient, token, amount, reason, timestamp |
| **BillingCompactedEvent** | `("billing_compacted",)` | Statements compacted | admin, subscription_id, pruned_count, kept_count, total_pruned_amount, timestamp, aggregate_* |
| **MerchantPausedEvent** | `("merchant_paused", merchant)` | Merchant pause enabled | merchant, timestamp |
| **MerchantUnpausedEvent** | `("merchant_unpaused", merchant)` | Merchant pause disabled | merchant, timestamp |

### Plans & Metadata (6 events)
| Event | Topic | When | Key Fields |
|-------|-------|------|-----------|
| **PlanTemplateUpdatedEvent** | `("plan_updated",)` | Plan template versioned | template_key, old_plan_id, new_plan_id, version, merchant, timestamp |
| **PlanMaxActiveUpdatedEvent** | `("plan_max_active_updated",)` | Concurrency limit set | plan_template_id, merchant, max_active, timestamp |
| **SubscriptionMigratedEvent** | `("subscription_migrated", id)` | Subscription moves to new plan | subscription_id, template_key, from_plan_id, to_plan_id, merchant, subscriber, timestamp |
| **MetadataSetEvent** | `("metadata_set", id)` | Subscription metadata updated | subscription_id, key, authorizer |
| **MetadataDeletedEvent** | `("metadata_deleted", id)` | Subscription metadata deleted | subscription_id, key, authorizer |
| **MigrationExportEvent** | `("export_subscriptions",)` | Subscriptions exported | admin, start_id, limit, exported, timestamp |

---

## Event Emission Guarantees

1. **Exactly once per state change** — Each successful mutation emits exactly one event
2. **Atomic with state update** — Event and state change are in same transaction
3. **Deterministic order in batch** — `batch_charge()` events emitted in order
4. **No success-on-failure events** — Failed operations never emit success-like events
5. **Stable schema** — Event struct fields never change (backward compatible extensions only)

## For Indexers: Topic Format

Topics are tuples that enable efficient filtering:

```
Topic[0] = event name ("charged", "deposited", etc.)
Topic[1] = resource ID (subscription_id, merchant address, etc.) - optional
Topic[2] = token address - for multi-token events - optional
```

Example topic parsing:
```rust
if topics[0] == "charged" {
    let subscription_id = topics[1];  // subscription_id
    // ... process charge event
}
```

All event data is in the structured event object (never raw tuples).

## Security Properties

✓ **No sensitive metadata** — Only customer-visible fields
✓ **Authorization tracked** — All privileged actions include authorizer address
✓ **Deterministic order** — Batch operations emit events in processing order
✓ **Explicit failures** — Charge failures emit dedicated FailureEvent, never SuccessEvent
✓ **Immutable history** — Events cannot be edited or deleted

See [events-schema-canonical.md](./events-schema-canonical.md) for detailed security implications and recommendations.
