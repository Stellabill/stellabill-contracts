# Merchant Configuration Storage

`SubscriptionVault` supports per-merchant configuration records to control subscription defaults, payout settings, and operational flags.

## Overview

The merchant configuration module provides:
- **Payout settings**: Address where merchant receives earnings
- **Fee configuration**: Fee percentage in bips (0-100%)
- **Operational flags**: Bitmap controlling allowed operations
- **Configurable pausing**: Per-merchant pause capability

## Data Structure

```rust
pub struct MerchantConfig {
    /// Schema version for forward-compatible upgrades
    pub version: i32,
    /// Address where merchant receives payouts
    pub payout_address: Address,
    /// Fee percentage in bips (0-10000, where 10000 = 100%)
    pub fee_bips: i32,
    /// Bitmap of allowed operations (see OP_* constants)
    pub allowed_operations: i32,
    /// Whether merchant can receive charges and payouts
    pub is_active: bool,
    /// Optional address for platform fee routing
    pub fee_address: Option<Address>,
    /// Redirect URL for off-chain callbacks
    pub redirect_url: String,
    /// Global pause for merchant subscriptions
    pub is_paused: bool,
    /// Timestamp of last config update
    pub last_updated: u64,
}
```

## Operation Flags

| Constant | Value | Description |
|---------|-------|-------------|
| `OP_CHARGE` | 0x01 (1<<0) | Can charge subscribers |
| `OP_WITHDRAW` | 0x02 (1<<1) | Can withdraw earnings |
| `OP_REFUND` | 0x04 (1<<2) | Can issue refunds |
| `OP_BILLING_PAUSE` | 0x08 (1<<3) | Can pause subscriptions globally |
| `OP_AUTO_RENEWAL` | 0x10 (1<<4) | Auto-renewal enabled |

Default: `OP_CHARGE | OP_WITHDRAW | OP_REFUND | OP_AUTO_RENEWAL`

## Constants

- `MAX_FEE_BIPS`: 10000 (100%)
- `DEFAULT_ALLOWED_OPS`: All operations except OP_BILLING_PAUSE

## Storage

Configs are stored under `DataKey::MerchantConfig(Address)` in instance storage.

## Entry Points

### initialize_merchant_config

Creates a new merchant config with validation.

```rust
pub fn initialize_merchant_config(
    env: Env,
    merchant: Address,           // Must authorize
    payout_address: Address,     // Where merchant receives payouts
    fee_bips: i32,              // Fee in bips (0-10000)
    allowed_operations: i32,    // Operation bitmap
    fee_address: Option<Address>,
    redirect_url: String,
) -> Result<MerchantConfig, Error>
```

Errors:
- `InvalidFeeBips` - fee exceeds 100%
- `InvalidOperations` - invalid operation bits
- `MustAllowChargeOperation` - CHARGE must be enabled

### set_merchant_config

Full overwrite with validation.

```rust
pub fn set_merchant_config(
    env: Env,
    merchant: Address,
    config: MerchantConfig,
) -> Result<(), Error>
```

### update_merchant_config

Partial update - `None` leaves fields unchanged.

```rust
pub fn update_merchant_config(
    env: Env,
    merchant: Address,
    new_payout_address: Option<Address>,
    new_fee_bips: Option<i32>,
    new_allowed_operations: Option<i32>,
    new_is_active: Option<bool>,
    new_fee_address: Option<Option<Address>>,
    new_redirect_url: Option<String>,
    new_is_paused: Option<bool>,
) -> Result<MerchantConfig, Error>
```

### get_merchant_config

Query configuration.

```rust
pub fn get_merchant_config(
    env: Env,
    merchant: Address,
) -> Option<MerchantConfig>
```

## Validation Functions

```rust
pub fn is_valid_allowed_operations(ops: i32) -> bool
```

Validates:
- Only valid operation bits are set
- OP_CHARGE is enabled (required for operation)

## Events

### MerchantConfigInitializedEvent

```rust
pub struct MerchantConfigInitializedEvent {
    pub merchant: Address,
    pub payout_address: Address,
    pub fee_bips: i32,
    pub allowed_operations: i32,
    pub timestamp: u64,
}
```

### MerchantConfigUpdatedEvent

```rust
pub struct MerchantConfigUpdatedEvent {
    pub merchant: Address,
    pub payout_address: Address,
    pub fee_bips: i32,
    pub allowed_operations: i32,
    pub is_active: bool,
    pub timestamp: u64,
}
```

## Security Assumptions

1. **Merchant authorization**: Only merchant can modify their own config
2. **Fee validation**: Fee cannot exceed 100% (10000 bips)
3. **Operation validation**: CHARGE operation must be enabled
4. **Payout safety**: Payout address validation is delegated to caller context
5. **Event auditability**: All config changes emit events for indexers

## Storage Schema

```text
DataKey::MerchantConfig(Address) => MerchantConfig
```

## Usage Examples

### Initialize merchant config
```rust
let config = client.initialize_merchant_config(
    &merchant,           // authorized signer
    payout_address,
    500,                 // 5% fee (500 bips)
    0x1F,                // all operations enabled
    None,                // no fee routing
    String::from_str(&env, "https://example.com/callback"),
)?;
```

### Query config
```rust
let config = client.get_merchant_config(&merchant);
match config {
    Some(c) => assert!(c.is_active),
    None => panic!("merchant not initialized"),
}
```

### Update specific field
```rust
let updated = client.update_merchant_config(
    &merchant,
    None,           // payout unchanged
    Some(1000),     // update fee to 10%
    None,           // operations unchanged
    None,           // active unchanged
    None,           // fee_address unchanged
    None,           // redirect unchanged
    None,           // paused unchanged
)?;
```

## Upgradability

The config struct includes `version` field for forward-compatible upgrades. New fields can be added with migration logic while preserving existing storage layout.