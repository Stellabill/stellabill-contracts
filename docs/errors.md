# Error Codes and Messages

This document describes the error codes and semantics used in the `stellabill-contracts` project, specifically within the `subscription_vault` contract.

## Error Categories

Errors are categorized by their numeric range to help clients identify the type of failure and how to handle it.

### Auth Errors (401-403)

| Code | Name | Meaning | Recommended Client Action |
|------|------|---------|---------------------------|
| 401  | `Unauthorized` | Caller does not have the required authorization or is not the admin. | Ensure the correct account is signing the transaction. |
| 403  | `Forbidden` | Caller is authorized but does not have permission for this specific action. | Check if the account has the necessary roles/permissions. |

### Not Found (404)

| Code | Name | Meaning | Recommended Client Action |
|------|------|---------|---------------------------|
| 404  | `NotFound` | The requested resource (e.g. subscription) was not found. | Verify the subscription ID or resource identifier. |

### Invalid Input (400, 405-408)

| Code | Name | Meaning | Recommended Client Action |
|------|------|---------|---------------------------|
| 400  | `InvalidStatusTransition` | The requested state transition is not allowed by the state machine. | Review the subscription lifecycle documentation. |
| 402  | `BelowMinimumTopup` | The top-up amount is below the minimum required threshold. | Increase the deposit amount to at least the minimum. |
| 405  | `InvalidAmount` | The provided amount is zero or negative. | Ensure the amount is a positive value. |
| 406  | `InvalidRecoveryAmount` | Recovery amount is zero or negative. | (Admin only) Use a positive amount for recovery. |
| 407  | `UsageNotEnabled` | Usage-based charge attempted on a subscription with usage disabled. | Enable usage-based charging for this subscription. |
| 408  | `InvalidInput` | Invalid parameters provided to the function (e.g. limit=0). | Review the function parameters and constraints. |

### Insufficient Funds (10xx)

| Code | Name | Meaning | Recommended Client Action |
|------|------|---------|---------------------------|
| 1001 | `InsufficientBalance` | Subscription failed due to insufficient prepaid balance in the vault for an interval charge. | Top up the prepaid balance for the subscription. |
| 1002 | `InsufficientPrepaidBalance` | Usage-based charge exceeds the available prepaid balance. | Top up the prepaid balance. |

### Timing & Lifecycle Errors (11xx)

| Code | Name | Meaning | Recommended Client Action |
|------|------|---------|---------------------------|
| 1101 | `IntervalNotElapsed` | Charge attempted before the required interval has elapsed. | Wait until the billing interval has passed. |
| 1102 | `Replay` | Charge already processed for this billing period (replay protection). | No action needed; the charge was already successful for this period. |
| 1103 | `NotActive` | Subscription is not in the 'Active' state (e.g. Paused or Cancelled). | Resume or check the status of the subscription. |

### Algebra & Overflow (12xx)

| Code | Name | Meaning | Recommended Client Action |
|------|------|---------|---------------------------|
| 1201 | `Overflow` | Arithmetic overflow in computation. | Ensure amounts are within reasonable bounds. |
| 1202 | `Underflow` | Arithmetic underflow (e.g. balance would go negative). | Ensure balances and amounts result in positive values. |

### Configuration & System (13xx)

| Code | Name | Meaning | Recommended Client Action |
|------|------|---------|---------------------------|
| 1301 | `AlreadyInitialized` | Contract is already initialized. | No action needed; contract is already set up. |
| 1302 | `NotInitialized` | Contract has not been initialized. | Admin must call `init` before other operations. |

## HTTP Mapping

While these are smart contract errors, they can be mapped to HTTP status codes for API consumers:

- `401`, `403`, `404`, `400` map directly.
- `10xx`, `11xx` can be mapped to `409 Conflict` or `422 Unprocessable Entity`.
- `12xx`, `13xx` can be mapped to `500 Internal Server Error` (if unexpected) or `400 Bad Request` (if user-driven).
