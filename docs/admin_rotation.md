# Admin Rotation and Access Control

## Overview

The Subscription Vault contract uses a single admin address stored in instance
storage under `DataKey::Admin`. Admin rotation allows the current administrator
to transfer all privileges to a new address in a single atomic transaction.
All admin-protected operations read from this single canonical key.

## Admin-Protected Operations

| Operation | Auth pattern | Purpose |
|-----------|-------------|---------|
| `set_min_topup` | explicit admin param | Configure minimum deposit amount |
| `set_grace_period` | explicit admin param | Configure grace period duration |
| `add_accepted_token` | explicit admin param | Add a new accepted payment token |
| `remove_accepted_token` | explicit admin param | Remove an accepted payment token |
| `recover_stranded_funds` | explicit admin param | Recover excess funds in emergency scenarios |
| `rotate_admin` | explicit admin param | Transfer administrative privileges |
| `enable_emergency_stop` | explicit admin param | Pause all critical operations |
| `disable_emergency_stop` | explicit admin param | Resume normal operations |
| `set_protocol_fee` | explicit admin param | Configure protocol fee and treasury |
| `add_to_blocklist` | explicit admin param | Block a subscriber (admin path) |
| `remove_from_blocklist` | explicit admin param | Unblock a subscriber |
| `batch_charge` | stored admin auth | Charge multiple subscriptions |
| `charge_subscription` | stored admin auth | Charge a single subscription |
| `charge_usage` | stored admin auth | Charge usage-based billing |
| `export_contract_snapshot` | explicit admin param | Export audit snapshot |
| `set_billing_retention` | explicit admin param | Configure billing retention |
| `set_oracle_config` | explicit admin param | Configure oracle pricing |

**Auth patterns:**
- **Explicit admin param**: caller passes `admin: Address`; contract calls
  `admin.require_auth()` then checks `admin == stored_admin`.
- **Stored admin auth**: contract loads the stored admin, calls `require_auth()`
  on it. After rotation the stored admin is the new admin, so only the new
  admin's signature satisfies the check.

## Rotation Procedure

### Prerequisites

- You are the **current admin** (the address stored under `DataKey::Admin`).
- You hold the signing keys or multisig authorization for the current admin.
- You know the current nonce for `(current_admin, DOMAIN_ADMIN_ROTATION)`.
  Read it with `get_admin_nonce(current_admin, 1)`.

### Function signature

```rust
pub fn rotate_admin(
    env: Env,
    current_admin: Address,   // must match stored admin; must sign the tx
    new_admin: Address,       // the address that will become admin
    nonce: u64,               // must equal get_admin_nonce(current_admin, 1)
) -> Result<(), Error>
```

### Steps

1. **Query the current nonce**
   ```
   nonce = get_admin_nonce(current_admin, 1)   // domain 1 = DOMAIN_ADMIN_ROTATION
   ```

2. **Call `rotate_admin`**
   - `current_admin`: the currently stored admin address (must sign the tx).
   - `new_admin`: the address that will become admin.
   - `nonce`: the value returned in step 1.

3. **Effect**
   - `DataKey::Admin` is updated to `new_admin` atomically.
   - `current_admin`'s rotation nonce advances to `nonce + 1`.
   - An `admin_rotated` event is emitted (see below).
   - Previous admin loses all privileges instantly; new admin gains them
     in the same transaction.

### Post-Rotation

- Confirm with `get_admin()`.
- Monitor `admin_rotated` events for audit and indexing.

## Event Payload

Every successful rotation emits an `AdminRotatedEvent` under the
`admin_rotated` topic:

```json
{
  "topic": "admin_rotated",
  "old_admin": "<Address>",
  "new_admin": "<Address>",
  "timestamp": <u64>
}
```

Rust type:
```rust
pub struct AdminRotatedEvent {
    pub old_admin: Address,
    pub new_admin: Address,
    pub timestamp: u64,
}
```

A `NonceConsumedEvent` is emitted first (by the replay-protection layer):

```json
{
  "topic": "nonce_consumed",
  "signer": "<current_admin Address>",
  "domain": 1,
  "nonce": <u64>,
  "timestamp": <u64>
}
```

## Nonce Protection

`rotate_admin` is protected by a per-`(signer, domain)` monotonic counter
stored in persistent storage.

- **Domain**: `DOMAIN_ADMIN_ROTATION = 1` (separate from `DOMAIN_BATCH_CHARGE = 0`).
- **Starting value**: `0` for every new address.
- **Acceptance rule**: `provided_nonce == stored_nonce` exactly; any other value
  returns `Error::NonceAlreadyUsed`.
- **Advance rule**: on success the counter is incremented to `stored + 1` before
  the event is emitted (effects-before-interactions).
