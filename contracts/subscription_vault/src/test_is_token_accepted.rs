//! Focused tests for `admin::is_token_accepted`.
//!
//! # What is under test
//!
//! `is_token_accepted(env, token)` checks whether a token address is registered
//! in the contract's accepted-token allowlist. Internally it queries
//! `env.storage().instance().has(&DataKey::TokenDecimals(token.clone()))`.
//!
//! # Cases exercised
//!
//! | # | Scenario | Expected |
//! |---|----------|----------|
//! | 1 | Default token set during `init` is accepted | `true` |
//! | 2 | Second token added with `add_accepted_token` is accepted | `true` |
//! | 3 | Never-registered token is not accepted | `false` |
//! | 4 | Token removed via `remove_accepted_token` is no longer accepted | `false` |
//! | 5 | Uninitialized contract: arbitrary token is not accepted | `false` |
//! | 6 | Contract's own address as the queried token returns false | `false` |
//! | 7 | Unauthorized `add_accepted_token` call leaves state unchanged | `false` |
//! | 8 | `is_token_accepted` agrees with `list_accepted_tokens` after add | consistent |
//! | 9 | `is_token_accepted` agrees with `list_accepted_tokens` after remove | consistent |
//! | 10| Multiple tokens added sequentially; all individually accepted | `true` each |
//! | 11| Adding same token twice does not break acceptance | `true` |
//! | 12| Token with zero decimals is still accepted | `true` |

use crate::{admin, SubscriptionVault};
use soroban_sdk::{
    testutils::Address as _,
    Address, Env,
};

// ── helpers ───────────────────────────────────────────────────────────────────

/// Register a fresh `SubscriptionVault` contract, initialize it with a real
/// SAC token, and return `(env, contract_id, admin_addr, default_token_addr)`.
fn setup_initialized() -> (Env, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 1_000_000);

    let token_issuer = Address::generate(&env);
    let default_token = env
        .register_stellar_asset_contract_v2(token_issuer)
        .address();

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());

    env.as_contract(&contract_id, || {
        admin::do_init(
            &env,
            default_token.clone(),
            6,
            admin.clone(),
            1_000_000i128,
            7 * 24 * 60 * 60,
        )
        .expect("init must succeed");
    });

    (env, contract_id, admin, default_token)
}

/// Advance the ledger past the config-change cooldown so the next admin config
/// call is not rejected with `Error::CooldownActive`.
fn skip_cooldown(env: &Env) {
    env.ledger()
        .with_mut(|l| l.timestamp += admin::CONFIG_COOLDOWN_SECS + 1);
}

// ── Case 1 ─────────────────────────────────────────────────────────────────────

/// The default token registered during `init` must immediately be accepted.
#[test]
fn test_default_token_is_accepted_after_init() {
    let (env, contract_id, _admin, default_token) = setup_initialized();

    let result = env.as_contract(&contract_id, || {
        admin::is_token_accepted(&env, &default_token)
    });

    assert!(
        result,
        "default token should be accepted right after init"
    );
}

// ── Case 2 ─────────────────────────────────────────────────────────────────────

/// A second token added by the admin via `add_accepted_token` must be accepted.
#[test]
fn test_added_token_is_accepted() {
    let (env, contract_id, admin, _default_token) = setup_initialized();

    let token_issuer = Address::generate(&env);
    let new_token = env
        .register_stellar_asset_contract_v2(token_issuer)
        .address();

    skip_cooldown(&env);

    env.as_contract(&contract_id, || {
        admin::add_accepted_token(&env, admin.clone(), new_token.clone(), 6)
            .expect("add_accepted_token must succeed");
    });

    let result = env.as_contract(&contract_id, || {
        admin::is_token_accepted(&env, &new_token)
    });

    assert!(result, "newly added token should be accepted");
}

// ── Case 3 ─────────────────────────────────────────────────────────────────────

/// An arbitrary address that was never registered must not be accepted.
#[test]
fn test_unregistered_token_is_not_accepted() {
    let (env, contract_id, _admin, _default_token) = setup_initialized();

    let unknown_token = Address::generate(&env);

    let result = env.as_contract(&contract_id, || {
        admin::is_token_accepted(&env, &unknown_token)
    });

    assert!(
        !result,
        "a never-registered token must not be accepted"
    );
}

// ── Case 4 ─────────────────────────────────────────────────────────────────────

