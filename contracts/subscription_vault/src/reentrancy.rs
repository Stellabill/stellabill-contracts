//! Reentrancy guard for fund-moving entrypoints.
//!
//! Uses a per-entrypoint storage flag in instance storage.
//! The flag is set before any external token transfer and cleared
//! unconditionally on return (success or error).
//!
//! # Usage
//! ```ignore
//! let _guard = ReentrancyGuard::lock(&env, "deposit_funds")?;
//! // _guard is dropped at end of scope, releasing the lock
//! ```

use soroban_sdk::{Env, Symbol};
use crate::types::Error;

/// RAII guard that holds a reentrancy lock for the duration of a scope.
///
/// Acquiring the guard sets a per-entrypoint flag in instance storage.
/// Dropping the guard clears it, even if the function returns an error.
pub struct ReentrancyGuard<'a> {
    env: &'a Env,
    key: Symbol,
}

impl<'a> ReentrancyGuard<'a> {
    /// Attempt to acquire the reentrancy lock for `entrypoint`.
    ///
    /// Returns `Err(Error::Reentrancy)` immediately if the lock is already
    /// held, indicating a reentrant call is in progress.
    pub fn lock(env: &'a Env, entrypoint: &str) -> Result<Self, Error> {
        let key = Symbol::new(env, entrypoint);
        if env.storage().instance().has(&key) {
            return Err(Error::Reentrancy);
        }
        env.storage().instance().set(&key, &true);
        Ok(Self { env, key })
    }
}

impl<'a> Drop for ReentrancyGuard<'a> {
    /// Release the lock unconditionally when the guard goes out of scope.
    fn drop(&mut self) {
        self.env.storage().instance().remove(&self.key);
    }
}
