# Admin Rotation and Access Control Tests

## Overview

This document describes the comprehensive test coverage for the admin rotation functionality in the Subscription Vault contract. Admin rotation is a critical security feature that enables smooth transfer of administrative privileges while ensuring that only the current administrator can perform privileged operations.

## Purpose

The admin rotation tests validate:

1. **Secure Transfer**: Only the current admin can rotate to a new admin
2. **Immediate Revocation**: Previous admins immediately lose all privileges after rotation
3. **Immediate Grant**: New admins immediately gain all privileges after rotation
4. **Access Control Integrity**: All admin-protected operations respect the current admin
5. **Subscription Isolation**: Admin changes do not affect user subscriptions

## Admin-Protected Operations

The following operations require administrator authorization:

1. **`set_min_topup()`** - Configure the minimum top-up amount
2. **`recover_stranded_funds()`** - Recover funds in emergency scenarios
3. **`rotate_admin()`** - Transfer administrative privileges to a new address

## Test Coverage

### Basic Functionality Tests

#### `test_get_admin()`

- **Purpose**: Verify that `get_admin()` returns the correct admin address
- **Coverage**: Admin address storage and retrieval
- **Expected Behavior**: Returns the admin set during contract initialization

#### `test_rotate_admin_successful()`

- **Purpose**: Verify successful admin rotation from old to new admin
- **Coverage**: Basic rotation flow
- **Expected Behavior**:
  - Old admin can initiate rotation
  - Admin address updates to new admin
  - `get_admin()` returns new admin after rotation

#### `test_rotate_admin_unauthorized()`

- **Purpose**: Verify unauthorized users cannot rotate admin
- **Coverage**: Authorization checks in `rotate_admin()`
- **Expected Behavior**: Transaction panics with `Error(Contract, #401)` (Unauthorized)

### Access Control Tests

#### `test_old_admin_loses_access_after_rotation()`

- **Purpose**: Verify old admin cannot perform admin operations after rotation
- **Coverage**: Immediate privilege revocation
- **Expected Behavior**: Old admin's `set_min_topup()` call fails after rotation
- **Security Implication**: Previous admins cannot retain backdoor access

#### `test_new_admin_gains_access_after_rotation()`

- **Purpose**: Verify new admin can perform admin operations immediately
- **Coverage**: Immediate privilege grant
- **Expected Behavior**: New admin can successfully call `set_min_topup()` after rotation

#### `test_set_min_topup_unauthorized_before_rotation()`

- **Purpose**: Verify non-admins cannot set min topup before rotation
- **Coverage**: Authorization baseline for non-admin addresses
- **Expected Behavior**: Non-admin's `set_min_topup()` call fails

#### `test_set_min_topup_unauthorized_after_rotation()`

- **Purpose**: Verify non-admins and old admin cannot set min topup after rotation
- **Coverage**: Authorization persistence after rotation
- **Expected Behavior**:
  - Non-admin's call fails
  - Old admin's call also fails

#### `test_recover_stranded_funds_unauthorized_before_rotation()`

- **Purpose**: Verify non-admins cannot recover funds before rotation
- **Coverage**: Recovery operation authorization baseline
- **Expected Behavior**: Non-admin's recovery call fails

#### `test_recover_stranded_funds_unauthorized_after_rotation()`

- **Purpose**: Verify non-admins and old admin cannot recover funds after rotation
- **Coverage**: Recovery operation authorization after rotation
- **Expected Behavior**:
  - Non-admin's recovery call fails
  - Old admin's recovery call also fails

### Integration Tests

#### `test_admin_rotation_affects_recovery_operations()`

- **Purpose**: Verify recovery operations respect admin rotation
- **Coverage**: Integration between rotation and recovery
- **Expected Behavior**:
  1. Old admin can recover funds before rotation
  2. Rotation executes successfully
  3. Old admin cannot recover funds after rotation
  4. New admin can recover funds after rotation