/// After a token is removed via `remove_accepted_token` it must no longer be accepted.
#[test]
fn test_removed_token_is_not_accepted() {
    let (env, contract_id, admin, default_token) = setup_initialized();

    // Add a second token first (the default token cannot be removed).
    let token_issuer = Address::generate(&env);
    let extra_token = env
        .register_stellar_asset_contract_v2(token_issuer)
        .address();

    skip_cooldown(&env);

    env.as_contract(&contract_id, || {
        admin::add_accepted_token(&env, admin.clone(), extra_token.clone(), 6)
            .expect("add_accepted_token must succeed");
    });

    // Confirm it is accepted before removal.
    let before = env.as_contract(&contract_id, || {
        admin::is_token_accepted(&env, &extra_token)
    });
    assert!(before, "token must be accepted before removal");

    skip_cooldown(&env);

    // Now remove it.
    env.as_contract(&contract_id, || {
        admin::remove_accepted_token(&env, admin.clone(), extra_token.clone())
            .expect("remove_accepted_token must succeed");
    });

    let after = env.as_contract(&contract_id, || {
        admin::is_token_accepted(&env, &extra_token)
    });

    assert!(
        !after,
        "token must not be accepted after removal"
    );

    // Default token must still be accepted (removal does not affect others).
    let default_still_ok = env.as_contract(&contract_id, || {
        admin::is_token_accepted(&env, &default_token)
    });
    assert!(default_still_ok, "default token must remain accepted");
}

// ── Case 5 ─────────────────────────────────────────────────────────────────────

/// On an uninitialized contract (no `init` call) any token query must return false.
#[test]
fn test_uninitialized_contract_returns_false() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let arbitrary_token = Address::generate(&env);

    let result = env.as_contract(&contract_id, || {
        admin::is_token_accepted(&env, &arbitrary_token)
    });

    assert!(
        !result,
        "no token should be accepted on an uninitialized contract"
    );
}

// ── Case 6 ─────────────────────────────────────────────────────────────────────

/// The contract's own address is rejected by `do_init` (InvalidToken), so it
/// can never be in the accepted-token list and `is_token_accepted` must return false.
#[test]
fn test_contract_self_address_is_not_accepted() {
    let (env, contract_id, _admin, _default_token) = setup_initialized();

    let result = env.as_contract(&contract_id, || {
        admin::is_token_accepted(&env, &contract_id)
    });

    assert!(
        !result,
        "contract's own address must never be an accepted token"
    );
}

// ── Case 7 ─────────────────────────────────────────────────────────────────────

/// An `add_accepted_token` call from a non-admin caller must be rejected, and
/// the target token must remain not accepted — state must be unchanged.
#[test]
fn test_unauthorized_add_leaves_token_unaccepted() {
    let (env, contract_id, _admin, _default_token) = setup_initialized();

    // Generate a fake (non-admin) caller.
    let impostor = Address::generate(&env);
    let token_issuer = Address::generate(&env);
    let candidate_token = env
        .register_stellar_asset_contract_v2(token_issuer)
        .address();

    skip_cooldown(&env);

    // The call must fail because `impostor` is not the stored admin.
    let err = env.as_contract(&contract_id, || {
        admin::add_accepted_token(&env, impostor.clone(), candidate_token.clone(), 6)
    });

    assert!(
        err.is_err(),
        "add_accepted_token with a non-admin caller must return an error"
    );

    // After the rejected call, the token must still not be accepted.
    let still_not_accepted = env.as_contract(&contract_id, || {
        admin::is_token_accepted(&env, &candidate_token)
    });

    assert!(
        !still_not_accepted,
        "token must remain unaccepted after a rejected add_accepted_token call"
    );
}

// ── Case 8 ─────────────────────────────────────────────────────────────────────

/// After adding a token, `is_token_accepted` must agree with `list_accepted_tokens`.
#[test]
fn test_is_token_accepted_consistent_with_list_after_add() {
    let (env, contract_id, admin, _default_token) = setup_initialized();

    let token_issuer = Address::generate(&env);
    let new_token = env
        .register_stellar_asset_contract_v2(token_issuer)
        .address();

    skip_cooldown(&env);

    env.as_contract(&contract_id, || {
        admin::add_accepted_token(&env, admin.clone(), new_token.clone(), 8)
            .expect("add_accepted_token must succeed");
    });

    let accepted_flag = env.as_contract(&contract_id, || {
        admin::is_token_accepted(&env, &new_token)
    });

    let listed = env.as_contract(&contract_id, || admin::list_accepted_tokens(&env));
    let in_list = listed.iter().any(|at| at.token == new_token);

    assert_eq!(
        accepted_flag, in_list,
        "is_token_accepted ({accepted_flag}) must match presence in list_accepted_tokens ({in_list})"
    );
}

