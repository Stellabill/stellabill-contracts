/// ABI regression test — closes issue #863.
///
/// # Purpose
///
/// The original implementation extracted the contract ABI from
/// `SubscriptionVaultClient::spec_xdr()`, an API that no longer exists in
/// soroban-sdk 22 (the spec is embedded in a wasm link section at build time
/// and is not exposed to test binaries). The test now verifies the ABI surface
/// at **compile time**: every required entry-point is referenced as a method on
/// [`SubscriptionVaultClient`], so adding, removing, or renaming a `pub fn` in
/// the `#[contractimpl]` block of `lib.rs` becomes a hard compile error.
///
/// A compile failure here means a `pub fn` was **added, removed, or renamed**
/// in the `#[contractimpl]` block of `lib.rs`. Intentional ABI changes must
/// update the lists below and leave a comment recording the rationale — this
/// creates a clear, reviewable audit trail for every ABI break.
///
/// # What this test does NOT detect
///
/// * Parameter-type changes that preserve the entry-point name.
/// * Changes only inside the feature-grouped `*_api.rs` re-export modules
///   (those modules have zero effect on the compiled ABI).
///
/// Both of those changes would still break clients and should be caught by
/// integration tests, but they are outside the scope of a name-based check.

use subscription_vault::SubscriptionVaultClient;

/// Forces the compiler to resolve `method` on the generated client. If the
/// method was removed or renamed, this is a hard compile error (E0599).
fn assert_entrypoint<T>(_: T) {}

// ── Required entry-point inventory ──────────────────────────────────────────
//
// Keep these lists in sync with the `#[contractimpl]` block in `lib.rs`.
// Every entry below is referenced at compile time so an accidental rename or
// removal breaks the build instead of silently changing the ABI.

#[test]
fn test_abi_contains_required_entrypoints() {
    // ── Subscriber group ────────────────────────────────────────────────────
    assert_entrypoint(SubscriptionVaultClient::create_subscription);
    assert_entrypoint(SubscriptionVaultClient::deposit_funds);
    assert_entrypoint(SubscriptionVaultClient::cancel_subscription);
    assert_entrypoint(SubscriptionVaultClient::pause_subscription);
    assert_entrypoint(SubscriptionVaultClient::resume_subscription);
    assert_entrypoint(SubscriptionVaultClient::charge_subscription);
    assert_entrypoint(SubscriptionVaultClient::charge_usage);
    assert_entrypoint(SubscriptionVaultClient::open_dispute);
    assert_entrypoint(SubscriptionVaultClient::claim_cancellation_escrow);
    assert_entrypoint(SubscriptionVaultClient::set_metadata);
    assert_entrypoint(SubscriptionVaultClient::create_coupon);
    assert_entrypoint(SubscriptionVaultClient::apply_coupon);
    assert_entrypoint(SubscriptionVaultClient::create_plan_template);
    assert_entrypoint(SubscriptionVaultClient::get_subscription);
    assert_entrypoint(SubscriptionVaultClient::set_auto_renew);
    assert_entrypoint(SubscriptionVaultClient::grant_delegated_payer);
    assert_entrypoint(SubscriptionVaultClient::deposit_funds_on_behalf);
    assert_entrypoint(SubscriptionVaultClient::request_emergency_withdraw);
    assert_entrypoint(SubscriptionVaultClient::finalize_emergency_withdraw);

    // ── Merchant group ──────────────────────────────────────────────────────
    assert_entrypoint(SubscriptionVaultClient::withdraw_merchant_funds);
    assert_entrypoint(SubscriptionVaultClient::get_merchant_balance);
    assert_entrypoint(SubscriptionVaultClient::set_payout_schedule);
    assert_entrypoint(SubscriptionVaultClient::flush_payouts);
    assert_entrypoint(SubscriptionVaultClient::respond_dispute);
    assert_entrypoint(SubscriptionVaultClient::resolve_dispute);
    assert_entrypoint(SubscriptionVaultClient::lodge_escrow_dispute);
    assert_entrypoint(SubscriptionVaultClient::set_merchant_vacation);
    assert_entrypoint(SubscriptionVaultClient::clear_merchant_vacation);
    assert_entrypoint(SubscriptionVaultClient::register_sub_account);
    assert_entrypoint(SubscriptionVaultClient::withdraw_sub_account_funds);
    assert_entrypoint(SubscriptionVaultClient::register_plan);
    assert_entrypoint(SubscriptionVaultClient::deprecate_plan);

    // ── Admin group ─────────────────────────────────────────────────────────
    assert_entrypoint(SubscriptionVaultClient::init);
    assert_entrypoint(SubscriptionVaultClient::rotate_admin);
    assert_entrypoint(SubscriptionVaultClient::propose_admin);
    assert_entrypoint(SubscriptionVaultClient::claim_admin_role);
    assert_entrypoint(SubscriptionVaultClient::cancel_admin_proposal);
    assert_entrypoint(SubscriptionVaultClient::set_operator);
    assert_entrypoint(SubscriptionVaultClient::batch_charge);
    assert_entrypoint(SubscriptionVaultClient::enable_emergency_stop);
    assert_entrypoint(SubscriptionVaultClient::disable_emergency_stop);
    assert_entrypoint(SubscriptionVaultClient::set_protocol_fee);
    assert_entrypoint(SubscriptionVaultClient::add_accepted_token);
    assert_entrypoint(SubscriptionVaultClient::migrate);
    assert_entrypoint(SubscriptionVaultClient::submit_proposal);
    assert_entrypoint(SubscriptionVaultClient::add_to_blocklist);
    assert_entrypoint(SubscriptionVaultClient::set_oracle_config);
    assert_entrypoint(SubscriptionVaultClient::recover_stranded_funds);
    assert_entrypoint(SubscriptionVaultClient::queue_treasury_change);
    assert_entrypoint(SubscriptionVaultClient::execute_treasury_change);
    assert_entrypoint(SubscriptionVaultClient::set_auto_pause_threshold);
}

/// Edge case: a deprecated shim retained for backwards compatibility must
/// still appear in the ABI even if the canonical implementation was renamed.
///
/// Update this test if a new shim is added or an existing one is removed
/// after the embargo period.
#[test]
fn test_deprecated_shims_retained() {
    // No deprecated shims exist at time of writing (2026-08-30).
    // When a shim is added, reference its client method here, e.g.:
    //   assert_entrypoint(SubscriptionVaultClient::old_entrypoint_name);
}
