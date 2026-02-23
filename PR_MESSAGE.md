# PR: Implement Subscription Pause and Resume Lifecycle Controls

## Description
This PR implements the pause and resume functionality for the `subscription_vault` contract, allowing subscribers and merchants to temporarily suspend billing without cancelling the subscription entirely. This includes the necessary state machine transitions, authorization logic, and safety checks to prevent unauthorized charges while a subscription is paused.

## Key Changes
*   **New Entrypoints**:
    *   `pause_subscription`: Transitions an `Active` subscription to `Paused`.
    *   `resume_subscription`: Restores a `Paused` or `InsufficientBalance` subscription back to `Active`.
*   **Security & Authorization**:
    *   Strictly enforces that only the **Subscriber** or the **Merchant** can trigger pause/resume actions.
    *   Added validation to prevent charging any subscription that is in the `Paused` or `Cancelled` state, returning a specific `NotActive` error.
*   **Events**:
    *   Introduced `SubscriptionPausedEvent` and `SubscriptionResumedEvent` for enhanced on-chain auditability and indexing.
*   **Error Handling**:
    *   Added `Error::NotActive (1002)` to clearly distinguish between state machine violation and a subscription merely being suspended.
*   **Documentation**:
    *   Added `docs/pause_resume.md` which includes a Mermaid state diagram and detailed rules regarding billing consistency and idempotency.

## State Machine Invariants
*   **Active ↔ Paused**: Primary toggle for temporary suspension.
*   **InsufficientBalance → Active**: Allows a merchant or subscriber to "retry" or "unlock" a subscription that failed a previous charge.
*   **Blocked Charging**: The `charge_subscription` function now rejects charges for non-active statuses with a pre-transfer guard.
*   **Terminal State**: Reconfirmed that `Cancelled` remains a terminal state (cannot be paused or resumed).

## Testing & Verification
*   **101 Tests Passing**: Comprehensive suite updated to cover:
    *   Successful pause/resume round-trips.
    *   Authorization failures (unauthorized caller).
    *   Idempotency (e.g., pausing an already paused sub).
    *   Invalid transitions (e.g., pausing from `InsufficientBalance` or `Cancelled`).
    *   Verification that paused subscriptions cannot be charged.

## How to Review
1.  Check the state transition logic in `contracts/subscription_vault/src/lib.rs`.
2.  Review the new test cases in `contracts/subscription_vault/src/test.rs` to ensure edge cases are adequately handled.
3.  Read the lifecycle documentation in `docs/pause_resume.md`.
