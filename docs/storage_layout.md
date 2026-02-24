# Storage Layout and Upgrade Strategy

This document describes the subscription vault contract's storage layout, keys, and safe upgrade procedures.

---

## Storage Overview

The contract uses Soroban's **instance storage** for all persistent data. Instance storage is tied to the contract instance and persists across invocations.

### Storage Type

- **Instance Storage**: All contract data uses `env.storage().instance()`
- **Persistence**: Data survives contract upgrades when keys remain compatible
- **Access Pattern**: Key-value store with typed keys and values

---

## Storage Keys and Data Types

### 1. Configuration Keys

| Key | Type | Value Type | Description |
|-----|------|------------|-------------|
| `"token"` | `Symbol` | `Address` | USDC token contract address |
| `"admin"` | `Symbol` | `Address` | Admin address (authorized for batch operations) |
| `"min_topup"` | `Symbol` | `i128` | Minimum deposit amount enforced |
| `"next_id"` | `Symbol` | `u32` | Auto-incrementing subscription ID counter |

**Storage Location**: `contracts/subscription_vault/src/admin.rs` (token, admin, min_topup), `contracts/subscription_vault/src/subscription.rs` (next_id)

**Initialization**: Set once via `init()`, `min_topup` updatable via `set_min_topup()`

---

### 2. Subscription Records

| Key | Type | Value Type | Description |
|-----|------|------------|-------------|
| `{subscription_id}` | `u32` | `Subscription` | Individual subscription data keyed by ID |

**Subscription Structure** (`contracts/subscription_vault/src/types.rs`):

```rust
pub struct Subscription {
    pub subscriber: Address,           // Subscriber's address
    pub merchant: Address,             // Merchant receiving payments
    pub amount: i128,                  // Payment amount per interval
    pub interval_seconds: u64,         // Billing interval duration
    pub last_payment_timestamp: u64,   // Last successful charge time
    pub status: SubscriptionStatus,    // Current state (Active/Paused/Cancelled/InsufficientBalance)
    pub prepaid_balance: i128,         // Available funds in vault
    pub usage_enabled: bool,           // Usage-based billing flag
}
```

**Status Enum**:
```rust
pub enum SubscriptionStatus {
    Active = 0,
    Paused = 1,
    Cancelled = 2,
    InsufficientBalance = 3,
}
```

**Key Generation**: Sequential u32 IDs from `next_id` counter

**Storage Operations**:
- Create: `do_create_subscription()` → sets `{id}` key
- Read: `get_subscription()` → reads `{id}` key
- Update: All lifecycle functions modify and re-set `{id}` key
- Delete: Not implemented (cancelled subscriptions remain in storage)

---

## Storage Access Patterns

### Read Operations
- `get_subscription(id)` → Single subscription lookup
- `get_min_topup()` → Config read
- No batch reads or iteration (IDs tracked off-chain)

### Write Operations
- `create_subscription()` → Increments `next_id`, writes new subscription
- `deposit_funds()` → Read-modify-write subscription
- `charge_subscription()` / `batch_charge()` → Read-modify-write subscription(s)
- `pause/resume/cancel_subscription()` → Read-modify-write subscription
- `set_min_topup()` → Config update

### Storage Costs
- Each subscription: ~200 bytes (Address: 32 bytes × 2, i128 × 2, u64 × 2, enum, bool)
- Config keys: ~100 bytes total
- No automatic cleanup (cancelled subscriptions persist)

---

## Versioning and Compatibility

### Current Version
**v1.0** - Initial storage schema (no version field stored)

### Compatibility Guarantees

#### Backward Compatible Changes ✅
- Adding new optional fields to `Subscription` (requires default values)
- Adding new config keys (e.g., `"fee_percentage"`)
- Adding new subscription status variants (append only, preserve existing values)
- Changing function logic without storage schema changes

#### Breaking Changes ❌
- Removing fields from `Subscription`
- Changing field types (e.g., `i128` → `u128`)
- Renaming storage keys
- Reordering enum variants (changes discriminant values)
- Changing key types (e.g., `u32` → `u64` for subscription IDs)

### Schema Version Field (Recommended Future Addition)

To enable safe migrations, add a version key:

```rust
// In init():
env.storage().instance().set(&Symbol::new(env, "schema_version"), &1u32);
```

Check version before operations:
```rust
let version: u32 = env.storage().instance()
    .get(&Symbol::new(env, "schema_version"))
    .unwrap_or(1);
```

---

## Upgrade Procedures

### Soroban Contract Upgrades

Soroban supports **contract code upgrades** while preserving storage:
1. Deploy new WASM with `soroban contract install`
2. Upgrade instance with `soroban contract upgrade`
3. Storage keys/values remain intact if schema is compatible

### Safe Upgrade Checklist

**Before Upgrade**:
- [ ] Review storage schema changes (use diff on `types.rs`)
- [ ] Verify enum variant order unchanged
- [ ] Test new code against existing storage in testnet
- [ ] Document any new storage keys or fields
- [ ] Plan migration if breaking changes required

**During Upgrade**:
- [ ] Deploy to testnet first
- [ ] Verify existing subscriptions readable with new code
- [ ] Test all state transitions with upgraded contract
- [ ] Monitor for storage-related errors

**After Upgrade**:
- [ ] Verify critical subscriptions still accessible
- [ ] Check config values (token, admin, min_topup)
- [ ] Test charge operations on existing subscriptions

