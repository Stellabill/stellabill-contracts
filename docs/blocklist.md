# Subscriber Blocklist

## Overview

The subscriber blocklist mechanism allows admins and merchants to prevent specific subscriber addresses from creating new subscriptions or depositing funds into the contract. This feature is designed for fraud prevention, dispute management, and access control while preserving existing financial obligations and balances.

## Key Principles

1. **Preventive, Not Punitive**: The blocklist prevents new subscriptions and deposits but does not seize or move existing funds.
2. **Existing Obligations Preserved**: Blocklisted subscribers retain access to their existing subscriptions and prepaid balances.
3. **Dual Authorization**: Both admins (global) and merchants (scoped) can add subscribers to the blocklist.
4. **Admin-Only Removal**: Only admins can remove subscribers from the blocklist.

## Authorization Model

### Admin Authorization
- **Scope**: Global - can blocklist any subscriber address
- **Operations**: Add to blocklist, remove from blocklist
- **Use Cases**: Platform-wide fraud prevention, regulatory compliance, terms of service violations

### Merchant Authorization
- **Scope**: Limited - can only blocklist subscribers with whom they have an active subscription relationship
- **Operations**: Add to blocklist only (cannot remove)
- **Use Cases**: Payment disputes, chargebacks, merchant-specific fraud prevention

## Blocklist Enforcement

### Blocked Operations

When a subscriber is blocklisted, the following operations are **blocked**:

1. **`create_subscription`**: Cannot create new subscriptions
2. **`create_subscription_from_plan`**: Cannot create subscriptions from plan templates
3. **`deposit_funds`**: Cannot deposit additional funds into existing subscriptions

All blocked operations return `Error::SubscriberBlocklisted`.

### Allowed Operations

Blocklisted subscribers can still perform the following operations on **existing** subscriptions:

1. **`cancel_subscription`**: Can cancel their subscriptions
2. **`pause_subscription`**: Can pause their subscriptions
3. **`resume_subscription`**: Can resume paused subscriptions
4. **`withdraw_subscriber_funds`**: Can withdraw remaining balance after cancellation
5. **Charging**: Existing subscriptions continue to be charged normally (admin/automated charges)

## Storage Schema

### Blocklist Entry

```rust
pub struct BlocklistEntry {
    pub subscriber: Address,
    pub added_by: Address,      // Admin or merchant who added the entry
    pub added_at: u64,           // Timestamp when added
    pub reason: Option<String>,  // Optional reason for blocklisting
}
```

### Storage Key

Blocklist entries are stored with the key pattern:
```
(Symbol("blocklist"), subscriber_address) -> BlocklistEntry
```

This allows O(1) lookup during subscription creation and deposit operations.

## Events

### BlocklistAddedEvent

Emitted when a subscriber is added to the blocklist.

```rust
pub struct BlocklistAddedEvent {
    pub subscriber: Address,
    pub added_by: Address,
    pub timestamp: u64,
    pub reason: Option<String>,
}
```

**Topic**: `("blocklist_added",)`

### BlocklistRemovedEvent

Emitted when a subscriber is removed from the blocklist.

```rust
pub struct BlocklistRemovedEvent {
    pub subscriber: Address,
    pub removed_by: Address,
    pub timestamp: u64,
}
```

**Topic**: `("blocklist_removed",)`

## API Reference

### `add_to_blocklist`

Add a subscriber to the blocklist.

```rust
pub fn add_to_blocklist(
    env: Env,
    authorizer: Address,
    subscriber: Address,
    reason: Option<String>,
) -> Result<(), Error>
```

**Authorization**:
- Admin: Can blocklist any subscriber
- Merchant: Can only blocklist subscribers they have subscriptions with

**Errors**:
- `Error::Forbidden`: Merchant attempting to blocklist unrelated subscriber
- `Error::Unauthorized`: Invalid authorization

### `remove_from_blocklist`

Remove a subscriber from the blocklist. Admin only.

```rust
pub fn remove_from_blocklist(
    env: Env,
    admin: Address,
    subscriber: Address,
) -> Result<(), Error>
```

**Authorization**: Admin only

**Errors**:
- `Error::Unauthorized`: Caller is not admin
- `Error::NotFound`: Subscriber is not blocklisted

### `is_blocklisted`

Check if a subscriber is blocklisted.

```rust
pub fn is_blocklisted(
    env: Env,
    subscriber: Address,
) -> bool
```

**Returns**: `true` if subscriber is blocklisted, `false` otherwise

### `get_blocklist_entry`

Get blocklist entry details for a subscriber.

```rust
pub fn get_blocklist_entry(
    env: Env,
    subscriber: Address,
) -> Result<BlocklistEntry, Error>
```

**Errors**:
- `Error::NotFound`: Subscriber is not blocklisted

## Use Cases

### 1. Fraud Prevention (Admin)

An admin detects fraudulent activity from a subscriber address:

```rust
// Admin blocklists the fraudulent subscriber
client.add_to_blocklist(
    &admin,
    &fraudulent_subscriber,
    &Some(String::from_str(&env, "Fraudulent chargebacks detected"))
);

// Subscriber cannot create new subscriptions
let result = client.try_create_subscription(
    &fraudulent_subscriber,
    &merchant,
    &amount,
    &interval,
    &false,
    &None
);
assert_eq!(result, Err(Ok(Error::SubscriberBlocklisted)));

// But existing subscriptions are preserved
let existing_sub = client.get_subscription(&existing_sub_id);
assert_eq!(existing_sub.status, SubscriptionStatus::Active);
```

### 2. Payment Disputes (Merchant)

A merchant experiences repeated payment disputes with a subscriber:

