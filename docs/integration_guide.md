# Stellabill: Billing & Indexing Integration Guide

This guide describes how backend billing engines, indexers, and analytics services should interact with the Stellabill subscription vault smart contract on the Stellar network.

## Table of Contents
1. [Overview](#overview)
2. [Required Contract Entrypoints](#required-contract-entrypoints)
3. [Recommended Flows](#recommended-flows)
4. [Indexing & Analytics (Events & View Helpers)](#indexing--analytics)
5. [Error Handling & Idempotency](#error-handling--idempotency)

---

## Overview

The Stellabill subscription vault is a Soroban smart contract that manages prepaid USDC subscriptions. The contract acts as an escrow, holding prepaid funds from users (subscribers) and allowing an authorized billing engine (admin) to securely charge subscriptions at a defined interval, transferring funds to the merchant.

The key roles from an integration perspective are:
- **Billing Engine (Admin):** An authorized service responsible for triggering charges at the appropriate billing intervals.
- **Indexers:** Services that track on-chain state, changes, and events to provide historical data or power UIs.

---

## Required Contract Entrypoints

The integration interacts with specific functions defined in `contracts/subscription_vault/src/lib.rs`. 

### For the Billing Engine (Admin)

1. **`charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error>`**
   - **Purpose:** Charges a single subscription. Deducts the `amount` from the `prepaid_balance` and transfers it to the merchant. Updates the `last_payment_timestamp`.
   - **Authorization:** Requires the signature of the `admin` address.
   - **Errors to handle:** 
     - `Error::IntervalNotElapsed` (1001) if called too early.
     - `Error::NotActive` (1002) if paused or cancelled.
     - `Error::InsufficientBalance` (1003) if the prepaid balance is too low.

2. **`batch_charge(env: Env, subscription_ids: Vec<u32>) -> Result<Vec<BatchChargeResult>, Error>`**
   - **Purpose:** Process multiple subscriptions in a single transaction. Recommended for efficiency.
   - **Parameters:** A vector of `subscription_id`s.
   - **Returns:** A vector of `BatchChargeResult` objects `{ success: bool, error_code: u32 }`. If `success` is false, `error_code` reflects why the individual charge failed. The transaction *does not revert* if a single charge within the batch fails.
   - **Authorization:** Requires the signature of the `admin` address.

### For Indexers & UIs (View Helpers)

1. **`get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error>`**
   - **Purpose:** Fetches the current state of a subscription.
   - **Returns:** A `Subscription` struct containing `subscriber`, `merchant`, `amount`, `interval_seconds`, `last_payment_timestamp`, `status`, `prepaid_balance`, and `usage_enabled`.

2. **`estimate_topup_for_intervals(env: Env, subscription_id: u32, num_intervals: u32) -> Result<i128, Error>`**
   - **Purpose:** Calculates how much USDC a user needs to deposit to cover the next `num_intervals`. Handy for reminding users to top-up before their balance runs out.

---

## Recommended Flows

### 1. Subscription Creation & Top-up (User Flow)
1. User calls `create_subscription` directly on-chain, defining the merchant, amount, and interval. This returns a `u32` subscription ID.
2. User calls `deposit_funds` with their `subscription_id` to prepay their balance.
3. *Indexer Action:* The indexer detects the new subscription and deposit, updating the backend database.

### 2. The Billing Cycle (Admin Flow)
1. **Identify targets:** The backend queries its database (populated by the indexer) to find `subscription_id`s where `current_time >= last_payment_timestamp + interval_seconds` and `status == Active`.
2. **Execute charge:** The billing engine constructs a `batch_charge` transaction with up to ~50-100 IDs (depending on network limits) and submits it to the Stellar network.
3. **Handle results:** The backend parses the returned `Vec<BatchChargeResult>`. 
   - If a charge fails with `InsufficientBalance` (1003), the backend should trigger a notification to the user to top-up, and optionally transition the subscription to a paused/failed state if policy dictates.

### 3. Merchant Withdrawals
Merchants call `withdraw_merchant_funds(merchant: Address, amount: i128)` to claim their revenue. Backend systems do not need to trigger this, but indexers should listen for the withdrawal events to update merchant balance displays.

---

## Indexing & Analytics

*Note: The current contract implementation relies primarily on state queries. Event emission for specific actions (like `SubscriptionCharged`, `FundsDeposited`) is a planned enhancement.*

### Polling vs. Event Sourcing
Until custom contract events are fully implemented, indexers should rely on:
1. **Transaction parsing:** Monitor the ledger for transactions invoking `create_subscription`, `deposit_funds`, `batch_charge`, etc.
2. **State queries:** Periodically poll `get_subscription` for active IDs to ensure local database synchrony with the on-chain `last_payment_timestamp` and `prepaid_balance`.

### Key Metrics to Track
- **MRR (Monthly Recurring Revenue):** Aggregate the `amount` of all `Active` subscriptions for a merchant, normalized to a 30-day interval.
- **Churn Risk:** Track subscriptions where `prepaid_balance < amount`. Use `estimate_topup_for_intervals(id, 1)` to trigger low-balance alerts.

---

## Error Handling & Idempotency

When integrating a backend billing engine with the blockchain, network volatility and transaction timeouts are common.

### Idempotency
Transactions on Soroban require sequence numbers, providing baseline protection against replay attacks. However, specifically for billing:
- **Safe Retries:** If a `batch_charge` transaction fails due to network issues (e.g., timeout before inclusion), it is **safe to retry**. The contract explicitly checks `last_payment_timestamp + interval_seconds`. If the original transaction actually succeeded, the retry will gracefully fail with `Error::IntervalNotElapsed` (1001) rather than double-charging the user.

### Handling Batch Result Errors
Because `batch_charge` does not revert the entire transaction if one sub-charge fails, you must parse the result array.
- Code `404` (NotFound): The subscription ID doesn't exist. Remove it from your billing queue.
- Code `1002` (NotActive): The user paused or cancelled. Suspend billing attempts.
- Code `1003` (InsufficientBalance): Keep in queue, but alert the user. Do not attempt to charge again until the indexer detects a `deposit_funds` action.
