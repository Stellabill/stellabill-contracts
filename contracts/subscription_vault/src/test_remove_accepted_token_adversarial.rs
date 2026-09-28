//! Adversarial coverage for `admin::remove_accepted_token` (issue #993).
//!
//! The admin surface exposes `remove_accepted_token(env, admin, token)` through
//! the `SubscriptionVault` entrypoints without a directly associated test
//! fixture. These tests pin its success path, ordering guarantee, default-token
//! guard, auth guard, per-key config cooldown semantics, unknown-token
//! idempotency, and the decimals-entry/list-entry split, and they assert the
//! stored state after every rejected call.

use crate::test_utils::setup::TestEnv;
use crate::types::{AcceptedToken, DataKey, Error};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Vec,
};

/// Ledger time used by every test so the `"AcceptedTokens"` cooldown bucket
/// (which is keyed only by timestamp) starts from a non-zero value. A zero
/// timestamp would make `enforce_config_cooldown` treat every mutation as the
/// first one, which would hide the cooldown behaviour entirely.
const T0: u64 = 1_000_000;

/// Per-key protocol-config cooldown, as enforced by `admin::enforce_config_cooldown`.
const COOLDOWN: u64 = crate::admin::CONFIG_COOLDOWN_SECS;

/// Initialise the contract at [`T0`]; `te.token` is the default token accepted
/// by `init` and its `"AcceptedTokens"` cooldown bucket is still unarmed.
fn setup() -> TestEnv {
    let te = TestEnv::default();
    te.env.ledger().set_timestamp(T0);
    te
}

/// Read the accepted-token list through the public entrypoint.
fn accepted_list(te: &TestEnv) -> Vec<AcceptedToken> {
    te.client.list_accepted_tokens()
}

/// Whether `token` appears in the accepted-token list entry.
fn list_contains(te: &TestEnv, token: &Address) -> bool {
    let list = accepted_list(te);
    for i in 0..list.len() {
        if &list.get(i).unwrap().token == token {
            return true;
        }
    }
    false
}

/// The production acceptance predicate: presence of the decimals entry.
fn is_accepted(te: &TestEnv, token: &Address) -> bool {
    te.env.as_contract(&te.client.address, || {
        crate::admin::is_token_accepted(&te.env, token)
    })
}

/// The decimals stored for `token`, or `None` when the token is not accepted.
fn token_decimals(te: &TestEnv, token: &Address) -> Option<u32> {
    te.env.as_contract(&te.client.address, || {
        crate::admin::get_token_decimals(&te.env, token).ok()
    })
}

/// Seed a decimals entry without touching the accepted-token list. The public
/// API cannot reach this split state, so the boundary test writes storage
/// directly.
fn seed_decimals_only(te: &TestEnv, token: &Address, decimals: u32) {
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::TokenDecimals(token.clone()), &decimals);
    });
}

// ── 1. Happy path ────────────────────────────────────────────────────────────

#[test]
fn remove_accepted_token_happy_path_removes_token() {
    let te = setup();
    let extra = Address::generate(&te.env);

    // `add_accepted_token` arms the "AcceptedTokens" cooldown bucket at T0.
    te.client.add_accepted_token(&te.admin, &extra, &7);
    assert!(is_accepted(&te, &extra));
    assert_eq!(token_decimals(&te, &extra), Some(7));
    assert!(list_contains(&te, &extra));
    assert_eq!(accepted_list(&te).len(), 2);

    te.jump(COOLDOWN);
    te.client.remove_accepted_token(&te.admin, &extra);

    // Both the decimals entry and the list entry must be gone.
    assert!(!is_accepted(&te, &extra));
    assert_eq!(token_decimals(&te, &extra), None);
    assert!(!list_contains(&te, &extra));

    let list = accepted_list(&te);
    assert_eq!(list.len(), 1);
    assert_eq!(list.get(0).unwrap().token, te.token);
    assert_eq!(list.get(0).unwrap().decimals, 6);
    // The default token is untouched by the removal.
    assert!(is_accepted(&te, &te.token));
    assert_eq!(token_decimals(&te, &te.token), Some(6));
}