// ── Case 9 ─────────────────────────────────────────────────────────────────────

/// After removing a token, `is_token_accepted` must agree with `list_accepted_tokens`.
#[test]
fn test_is_token_accepted_consistent_with_list_after_remove() {
    let (env, contract_id, admin, _default_token) = setup_initialized();

    let token_issuer = Address::generate(&env);
    let extra_token = env
        .register_stellar_asset_contract_v2(token_issuer)
        .address();

    skip_cooldown(&env);

    env.as_contract(&contract_id, || {
        admin::add_accepted_token(&env, admin.clone(), extra_token.clone(), 6)
            .expect("add_accepted_token must succeed");
    });

    skip_cooldown(&env);

    env.as_contract(&contract_id, || {
        admin::remove_accepted_token(&env, admin.clone(), extra_token.clone())
            .expect("remove_accepted_token must succeed");
    });

    let accepted_flag = env.as_contract(&contract_id, || {
        admin::is_token_accepted(&env, &extra_token)
    });

    let listed = env.as_contract(&contract_id, || admin::list_accepted_tokens(&env));
    let in_list = listed.iter().any(|at| at.token == extra_token);

    assert_eq!(
        accepted_flag, in_list,
        "is_token_accepted ({accepted_flag}) must match presence in list_accepted_tokens ({in_list}) after remove"
    );
}

// ── Case 10 ────────────────────────────────────────────────────────────────────

/// Multiple tokens added sequentially must each be individually accepted.
#[test]
fn test_multiple_tokens_each_individually_accepted() {
    let (env, contract_id, admin, _default_token) = setup_initialized();

    let token_issuer = Address::generate(&env);
    let mut tokens = std::vec::Vec::new();

    for _ in 0..4 {
        let t = env
            .register_stellar_asset_contract_v2(token_issuer.clone())
            .address();
        tokens.push(t);
    }

    for token in &tokens {
        skip_cooldown(&env);
        env.as_contract(&contract_id, || {
            admin::add_accepted_token(&env, admin.clone(), token.clone(), 6)
                .expect("add_accepted_token must succeed");
        });
    }

    for token in &tokens {
        let result = env.as_contract(&contract_id, || {
            admin::is_token_accepted(&env, token)
        });
        assert!(result, "each added token must be accepted: {token:?}");
    }
}

// ── Case 11 ────────────────────────────────────────────────────────────────────

/// Adding the same token a second time (idempotent upsert) must not break acceptance.
#[test]
fn test_adding_same_token_twice_is_still_accepted() {
    let (env, contract_id, admin, _default_token) = setup_initialized();

    let token_issuer = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_issuer)
        .address();

    skip_cooldown(&env);

    env.as_contract(&contract_id, || {
        admin::add_accepted_token(&env, admin.clone(), token.clone(), 6)
            .expect("first add must succeed");
    });

    skip_cooldown(&env);

    // Second add of the same token — only updates the decimal entry, no duplicate
    // in the list because the code skips the push when the key already exists.
    env.as_contract(&contract_id, || {
        admin::add_accepted_token(&env, admin.clone(), token.clone(), 8)
            .expect("second add (decimal update) must succeed");
    });

    let result = env.as_contract(&contract_id, || {
        admin::is_token_accepted(&env, &token)
    });

    assert!(result, "token must still be accepted after duplicate add");

    // Verify no duplicate in list.
    let listed = env.as_contract(&contract_id, || admin::list_accepted_tokens(&env));
    let count = listed.iter().filter(|at| at.token == token).count();
    assert_eq!(count, 1, "token must appear exactly once in list after duplicate add");
}

// ── Case 12 ────────────────────────────────────────────────────────────────────

/// A token registered with zero decimals is still accepted (boundary value).
#[test]
fn test_token_with_zero_decimals_is_accepted() {
    let (env, contract_id, admin, _default_token) = setup_initialized();

    let token_issuer = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_issuer)
        .address();

    skip_cooldown(&env);

    env.as_contract(&contract_id, || {
        admin::add_accepted_token(&env, admin.clone(), token.clone(), 0)
            .expect("add_accepted_token with 0 decimals must succeed");
    });

    let result = env.as_contract(&contract_id, || {
        admin::is_token_accepted(&env, &token)
    });

    assert!(result, "token with 0 decimals must still be accepted");
}
