# Subscription Serialization & Storage Layout

This document describes how the `Subscription` data model is serialized and
stored on-chain for the `subscription_vault` contract, and how to evolve the
schema safely over time.

## Core Types

The main types involved in serialization are:

- `SubscriptionStatus` (`contracts/subscription_vault/src/types.rs`)
- `Subscription` (`contracts/subscription_vault/src/types.rs`)

Both are annotated with `#[contracttype]`, which derives the Soroban
serialization logic used for:

- Persisting values in contract storage.
- Emitting events.
- Cross-contract calls.

The same shapes are mirrored in `contracts/subscription_vault/src/lib.rs` for
use in public helpers such as `compute_next_charge_info`.

## Enum Layout: `SubscriptionStatus`

```rust
pub enum SubscriptionStatus {
    Active = 0,
    Paused = 1,
    Cancelled = 2,
    InsufficientBalance = 3,
}
```

The numeric discriminants (`0..=3`) are part of the serialized form and are
treated as stable:

- Do **not** reorder existing variants.
- Do **not** change existing discriminant values.
- Only append new variants with new discriminants if absolutely necessary.

The test `test_subscription_status_discriminant_values_are_stable` in
`contracts/subscription_vault/src/test.rs` asserts these discriminant values
and will fail if they change.

## Struct Layout: `Subscription`

```rust
pub struct Subscription {
    pub subscriber: Address,
    pub merchant: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub last_payment_timestamp: u64,
    pub status: SubscriptionStatus,
    pub prepaid_balance: i128,
    pub usage_enabled: bool,
}
```

The serialized layout is defined by:

- Field order.
- Field types.

This layout is used for all on-chain subscription records and must be treated
as a stable storage schema:

- Field order must remain unchanged.
- Existing field types must not change.
- Removing a field is a breaking change for existing data.

## Evolving the Schema Safely

When new data needs to be stored for subscriptions:

- Append new fields to the end of the struct.
- Prefer using `Option<T>` for new fields (for example `Option<u64>` or
  `Option<Config>`), so that existing records without the field continue to
  deserialize.
- Keep the meaning of existing fields stable; avoid repurposing them.

If a change requires a fundamentally different shape (for example, splitting a
subscription into multiple records), introduce a new contract or a new
versioned type instead of mutating the existing layout in-place.

## Tests Guarding Serialization

Serialization-related guarantees are validated by tests in
`contracts/subscription_vault/src/test.rs`:

- `test_subscription_serialization_round_trip_all_statuses`:
  - Creates subscriptions covering every `SubscriptionStatus` variant.
  - Serializes them via `IntoVal` and deserializes via `TryFromVal`.
  - Asserts that all fields round-trip exactly, catching incompatible struct
    changes.

- `test_subscription_status_discriminant_values_are_stable`:
  - Asserts the discriminant values for all `SubscriptionStatus` variants.
  - Fails if variants are reordered or discriminant values are modified.

These tests must be updated carefully if new fields or status variants are
added. Any change that modifies the serialized form should be intentional,
reviewed, and accompanied by a clear migration strategy.

