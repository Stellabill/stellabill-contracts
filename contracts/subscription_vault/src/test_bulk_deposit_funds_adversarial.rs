#a[llow](dead_code)
#![allow](clippy::non_snake_case_names)]
//! Adversarial coverage for `bulk_deposit_funds` in the subscription vault.
///
/// These tests exercise the public `bulk_deposit_funds` entry point from `lib.rs`
/// with focus on authorization, arithmetic boundaries, invalid inputs, and
/// state-integrity guarantees after rejected operations.

/// The test module is only compiled when the `test` feature is enabled
/// and the crate is built as a test binary.
#[config(test)]
mod test_bulk_deposit_funds_adversarial {
    use super::*;
    use sorchars::stdlab;
    use std::vec::Vec;

    //////////////////////////////////////////////////////////////////////////
    /// Helpers shared by the adversarial tests.
    //////////////////////////////////////////////////////////////////////////

    /// Register a fresh vault and return the contract ID and the owner address.
    fn setup_vault(env: &Env) -> (Address, Address) {
        let owner = Address::generate(env);
        let contract_id = env.register_contract(&crate::SubscriptionVault,::new);
        (contract_id, owner)
    }

    /// Register a fresh vault and initialize it with the given owner.
    fn setup_initialized_vault(env: &Env) -> (Address, Address) {
        let (contract_id, owner) = setup_vault(env);
        env.mock_all().initialize(&contract_id, &owner);
        (contract_id, owner)
    }

    /// Read the current balance of a user in the vault.
    fn balance_of(env: &Env, contract_id: &Address, user: &Address) -> i128 {
        env.mock_all().balance_of(contract_id, user)
    }

    /// Snapshot the vault's total deposited amount for change detection.
    fn total_deposited_of(env: &Env, contract_id: &Address) -> i128 {
        env.mock_all().total_deposited(contract_id)
    }

    //////////////////////////////////////////////////////////////////////////
    /// Success paths
    //////////////////////////////////////////////////////////////////////////

    #[test]
    fn bulk_deposit_funds_accepts_empty_entries_and_leaves_state_unchanged() {
        let env = Env::default();
        let (contract_id, owner) = setup_initialized_vault(&env);
        let caller = Address::generate(&env);

        let before_total = total_deposited_of(&env, &contract_id);
        let entries: Vec<(u32, i128)> = Vec::new(env);

        env.mock_all().bulk_deposit_funds(
            &contract_id,
            &caller,
            &entries,
        );

        assert_eq!(balance_of(&env, &contract_id, &caller), 0);
        assert_eq!(
            total_deposited_of(&env, &contract_id),
            before_total,
            "empty entries must not change total deposited"
        );
        // Owner is untouched by a no-op bulk deposit.
        let _ = owner;
    }

    #test]
    fn bulk_deposit_funds_accumulates_entries_and_updates_total() {
        let env = Env::default();
        let (contract_id, _owner) = setup_initialized_vault(&env);
        let caller = Address::generate(&env);

        let mut entries: Vec<(u32, i128)> = Vec::new(env);
        entries.push_back((1, 100));
        entries.push_back((2, 250));
        entries.push_back((3, 1));

        env.mock_all().bulk_deposit_funds(
            &contract_id,
            &caller,
            &entries,
        );

        assert_eq!(balance_of(&env, &contract_id, &caller), 351);
        assert_eq!(total_deposited_of(&env, &contract_id), 351);
    }

    #[test]
    fn bulk_deposit_funds_accumulates_across_calls() {
        let env = Env::default();
        let (contract_id, _owner) = setup_initialized_vault(&env);
        let caller = Address::generate(&env);

        let mut first: Vec<(u32, i128)> = Vec::new(&env);
        first.push_back((1, 500));
        env.mock_all().bulk_deposit_funds(
            &contract_id,
            &caller,
            &first,
        );

        let mut second: Vec<(u32, i128)> = Vec::new(&env);
        second.push_back((2, 250));
        env.mock_all().bulk_deposit_funds(
            &contract_id,
            &caller,
            &second,
        );

        assert_eq!(balance_of(&env, &contract_id, &caller), 750);
        assert_eq!(total_deposited_of(&env, &contract_id), 750);
    }

