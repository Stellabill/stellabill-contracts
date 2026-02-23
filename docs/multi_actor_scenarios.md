# Multi-Merchant and Multi-Subscriber Scenarios

This document describes the integration-style tests and scenarios that validate the subscription vault with many actors (subscribers, merchants, subscriptions) interacting concurrently.

## Purpose

- **Correctness**: Verify balances, statuses, and view helpers when multiple subscriptions exist across different subscriber/merchant pairs.
- **Stress**: Exercise internal indexing and accounting (subscription IDs, per-subscription state) under combined operations.
- **Realism**: Reflect real-world usage: multiple merchants, subscribers with multiple subscriptions, batch charges, one-off and recurring mix.

## Scenario Setup

The multi-actor tests use a fixed topology:

- **2 merchants**, **3 subscribers**.
- **5 subscriptions**:
  - Subscriber 0 → Merchant 0, Subscriber 0 → Merchant 1
  - Subscriber 1 → Merchant 0, Subscriber 1 → Merchant 1
  - Subscriber 2 → Merchant 0

Each subscription is created with the same interval and amount, and receives an initial deposit so that charges can succeed.

## Scenarios Covered

1. **Balances and statuses after setup**  
   All subscriptions are Active, have the expected subscriber/merchant and prepaid balance. Confirms `get_subscription` and storage consistency.

2. **Batch charge all then verify**  
   All 5 subscriptions are charged in one `batch_charge` call. Every result is success; each subscription’s balance and `last_payment_timestamp` are updated correctly.

3. **One-off and recurring mixed**  
   One-off charges are applied to two subscriptions (same merchant); then a batch charge runs for all. Verifies that one-off and interval-based charges share the same prepaid balance and that batch and single-charge semantics stay consistent.

4. **Pause and resume subset**  
   Two subscriptions are paused (different subscribers); a third stays active. Resume one of the paused. Confirms state transitions and that only the intended subscriptions change status.

5. **Cancel one subscription, others unchanged**  
   One subscription is cancelled; the rest remain Active. Confirms cancellation is isolated and does not affect other subscriptions.

6. **View helpers consistent**  
   For each subscription, `estimate_topup_for_intervals(0)` returns 0 and `estimate_topup_for_intervals(2)` matches the expected shortfall from current balance and amount. Validates query logic in a multi-subscription setup.

## Expectations

- **No cross-subscription leakage**: Operations on one subscription do not change another’s balance, status, or charged period.
- **Batch and single charge parity**: Batch charge results and per-subscription state match what would occur from individual `charge_subscription` calls (subject to replay/period rules).
- **Events and indexing**: Events are emitted per action; indexers can rely on subscription_id and merchant/subscriber for filtering (see events.md).

## Performance

Tests are kept fast by using a single env and a small, fixed number of actors (5 subscriptions). For heavier stress (e.g. many more IDs in one batch), consider running benchmarks or dedicated performance tests; the same invariants should hold.
