# Expiration Rules and Cleanup Semantics

This document outlines the expiration lifecycle, cleanup mechanisms, and fund safety guarantees for subscriptions in the StellaBill system.

## 1. Expiration Model

Subscriptions carry two independent expiration bounds. Either being met is sufficient to consider the subscription expired for charging, deposit, and state-transition purposes.

| Field | Type | Meaning |
|---|---|---|
| `start_time` | `u64` | Timestamp when the subscription was created. |
| `expires_at` | `Option<u64>` | Wall-clock bound. `None` means no time-based expiration. |
| `expires_at_ledger` | `Option<u32>` | Ledger-sequence bound. `None` means no sequence-based expiration. |

A subscription is considered expired when:
```rust
current_time    >= expires_at        // wall-clock bound
    OR
current_ledger >= expires_at_ledger // ledger-sequence bound
```
(Each `None` disables its respective check.)

The dual-bound model supports two distinct use-cases:

- **`expires_at`** — wall-clock guarantees for time-sensitive plans (subscriptions, monthly billings).
- **`expires_at_ledger`** — deterministic termination for **auction-style** plans (a fixed slot count regardless of network speed) and **testnet reproducibility** (the test terminates at a known slot regardless of clock drift). Either or both bounds may be set.

## 2. State Transitions

Subscriptions transition through several states, but expiration introduces explicit guards:

- **Active**: The subscription is actively charging and valid.
- **Expired**: Evaluated dynamically based on `is_expired()`. This state takes precedence over active billing operations.
- **Cancelled**: A terminal state explicitly triggered by the user or system.
- **Archived**: A clean-up state that preserves essential data and allows fund withdrawals while preventing all other operations.

**Important Distinction:**
- **Expired** is automatic. A subscription whose `expires_at` (wall-clock) **or** `expires_at_ledger` (sequence) is reached is immediately ineligible for charging, even if its state is nominally `Active`.
- **Cancelled** is user-driven or system-driven (e.g., reaching a lifetime cap).
- These states are mutually exclusive in behavior. An expired subscription cannot be cancelled, but both can be **Archived**.

## 3. Expiration Effects

When a subscription is expired (`is_expired == true`):

- **Rejected Operations**:
  - New periodic charges (`charge_subscription`)
  - New usage-based charges (`charge_usage`)
  - New fund deposits (`deposit_funds`)
  - Explicit cancellation (`cancel_subscription`)

- **Allowed Operations**:
  - Subscriber fund withdrawals (`withdraw_subscriber_funds`)
  - Metadata reads and general state queries
  - Archival cleanup (`cleanup_subscription`)

## 4. Ledger-Sequence Expiration Bound

The ledger-sequence bound (`expires_at_ledger`) provides a deterministic, time-independent termination condition. It is set in either of two ways:

1. **At creation** — pass `Some(seq)` as the 9th argument to `create_subscription` / `create_subscription_with_token`. The contract rejects `seq <= current_ledger` with `Error::InvalidExpiration` (zombie prevention).
2. **After creation** — call `set_subscription_expiration_ledger(subscription_id, authorizer, Some(seq))`. Authorized by the subscriber **or** the merchant (mirroring the auth surface of `cancel_subscription`, `pause_subscription`, and `schedule_cancel`). Pass `None` to clear an existing bound.

### Setter validation
- Subscription must exist and not be in a terminal state (`Cancelled` / `Expired` / `Archived`).
- `Some(seq)` must be strictly greater than the current ledger sequence.

### Event
`ExpirationLedgerSetEvent { subscription_id, expires_at_ledger, previous_expires_at_ledger, authorizer, timestamp, schema_version }` is emitted on every successful call. The `previous_expires_at_ledger` field lets indexers reconstruct the bound's lifecycle even when `None` clears an existing value.

## 5. Cleanup Semantics & Archival Strategy

Instead of deleting expired or cancelled subscriptions (which could corrupt the state and lead to fund loss), StellaBill uses an **Archival Strategy**.

The `cleanup_subscription` function allows moving a terminal subscription (either Cancelled or Expired) into the `Archived` state.

### Archival Guarantees:
- **No Deletion**: The subscription entity is preserved. Critical fields (balances, identities) remain intact.
- **Readability**: Archived entities can still be read by indexers and clients.
- **Safety**: Moving to `Archived` enforces strict terminal behavior, ensuring no accidental resumption or modification.

## 6. Fund Safety Guarantee

A core invariant of the StellaBill protocol is that **funds are never deleted**.
- If a subscription expires or is archived, any remaining escrowed funds in `prepaid_balance` remain assigned to that subscription.
- The `withdraw_subscriber_funds` function explicitly permits withdrawals when the status is `Expired`, `Cancelled`, or `Archived`.
- This ensures subscribers can always retrieve their unused prepaid balances, regardless of the subscription's terminal state.

## 7. Examples

### Flow 1: Expiration without Cancellation
1. Subscription created with `expires_at = T` (and optionally `expires_at_ledger = S`).
2. Time passes. Current time becomes `>= T` (or current ledger sequence becomes `>= S`).
3. The subscription is now automatically **Expired**. New charges fail.
4. The user or merchant calls `cleanup_subscription`.
5. State transitions to **Archived**.
6. The user withdraws their remaining funds.

### Flow 2: Cancellation before Expiration
1. Subscription created with `expires_at = T` (and optionally `expires_at_ledger = S`).
2. Current time is `< T` **and** current sequence is `< S`. User calls `cancel_subscription`.
3. State explicitly transitions to **Cancelled**.
4. The user or merchant calls `cleanup_subscription`.
5. State transitions to **Archived**.
6. User withdraws funds.

### Flow 3: Ledger-Bound Only (Auction / Testnet)
1. Subscription created with `expires_at = None, expires_at_ledger = S`.
2. Charges succeed while `current_ledger < S`.
3. Once `current_ledger >= S`, every charge / deposit / cancel attempt is rejected.
4. Cleanup → Archived → withdraw.

## 8. Storage Migration

Adding `expires_at_ledger` is a schema change. The contract's `STORAGE_VERSION` constant is bumped to **4** in this release, and the `v3 → v4` step in [`admin::do_migrate`](../contracts/subscription_vault/src/admin.rs) walks every `DataKey::Sub(id)` record and rewrites it so the new trailing field deserializes cleanly.

**Operators MUST call `migrate(admin)` once after deploying the new binary** to prevent `get_subscription` from panicking on existing records. The migration is idempotent and only touches the persistent storage tier.

## 9. Indexer Guidance

Indexers tracking the state of subscriptions should:
1. Always compute `is_expired = (current_time >= expires_at) OR (current_ledger >= expires_at_ledger)` when displaying active subscriptions. Either bound being met terminates charging.
2. Treat `Archived` subscriptions as immutable, terminal records.
3. Monitor `SubscriptionExpiredEvent`, `SubscriptionArchivedEvent`, and `ExpirationLedgerSetEvent` to trigger backend cleanups or UI updates.
4. Use `previous_expires_at_ledger` in `ExpirationLedgerSetEvent` to reconstruct the bound's lifecycle when `None` clears an existing value.