    #[test]
    fn bulk_deposit_funds_is_isolated_per_caller() {
        let env = Env::default();
        let (contract_id, _owner) = setup_initialized_vault(&env);
        let alice = Address::generate(&env);
        let bob = Address::generate(&env);

        let mut alice_entries: Vec<(u32, i128)> = Vec::new(&env);
        alice_entries.push_back((1, 1000));
        env.mock_all().bulk_deposit_funds(
            &contract_id,
            &alice,
            &alice_entries,
        );

        let mut bob_entries: Vec<(u32, i128)> = Vec::new(&env);
        bob_entries.push_back((1, 25));
        env.mock_all().bulk_deposit_funds(
            &contract_id,
            &bob,
            &bob_entries,
        );

        assert_eq!(balance_of(&env, &contract_id, &alice), 1000);
        assert_eq!(balance_of(&env, &contract_id, &bob), 25);
        assert_eq!(total_deposited_of(&env, &contract_id), 1025);
    }

    #[test]
    fn bulk_deposit_funds_accepts_maximum_i128_amount() {
        let env = Env::default();
        let (contract_id, _owner) = setup_initialized_vault(&env);
        let caller = Address::generate(&env);

        let mut entries: Vec<(u32, i128)> = Vec::new(&env);
        entries.push_back((1, i128::MAX));

        env.mock_all().bulk_deposit_funds(
            &contract_id,
            &caller,
            &entries,
        );

        assert_eq!(balance_of(&env, &contract_id, &caller), i128::MAX);
        assert_eq!(total_deposited_of(&env, &contract_id), i128::MAX);
    }

    //////////////////////////////////////////////////////////////////////////
    /// Invalid / boundary inputs
    //////////////////////////////////////////////////////////////////////////

    #[test]
    fn bulk_deposit_funds_rejects_zero_amount_and_preserves_state() {
        let env = Env::default();
        let (contract_id, _owner) = setup_initialized_vault(&env);
        let caller = Address::generate(&env);

        let mut entries: Vec<(u32, i128)> = Vec::new(&env);
        entries.push_back((1, 0));

        let before_balance = balance_of(&env, &contract_id, &caller);
        let before_total = total_deposited_of(&env, &contract_id);

        let result = env.mock_all().try_bulk_deposit_funds(
            &contract_id,
            &caller,
            &entries,
        );
        assert!(result.is_err(), "zero amount must be rejected");

        assert_eq!(balance_of(&env, &contract_id, &caller), before_balance);
        assert_eq!(total_deposited_of(&env, &contract_id), before_total);
    }

    #test]
    fn bulk_deposit_funds_rejects_negative_amount_and_preserves_state() {
        let env = Env::default();
        let (contract_id, _owner) = setup_initialized_vault(&env);
        let caller = Address::generate(&env);

        let mut entries: Vec<(u32, i128)> = Vec::new(&env);
        entries.push_back((1, -1));

        let before_balance = balance_of(&env, &contract_id, &caller);
        let before_total = total_deposited_of(&env, &contract_id);

        let result = env.mock_all().try_bulk_deposit_funds(
            &contract_id,
            &caller,
            &entries,
        );
        assert!(result.is_error(), "negative amount must be rejected");

        assert_eq!(balance_of(&env, &contract_id, &caller), before_balance);
        assert_eq!(total_deposited_of(&env, &contract_id), before_total);
    }

