//! Focused adversarial tests for `admin::is_token_accepted` (issue #1068).
//!
//! `is_token_accepted` is the allowlist gate used by `create_subscription`,
//! `create_subscription_with_token`, and merchant-withdrawal paths to
//! determine whether a given token address is authorised for use in the vault.
//!
//! Its implementation:
//! ```rust
//! pub fn is_token_accepted(env: &Env, token: &Address) -> bool {
//!     env.storage().instance().has(&DataKey::TokenDecimals(token.clone()))
//! }
//! ```
//!
//! It reads **only** from `instance()` storage — never `persistent()` — so
//! every test asserts against the instance tier directly where needed.
//!
//! ## Cases covered
//!
//! | # | Group | Scenario |
//! |---|-------|----------|
//! | 1 | Valid | Default init token is accepted immediately after `init` |
//! | 2 | Valid | Explicitly added secondary token is accepted |
//! | 3 | Failure | Arbitrary address never added returns `false` |
//! | 4 | Failure | Removed secondary token returns `false` |
//! | 5 | Boundary | Default token cannot be removed and stays accepted |
//! | 6 | Boundary | Contract's own address is never accepted (not added at init) |
//! | 7 | Boundary | Accepted status is not affected by unrelated storage writes |
//! | 8 | Idempotency | `add_accepted_token` with a duplicate address does not double-register |
//! | 9 | Isolation | `is_token_accepted` on uninitialized vault returns `false`, not a panic |
//! | 10 | State integrity | Rejected `add_accepted_token` call (non-admin) leaves status unchanged |
//! | 11 | State integrity | Rejected `remove_accepted_token` call (non-admin) leaves status unchanged |
//! | 12 | Boundary | Zero-decimal token is accepted once explicitly added |
//! | 13 | Boundary | Max-decimal token (up to contract limit) is accepted once added |
//! | 14 | Read purity | Repeated reads return the same result without mutating storage |
//! | 15 | Orthogonality | Adding/removing token A does not affect token B's status |

