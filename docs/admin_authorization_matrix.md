# Admin Authorization Matrix

This document defines the expected authorization behavior for every admin-only
and operator entrypoint in `subscription_vault`.

## Roles

| Role | Description |
|------|-------------|
| **Admin** | Full governance. Stored at `DataKey::Admin`. Set at `init`; changed via `rotate_admin`. |
| **Operator** | Least-privilege billing agent. Stored at `DataKey::Operator`. Set/removed by the admin only. May execute charge operations; has no governance, withdrawal, or config access. |

## Error semantics

- `Unauthorized` (`1001`): the caller signed the transaction, but the signed
  address does not match the required role (admin or operator).
- `Forbidden` (`1002`): the caller is authenticated, but the route is governed
  by a different role model entirely, such as subscriber-or-merchant ownership.
- Host auth failure (`Error(Auth, InvalidAction)` in tests): no valid signature
  was supplied for a required `require_auth()` check.

Admin-only routes return `Unauthorized` for a stale or non-admin signer, or fail at
the host layer when no signature is present.

## Admin entrypoint matrix

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
| `batch_charge` | Stored admin is loaded from state and must sign | Host auth failure if unsigned | New admin signature required after rotation | No caller-supplied admin parameter; nonce domain 0 |
| **`set_operator`** | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Admin manages operator lifecycle |
| **`remove_operator`** | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | No-op when no operator is set |

## Operator entrypoint matrix

| Entrypoint | Authorization model | Non-operator result | After `remove_operator` | Notes |
| --- | --- | --- | --- | --- |
| `operator_batch_charge` | Explicit `operator` arg must equal stored operator | `Unauthorized` | `Unauthorized` | Nonce domain 2 (`DOMAIN_OPERATOR_BATCH_CHARGE`); emergency-stop gated |
| `operator_charge_subscription` | Explicit `operator` arg must equal stored operator | `Unauthorized` | `Unauthorized` | Emergency-stop gated; reentrancy guard |
| `operator_charge_usage` | Explicit `operator` arg must equal stored operator | `Unauthorized` | `Unauthorized` | Emergency-stop gated; reentrancy guard |
| `operator_charge_usage_with_reference` | Explicit `operator` arg must equal stored operator | `Unauthorized` | `Unauthorized` | Emergency-stop gated; reentrancy guard |

### Read-only operator helpers (no auth)

| Entrypoint | Returns |
| --- | --- |
| `get_operator` | `Option<Address>` — current stored operator, or `None` |
| `get_operator_nonce(op)` | `u64` — next expected nonce for `operator_batch_charge` |

## Operator security notes

### Privilege isolation

The operator has access to **exactly four** charge entrypoints and **zero**
governance or financial-management entrypoints. The privilege boundary is
enforced structurally: operator charge functions call `require_operator_auth`
which is a separate code path from `require_admin_auth`. There is no shared
fallback that would allow the operator to elevate to admin.

Privilege escalation vectors that are explicitly blocked:

| Attempted escalation | Result |
|---|---|
| Operator calls `set_operator` | `Unauthorized` |
| Operator calls `remove_operator` | `Unauthorized` |
| Operator calls any governance op | `Unauthorized` |
| Operator calls `recover_stranded_funds` | `Unauthorized` |
| Operator calls `set_min_topup` | `Unauthorized` |

### Revocation is immediate

`remove_operator` clears `DataKey::Operator` in the same transaction. Any
subsequent call to an `operator_*` entrypoint by the revoked address will fail
with `Unauthorized` because `require_operator_auth` reads the stored key on
every invocation; there is no cached session state.

### Replay protection for `operator_batch_charge`

`operator_batch_charge` uses a monotonic nonce stored under
`DataKey::AdminNonce(operator, DOMAIN_OPERATOR_BATCH_CHARGE)`. Domain `2` is
separate from the admin batch-charge domain (`0`) and the admin rotation domain
(`1`). A captured operator batch-charge transaction cannot be replayed as an
admin operation or rotation.

### Admin rotation and operator continuity

`rotate_admin` does **not** touch `DataKey::Operator`. After rotation:
- The existing operator continues to function unchanged.
- The **new admin** can call `set_operator` or `remove_operator` to update it.
- The **old admin** cannot call `set_operator` or `remove_operator` (returns `Unauthorized`).

This design gives the new admin full control over the operator key without
requiring an extra migration step.

### Storage

| Key | Tier | Value |
|-----|------|-------|
| `DataKey::Operator` (discriminant 38) | instance | `Address` — the stored operator, absent when unset |
| `DataKey::AdminNonce(operator, 2)` | persistent | `u64` — next expected operator batch-charge nonce |

## Rotation and replay notes

- Admin rotation is atomic: once `rotate_admin` succeeds, the old admin loses
  access to all admin-only routes in the same state version.
- Reusing an old auth context after rotation must fail as `Unauthorized` for
  explicit-admin routes and host auth failure for stored-admin routes.
- `batch_charge` reads the stored admin internally; after rotation, only the
  new stored admin's signature satisfies the host auth check.
- `operator_batch_charge` reads the stored operator internally; after
  `remove_operator`, any call fails immediately regardless of the nonce.

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
