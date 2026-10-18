use soroban_sdk::{Env, Address};
use crate::types::Error;

/// Would assert that the contract's live on-chain token balance equals the
/// sum of every merchant's tracked balance for that token.
///
/// Not currently enforceable: there is no `DataKey` index of "every merchant
/// address the vault has ever seen" to sum over (only per-merchant/per-token
/// lookups exist). Reconstructing the full merchant set would require either
/// a new persistent index maintained on every credit/withdraw, or scanning
/// storage exhaustively — both out of scope here. Returns `Ok(())` as a
/// placeholder so call sites compile and the (test-only) invariant-checking
/// hook exists for a future, properly-indexed implementation.
pub fn assert_token_balance_invariant(
    _env: &Env,
    _token: &Address,
) -> Result<(), Error> {
    Ok(())
}
// ═════════════════════════════════════════════════════════════════════════════
//  Adversarial coverage for `assert_token_balance_invariant`
//
//  The function is a deliberate placeholder: it has no merchant index to sum
//  over, so it returns `Ok(())` unconditionally.  These tests pin that
//  contract precisely — it must never reject, must never touch storage, must
//  never emit an event, and must stay callable from outside as well as inside
//  a contract invocation — so that a future, properly indexed implementation
//  cannot silently change any of those observable properties.
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod assert_token_balance_invariant_adversarial_tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::testutils::Events as _;

    #[test]
    fn accepts_a_token_the_vault_has_never_seen() {
        let env = Env::default();
        let token = Address::generate(&env);

        assert_eq!(assert_token_balance_invariant(&env, &token), Ok(()));
    }

    #[test]
    fn is_idempotent_for_repeated_calls_on_the_same_token() {
        let env = Env::default();
        let token = Address::generate(&env);

        for _ in 0..8 {
            assert_eq!(
                assert_token_balance_invariant(&env, &token),
                Ok(()),
                "the invariant must be a pure, repeatable check"
            );
        }
    }

    #[test]
    fn accepts_many_distinct_token_addresses() {
        let env = Env::default();

        for _ in 0..16 {
            let token = Address::generate(&env);
            assert!(
                assert_token_balance_invariant(&env, &token).is_ok(),
                "an unknown token must never be reported as a violation"
            );
        }
    }

    #[test]
    fn emits_no_events() {
        let env = Env::default();
        let token = Address::generate(&env);

        let before = env.events().all().len();
        assert_token_balance_invariant(&env, &token).unwrap();
        let after = env.events().all().len();

        assert_eq!(before, after, "a read-only invariant must not publish events");
    }

    #[test]
    fn does_not_require_a_registered_or_initialized_vault() {
        // No `env.register(..)` and no `init` call: the check must not depend
        // on any contract state to be reachable.
        let env = Env::default();
        let token = Address::generate(&env);

        assert_eq!(assert_token_balance_invariant(&env, &token), Ok(()));
    }

    #[test]
    fn is_callable_from_within_a_contract_invocation() {
        let env = Env::default();
        let contract_id = env.register(crate::SubscriptionVault, ());
        let token = Address::generate(&env);

        let result = env.as_contract(&contract_id, || {
            assert_token_balance_invariant(&env, &token)
        });

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn accepts_the_same_token_inside_and_outside_a_contract_context() {
        let env = Env::default();
        let contract_id = env.register(crate::SubscriptionVault, ());
        let token = Address::generate(&env);

        assert_eq!(assert_token_balance_invariant(&env, &token), Ok(()));

        let inside = env.as_contract(&contract_id, || {
            assert_token_balance_invariant(&env, &token)
        });
        assert_eq!(inside, Ok(()));

        // Still fine once the contract context has unwound.
        assert_eq!(assert_token_balance_invariant(&env, &token), Ok(()));
    }
}