// ── 2. Order preservation ────────────────────────────────────────────────────

#[test]
fn remove_accepted_token_preserves_order_of_survivors() {
    let te = setup();
    let middle = Address::generate(&te.env);
    let last = Address::generate(&te.env);

    te.client.add_accepted_token(&te.admin, &middle, &6);
    te.jump(COOLDOWN);
    te.client.add_accepted_token(&te.admin, &last, &8);
    assert_eq!(accepted_list(&te).len(), 3);

    let before = accepted_list(&te);
    assert_eq!(before.get(0).unwrap().token, te.token);
    assert_eq!(before.get(1).unwrap().token, middle);
    assert_eq!(before.get(2).unwrap().token, last);

    te.jump(COOLDOWN);
    te.client.remove_accepted_token(&te.admin, &middle);

    // Removing the middle entry keeps the relative order of the survivors.
    let after = accepted_list(&te);
    assert_eq!(after.len(), 2);
    assert_eq!(after.get(0).unwrap().token, te.token);
    assert_eq!(after.get(1).unwrap().token, last);
    assert_eq!(after.get(1).unwrap().decimals, 8);
    assert!(!is_accepted(&te, &middle));
    assert!(is_accepted(&te, &last));
    assert!(is_accepted(&te, &te.token));
}

// ── 3. Default-token rejection ───────────────────────────────────────────────

#[test]
fn remove_accepted_token_rejects_default_token() {
    let te = setup();
    let extra = Address::generate(&te.env);

    let res = te.client.try_remove_accepted_token(&te.admin, &te.token);
    assert_eq!(res, Err(Ok(Error::InvalidInput)));

    // State unchanged after the rejected call.
    assert!(is_accepted(&te, &te.token));
    assert_eq!(token_decimals(&te, &te.token), Some(6));
    assert!(list_contains(&te, &te.token));
    assert_eq!(accepted_list(&te).len(), 1);

    // The guard runs before the cooldown, so the very next config mutation at
    // the same ledger time must still succeed. This would be CooldownActive if
    // the rejected removal had armed the "AcceptedTokens" bucket.
    te.client.add_accepted_token(&te.admin, &extra, &9);
    assert!(is_accepted(&te, &extra));
    assert_eq!(token_decimals(&te, &extra), Some(9));
    assert_eq!(accepted_list(&te).len(), 2);
}

// ── 4. Unauthorized caller ───────────────────────────────────────────────────

#[test]
fn remove_accepted_token_rejects_non_admin_and_leaves_state() {
    let te = setup();
    let extra = Address::generate(&te.env);
    let stranger = Address::generate(&te.env);

    te.client.add_accepted_token(&te.admin, &extra, &7);
    te.jump(COOLDOWN);

    let res = te.client.try_remove_accepted_token(&stranger, &extra);
    assert_eq!(res, Err(Ok(Error::Forbidden)));

    // A rejected non-admin call must leave the registry byte-for-byte intact.
    assert!(is_accepted(&te, &extra));
    assert_eq!(token_decimals(&te, &extra), Some(7));
    assert!(list_contains(&te, &extra));
    assert_eq!(accepted_list(&te).len(), 2);

    // The stored admin can still remove it immediately afterwards: the
    // rejected call consumed no cooldown and mutated nothing.
    te.client.remove_accepted_token(&te.admin, &extra);
    assert!(!is_accepted(&te, &extra));
    assert!(!list_contains(&te, &extra));
    assert_eq!(accepted_list(&te).len(), 1);
}

// ── 5. Cooldown: rejection must not advance the window ───────────────────────

