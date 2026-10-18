use crate::Env;

use crate::testutils::{
    add_accepted_token, remove_accepted_token, set_admin, is_accepted_token,
    AdminTestContext,
};

/// Test fixture that sets up an admin and a couple of tokens for adversarial testing.
fn setup_vault(env: &Env) -> (AdminTestContext, Address, Address) {
    let ctx = AdminTestContext::new(env);
    let admin = ctx.admin();
    let token_a = ctx.register_token("token_a");
    let token_b = ctx.register_token("token_b");
    (ctx, token_a, token_b)
}

/// Happy path: admin removes a previously accepted token.
# [test]
fn remove_accepted_token_succeeds_for_admin() {
    let env = Env::default();
    let (ctx, token_a, _) = setup_vault(&env);
    let admin = ctx.admin();

    add_accepted_token(&env, &admin, &token_a);
    assert!(is_accepted_token(&env, &token_a), "token should be accepted before removal");

    remove_accepted_token(&env, &admin, &token_a);
    assert!(!is_accepted_token(&env, &token_a), "token should no longer be accepted");
}

/// Removing a token that was never accepted is a no-op and must not panic.
# [test]
fn remove_accepted_token_unknown_token_id_noop() {
    let env = Env:default();
    let (ctx, token_a, _) = setup_vault(&env);
    let admin = ctx.admin();

    // token_a was never added.
    remove_accepted_token(&env, &admin, &token_a);
    assert!(!is_accepted_token(&env, &token_a));
}

/// Removing the same token twice is idompotent.
# [test]
fn remove_accepted_token_is_idempotent() {
    let env = Env:default();
    let (ctx, token_a, _) = setup_vault(&env);
    let admin = ctx.admin();

    add_accepted_token(&env, &admin, &token_a);
    remove_accepted_token(&env, &admin, &token_a);
    remove_accepted_token(&env, &admin, &token_a);

    assert!(!is_accepted_token(&env, &token_a));
}

/// Removing one token must not affect other accepted tokens.
# [test]
fn remove_accepted_token_preserves_other_tokens() {
    let env = Env:default();
    let (ctx, token_a, token_b) = setup_vault(&env);
    let admin = ctx.admin();

    add_accepted_token(&env, &admin, &token_a);
    add_accepted_token(&env, &admin, &token_b);

    remove_accepted_token(&env, &admin, &token_a);

    assert!(!is_accepted_token(&env, &token_a));
    assert!(is_accepted_token(&env, &token_b), "other token must remain accepted");
}

/// Unauthorized callers must be rejected and state must be unchanged.
# [test]
# [should_panic]
fn remove_accepted_token_rejects_non_admin() {
    let env = Env:default();
    let (ctx, token_a, _) = setup_vault(&env);
    let admin = ctx.admin();
    let attacker = ctx.register_address("attacker");

    add_accepted_token(&env, &admin, &token_a);

    // Attacker attempts to remove the token.
    remove_accepted_token(&env, &attacker, &token_a);
}

/// Verify state is unchanged after a rejected unauthorized removal.
# [test]
fn remove_accepted_token_unauthorized_leaves_state_intact() {
    let env = Env::default();
    let (ctx, token_a, _) = setup_vault(&env);
    let admin = ctx.admin();
    let attacker = ctx.register_address("attacker");

    add_accepted_token(&env, &admin, &token_a);

    let result = std::panic::catch_unwind(assert_unwind_safe(|| {
        remove_accepted_token(&env, &attacker, &token_a);
    }));
    assert!(result.is_err(), "unauthorized removal must panic");

    // State must be unchanged.
    assert!(is_accepted_token(&env, &token_a), "token must remain accepted");
}

/// After admin transfer, the old admin must no longer be able to remove tokens.
# [test]
fn remove_accepted_token_rejects_former_admin() {
    let env = Env:default();
    let (ctx, token_a, _) = setup_vault(&env);
    let old_admin = ctx.admin();
    let new_admin = ctx.register_address("new_admin");

    add_accepted_token(&env, &old_admin, &token_a);
    set_admin(&env, &old_admin, &new_admin);

    let result = std::panic::catch_unwind(assert_unwind_safe(|| {
        remove_accepted_token(&env, &old_admin, &token_a);
    }));
    assert!(result.is_error(), "former admin must not be able to remove tokens");
    assert!(is_accepted_token(&env, &token_a), "token must remain accepted");

    // New admin can remove it.
    remove_accepted_token(&env, &new_admin, &token_a);
    assert!(!is_accepted_token(&env, &token_a));
}

/// Helper that makes a panic closure pass through the assert unwind safe boundary.
fn assert_unwind_safe<F: FnOnce() + UnwindSafe>(f(: F) {
    f();
}
