# Stellabill Error Codes and Semantics

This document details the error codes used in the Stellabill contracts, specifically in the `subscription_vault` contract. These codes are designed to provide clear, consistent feedback to clients and simplified monitoring for operators.

## Error Categories

| Category | Range | HTTP Equivalent | Description |
|----------|-------|-----------------|-------------|
| Auth | 401-403 | 401/403 | Authentication and authorization failures. |
| Not Found | 404-406 | 404 | Resources or configuration not found. |
| Invalid Input | 400, 407-409 | 400 | Client-side argument or state machine errors. |
| Funds | 402, 410-412 | 402/412 | Insufficient balance or threshold failures. |
| Timing | 1001-1005 | 425/409 | Business logic timing constraints. |
| Config | 1101-1105 | 500/503 | Contract initialization or setup errors. |

## Error Variants

### Auth & Access Control
- **`Unauthorized` (401)**: The caller is not authorized to perform this action. Occurs when `require_auth` fails or an explicit address check fails.
- **`NotAdmin` (403)**: Action requires administrative privileges. Occurs when a non-admin attempts to call admin-only functions like `set_min_topup`.

### Not Found
- **`NotFound` (404)**: Generic resource not found.
- **`SubscriptionNotFound` (405)**: The specific subscription ID does not exist in storage.
- **`ConfigNotFound` (406)**: Contract configuration (admin, token, etc.) has not been initialized.

### Invalid Input & Arguments
- **`InvalidArguments` (400)**: A provided argument is malformed or invalid.
- **`InvalidAmount` (407)**: Amount must be greater than zero and valid.
- **`InvalidInterval` (408)**: Billing interval must be within allowed bounds (e.g., > 0).
- **`InvalidStatusTransition` (409)**: The requested status transition is not allowed by the state machine (e.g., trying to resume a cancelled subscription).

### Financial & Funds
- **`InsufficientBalance` (402)**: The subscription vault has insufficient funds to cover the charge. Maps to `SubscriptionStatus::InsufficientBalance`.
- **`BelowMinimumTopup` (410)**: Deposit amount is below the required minimum threshold configured in the contract.
- **`InsufficientMerchantBalance` (411)**: Withdrawal failed due to insufficient merchant balance.

### Timing & Lifecycle
- **`IntervalNotElapsed` (1001)**: Charge attempted before `last_payment_timestamp + interval_seconds`.
- **`NotActive` (1002)**: Subscription is not in an `Active` state.
- **`SubscriptionExpired` (1003)**: Subscription has reached its end date or maximum billing cycles.

### Configuration
- **`NotConfigured` (1101)**: The contract has not been properly initialized. Usually returned if required storage keys are missing.
- **`InvalidConfig` (1102)**: Provided configuration values are invalid or inconsistent.

## Client Handling Recommendations

- **401/403**: Verify the signer and ensure the correct wallet is connected.
- **404/405**: Ensure the subscription ID is correct and has been successfully created.
- **400/407-409**: Validate inputs on the client-side before submission.
- **402/410**: Prompt the user to top up their account or increase the deposit amount.
- **1001**: Retry the charge after the calculated interval.
- **1101/1102**: Contact protocol administrators for contract health checks.