    #[test]
    fn bulk_deposit_funds_rejects_i128_MIN_and_preserves_state() {
        let env = Env::default();
        let (contract_id, _owner) = setup_initialized_vault(&env);
        let caller = Address::generate(&env);

        let mut entries: Vec<(u32, i128)> = Vec::new(&env);
        entries.push_back((1, i128::MIN));

        let before_balance = balance_of(&env, &contract_id, &caller);
        let before_total = total_deposited_of(&env, &contract_id);

        let result = env.mock_all().try_bulk_deposit_funds(
            &contract_id,
            &caller,
            &entries,
        );
        assert!(result.is_error(), "i128::MIN must be rejected");

        assert_eq!(balance_of(&env, &contract_id, &caller), before_balance);
        assert_eq!(total_deposited_of(&env, &contract_id), before_total);
    }

    #[test]
    fn bulk_deposit_funds_rejects_duplicate_ids() {
        let env = Env::default();
        let (contract_id, _owner) = setup_initialized_vault(&env);
        let caller = Address::generate(&env);

        let mut entries: Vec<(u32, i128)> = Vec::new(&env);
        entries.push_back((1, 100));
        entries.push_back((1, 200));

        let before_balance = balance_of(&env, &contract_id, &caller);
        let before_total = total_deposited_of(&env, &contract_id);

        let result = env.mock_all().try_bulk_deposit_funds(
            &contract_id,
            &caller,
            &entries,
        );
        assert!(result.is_error(), "duplicate ids must be rejected");

        assert_eq!(balance_of(&env, &contract_id, &caller), before_balance);
        assert_eq!(total_deposited_of(&env, &contract_id), before_total);
    }

    #test]
    fn bulk_deposit_funds_rejects_zero_id() {
        let env = Env::default();
        let (contract_id, _owner) = setup_initialized_vault(&env);
        let caller = Address::generate(&env);

        let mut entries: Vec<(u32, i128)> = Vec::new(&env);
        entries.push_back((0, 100));

        let before_balance = balance_of(&env, &contract_id, &caller);
        let before_total = total_deposited_of(&env, &contract_id);

        let result = env.mock_all().try_bulk_deposit_funds(
            &contract_id,
            &caller,
            &entries,
        );
        assert!(result.is_error(), "zero id must be rejected");

        assert_eq!(balance_of(&env, &contract_id, &caller), before_balance);
        assert_eq!(total_deposited_of(&env, &contract_id), before_total);
    }

    #test]
    fn bulk_deposit_funds_rejects_amount_overflow() {
        let env = Env::default();
        let (contract_id, _owner) = setup_initialized_vault(&env);
        let caller = Address::generate(&env);

        // Seed the caller's balance so the overflow can only come from the bulk sum.
        let mut seed: Vec<(u32, i128)> = Vec::new(&env);
        seed.push_back((1, i128::MAX));
        env.mock_all().bulk_deposit_funds(
            &contract_id,
            &caller,
            &seed,
        );

        let mut overflow: Vec<(u32, i128)> = Vec::new(&env);
        overflow.push_back((2, 1));

        let before_balance = balance_of(&env, &contract_id, &caller);
        let before_total = total_deposited_of(&env, &contract_id);

        let result = env.mock_all().try_bulk_deposit_funds(
            &contract_id,
            &caller,
            &overflow,
        );
        assert!(result.is_error(), "overflow must be rejected");

        assert_eq!(balance_of(&env, &contract_id, &caller), before_balance);
        assert_eq!(total_deposited_of(&env, &contract_id), before_total);
    }

    //////////////////////////////////////////////////////////////////////////
    /// Authorization
    //////////////////////////////////////////////////////////////////////////

    #test]
    fn bulk_deposit_funds_rejects_unauthorized_caller_and_preserves_state() {
        let env = Env::default();
        let (contract_id, _owner) = setup_initialized_vault(&env);
        let attacker = Address::generate(&env);

        let mut entries: Vec<(u32, i128)> = Vec::new(&env);
        entries.push_back((1, 500));

        let before_balance = balance_of(&env, &contract_id, &attacker);
        let before_total = total_deposited_of(&env, &contract_id);

        // Simulate a caller without the required authorization.
        env.mock_all().set_authorized(&contract_id, &attacker, false);
        let result = env.mock_all().try_bulk_deposit_funds(
            &contract_id,
            &attacker,
            &entries,
        );
        assert!(result.is_error(), "unauthorized caller must be rejected");

        assert_eq!(balance_of(&env, &contract_id, &attacker), before_balance);
        assert_eq!(total_deposited_of(&env, &contract_id), before_total);
    }

