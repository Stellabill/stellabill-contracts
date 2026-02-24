# Admin Rotation and Access Control

## Overview

The Subscription Vault contract uses a single admin address stored in contract instance storage. Admin rotation allows the current administrator to transfer privileges to a new address via the `rotate_admin` entrypoint. All admin-protected operations read from this single source of truth.

## Admin-Protected Operations

The following operations require the caller to be the current admin:

| Operation | Purpose |
|-----------|---------|
| `set_min_topup` | Configure the minimum deposit amount for subscriptions |
| `recover_stranded_funds` | Recover funds in emergency scenarios (e.g., accidental transfers) |
| `batch_charge` | Charge multiple subscriptions in one transaction |
| `rotate_admin` | Transfer administrative privileges to a new address |

## Rotation Procedure

### Prerequisites

- You must be the **current admin** (address stored in contract storage).
- You must hold the signing keys or multisig authorization for the current admin address.

### Steps

1. **Prepare the new admin address**  
   Ensure the new admin address (e.g., multisig or EOA) is controlled and ready.

2. **Call `rotate_admin`**  
   ```rust
   rotate_admin(env, current_admin: Address, new_admin: Address) -> Result<(), Error>
   ```
   - `current_admin`: The address that is currently admin (must match storage).
   - `new_admin`: The address that will become admin.

3. **Authorization**  
   The transaction must be authorized by `current_admin` via Soroban `require_auth()`.

4. **Effect**  
   - Admin storage is updated to `new_admin` immediately.
   - An `admin_rotation` event is emitted with `(current_admin, new_admin, timestamp)`.
   - Previous admin loses all privileges instantly; new admin gains them immediately.

### Post-Rotation

- Use `get_admin()` to confirm the new admin address.
- Monitor `admin_rotation` events for audit and indexing.

## Risks

### Irreversibility

- Rotation is **irreversible** without the new admin's cooperation.
- If you rotate to the wrong address or lose access to the new admin keys, you cannot roll back.

### Loss of Access

- If keys to the **current** admin are lost before rotation, rotation cannot be performed.
- If keys to the **new** admin are lost after rotation, no one can rotate again or perform admin operations.

### No Grace Period

- There is no delay or confirmation step. The change takes effect in the same transaction.
- Ensure the new admin address is correct before submitting the transaction.

### Accidental Rotation

- Rotating to the zero address or an uncontrolled address permanently locks admin privileges.
- Always verify the `new_admin` address before calling `rotate_admin`.

## Security Model

### Prevents Admin Hijacking

The contract enforces:

1. **Authentication**: `current_admin.require_auth()` ensures the transaction is signed by the current admin.
2. **Storage check**: The caller's address must match the stored admin. Non-admins and previous admins cannot pass this check.
3. **Single source of truth**: All admin-protected operations read the admin from storage; there is no cached or alternate admin path.

### Access Control Matrix

| Operation | Current Admin | Previous Admin | Non-Admin |
|-----------|---------------|----------------|-----------|
| `rotate_admin` | Allowed | Denied | Denied |
| `set_min_topup` | Allowed | Denied | Denied |
| `recover_stranded_funds` | Allowed | Denied | Denied |
| `batch_charge` | Allowed | Denied | Denied |

## Best Practices

1. **Use a multisig or governance address** for production admin.
2. **Test rotation in staging** before performing it in production.
3. **Monitor `admin_rotation` events** for audit trails and alerting.
4. **Rotate during low-activity periods** to minimize operational risk.
5. **Document rotation policy** off-chain (who can propose, who approves, how often).
6. **Maintain an off-chain record** of all rotations and admin addresses.

## Test Coverage

Admin rotation and access control are covered by 20+ tests in the test suite. To verify:

```bash
cargo test -p subscription_vault
```

To measure coverage, install `cargo-tarpaulin` and run:

```bash
cargo install cargo-tarpaulin
cargo tarpaulin -p subscription_vault --out Stdout
```

## Related Documentation

- [Admin Rotation Tests](./admin_rotation_tests.md) – Test coverage and validation details.
- [Recovery](./recovery.md) – Admin recovery of stranded funds.
- [Events](./events.md) – Event schemas for `admin_rotation` and `recovery`.