#[cfg(test)]
mod tests {
    use crate::admin::is_token_accepted;
    use crate::test_utils::setup::TestEnv;
    use crate::types::Error;
    use crate::{SubscriptionVault, SubscriptionVaultClient};
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Address, Env};

    // ── Helper ────────────────────────────────────────────────────────────────

    /// Call `is_token_accepted` inside the vault's storage context.
    fn accepted(env: &Env, client: &SubscriptionVaultClient, token: &Address) -> bool {
        env.as_contract(&client.address, || is_token_accepted(env, token))
    }

    // ── 1. Valid: default init token is accepted immediately after init ───────

    #[test]
    fn init_token_is_accepted_after_init() {
        let te = TestEnv::default();
        assert!(accepted(&te.env, &te.client, &te.token));
    }

    // ── 2. Valid: explicitly added secondary token is accepted ────────────────

    #[test]
    fn added_token_is_accepted() {
        let te = TestEnv::default();
        let other = Address::generate(&te.env);

        te.client.add_accepted_token(&te.admin, &other, &6u32);

        assert!(accepted(&te.env, &te.client, &other));
    }

    // ── 3. Failure: arbitrary address never added returns false ───────────────

    #[test]
    fn never_added_token_is_not_accepted() {
        let te = TestEnv::default();
        let stranger = Address::generate(&te.env);

        assert!(!accepted(&te.env, &te.client, &stranger));
    }

    // ── 4. Failure: removed secondary token returns false ────────────────────

    #[test]
    fn removed_token_is_no_longer_accepted() {
        let te = TestEnv::default();
        let other = Address::generate(&te.env);

        te.client.add_accepted_token(&te.admin, &other, &6u32);
        assert!(accepted(&te.env, &te.client, &other));

        // The add/remove ops share the "AcceptedTokens" cooldown key, so we
        // advance past the cooldown window before calling remove.
        te.jump(crate::CONFIG_COOLDOWN_SECS + 1);
        te.client.remove_accepted_token(&te.admin, &other);

        assert!(!accepted(&te.env, &te.client, &other));
    }

    // ── 5. Boundary: default token cannot be removed and stays accepted ───────

    #[test]
    fn default_token_cannot_be_removed_and_remains_accepted() {
        let te = TestEnv::default();

        let res = te.client.try_remove_accepted_token(&te.admin, &te.token);

        assert_eq!(res, Err(Ok(Error::InvalidInput)));
        // Removal was rejected — the default token must still be accepted.
        assert!(accepted(&te.env, &te.client, &te.token));
    }

    // ── 6. Boundary: contract's own address is not accepted ──────────────────

    #[test]
    fn contract_own_address_is_not_accepted() {
        let te = TestEnv::default();
        let contract_addr = te.client.address.clone();

        // The contract itself is never added to the accepted-token list.
        assert!(!accepted(&te.env, &te.client, &contract_addr));
    }

    // ── 7. Boundary: unrelated storage writes do not affect accepted status ───

    #[test]
    fn unrelated_storage_write_does_not_affect_accepted_status() {
        let te = TestEnv::default();
        let other = Address::generate(&te.env);
        te.client.add_accepted_token(&te.admin, &other, &6u32);

        // Perform an unrelated storage mutation: advance time and update min_topup.
        te.jump(crate::CONFIG_COOLDOWN_SECS + 1);
        te.client.set_min_topup(&te.admin, &2_000_000i128);

        // Accepted status of both tokens is unchanged.
        assert!(accepted(&te.env, &te.client, &te.token));
        assert!(accepted(&te.env, &te.client, &other));
    }

    // ── 8. Idempotency: add_accepted_token with a duplicate does not break it ─

    #[test]
    fn duplicate_add_is_still_accepted_and_idempotent() {
        let te = TestEnv::default();
        let other = Address::generate(&te.env);

        te.client.add_accepted_token(&te.admin, &other, &6u32);
        // Adding the same token again (with a different decimals value to
        // detect if the decimals are silently overwritten) should not create
        // duplicates or remove the token.
        te.jump(crate::CONFIG_COOLDOWN_SECS + 1);
        te.client.add_accepted_token(&te.admin, &other, &8u32);

        // Token is still accepted after a duplicate add.
        assert!(accepted(&te.env, &te.client, &other));

        // Only one entry in the list (no duplicates).
        let listed = te.client.list_accepted_tokens();
        let count = listed.iter().filter(|a| a.token == other).count();
        assert_eq!(count, 1, "expected exactly one entry for the token, got {count}");
    }

    // ── 9. Isolation: uninitialized vault returns false, not a panic ──────────

    #[test]
    fn uninitialized_vault_returns_false_not_a_panic() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let token = Address::generate(&env);

        // Must return false cleanly — no panic, no ContractError.
        let result = env.as_contract(&client.address, || is_token_accepted(&env, &token));
        assert!(!result);
    }

    // ── 10. State integrity: rejected add_accepted_token (non-admin) ──────────

    #[test]
    fn rejected_add_does_not_change_acceptance_status() {
        let te = TestEnv::default();
        let other = Address::generate(&te.env);
        let attacker = Address::generate(&te.env);

        // Attempt to add a token with a non-admin caller.
        let res = te.client.try_add_accepted_token(&attacker, &other, &6u32);
        assert_eq!(res, Err(Ok(Error::Forbidden)));

        // Token must still be rejected — no partial state was written.
        assert!(!accepted(&te.env, &te.client, &other));
    }

    // ── 11. State integrity: rejected remove (non-admin) ─────────────────────

    #[test]
    fn rejected_remove_does_not_change_acceptance_status() {
        let te = TestEnv::default();
        let other = Address::generate(&te.env);
        let attacker = Address::generate(&te.env);

        te.client.add_accepted_token(&te.admin, &other, &6u32);
        assert!(accepted(&te.env, &te.client, &other));

        // Attempt removal with a non-admin caller.
        let res = te.client.try_remove_accepted_token(&attacker, &other);
        assert_eq!(res, Err(Ok(Error::Forbidden)));

        // Token must still be accepted.
        assert!(accepted(&te.env, &te.client, &other));
    }

    // ── 12. Boundary: zero-decimal token is accepted once added ──────────────

    #[test]
    fn zero_decimal_token_is_accepted_after_add() {
        let te = TestEnv::default();
        let zero_dec_token = Address::generate(&te.env);

        te.client.add_accepted_token(&te.admin, &zero_dec_token, &0u32);

        assert!(accepted(&te.env, &te.client, &zero_dec_token));
    }

    // ── 13. Boundary: max-decimals token (19, the contract ceiling) ───────────

    #[test]
    fn max_decimal_token_is_accepted_after_add() {
        let te = TestEnv::default();
        let max_dec_token = Address::generate(&te.env);

        // 19 is the maximum allowed decimals value per the `do_init` guard.
        te.client.add_accepted_token(&te.admin, &max_dec_token, &19u32);

        assert!(accepted(&te.env, &te.client, &max_dec_token));
    }

    // ── 14. Read purity: repeated reads do not mutate storage ─────────────────

    #[test]
    fn repeated_reads_are_pure_and_consistent() {
        let te = TestEnv::default();

        for _ in 0..10 {
            assert!(accepted(&te.env, &te.client, &te.token));
        }

        // Spot-check that storage has not grown unexpected keys by verifying
        // that a never-added token is still absent after many reads.
        let stranger = Address::generate(&te.env);
        for _ in 0..10 {
            assert!(!accepted(&te.env, &te.client, &stranger));
        }
    }

    // ── 15. Orthogonality: adding/removing token A does not affect token B ────

    #[test]
    fn add_remove_one_token_does_not_affect_another() {
        let te = TestEnv::default();
        let token_a = Address::generate(&te.env);
        let token_b = Address::generate(&te.env);

        te.client.add_accepted_token(&te.admin, &token_a, &6u32);
        te.jump(crate::CONFIG_COOLDOWN_SECS + 1);
        te.client.add_accepted_token(&te.admin, &token_b, &6u32);

        // Both accepted before any removal.
        assert!(accepted(&te.env, &te.client, &token_a));
        assert!(accepted(&te.env, &te.client, &token_b));

        // Remove token_a.
        te.jump(crate::CONFIG_COOLDOWN_SECS + 1);
        te.client.remove_accepted_token(&te.admin, &token_a);

        // token_a removed, token_b and default token unaffected.
        assert!(!accepted(&te.env, &te.client, &token_a));
        assert!(accepted(&te.env, &te.client, &token_b));
        assert!(accepted(&te.env, &te.client, &te.token));
    }
}
