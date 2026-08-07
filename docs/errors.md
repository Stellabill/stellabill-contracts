# Error Codes

## Emergency Stop Errors
- `4007`: EmergencyStopActive - Emergency stop is active, critical operations are blocked
- `4100`: EmergencyStopAlreadyActive - Emergency stop is already active
- `4101`: EmergencyStopAlreadyDisabled - Emergency stop is already disabled

## Auth Errors (1000-1099)
- `1001`: Unauthorized - Caller does not have the required authorization
- `1002`: Forbidden - Caller is authorized but does not have permission
- `1003`: SubscriberBlocklisted - Subscriber is on the blocklist
- `1004`: SelfRotation - Rotation to the same admin address is not allowed
- `1005`: NonceAlreadyUsed - Nonce has already been used
- `1006`: BatchTooLarge - Batch size exceeds maximum allowed

## Not Found (2000-2099)
- `2001`: NotFound - The requested resource was not found
- `2002`: NotInitialized - The contract is not initialized

... (add all other error codes as needed)