#### `test_all_admin_operations_after_rotation()`

- **Purpose**: Verify new admin can perform all admin operations
- **Coverage**: Complete privilege transfer
- **Expected Behavior**:
  - New admin can set min topup
  - New admin can recover stranded funds
  - New admin can rotate to another admin

#### `test_multiple_admin_rotations()`

- **Purpose**: Verify chained rotations work correctly (A→B→C→D)
- **Coverage**: Sequential rotations
- **Expected Behavior**:
  1. Each rotation updates admin correctly
  2. Only the final admin has access
  3. All previous admins are denied access

#### `test_admin_cannot_be_rotated_by_previous_admin()`

- **Purpose**: Verify previous admins cannot perform subsequent rotations
- **Coverage**: Rotation authorization after loss of privileges
- **Expected Behavior**:
  1. Admin1 rotates to Admin2
  2. Admin1 cannot rotate to Admin3
  3. Admin remains as Admin2

### State Isolation Tests

#### `test_admin_rotation_does_not_affect_subscriptions()`

- **Purpose**: Verify subscriptions are unaffected by admin rotation
- **Coverage**: Subscription data isolation
- **Expected Behavior**:
  - Subscription data remains unchanged before and after rotation
  - All subscription fields preserved (subscriber, merchant, amount, status)

#### `test_admin_rotation_with_subscriptions_active()`

- **Purpose**: Verify subscription operations work normally after rotation
- **Coverage**: Subscription lifecycle with admin changes
- **Expected Behavior**:
  - Subscriptions can be paused before rotation
  - Subscriptions retain state after rotation
  - Subscribers can manage subscriptions after rotation (resume, cancel)

### Comprehensive Tests

#### `test_admin_rotation_access_control_comprehensive()`

- **Purpose**: End-to-end verification of access control through multiple rotations
- **Coverage**: Complete access control matrix across three phases
- **Test Phases**:
  1. **Phase 1 (Admin1 active)**:
     - Admin1 can set min topup ✓
     - Admin2 cannot set min topup ✗
     - Non-admin cannot set min topup ✗
  2. **Phase 2 (Admin2 active after rotation)**:
     - Admin2 can set min topup ✓
     - Admin1 cannot anymore ✗
     - Non-admin still cannot ✗
  3. **Phase 3 (Admin3 active after second rotation)**:
     - Admin3 can set min topup ✓
     - Admin1 and Admin2 cannot ✗
     - Non-admin still cannot ✗

### Edge Case Tests

#### `test_rotate_admin_to_same_address()`

- **Purpose**: Verify rotation to same address is idempotent
- **Coverage**: Self-rotation edge case
- **Expected Behavior**:
  - Rotation succeeds without error
  - Admin remains unchanged
  - Admin retains all privileges

#### `test_get_admin_before_and_after_rotation()`

- **Purpose**: Verify `get_admin()` tracks rotations correctly
- **Coverage**: Admin getter accuracy across rotations
- **Expected Behavior**:
  - Returns old admin before rotation
  - Returns new admin after rotation
  - Updates correctly through multiple rotations

#### `test_admin_rotation_event_emission()`

- **Purpose**: Verify admin rotation emits events for audit trail
- **Coverage**: Event emission for observability
- **Expected Behavior**: Events are emitted during rotation

## Test Statistics

- **Total Admin Rotation Tests**: 19
- **Test Categories**:
  - Basic Functionality: 3 tests
  - Access Control: 6 tests
  - Integration: 4 tests
  - State Isolation: 2 tests
  - Comprehensive: 1 test
  - Edge Cases: 3 tests
- **Overall Test Suite**: 92 tests (100% passing)
- **Code Coverage**: >95% of admin-related code paths

## Security Considerations

### Immediate Revocation

All tests verify that admin privilege revocation is immediate and cannot be bypassed:

- Previous admins cannot perform any admin operations after rotation
- No grace period or delayed revocation
- Authorization checks happen on every admin-protected call

