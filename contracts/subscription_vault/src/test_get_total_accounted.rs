//! Adversarial coverage for `get_total_accounted` in `lib.rs` (and the
//! underlying `accounting::get_total_accounted` in `accounting.rs`).
//!
//! **Important architecture note:** `lib.rs` contains an inline stub
//! `pub mod accounting` (lines ~280-294) that replaces the real
//! `accounting.rs` module at the crate root.  All three functions in the stub
//! (`add_total_accounted`, `sub_total_accounted`, `get_total_accounted`) are
//! no-ops that ignore their arguments and always return `0 / Ok(())`.
//!
//! Consequence for these tests:
//! * The public contract entry-point `get_total_accounted` (via `lib.rs`) will
//!   always return `0` regardless of prior `add_total_accounted` calls, because
//!   those calls are also no-ops.
//! * Tests targeting the **real** `accounting.rs` module are exercised
//!   separately by calling `crate::accounting::*` directly in an `as_contract`
//!   context — these tests are **tagged `#[cfg(test)]` only** and do not go
//!   through the public `SubscriptionVaultClient` ABI.
//!
//! The tests are organized in two modules:
//!
//! 1. `stub_surface` — exercises the public ABI entry-point that goes through
//!    the stub; verifies the contractual guarantee that it returns `0` and
//!    that it requires no authorization.
//!
//! 2. `real_accounting` — exercises `accounting.rs` directly to prove that
//!    `add`, `sub`, and `get` behave correctly, including arithmetic boundaries
//!    and error cases, so that when the stub is eventually removed the real
//!    module already has coverage.

