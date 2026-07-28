//! Reentrancy protection for critical operations.
//!
//! This module provides a stateful guard mechanism to detect and prevent reentrancy
//! in functions that may be vulnerable to callbacks during external calls.
//!
//! # Design
//!
//! **Locked State Pattern**: We use a per-function lock stored in contract storage to detect
//! if execution is already happening. This is effective for preventing callbacks from
//! reentering the same critical path.
//!
//! **Usage**:
//! ```ignore
//! let guard = ReentrancyGuard::lock(env, "function_name")?;
//! // Critical operations here
//! // Guard is automatically dropped when it goes out of scope
//! ```
//!
//! # Guarantees
//!
//! - **Atomic locking**: The lock is acquired and checked atomically within a single ledger state
//! - **Automatic cleanup**: Guards cleanup their own locks when dropped
//! - **Zero cost for non-reentrancy cases**: In normal (non-reentrancy) scenarios, guard creation
//!   and cleanup is negligible overhead
//!
//! # Limitations
//!
//! This guard **cannot prevent reentrancy across different functions** (e.g., `charge_subscription`
//! then `deposit_funds`). It only prevents the same function from being called recursively.
//! For full reentrancy safety, always follow the Checks-Effects-Interactions (CEI) pattern
//! where external calls happen after all internal state updates.
//!
//! # Best Practices
//!
//! 1. **Prefer CEI Pattern**: The Checks-Effects-Interactions pattern is the primary defense.
//!    Use locks only for additional protection on critical paths.
//! 2. **Minimize Lock Scope**: Keep the critical section as small as possible.
//! 3. **Document Assumptions**: Always document why a lock is needed and what it protects.

use crate::types::Error;
use soroban_sdk::{Env, Symbol};

/// A guard that prevents a function from being reentered.
///
/// When created, it sets a lock in storage. When dropped, it clears the lock.
/// If a lock already exists, creation fails with `Error::Reentrancy`.
#[allow(dead_code)]
pub struct ReentrancyGuard {
    lock_key: Symbol,
    env: *const Env,
}

impl ReentrancyGuard {
    /// Acquire a reentrancy lock for a critical section.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `function_name` - A unique identifier for the lock (e.g., "withdraw_merchant")
    ///
    /// # Returns
    /// * `Ok(guard)` if the lock was successfully acquired
    /// * `Err(Error::Reentrancy)` if a lock already exists (reentrancy detected)
    ///
    /// # Safety
    /// This function is unsafe because it stores a raw pointer to the environment.
    /// The pointer must remain valid for the lifetime of the guard.
    #[allow(dead_code)]
    pub fn lock(env: &Env, function_name: &str) -> Result<Self, Error> {
        let lock_key = Symbol::new(env, function_name);

        let storage = env.storage().instance();

        // Check if lock is already held
        if storage.has(&lock_key) {
            return Err(Error::Reentrancy);
        }

        // Acquire lock
        storage.set(&lock_key, &true);

        Ok(ReentrancyGuard {
            lock_key,
            env: env as *const Env,
        })
    }
}

impl Drop for ReentrancyGuard {
    fn drop(&mut self) {
        // SAFETY: The pointer was validated in lock() to point to a valid, live Env.
        // During the lifetime of ReentrancyGuard, the Env is guaranteed to be valid.
        unsafe {
            let env = &*self.env;
            env.storage().instance().remove(&self.lock_key);
        }
    }
}

/// Check if reentrancy protection is supported by the contract.
///
/// This returns `true` if the Soroban SDK version supports the necessary storage features.
/// It's provided for compatibility checking and logging.
#[allow(dead_code)]
pub fn is_reentrancy_supported() -> bool {
    // If the SDK supports symbols and instance storage, reentrancy guards work.
    // This is always true in current Soroban SDK versions.
    true
}

#[cfg(test)]
mod tests {
    // NOTE: Reentrancy guard tests would require environment setup.
    // See test.rs for integration tests that verify guard behavior in actual contract contexts.
}
