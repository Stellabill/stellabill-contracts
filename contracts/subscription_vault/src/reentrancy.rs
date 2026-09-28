//! Reentrancy guard for fund-moving entrypoints.
//!
//! Uses a per-entrypoint storage flag in instance storage, namespaced under
//! `DataKey::ReentrancyLock(Symbol)` so it cannot collide with other storage
//! keys or with raw `Symbol` keys used elsewhere.
//!
//! The flag is set before any external token transfer and cleared
//! unconditionally on return (success or error) via the `Drop` impl.
//!
//! # Usage
//! ```ignore
//! let _guard = ReentrancyGuard::lock(&env, "deposit_funds")?;
//! // _guard is dropped at end of scope, releasing the lock
//! ```

use crate::types::{DataKey, Error};
use soroban_sdk::{Env, Symbol};

/// RAII guard that holds a reentrancy lock for the duration of a scope.
///
/// Acquiring the guard sets a per-entrypoint flag in instance storage under
/// `DataKey::ReentrancyLock(entrypoint_symbol)`.
/// Dropping the guard clears it, even if the function returns an error.
pub struct ReentrancyGuard<'a> {
    env: &'a Env,
    key: DataKey,
}

impl<'a> ReentrancyGuard<'a> {
    /// Attempt to acquire the reentrancy lock for `entrypoint`.
    ///
    /// Returns `Err(Error::Reentrancy)` immediately if the lock is already
    /// held, indicating a reentrant call is in progress.
    ///
    /// The lock lives in instance storage under
    /// `DataKey::ReentrancyLock(Symbol::new(env, entrypoint))`, guaranteeing
    /// it does not collide with other storage keys.
    pub fn lock(env: &'a Env, entrypoint: &str) -> Result<Self, Error> {
        let sym = Symbol::new(env, entrypoint);
        let key = DataKey::ReentrancyLock(sym);
        if env.storage().instance().has(&key) {
            return Err(Error::Reentrancy);
        }
        env.storage().instance().set(&key, &true);
        Ok(Self { env, key })
    }
}

impl<'a> Drop for ReentrancyGuard<'a> {
    /// Release the lock unconditionally when the guard goes out of scope.
    ///
    /// Because the guard is bound in a `let _guard = ...` binding at each
    /// call site, drop runs on every control-flow path — including `?`
    /// early returns, panics converted to errors, and normal completion —
    /// ensuring the lock is never left held after the entrypoint exits.
    fn drop(&mut self) {
        self.env.storage().instance().remove(&self.key);
    }
}