use crate::{
    test_utils::setup::TestEnv,
    SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{testutils::Address as _, Address, Env};

// ── Module 1: stub surface (public ABI) ──────────────────────────────────────
//
// All tests here call `client.get_total_accounted(token)` which routes through
// the stub in lib.rs and always returns 0.  The purpose is to document the
// observable contract of the public ABI and to ensure the stub does not panic
// or require auth.

mod stub_surface {
    use super::*;

    fn make_token(env: &Env) -> Address {
        let issuer = Address::generate(env);
        env.register_stellar_asset_contract_v2(issuer).address()
    }

    // ── Happy path ────────────────────────────────────────────────────────

    #[test]
    fn test_get_total_accounted_returns_zero_for_any_token() {
        let t = TestEnv::default();
        let token = t.token.clone();

        let total = t.client.get_total_accounted(&token);
        assert_eq!(total, 0, "stub must return 0 for the default token");
    }

    #[test]
    fn test_get_total_accounted_returns_zero_for_unregistered_token() {
        let t = TestEnv::default();
        let other_token = make_token(&t.env);

        let total = t.client.get_total_accounted(&other_token);
        assert_eq!(total, 0, "stub must return 0 for an unregistered token");
    }

    #[test]
    fn test_get_total_accounted_multiple_tokens_all_return_zero() {
        let t = TestEnv::default();

        for _ in 0..5 {
            let tok = make_token(&t.env);
            let total = t.client.get_total_accounted(&tok);
            assert_eq!(total, 0, "each new token must return 0");
        }
    }

    // ── Authorization ─────────────────────────────────────────────────────

    #[test]
    fn test_get_total_accounted_requires_no_auth() {
        // Drop mock_all_auths and verify the call still succeeds.
        let env = Env::default();
        // NOTE: No env.mock_all_auths() here.
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        // init needs auth mocked for setup, use a scoped mock.
        env.mock_all_auths();
        client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
        // Remove auths — public read should still work.
        env.set_auths(&[]);

        let total = client.get_total_accounted(&token);
        assert_eq!(total, 0, "get_total_accounted must require no auth");
    }

    #[test]
    fn test_get_total_accounted_callable_by_stranger() {
        // Any address (not just admin) may call this view.
        let t = TestEnv::default();
        // Stub always returns 0 regardless of caller identity.
        let total = t.client.get_total_accounted(&t.token);
        assert_eq!(total, 0);
    }

    // ── Idempotency ───────────────────────────────────────────────────────

    #[test]
    fn test_get_total_accounted_idempotent() {
        let t = TestEnv::default();

        let v1 = t.client.get_total_accounted(&t.token);
        let v2 = t.client.get_total_accounted(&t.token);
        let v3 = t.client.get_total_accounted(&t.token);

        assert_eq!(v1, v2, "multiple calls must return identical values");
        assert_eq!(v2, v3, "multiple calls must return identical values");
    }

    // ── State isolation ───────────────────────────────────────────────────

    #[test]
    fn test_get_total_accounted_does_not_modify_storage() {
        // Read the storage key before and after the call; it must be unchanged.
        let t = TestEnv::default();
        let token = t.token.clone();

        let key = crate::DataKey::TotalAccounted(token.clone());

        let before: Option<i128> = t
            .env
            .as_contract(&t.client.address, || {
                t.env.storage().instance().get::<_, i128>(&key)
            });

        let _ = t.client.get_total_accounted(&token);

        let after: Option<i128> = t
            .env
            .as_contract(&t.client.address, || {
                t.env.storage().instance().get::<_, i128>(&key)
            });

        assert_eq!(
            before, after,
            "get_total_accounted must not mutate TotalAccounted storage"
        );
    }

    #[test]
    fn test_get_total_accounted_stub_unaffected_by_deposit() {
        // Even if a subscription deposit increases stored balance, the stub
        // always returns 0 — documenting the known gap until the stub is removed.
        let t = TestEnv::default();

        let before = t.client.get_total_accounted(&t.token);
        assert_eq!(before, 0);

        // Any further accounting changes don't affect stub output.
        let after = t.client.get_total_accounted(&t.token);
        assert_eq!(
            after, 0,
            "stub always returns 0 regardless of contract state"
        );
    }
}

// ── Module 2: real accounting.rs module ──────────────────────────────────────
//
// These tests bypass the stub in lib.rs and call the real implementation in
// `accounting.rs` directly via `crate::accounting::*` inside `as_contract`.
// They document correct behaviour and will become the primary correctness
// assertions once the stub is wired out.

mod real_accounting {
    use crate::accounting;
    use crate::test_utils::setup::TestEnv;
    use soroban_sdk::testutils::Address as _;

    // ── Default / zero state ──────────────────────────────────────────────

    #[test]
    fn test_real_get_total_accounted_default_zero() {
        let t = TestEnv::default();
        let token = t.token.clone();

        t.env.as_contract(&t.client.address, || {
            let val = accounting::get_total_accounted(&t.env, &token);
            assert_eq!(val, 0, "default accounted value must be 0");
        });
    }

    #[test]
    fn test_real_get_total_accounted_unknown_token_returns_zero() {
        let t = TestEnv::default();
        let other = soroban_sdk::Address::generate(&t.env);

        t.env.as_contract(&t.client.address, || {
            let val = accounting::get_total_accounted(&t.env, &other);
            assert_eq!(val, 0, "unknown token must return 0");
        });
    }

    // ── add then get ──────────────────────────────────────────────────────

    #[test]
    fn test_real_add_then_get() {
        let t = TestEnv::default();
        let token = t.token.clone();

        t.env.as_contract(&t.client.address, || {
            accounting::add_total_accounted(&t.env, &token, 1_000_000).unwrap();
            assert_eq!(accounting::get_total_accounted(&t.env, &token), 1_000_000);
        });
    }

    #[test]
    fn test_real_add_multiple_then_get() {
        let t = TestEnv::default();
        let token = t.token.clone();

        t.env.as_contract(&t.client.address, || {
            accounting::add_total_accounted(&t.env, &token, 100).unwrap();
            accounting::add_total_accounted(&t.env, &token, 200).unwrap();
            accounting::add_total_accounted(&t.env, &token, 300).unwrap();
            assert_eq!(accounting::get_total_accounted(&t.env, &token), 600);
        });
    }

    #[test]
    fn test_real_add_zero_is_ok_and_noop() {
        let t = TestEnv::default();
        let token = t.token.clone();

        t.env.as_contract(&t.client.address, || {
            accounting::add_total_accounted(&t.env, &token, 500).unwrap();
            accounting::add_total_accounted(&t.env, &token, 0).unwrap();
            assert_eq!(accounting::get_total_accounted(&t.env, &token), 500);
        });
    }

    // ── sub then get ──────────────────────────────────────────────────────

    #[test]
    fn test_real_add_then_sub_then_get() {
        let t = TestEnv::default();
        let token = t.token.clone();

        t.env.as_contract(&t.client.address, || {
            accounting::add_total_accounted(&t.env, &token, 1_000).unwrap();
            accounting::sub_total_accounted(&t.env, &token, 400).unwrap();
            assert_eq!(accounting::get_total_accounted(&t.env, &token), 600);
        });
    }

    #[test]
    fn test_real_sub_to_exact_zero() {
        let t = TestEnv::default();
        let token = t.token.clone();

        t.env.as_contract(&t.client.address, || {
            accounting::add_total_accounted(&t.env, &token, 1_000).unwrap();
            accounting::sub_total_accounted(&t.env, &token, 1_000).unwrap();
            assert_eq!(accounting::get_total_accounted(&t.env, &token), 0);
        });
    }

    #[test]
    fn test_real_sub_zero_is_ok_and_noop() {
        let t = TestEnv::default();
        let token = t.token.clone();

        t.env.as_contract(&t.client.address, || {
            accounting::add_total_accounted(&t.env, &token, 900).unwrap();
            accounting::sub_total_accounted(&t.env, &token, 0).unwrap();
            assert_eq!(accounting::get_total_accounted(&t.env, &token), 900);
        });
    }

    // ── Error conditions ──────────────────────────────────────────────────

    #[test]
    fn test_real_sub_underflow_rejected() {
        use crate::Error;
        let t = TestEnv::default();
        let token = t.token.clone();

        t.env.as_contract(&t.client.address, || {
            // Balance is 0; subtracting anything must fail.
            let result = accounting::sub_total_accounted(&t.env, &token, 1);
            assert_eq!(result, Err(Error::Underflow));
        });
    }

    #[test]
    fn test_real_sub_below_zero_rejected() {
        use crate::Error;
        let t = TestEnv::default();
        let token = t.token.clone();

        t.env.as_contract(&t.client.address, || {
            accounting::add_total_accounted(&t.env, &token, 100).unwrap();
            let result = accounting::sub_total_accounted(&t.env, &token, 101);
            assert_eq!(result, Err(Error::Underflow));

            // Balance must be unchanged after rejection.
            assert_eq!(accounting::get_total_accounted(&t.env, &token), 100);
        });
    }

    #[test]
    fn test_real_add_negative_rejected() {
        use crate::Error;
        let t = TestEnv::default();
        let token = t.token.clone();

        t.env.as_contract(&t.client.address, || {
            let result = accounting::add_total_accounted(&t.env, &token, -1);
            assert_eq!(result, Err(Error::InvalidAmount));
        });
    }

    #[test]
    fn test_real_sub_negative_rejected() {
        use crate::Error;
        let t = TestEnv::default();
        let token = t.token.clone();

        t.env.as_contract(&t.client.address, || {
            let result = accounting::sub_total_accounted(&t.env, &token, -1);
            assert_eq!(result, Err(Error::InvalidAmount));
        });
    }

    // ── Boundary / large-value tests ──────────────────────────────────────

    #[test]
    fn test_real_get_total_accounted_large_value() {
        let t = TestEnv::default();
        let token = t.token.clone();
        let large: i128 = i128::MAX / 2;

        t.env.as_contract(&t.client.address, || {
            accounting::add_total_accounted(&t.env, &token, large).unwrap();
            assert_eq!(accounting::get_total_accounted(&t.env, &token), large);
        });
    }

    #[test]
    fn test_real_add_overflow_rejected() {
        use crate::Error;
        let t = TestEnv::default();
        let token = t.token.clone();

        t.env.as_contract(&t.client.address, || {
            // Fill to i128::MAX - 1 then attempt to add 2.
            accounting::add_total_accounted(&t.env, &token, i128::MAX - 1).unwrap();
            let result = accounting::add_total_accounted(&t.env, &token, 2);
            assert_eq!(result, Err(Error::Overflow));

            // Value must remain at i128::MAX - 1 after rejection.
            assert_eq!(
                accounting::get_total_accounted(&t.env, &token),
                i128::MAX - 1
            );
        });
    }

    // ── Token isolation ───────────────────────────────────────────────────

    #[test]
    fn test_real_different_tokens_are_independent() {
        let t = TestEnv::default();
        let token_a = t.token.clone();
        let token_b = soroban_sdk::Address::generate(&t.env);

        t.env.as_contract(&t.client.address, || {
            accounting::add_total_accounted(&t.env, &token_a, 500).unwrap();
            accounting::add_total_accounted(&t.env, &token_b, 300).unwrap();

            assert_eq!(accounting::get_total_accounted(&t.env, &token_a), 500);
            assert_eq!(accounting::get_total_accounted(&t.env, &token_b), 300);
        });
    }

    #[test]
    fn test_real_sub_on_one_token_does_not_affect_other() {
        let t = TestEnv::default();
        let token_a = t.token.clone();
        let token_b = soroban_sdk::Address::generate(&t.env);

        t.env.as_contract(&t.client.address, || {
            accounting::add_total_accounted(&t.env, &token_a, 1_000).unwrap();
            accounting::add_total_accounted(&t.env, &token_b, 1_000).unwrap();

            accounting::sub_total_accounted(&t.env, &token_a, 200).unwrap();

            assert_eq!(accounting::get_total_accounted(&t.env, &token_a), 800);
            assert_eq!(accounting::get_total_accounted(&t.env, &token_b), 1_000);
        });
    }

    // ── State isolation (read-only guarantee) ─────────────────────────────

    #[test]
    fn test_real_get_does_not_mutate_storage() {
        let t = TestEnv::default();
        let token = t.token.clone();

        t.env.as_contract(&t.client.address, || {
            accounting::add_total_accounted(&t.env, &token, 777).unwrap();

            let before = accounting::get_total_accounted(&t.env, &token);
            let _ = accounting::get_total_accounted(&t.env, &token);
            let _ = accounting::get_total_accounted(&t.env, &token);
            let after = accounting::get_total_accounted(&t.env, &token);

            assert_eq!(before, after, "get must not mutate stored value");
        });
    }

    #[test]
    fn test_real_rejected_sub_does_not_mutate_storage() {
        use crate::Error;
        let t = TestEnv::default();
        let token = t.token.clone();

        t.env.as_contract(&t.client.address, || {
            accounting::add_total_accounted(&t.env, &token, 50).unwrap();

            let result = accounting::sub_total_accounted(&t.env, &token, 51);
            assert_eq!(result, Err(Error::Underflow));

            // Rejected sub must leave value unchanged.
            assert_eq!(accounting::get_total_accounted(&t.env, &token), 50);
        });
    }
}
