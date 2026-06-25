#![cfg(test)]

extern crate alloc;

use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::StellarAssetClient as TokenAdminClient,
    Address, Env, Vec as SorobanVec,
};
use subscription_vault::{
    DataKey, SubscriptionVault, SubscriptionVaultClient,
};

const NUM_TOKENS: usize = 10;

#[derive(Clone, Debug)]
enum Op {
    AddToken(usize),
    RemoveToken(usize),
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..NUM_TOKENS).prop_map(Op::AddToken),
        (0..NUM_TOKENS).prop_map(Op::RemoveToken),
    ]
}

fn read_accepted_tokens_raw(
    env: &Env,
    contract_id: &Address,
) -> SorobanVec<Address> {
    env.as_contract(contract_id, || {
        env.storage()
            .instance()
            .get(&DataKey::AcceptedTokens)
            .unwrap_or_else(|| SorobanVec::new(env))
    })
}

fn read_token_decimals_raw(
    env: &Env,
    contract_id: &Address,
    token: &Address,
) -> Option<u32> {
    env.as_contract(contract_id, || {
        env.storage()
            .instance()
            .get(&DataKey::TokenDecimals(token.clone()))
    })
}

fn check_invariants(
    env: &Env,
    vault: &SubscriptionVaultClient,
    contract_id: &Address,
) {
    let raw_tokens = read_accepted_tokens_raw(env, contract_id);
    let listed = vault.list_accepted_tokens();

    // Invariant 1: No duplicate entries in AcceptedTokens
    {
        let mut seen: std::vec::Vec<Address> = std::vec::Vec::new();
        for token in raw_tokens.iter() {
            if seen.contains(&token) {
                panic!("Duplicate token in AcceptedTokens: {:?}", token);
            }
            seen.push(token.clone());
        }
    }

    // Invariant 2: Every entry in AcceptedTokens has matching TokenDecimals
    for token in raw_tokens.iter() {
        assert!(
            read_token_decimals_raw(env, contract_id, &token).is_some(),
            "Token {:?} in AcceptedTokens but missing TokenDecimals",
            token,
        );
    }

    // Invariant 3: list_accepted_tokens is consistent
    for accepted in listed.iter() {
        let dec = read_token_decimals_raw(env, contract_id, &accepted.token);
        assert!(
            dec.is_some(),
            "Token {:?} returned by list_accepted_tokens but missing TokenDecimals",
            accepted.token,
        );
        assert_eq!(
            dec.unwrap(),
            accepted.decimals,
            "Token {:?} decimals mismatch: stored={}, listed={}",
            accepted.token,
            dec.unwrap(),
            accepted.decimals,
        );
    }

    assert_eq!(
        raw_tokens.len(),
        listed.len(),
        "list_accepted_tokens count ({}) != raw AcceptedTokens count ({})",
        listed.len(),
        raw_tokens.len(),
    );

    // Invariant 4: get_token_subscription_count is callable for all accepted tokens
    for token in raw_tokens.iter() {
        let _count = vault.get_token_subscription_count(&token);
    }
}

fn setup_env<'a>(
) -> (Env, SubscriptionVaultClient<'a>, Address, Address, std::vec::Vec<Address>) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000_000);

    let token_admin = Address::generate(&env);
    let default_token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_admin_client = TokenAdminClient::new(&env, &default_token);

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());
    let vault = SubscriptionVaultClient::new(&env, &contract_id);

    vault.init(&default_token, &7, &admin, &100, &(3 * 86400));

    let mut tokens = std::vec::Vec::new();
    tokens.push(default_token.clone());
    for _ in 1..NUM_TOKENS {
        let t = env
            .register_stellar_asset_contract_v2(token_admin.clone())
            .address();
        tokens.push(t);
    }

    for token in &tokens {
        vault.add_accepted_token(&admin, token, &7u32);
    }

    // Create subscriptions using some tokens to exercise subscription count path
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin_client.mint(&subscriber, &1_000_000_000);

    for i in 0..NUM_TOKENS.min(5) {
        let token = &tokens[i];
        vault.create_subscription_with_token(
            &subscriber,
            &merchant,
            token,
            &1000i128,
            &(30 * 24 * 60 * 60u64),
            &false,
            &None,
            &None::<u64>,
        );
    }

    (env, vault, admin, contract_id, tokens)
}

proptest! {
    #![proptest_config(Config {
        cases: 2,
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource("accepted_tokens_failures"))),
        .. Config::default()
    })]

    #[test]
    fn test_accepted_tokens_invariant(ops in prop::collection::vec(op_strategy(), 1000)) {
        let (env, vault, admin, contract_id, tokens) = setup_env();

        for op in ops {
            match op {
                Op::AddToken(idx) => {
                    let token = &tokens[idx];
                    let _ = vault.try_add_accepted_token(&admin, token, &7u32);
                }
                Op::RemoveToken(idx) => {
                    let token = &tokens[idx];
                    let _ = vault.try_remove_accepted_token(&admin, token);
                }
            }

            check_invariants(&env, &vault, &contract_id);
        }
    }
}