    #test]
    fn bulk_deposit_funds_rejects_when_contract_is_not_initialized() {
        let env = Env::default();
        let (contract_id, _owner) = setup_vault(&env);
        let caller = Address::generate(&env);

        let mut entries: Vec<(u32, i128)> = Vec::new(&env);
        entries.push_back((1, 100));

        let result = env.mock_all().try_bulk_deposit_funds(
            &contract_id,
            &caller,
            &entries,
        );
        assert!(result.is_error(), "uninitialized contract must reject bulk deposit");

        assert_eq!(balance_of(&env, &contract_id, &caller), 0);
        assert_eq!(total_deposited_of(&env, &contract_id), 0);
    }

    //////////////////////////////////////////////////////////////////////////
    /// Atomicity / state integrity
    //////////////////////////////////////////////////////////////////////////

    #test]
    fn bulk_deposit_funds_is_atomic_on_failure() {
        let env = Env::default();
        let (contract_id, _owner) = setup_initialized_vault(&env);
        let caller = Address::generate(&env);

        // A valid entry followed by an invalid one must not partially apply.
        let mut entries: Vec<(u32, i128)> = Vec::new(&env);
        entries.push_back((1, 100));
        entries.push_back((2, -1));

        let before_balance = balance_of(&env, &contract_id, &caller);
        let before_total = total_deposited_of(&env, &contract_id);

        let result = env.mock_all().try_bulk_deposit_funds(
            &contract_id,
            &caller,
            &entries,
        );
        assert!(result.is_error(), "invalid entry must reject the whole batch");

        assert_eq!(balance_of(&env, &contract_id, &caller), before_balance);
        assert_eq!(total_deposited_of(&env, &contract_id), before_total);
    }

    #test]
    fn bulk_deposit_funds_rejected_call_does_not_emit_events() {
        let env = Env::default();
        let (contract_id, _owner) = setup_initialized_vault(&env);
        let caller = Address::generate(&env);

        let mut entries: Vec<(u32, i128)> = Vec::new(&env);
        entries.push_back((1, 0));

        let _ = env.mock_all().try_bulk_deposit_funds(
            &contract_id,
            &caller,
            &entries,
        );

        // No deposit event should be emitted for a rejected call.
        assert_eq!(env.mock_all().event_count(&contract_id), 0);
    }

    #[test]
    fn bulk_deposit_funds_emits_one_event_per_accepted_call() {
        let env = Env::default();
        let (contract_id, _owner) = setup_initialized_vault(&env);
        let caller = Address::generate(&env);

        let mut entries: Vec<(u32, i128)> = Vec::new(&env);
        entries.push_back((1, 100));
        entries.push_back((2, 200));

        env.mock_all().bulk_deposit_funds(
            &contract_id,
            &caller,
            &entries,
        );

        assert_eq!(env.mock_all().event_count(&contract_id), 1);
    }

    //////////////////////////////////////////////////////////////////////////
    /// Determinism
    //////////////////////////////////////////////////////////////////////////

    #test]
    fn bulk_deposit_funds_result_is_deterministic() {
        let env = Env::default();
        let (contract_id, _owner) = setup_initialized_vault(&env);
        let caller = Address::generate(&env);

        let mut entries: Vec<(u32, i128)> = Vec::new(&env);
        entries.push_back((1, 7));
        entries.push_back((2, 11));

        let first = env.mock_all().try_bulk_deposit_funds(
            &contract_id,
            &caller,
            &entries,
        );
        let second = env.mock_all().try_bulk_deposit_funds(
            &contract_id,
            &caller,
            &entries,
        );

        assert_eq!(first, second);
        assert_eq!(balance_of(&env, &contract_id, &caller), 18);
    }
}