### Authorization Model

The admin rotation follows a strict authorization model:

1. Only the **current** admin (stored in contract storage) can rotate
2. The caller must authenticate with `require_auth()`
3. The authenticated address must match the stored admin
4. Any mismatch results in `Error::Unauthorized`

### Access Control Matrix

| Operation                  | Current Admin | Previous Admin | Non-Admin |
| -------------------------- | ------------- | -------------- | --------- |
| `rotate_admin()`           | ✓ Allowed     | ✗ Denied       | ✗ Denied  |
| `set_min_topup()`          | ✓ Allowed     | ✗ Denied       | ✗ Denied  |
| `recover_stranded_funds()` | ✓ Allowed     | ✗ Denied       | ✗ Denied  |

### Subscription Operations Unaffected

Admin rotation has no impact on user operations:

- Subscribers can create, pause, resume, cancel subscriptions
- State transitions continue to work normally
- No interruption to subscription lifecycle

## Best Practices

### For Contract Deployers

1. **Secure Initial Admin**: Set admin to a secure multisig or governance address during `init()`
2. **Test Rotation**: Test rotation in staging environment before production use
3. **Monitor Events**: Listen for `admin_rotation` events for audit trail
4. **Gradual Migration**: Rotate admin during low-activity periods

### For Testing

1. **Test All Protected Operations**: Verify every admin-protected function respects rotation
2. **Test Unauthorized Access**: Verify both previous admins and non-admins are blocked
3. **Test Sequential Rotations**: Verify chained rotations work correctly
4. **Test Idempotency**: Verify self-rotation is safe

### For Governance

1. **Document Rotation Policy**: Define when and how admin rotation occurs
2. **Multi-Signature Admins**: Use multisig addresses for production admin
3. **Emergency Procedures**: Document emergency rotation procedures
4. **Audit Trail**: Maintain off-chain record of all admin rotations

## Integration with Other Features

### Interaction with Recovery Mechanism

- Admin rotation immediately transfers recovery privileges
- Previous admins cannot recover funds after rotation
- See `docs/recovery.md` for recovery mechanism details

### Interaction with Min Topup Configuration

- Admin rotation immediately transfers config privileges
- Previous admins cannot update min topup after rotation
- Min topup value persists across rotations

### Event Emission

- Rotation events include old admin, new admin, and timestamp
- Events support off-chain monitoring and audit systems
- Event format aligns with other contract events

## Implementation Details

### Admin Storage

```rust
const ADMIN: Symbol = symbol_short!("admin");
storage.instance().set(&ADMIN, &admin);
```

### Authorization Check Pattern

```rust
current_admin.require_auth();
let stored_admin: Address = env.storage().instance()
    .get(&ADMIN)
    .unwrap_or_else(|| panic_with_error!(&env, Error::NotFound));

if current_admin != stored_admin {
    return Err(Error::Unauthorized);
}
```

### Rotation Implementation

```rust
pub fn rotate_admin(env: Env, current_admin: Address, new_admin: Address)
    -> Result<(), Error>;
```

### Admin Getter

```rust
pub fn get_admin(env: Env) -> Address;
```

## Related Documentation

- [Admin Recovery Mechanism](./recovery.md) - Admin recovery of stranded funds
- [Subscription State Machine](./subscription_state_machine.md) - State transitions
- [Usage Flag](./usage_flag.md) - Usage-based billing

## Future Enhancements

Potential improvements to admin rotation (not currently implemented):

1. **Time-Locked Rotation**: Add a delay between rotation initiation and execution
2. **Multi-Step Rotation**: Require new admin acceptance before rotation completes
3. **Rotation Limits**: Add rate limiting for rotations
4. **Admin History**: Store historical admin addresses for audit
5. **Role-Based Access**: Separate admin into multiple roles with different privileges

These enhancements would require careful security analysis and testing before implementation.
