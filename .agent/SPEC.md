# Coding Standards — DRiPS Protocol

## Strict Requirements

- Professional Rust naming conventions throughout.
- No emojis in code, comments, or documentation.
- All balance arithmetic uses `checked_add` / `checked_sub` / `checked_mul`.
- Storage access via `Symbol::new(env, "key")` — consistent with existing instance storage patterns.
- Every public contract function enforces authorization before any state mutation.
- Inline comments explain the *why*, not the *what*.

## Authorization Pattern

```rust
subscriber.require_auth(); // Must be the first statement in any subscriber-gated function.
```

## Token Transfer Pattern

```rust
let token_addr = crate::admin::get_token(env)?;
let token_client = soroban_sdk::token::Client::new(env, &token_addr);
token_client.transfer(&from, &to, &amount);
```

Transfer is performed *before* any state mutation. In Soroban, a failed transfer
panics and rolls back the entire transaction, preserving the atomicity invariant.

## Checked Math Pattern

```rust
let new_balance = old_balance
    .checked_add(amount)
    .ok_or(Error::Overflow)?;
```

Never use plain `+` for financial values.

## Storage Access Pattern

```rust
// Read
env.storage().instance().get(&key).ok_or(Error::NotFound)

// Write
env.storage().instance().set(&key, &value);
```

All contract-level configuration values (token, admin, min_topup, next_id) are stored
in instance storage with consistent `Symbol::new(env, "key")` keys.

## Document Standards

- Docs go in `docs/`.
- Each feature document must list its invariants explicitly.
- Security implications must be called out under a dedicated "Security" heading.