```rust
// Merchant blocklists their problematic subscriber
client.add_to_blocklist(
    &merchant,
    &subscriber,
    &Some(String::from_str(&env, "Repeated payment disputes"))
);

// Subscriber cannot create new subscriptions with this merchant
// But can still manage existing subscription (cancel, withdraw)
client.cancel_subscription(&sub_id, &subscriber);
client.withdraw_subscriber_funds(&sub_id, &subscriber);
```

### 3. Regulatory Compliance (Admin)

An admin needs to restrict access for regulatory reasons:

```rust
// Admin blocklists for compliance
client.add_to_blocklist(
    &admin,
    &restricted_subscriber,
    &Some(String::from_str(&env, "Regulatory compliance - sanctioned address"))
);

// Later, after compliance clearance
client.remove_from_blocklist(&admin, &restricted_subscriber);

// Subscriber can now create subscriptions again
let new_sub_id = client.create_subscription(
    &restricted_subscriber,
    &merchant,
    &amount,
    &interval,
    &false,
    &None
);
```

## Edge Cases and Limitations

### 1. Existing Subscriptions

**Behavior**: Blocklisting does not affect existing subscriptions. They continue to function normally.

**Rationale**: Preserves financial obligations and prevents unilateral fund seizure. Merchants and subscribers can still cancel subscriptions through normal flows.

### 2. Multiple Merchants

**Behavior**: If a subscriber is blocklisted by one merchant, they cannot create subscriptions with ANY merchant (including unrelated ones).

**Rationale**: The blocklist is global at the contract level. Merchant authorization is for adding entries, not for scoping enforcement.

**Workaround**: If merchant-specific blocklisting is needed, merchants should implement their own off-chain filtering before calling the contract.

### 3. Deposit Restrictions

**Behavior**: Blocklisted subscribers cannot deposit funds into existing subscriptions.

**Rationale**: Prevents blocklisted users from extending their access through top-ups.

**Alternative**: If a blocklisted subscriber needs to top up an existing subscription, they must:
1. Request removal from blocklist (admin decision)
2. Or cancel and withdraw, then create a new subscription after removal

### 4. Charging Continues

**Behavior**: Existing subscriptions continue to be charged normally even after blocklisting.

**Rationale**: Honors existing financial commitments. If a merchant wants to stop charging, they should cancel the subscription.

### 5. Withdrawal Rights

**Behavior**: Blocklisted subscribers can withdraw remaining balance after cancellation.

**Rationale**: Prevents fund seizure. The blocklist is preventive, not punitive.

## Security Considerations

### 1. Authorization Checks

- All blocklist operations require proper authorization (admin or merchant)
- Merchant authorization is scoped to subscribers they have relationships with
- Removal is admin-only to prevent unauthorized unblocking

### 2. Storage Efficiency

- Blocklist uses O(1) lookup via direct address key
- No iteration required during subscription creation or deposits
- Minimal gas overhead for blocklist checks

### 3. Event Transparency

- All blocklist additions and removals emit events
- Events include reason metadata for audit trails
- Off-chain systems can monitor blocklist changes

### 4. No Retroactive Enforcement

- Blocklist does not cancel or modify existing subscriptions
- Prevents unexpected state changes for active subscriptions
- Maintains contract predictability

## Governance Recommendations

### For Admins

1. **Document Blocklist Reasons**: Always provide a reason when blocklisting
2. **Review Periodically**: Regularly review blocklist entries for removal eligibility
3. **Communicate Policy**: Publish clear blocklist criteria and appeal process
4. **Monitor Events**: Track blocklist events for audit and compliance

### For Merchants

1. **Exhaust Alternatives First**: Use blocklist as last resort after other dispute resolution
2. **Coordinate with Admin**: For serious fraud, escalate to admin for global blocklist
3. **Document Disputes**: Maintain off-chain records of disputes leading to blocklist
4. **Consider Impact**: Remember that merchant blocklisting affects all merchants globally

### For Subscribers

1. **Maintain Good Standing**: Avoid payment disputes and fraudulent activity
2. **Resolve Disputes**: Work with merchants to resolve issues before blocklisting
3. **Appeal Process**: Contact admin for blocklist removal if circumstances change
4. **Existing Rights**: Understand that existing subscriptions remain accessible

## Testing Coverage

The blocklist implementation includes comprehensive tests covering:

- ✅ Admin can add any subscriber to blocklist
- ✅ Merchant can blocklist their subscribers only
- ✅ Merchant cannot blocklist unrelated subscribers
- ✅ Blocklisted subscribers cannot create subscriptions
- ✅ Blocklisted subscribers cannot create subscriptions from plans
- ✅ Blocklisted subscribers cannot deposit funds
- ✅ Existing subscriptions are preserved after blocklisting
- ✅ Blocklisted subscribers can cancel existing subscriptions
- ✅ Blocklisted subscribers can withdraw after cancellation
- ✅ Admin can remove from blocklist
- ✅ Removed subscribers can create subscriptions again
- ✅ Non-admin cannot remove from blocklist
- ✅ Blocklist entry not found returns error
- ✅ Events are emitted correctly
- ✅ Multiple subscriptions are preserved
- ✅ Charging continues for existing subscriptions
- ✅ Reason metadata is stored correctly
- ✅ Blocklist without reason works correctly

## Future Enhancements

Potential future improvements (not in current implementation):

1. **Merchant-Scoped Blocklist**: Allow merchants to maintain separate blocklists that only affect their own subscriptions
2. **Temporary Blocklist**: Support time-limited blocklist entries that auto-expire
3. **Blocklist Categories**: Different blocklist types with different enforcement rules
4. **Batch Operations**: Bulk add/remove operations for efficiency
5. **Blocklist Export**: Admin endpoint to export full blocklist for compliance reporting
