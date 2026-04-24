# Admin Authorization Matrix

This document defines the expected authorization behavior for every admin-only
entrypoint in `subscription_vault`.

## Error semantics

- `Unauthorized` (`401`): the caller signed the transaction, but the signed
  address is not the stored admin for an admin-only route.
- `Forbidden` (`403`): the caller is authenticated, but the route is governed
  by a different role model entirely, such as subscriber-or-merchant ownership.
- Host auth failure (`Error(Auth, InvalidAction)` in tests): no valid signature
  was supplied for a required `require_auth()` check.

Admin-only routes should not return `Forbidden`; they either succeed for the
stored admin, return `Unauthorized` for a stale or non-admin signer, or fail at
the host layer when no signature is present.

## Matrix

| Entrypoint | Authorization model | Non-admin result | Stale admin after rotation | Notes |
| --- | --- | --- | --- | --- |
| `set_min_topup` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Centralized via `require_admin_auth` |
| `rotate_admin` | Current admin must match stored admin | `Unauthorized` | `Unauthorized` | New admin takes effect immediately |
| `recover_stranded_funds` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Recovery reason still validated separately |
| `add_accepted_token` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Token config only |
| `remove_accepted_token` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Default token removal still rejected as `InvalidInput` |
| `enable_emergency_stop` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Idempotent when already enabled |
| `disable_emergency_stop` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Idempotent when already disabled |
| `export_contract_snapshot` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Export-only surface |
| `export_subscription_summary` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Single-subscription export |
| `export_subscription_summaries` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Paged export |
| `set_subscriber_credit_limit` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Subscriber risk control |
| `partial_refund` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Subscriber parameter mismatch is also `Unauthorized` |
| `set_billing_retention` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Statement retention policy |
| `compact_billing_statements` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Maintenance operation |
| `set_oracle_config` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Config validation happens after auth |
| `remove_from_blocklist` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Uses the same centralized guard |
| `set_protocol_fee` | Explicit `admin` arg must equal stored admin | `Forbidden` | `Forbidden` | Uses direct admin check, returns `Forbidden` not `Unauthorized` |
| `batch_charge` | Stored admin is loaded from state and must sign | Host auth failure if unsigned | New admin signature required after rotation | No caller-supplied admin parameter |

## Rotation and replay notes

- Admin rotation is atomic: once `rotate_admin` succeeds, the old admin loses
  access to all admin-only routes in the same state version.
- Reusing an old auth context after rotation must fail as `Unauthorized` for
  explicit-admin routes.
- `batch_charge` is the one exception to the explicit-admin pattern because it
  reads the stored admin internally; after rotation, only the new stored admin's
  signature satisfies the host auth check.

## Reviewer Checklist for New Admin Entrypoints

When adding new admin-only entrypoints, reviewers must verify:

### ✅ Authorization Implementation
- [ ] **Correct auth pattern used**: Either `require_admin_auth(&env, &admin)` for explicit admin parameter OR `require_stored_admin_auth(&env)` for stored admin loading
- [ ] **Consistent error handling**: Returns `Error::Unauthorized` for `require_admin_auth` calls, `Error::Forbidden` for direct admin checks
- [ ] **Auth check placement**: Authorization is validated BEFORE any state mutations or external calls
- [ ] **Parameter validation**: Input validation happens AFTER authorization checks

### ✅ Matrix Documentation
- [ ] **Entry added to matrix**: New function documented in the matrix table above
- [ ] **Authorization model correctly described**: Explicit admin parameter vs stored admin loading
- [ ] **Error behavior documented**: What happens with non-admin callers and stale admin after rotation
- [ ] **Notes section populated**: Special behaviors, edge cases, or implementation details

### ✅ Security Considerations
- [ ] **Reentrancy protection**: If the function performs token transfers, it must use `ReentrancyGuard::lock()`
- [ ] **Emergency stop compliance**: Function respects `require_not_emergency_stop()` if it should be disabled during emergency stop
- [ ] **Atomic operations**: State changes are atomic and don't leave partially updated state
- [ ] **Event emission**: Appropriate events are emitted for audit trails

### ✅ Testing Requirements
- [ ] **Authorization tests**: Tests verify both successful admin calls and unauthorized failures
- [ ] **Rotation tests**: Tests verify behavior after admin rotation (stale admin should fail)
- [ ] **Edge case tests**: Tests cover boundary conditions and error scenarios
- [ ] **Integration tests**: Function tested in realistic scenarios with other contract operations

### ✅ Code Quality
- [ ] **Function documentation**: Clear doc comments explaining purpose, parameters, errors, and auth requirements
- [ ] **Error messages**: Descriptive error variants for different failure modes
- [ ] **Naming conventions**: Function name clearly indicates admin-only nature
- [ ] **Modular design**: Implementation follows existing patterns in the codebase

### 🚫 Security Anti-Patterns (Must NOT exist)
- [ ] **No auth bypasses**: No code paths that skip authorization checks
- [ ] **No hardcoded admin**: No hardcoded addresses or special cases
- [ ] **No auth after state changes**: Authorization never happens after state mutations
- [ ] **No inconsistent error types**: All admin auth failures use consistent error types

### 📝 Documentation Checklist
- [ ] **README updated**: If the function changes contract behavior, update relevant README sections
- [ ] **Migration docs**: If function affects storage schema, update migration documentation
- [ ] **API docs**: Update any external API documentation or client SDKs
- [ ] **Changelog**: Add entry to changelog documenting the new admin capability

### ⚡ Performance Considerations
- [ ] **Gas efficiency**: Authorization checks are optimized and not duplicated unnecessarily
- [ ] **Storage reads**: Minimal storage access for authorization validation
- [ ] **Batch operations**: If applicable, consider batch variants for multiple operations

### 🔍 Review Process
1. **Code review**: Verify all checklist items above
2. **Security review**: Focus on authorization bypasses and edge cases
3. **Documentation review**: Ensure matrix and docs are accurate
4. **Test review**: Verify comprehensive test coverage
5. **Integration review**: Check compatibility with existing admin functions

**Note**: This checklist should be referenced for every PR that adds or modifies admin-only entrypoints. Missing items should be addressed before merge approval.