- **Per-address independence**: each admin address has its own counter. A newly
  appointed admin always starts at `0`, independent of any predecessor.

This prevents:
1. **Replay attacks** — resubmitting a captured transaction after a key compromise.
2. **Cross-domain collisions** — a batch-charge nonce cannot be reused for rotation.
3. **Out-of-order execution** — skipping nonces is rejected, preserving sequencing.

## Risks

### Irreversibility

Rotation is **irreversible** without the new admin's cooperation. If the new
admin's keys are lost, privileged operations cannot be performed.

### No Grace Period

The change takes effect in the same transaction. There is no delay or
confirmation step. Verify `new_admin` carefully before submitting.

### Self-rotation and Contract Locking

Both are explicitly rejected:
- `new_admin == current_admin` → `Error::SelfRotation`
- `new_admin == env.current_contract_address()` → `Error::InvalidNewAdmin`

## Access Control Matrix

| Operation | Current Admin | Previous Admin | Non-Admin |
|-----------|:---:|:---:|:---:|
| `rotate_admin` | ✓ | ✗ `Unauthorized` | ✗ `Unauthorized` |
| `set_min_topup` | ✓ | ✗ `Unauthorized` | ✗ `Unauthorized` |
| `set_grace_period` | ✓ | ✗ `Unauthorized` | ✗ `Unauthorized` |
| `add_accepted_token` | ✓ | ✗ `Unauthorized` | ✗ `Unauthorized` |
| `remove_accepted_token` | ✓ | ✗ `Unauthorized` | ✗ `Unauthorized` |
| `recover_stranded_funds` | ✓ | ✗ `Unauthorized` | ✗ `Unauthorized` |
| `enable_emergency_stop` | ✓ | ✗ `Unauthorized` | ✗ `Unauthorized` |
| `disable_emergency_stop` | ✓ | ✗ `Unauthorized` | ✗ `Unauthorized` |
| `set_protocol_fee` | ✓ | ✗ `Unauthorized` | ✗ `Unauthorized` |
| `batch_charge` | ✓ (via stored auth) | ✗ | ✗ |
| `charge_subscription` | ✓ (via stored auth) | ✗ | ✗ |
| `charge_usage` | ✓ (via stored auth) | ✗ | ✗ |

### Emergency Stop Interaction

`rotate_admin` is **not** gated by the emergency stop. This is intentional:
rotation is the primary mechanism for handing off control during an incident.
After rotation, the new admin can `disable_emergency_stop` to resume operations.

## Security Notes

### Single Canonical Key

There is exactly one admin stored at `DataKey::Admin`. There is no secondary
admin path, cached admin, or fallback. Every admin-protected operation reads
from this key.

### No Backdoor for Previous Admins

Once rotation completes, the previous admin's address returns
`Error::Unauthorized` on every admin-only entrypoint. There is no grace period,
delayed revocation, or override.

### Rotation Cannot Brick the Contract

Two guards prevent permanently locking admin access:
1. Self-rotation is rejected (`Error::SelfRotation`).
2. Rotation to the contract address is rejected (`Error::InvalidNewAdmin`),
   since the contract cannot produce Soroban auth signatures.

### Subscription and Balance Isolation

Rotation does not touch subscription storage, prepaid balances, or merchant
balances. Active subscriptions continue normally; pending charges remain
chargeable by the new admin via `batch_charge`.

## Best Practices

1. **Use a multisig or governance address** for the production admin.
2. **Query the nonce** with `get_admin_nonce(admin, 1)` before every rotation.
3. **Verify `new_admin`** controls the private key before signing.
4. **Monitor `admin_rotated` events** for real-time audit and alerting.
5. **Rotate during low-activity periods** to minimize operational risk.
6. **Maintain an off-chain record** of all rotations and admin addresses.

## Test Coverage

Admin rotation is covered by three test files:

| File | Focus |
|------|-------|
| `test_governance.rs` (module `admin_rotation_invariants`) | Invariant checks, event payload, emergency-stop integration, pending charges |
| `test_replay_protection.rs` | Nonce mechanics, domain separation, sequential nonces, event emission |
| `test.rs` (section `-- Admin tests --`) | Access control matrix, multi-rotation chains, subscription isolation |

Run all tests:

```bash
cargo test -p subscription_vault
```

Measure coverage:

```bash
cargo install cargo-tarpaulin
cargo tarpaulin -p subscription_vault --out Stdout
```

## Related Documentation

- [Admin Rotation Tests](./admin_rotation_tests.md) — Test coverage details.
- [Recovery](./recovery.md) — Admin recovery of stranded funds.
- [Events](./events.md) — Full event schema reference.
- [Admin Authorization Matrix](./admin_authorization_matrix.md) — Per-endpoint auth details.