---

## Migration Strategies

### Strategy 1: Additive Changes (Preferred)

Add new fields with defaults, keep old fields:

```rust
pub struct Subscription {
    // ... existing fields ...
    pub new_field: Option<i128>,  // Defaults to None for existing records
}
```

**Pros**: No migration needed, instant upgrade
**Cons**: Storage bloat from unused fields

---

### Strategy 2: Lazy Migration

Migrate records on first access:

```rust
pub fn get_subscription(env: &Env, id: u32) -> Result<Subscription, Error> {
    let mut sub: Subscription = env.storage().instance()
        .get(&id)
        .ok_or(Error::NotFound)?;
    
    // Detect old schema (e.g., missing field)
    if sub.new_field.is_none() {
        sub.new_field = Some(compute_default(&sub));
        env.storage().instance().set(&id, &sub);  // Migrate on read
    }
    Ok(sub)
}
```

**Pros**: Gradual migration, no downtime
**Cons**: Complex logic, inconsistent storage state during transition

---

### Strategy 3: Batch Migration

Separate migration contract or admin function:

```rust
pub fn migrate_subscriptions(env: Env, ids: Vec<u32>) -> Result<(), Error> {
    let admin = require_admin(&env)?;
    admin.require_auth();
    
    for id in ids.iter() {
        let old_sub: OldSubscription = env.storage().instance().get(&id)?;
        let new_sub = Subscription::from_old(old_sub);
        env.storage().instance().set(&id, &new_sub);
    }
    Ok(())
}
```

**Pros**: Clean separation, controlled migration
**Cons**: Requires off-chain ID tracking, multiple transactions

---

## Potential Pitfalls

### 1. Enum Discriminant Changes
**Problem**: Adding variants in the middle changes discriminant values
```rust
// Before upgrade
pub enum SubscriptionStatus {
    Active = 0,
    Paused = 1,
    Cancelled = 2,
}

// WRONG: Inserts new variant
pub enum SubscriptionStatus {
    Active = 0,
    Pending = 1,  // ❌ Shifts all subsequent values
    Paused = 2,   // Was 1, now 2 - breaks existing storage!
    Cancelled = 3,
}

// CORRECT: Append only
pub enum SubscriptionStatus {
    Active = 0,
    Paused = 1,
    Cancelled = 2,
    Pending = 3,  // ✅ New variant at end
}
```

### 2. Key Collision
**Problem**: New keys conflict with subscription IDs
```rust
// BAD: Using u32 for config could collide with subscription IDs
env.storage().instance().set(&0u32, &config);  // ❌ Collides with subscription ID 0

// GOOD: Use Symbol keys for config
env.storage().instance().set(&Symbol::new(env, "config"), &config);  // ✅
```

### 3. Missing Default Values
**Problem**: New required fields break deserialization
```rust
// Before
pub struct Subscription {
    pub amount: i128,
}

// After - BREAKS existing storage
pub struct Subscription {
    pub amount: i128,
    pub fee: i128,  // ❌ No default, deserialization fails
}

// Fix: Use Option or provide migration
pub struct Subscription {
    pub amount: i128,
    pub fee: Option<i128>,  // ✅ Defaults to None
}
```

### 4. ID Counter Overflow
**Problem**: `next_id` (u32) can overflow after 4B subscriptions
```rust
// Current implementation (subscription.rs:8)
let id: u32 = env.storage().instance().get(&key).unwrap_or(0);
env.storage().instance().set(&key, &(id + 1));  // ❌ Panics on overflow

// Better: Check for overflow
let id: u32 = env.storage().instance().get(&key).unwrap_or(0);
let next = id.checked_add(1).ok_or(Error::Overflow)?;
env.storage().instance().set(&key, &next);
```

### 5. Storage Bloat
**Problem**: Cancelled subscriptions never deleted
- **Impact**: Unbounded storage growth
- **Mitigation**: Implement archive/cleanup mechanism or off-chain indexing

---

## Recommendations

### Immediate Actions
1. **Add schema version field** in next upgrade
2. **Add overflow check** to `next_id()` counter
3. **Document enum variant order** as immutable in code comments

### Future Enhancements
1. **Storage cleanup**: Add admin function to archive old subscriptions
2. **Batch reads**: Add `get_subscriptions(Vec<u32>)` for efficiency
3. **Storage metrics**: Track total subscriptions, active count
4. **Migration hooks**: Add `on_upgrade()` entrypoint for automated migrations

### Testing Upgrades
1. Create testnet contract with sample data
2. Deploy upgraded WASM to separate instance
3. Copy storage snapshot to upgraded instance (if tooling available)
4. Verify all operations work with old data
5. Only then upgrade mainnet

---

## Related Documentation

- [Subscription State Machine](./subscription_state_machine.md) - Status transition rules
- [Billing Intervals](./billing_intervals.md) - Charge timing logic
- [Batch Charge](./batch_charge.md) - Bulk operations
- [Soroban Storage Docs](https://developers.stellar.org/docs/smart-contracts/guides/storage) - Official storage guide

---

## Summary

**Storage Model**: Instance storage with Symbol keys (config) and u32 keys (subscriptions)

**Upgrade Safety**: Additive changes safe, breaking changes require migration

**Key Risks**: Enum reordering, key collisions, missing defaults, ID overflow

**Best Practice**: Always test upgrades on testnet with production-like data before mainnet deployment