#[test]
fn remove_accepted_token_cooldown_rejects_and_does_not_advance_window() {
    let te = setup();
    let extra = Address::generate(&te.env);

    // Arms the shared "AcceptedTokens" bucket at T0.
    te.client.add_accepted_token(&te.admin, &extra, &7);
    assert!(is_accepted(&te, &extra));

    // One second before the window expires the mutation is rejected.
    te.env.ledger().set_timestamp(T0 + COOLDOWN - 1);
    let res = te.client.try_remove_accepted_token(&te.admin, &extra);
    assert_eq!(res, Err(Ok(Error::CooldownActive)));

    // The rejected call must not remove the token and must not re-arm the
    // cooldown timestamp.
    assert!(is_accepted(&te, &extra));
    assert_eq!(token_decimals(&te, &extra), Some(7));
    assert!(list_contains(&te, &extra));
    assert_eq!(accepted_list(&te).len(), 2);

    // Exactly at the original deadline the removal succeeds. If the rejected
    // call had written `T0 + COOLDOWN - 1` as the new baseline, this call would
    // still be inside the window and return CooldownActive.
    te.env.ledger().set_timestamp(T0 + COOLDOWN);
    te.client.remove_accepted_token(&te.admin, &extra);
    assert!(!is_accepted(&te, &extra));
    assert!(!list_contains(&te, &extra));
    assert_eq!(accepted_list(&te).len(), 1);
    assert!(is_accepted(&te, &te.token));
}

// ── 6. Unknown token: idempotent no-op success ───────────────────────────────

#[test]
fn remove_unknown_token_is_idempotent_noop() {
    let te = setup();
    let never_accepted = Address::generate(&te.env);

    // A token that was never accepted: removal succeeds as a no-op.
    te.client.remove_accepted_token(&te.admin, &never_accepted);
    assert!(!is_accepted(&te, &never_accepted));
    assert!(!list_contains(&te, &never_accepted));
    assert_eq!(accepted_list(&te).len(), 1);

    // A previously removed token behaves identically on a second removal.
    let extra = Address::generate(&te.env);
    te.jump(COOLDOWN);
    te.client.add_accepted_token(&te.admin, &extra, &7);
    te.jump(COOLDOWN);
    te.client.remove_accepted_token(&te.admin, &extra);
    assert!(!is_accepted(&te, &extra));
    assert!(!list_contains(&te, &extra));

    te.jump(COOLDOWN);
    te.client.remove_accepted_token(&te.admin, &extra);
    assert!(!is_accepted(&te, &extra));
    assert!(!list_contains(&te, &extra));

    // Throughout, the default token and the list are untouched.
    assert!(is_accepted(&te, &te.token));
    assert_eq!(token_decimals(&te, &te.token), Some(6));
    let list = accepted_list(&te);
    assert_eq!(list.len(), 1);
    assert_eq!(list.get(0).unwrap().token, te.token);
}

// ── 7. Boundary: orphan decimals entry without a list entry ──────────────────

#[test]
fn remove_accepted_token_clears_orphan_decimals_entry() {
    let te = setup();
    let orphan = Address::generate(&te.env);

    // Split state: a decimals entry exists but the token is absent from the
    // accepted-token list. The public API cannot produce this state, so seed it
    // directly.
    seed_decimals_only(&te, &orphan, &11);
    assert!(is_accepted(&te, &orphan));
    assert_eq!(token_decimals(&te, &orphan), Some(11));
    assert!(!list_contains(&te, &orphan));
    assert_eq!(accepted_list(&te).len(), 1);

    // `remove_accepted_token` still succeeds: it clears the decimals entry and
    // rewrites the list (which does not contain `orphan`) unchanged.
    te.client.remove_accepted_token(&te.admin, &orphan);
    assert!(!is_accepted(&te, &orphan));
    assert_eq!(token_decimals(&te, &orphan), None);

    let list = accepted_list(&te);
    assert_eq!(list.len(), 1);
    assert_eq!(list.get(0).unwrap().token, te.token);
    assert_eq!(list.get(0).unwrap().decimals, 6);
    assert!(is_accepted(&te, &te.token));
    assert_eq!(token_decimals(&te, &te.token), Some(6));
}
