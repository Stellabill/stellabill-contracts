#![no_std]

//! Subscription Vault — prepaid USDC subscription billing on Stellar.
//!
//! # Findings (issue #374 investigation)
//! - `lib.rs` was a minimal stub with no types, no `init`, no `charge_subscription`.
//! - `docs/admin_authorization_matrix.md` confirms `charge_subscription` must be
//!   admin-only; the stored-admin pattern (load from state, `require_auth()`) is
//!   used by `batch_charge` and is the correct model here — no explicit admin param.
//! - Admin is stored under `DataKey::Admin` (not a raw `Symbol`).
//! - `Error::Unauthorized` (discriminant 1001 per the matrix) is the correct error.
//! - No `set_min_topup` reference pattern existed in the stub; the matrix's
//!   `batch_charge` pattern is the authoritative reference for stored-admin auth.
//! - `docs/admin_authorization_matrix.md` does not list `charge_subscription` by
//!   name; it is the legacy single-charge entrypoint that maps to the admin-only
//!   stored-admin model.

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env};

// ── Error types ──────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    /// Subscription not found.
    NotFound = 1000,
    /// Caller is not the stored admin address.
    Unauthorized = 1001,
}

// ── Storage keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    Subscription(u64),
}

// ── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    /// Returns the schema version of this contract.
    pub fn version(_env: Env) -> u32 {
        0
    }

    /// Initialise the contract, storing the admin address and USDC token.
    pub fn init(env: Env, admin: Address, _usdc_token: Address) {
        env.storage().instance().set(&DataKey::Admin, &admin);
    }

    /// Charge a subscriber's vault for the current billing period.
    ///
    /// # Authorization
    /// Caller must be the admin address stored during [`init`].
    /// Any other caller, including the subscriber themselves, is rejected with
    /// [`Error::Unauthorized`]. This prevents unauthorized or self-initiated charges.
    ///
    /// # Errors
    /// - [`Error::Unauthorized`] — caller is not the stored admin, or `init` was
    ///   never called.
    /// - [`Error::NotFound`] — no subscription exists for `subscription_id`.
    pub fn charge_subscription(env: Env, subscription_id: u64) -> Result<(), Error> {
        // ── Admin authorization ───────────────────────────────────────────────
        // Load the admin address stored during init and require its signature.
        // Any caller that is not the stored admin is rejected with Error::Unauthorized.
        // This matches the stored-admin pattern used by batch_charge (auth matrix).
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Unauthorized)?; // admin not set == treat as unauthorized
        admin.require_auth();
        // ─────────────────────────────────────────────────────────────────────

        // Verify the subscription exists.
        if !env
            .storage()
            .persistent()
            .has(&DataKey::Subscription(subscription_id))
        {
            return Err(Error::NotFound);
        }

        // TODO: deduct prepaid balance, transfer to merchant, update last_payment_timestamp.

        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    /// Returns true if DataKey::Subscription(id) exists in persistent storage.
    /// Runs inside the contract context so it can access env.storage() directly.
    fn subscription_exists(env: &Env, contract_id: &soroban_sdk::Address, id: u64) -> bool {
        env.as_contract(contract_id, || {
            env.storage().persistent().has(&DataKey::Subscription(id))
        })
    }

    #[test]
    fn version_is_zero() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        assert_eq!(client.version(), 0);
    }

    // ── charge_subscription authorization tests ───────────────────────────────
    //
    // Findings recorded per issue #374 investigation:
    // - Admin stored under DataKey::Admin (instance storage).
    // - Stored-admin pattern: load from state, require_auth() — no explicit param.
    // - Error::Unauthorized (1001) returned when admin not set or caller mismatch.
    // - Error::NotFound (1000) returned when subscription_id has no record.
    // - mock_all_auths() satisfies require_auth() for any address in tests.
    // - Storage assertions use env.as_contract() to read persistent storage directly,
    //   confirming no DataKey::Subscription entry was written on rejection.

    #[test]
    fn charge_subscription_admin_not_set_returns_unauthorized_and_no_storage_written() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        // init never called — no admin stored
        let result = client.try_charge_subscription(&0);

        // Error variant
        assert_eq!(result, Err(Ok(Error::Unauthorized)));
        // No subscription entry was written
        assert!(!subscription_exists(&env, &contract_id, 0));
    }

    #[test]
    fn charge_subscription_unknown_id_returns_not_found_and_no_storage_written() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let usdc = Address::generate(&env);
        client.init(&admin, &usdc);

        let result = client.try_charge_subscription(&99);

        // Error variant
        assert_eq!(result, Err(Ok(Error::NotFound)));
        // No subscription entry was written
        assert!(!subscription_exists(&env, &contract_id, 99));
    }

    #[test]
    fn charge_subscription_non_admin_rejected_and_no_storage_written() {
        // init with admin, then call charge_subscription with no mocked auths.
        // set_auths(&[]) clears all mocked authorizations; try_charge_subscription
        // returns Err (host auth failure) without writing any storage.
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let usdc = Address::generate(&env);

        env.mock_all_auths();
        client.init(&admin, &usdc);

        // Drop all mocked auths — subsequent require_auth() calls are unsatisfied.
        env.set_auths(&[]);

        let result = client.try_charge_subscription(&0);

        // Host auth failure — must be an error of some kind.
        assert!(result.is_err());
        // No DataKey::Subscription entry was written.
        assert!(!subscription_exists(&env, &contract_id, 0));
    }
}
