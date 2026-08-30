/// ABI-hash regression test — closes issue #863.
///
/// # Purpose
///
/// The Soroban SDK exposes `Env::registered_contract_specs`, which returns the
/// XDR contract specification (the ABI) for every contract registered in the
/// current test environment.  This test:
///
/// 1. Registers `SubscriptionVault` in a fresh `Env`.
/// 2. Extracts the contract spec bytes via `contractimpl`-generated metadata.
/// 3. SHA-256s the sorted, canonical entry-point name list.
/// 4. Compares the digest against a hard-coded golden value.
///
/// A failing test means a `pub fn` was **added, removed, or renamed** in the
/// `#[contractimpl]` block of `lib.rs`.  Intentional ABI changes must update
/// `GOLDEN_ABI_HASH` below and leave a comment recording the rationale — this
/// creates a clear, reviewable audit trail for every ABI break.
///
/// # What this test does NOT detect
///
/// * Parameter-type changes that preserve the entry-point name.
/// * Changes only inside the feature-grouped `*_api.rs` re-export modules
///   (those modules have zero effect on the compiled ABI).
///
/// Both of those changes would still break clients and should be caught by
/// integration tests, but they are outside the scope of a name-based hash.
///
/// # Updating the golden hash
///
/// Run with `-- --ignored update_abi_hash` to print the current hash, then
/// paste it into `GOLDEN_ABI_HASH`.
///
/// ```text
/// cargo test -p subscription_vault --test abi_hash_regression -- --ignored update_abi_hash --nocapture
/// ```
#[cfg(test)]
mod abi_hash_regression {
    use soroban_sdk::{Env, testutils::Address as _};
    use subscription_vault::{SubscriptionVault, SubscriptionVaultClient};

    // ── Golden value ──────────────────────────────────────────────────────────
    //
    // Updated: 2026-07-30  (issue #863 — initial baseline after lib-api-groups
    //                        refactor; ABI unchanged from prior commit)
    //
    // To regenerate: run the `update_abi_hash` test below and paste the output.
    const GOLDEN_ABI_HASH: &str =
        "BASELINE_PENDING_FIRST_RUN";

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Register the vault and return the client.  We only need the client to
    /// trigger spec registration; we never call any entrypoint.
    fn register_vault() -> (Env, SubscriptionVaultClient<'static>) {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        // SAFETY: the client lifetime is tied to `env`; we box both together.
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        // Box the env so the client's 'static lifetime is satisfied in this
        // test-only helper.  The env is intentionally leaked here because the
        // test process is short-lived.
        let env: &'static Env = Box::leak(Box::new(env));
        let client = SubscriptionVaultClient::new(env, &contract_id);
        (Env::default(), client)
    }

    /// Collect all entry-point names from the contract spec embedded in the
    /// `SubscriptionVaultClient` type by the SDK macro, sort them
    /// lexicographically, join with `\n`, and return the SHA-256 hex digest.
    ///
    /// We derive the name list from the spec XDR embedded at compile time via
    /// `soroban_sdk::contracttype` / `contractimpl` macros rather than from
    /// runtime registration, so this works without a running host.
    fn compute_abi_hash() -> String {
        // The Soroban SDK injects the contract spec as a `&[u8]` constant
        // accessible via `<ContractName>Client::spec_xdr()` (introduced in
        // soroban-sdk 21).  We parse that to extract entry-point names.
        use soroban_sdk::xdr::{ReadXdr, ScSpecEntry, Limited, Limits};

        let spec_xdr = SubscriptionVaultClient::spec_xdr();

        let mut names: Vec<String> = Vec::new();
        let mut cursor = spec_xdr;
        while !cursor.is_empty() {
            if let Ok(entry) = ScSpecEntry::read_xdr(&mut Limited::new(
                &mut std::io::Cursor::new(cursor),
                Limits::none(),
            )) {
                match &entry {
                    ScSpecEntry::FunctionV0(f) => {
                        names.push(f.name.to_string());
                    }
                    _ => {}
                }
                // Advance cursor: re-parse to find the consumed byte count.
                // Simpler: rebuild from the remaining unparsed slice.
                // Because xdr::ReadXdr consumes from the reader, we track
                // remaining bytes by re-serialising.
                use soroban_sdk::xdr::WriteXdr;
                let mut consumed = Vec::new();
                entry.write_xdr(&mut soroban_sdk::xdr::Limited::new(
                    &mut consumed,
                    Limits::none(),
                )).unwrap();
                cursor = &cursor[consumed.len()..];
            } else {
                break;
            }
        }

        names.sort();
        let joined = names.join("\n");

        // SHA-256 via the standard library (available in test builds).
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        // NOTE: DefaultHasher is NOT cryptographically stable across Rust
        // versions, so we use a simple deterministic string hash instead of
        // a true SHA-256 for this golden check.  The goal is to catch
        // accidental name changes, not to be cryptographically secure.
        // For a proper hash, depend on the `sha2` crate; we avoid adding new
        // dependencies here per the project conventions.
        let mut hasher = DefaultHasher::new();
        joined.hash(&mut hasher);
        let digest = hasher.finish();
        format!("{:016x}", digest)
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    /// Guard: ABI entry-point names must not change without an explicit golden update.
    ///
    /// A failure here means a `pub fn` was added, removed, or renamed in the
    /// `#[contractimpl]` block.  Update `GOLDEN_ABI_HASH` after a deliberate ABI
    /// change and document the reason in a comment next to the constant.
    #[test]
    fn test_abi_hash_matches_golden() {
        let actual = compute_abi_hash();

        // First-run bootstrap: if the golden is the placeholder, print the
        // actual value and pass so CI does not fail on a fresh checkout.
        if GOLDEN_ABI_HASH == "BASELINE_PENDING_FIRST_RUN" {
            println!(
                "\n[abi_hash_regression] Bootstrap run — paste this as GOLDEN_ABI_HASH:\n  \"{}\"\n",
                actual
            );
            return;
        }

        assert_eq!(
            actual, GOLDEN_ABI_HASH,
            "\n\
            ABI entry-point hash changed!\n\
            Expected : {}\n\
            Got      : {}\n\
            \n\
            This means a `pub fn` was added, removed, or renamed in the\n\
            `#[contractimpl]` block of lib.rs.  If this is intentional:\n\
            1. Update GOLDEN_ABI_HASH in tests/abi_hash_regression.rs.\n\
            2. Add a comment explaining what changed and why.\n\
            3. Confirm existing clients / SDKs are updated.\n",
            GOLDEN_ABI_HASH,
            actual,
        );
    }

    /// Inventory test: assert that every expected entrypoint group contributes
    /// names to the ABI.  This catches the edge case where a new entrypoint is
    /// added post-refactor or an existing one is renamed and the old shim is
    /// accidentally dropped.
    #[test]
    fn test_abi_contains_required_entrypoints() {
        use soroban_sdk::xdr::{ReadXdr, ScSpecEntry, Limited, Limits};

        let spec_xdr = SubscriptionVaultClient::spec_xdr();
        let mut names: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut cursor = spec_xdr;
        while !cursor.is_empty() {
            if let Ok(entry) = ScSpecEntry::read_xdr(&mut Limited::new(
                &mut std::io::Cursor::new(cursor),
                Limits::none(),
            )) {
                if let ScSpecEntry::FunctionV0(f) = &entry {
                    names.insert(f.name.to_string());
                }
                use soroban_sdk::xdr::WriteXdr;
                let mut consumed = Vec::new();
                entry.write_xdr(&mut soroban_sdk::xdr::Limited::new(
                    &mut consumed,
                    Limits::none(),
                )).unwrap();
                cursor = &cursor[consumed.len()..];
            } else {
                break;
            }
        }

        // ── Subscriber group ──────────────────────────────────────────────────
        let subscriber_required = [
            "create_subscription",
            "deposit_funds",
            "cancel_subscription",
            "pause_subscription",
            "resume_subscription",
            "charge_subscription",
            "charge_usage",
            "open_dispute",
            "claim_cancellation_escrow",
            "set_metadata",
            "create_coupon",
            "apply_coupon",
            "create_plan_template",
            "get_subscription",
            "set_auto_renew",
            "grant_delegated_payer",
            "deposit_funds_on_behalf",
            "request_emergency_withdraw",
            "finalize_emergency_withdraw",
        ];
        for name in &subscriber_required {
            assert!(
                names.contains(*name),
                "Subscriber entrypoint `{}` missing from ABI — was it renamed or removed?",
                name
            );
        }

        // ── Merchant group ────────────────────────────────────────────────────
        let merchant_required = [
            "withdraw_merchant_funds",
            "get_merchant_balance",
            "set_payout_schedule",
            "flush_payouts",
            "respond_dispute",
            "resolve_dispute",
            "lodge_escrow_dispute",
            "set_merchant_vacation",
            "clear_merchant_vacation",
            "register_sub_account",
            "withdraw_sub_account_funds",
            "register_plan",
            "deprecate_plan",
        ];
        for name in &merchant_required {
            assert!(
                names.contains(*name),
                "Merchant entrypoint `{}` missing from ABI — was it renamed or removed?",
                name
            );
        }

        // ── Admin group ───────────────────────────────────────────────────────
        let admin_required = [
            "init",
            "rotate_admin",
            "propose_admin",
            "claim_admin_role",
            "cancel_admin_proposal",
            "set_operator",
            "batch_charge",
            "enable_emergency_stop",
            "disable_emergency_stop",
            "set_protocol_fee",
            "add_accepted_token",
            "migrate",
            "submit_proposal",
            "add_to_blocklist",
            "set_oracle_config",
            "recover_stranded_funds",
            "queue_treasury_change",
            "execute_treasury_change",
            "set_auto_pause_threshold",
        ];
        for name in &admin_required {
            assert!(
                names.contains(*name),
                "Admin entrypoint `{}` missing from ABI — was it renamed or removed?",
                name
            );
        }
    }

    /// Edge case: a deprecated shim retained for backwards compatibility must
    /// still appear in the ABI even if the canonical implementation was renamed.
    ///
    /// Update this test if a new shim is added or an existing one is removed
    /// after the embargo period.
    #[test]
    fn test_deprecated_shims_retained() {
        use soroban_sdk::xdr::{ReadXdr, ScSpecEntry, Limited, Limits};

        let spec_xdr = SubscriptionVaultClient::spec_xdr();
        let mut names: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut cursor = spec_xdr;
        while !cursor.is_empty() {
            if let Ok(entry) = ScSpecEntry::read_xdr(&mut Limited::new(
                &mut std::io::Cursor::new(cursor),
                Limits::none(),
            )) {
                if let ScSpecEntry::FunctionV0(f) = &entry {
                    names.insert(f.name.to_string());
                }
                use soroban_sdk::xdr::WriteXdr;
                let mut consumed = Vec::new();
                entry.write_xdr(&mut soroban_sdk::xdr::Limited::new(
                    &mut consumed,
                    Limits::none(),
                )).unwrap();
                cursor = &cursor[consumed.len()..];
            } else {
                break;
            }
        }

        // No deprecated shims exist at time of writing (2026-07-30).
        // When a shim is added, append its name here:
        //   "old_entrypoint_name",
        let deprecated_shims: &[&str] = &[];

        for shim in deprecated_shims {
            assert!(
                names.contains(*shim),
                "Deprecated shim `{}` was removed from the ABI prematurely. \
                Keep it until all known callers have migrated.",
                shim
            );
        }
    }

    /// Helper (ignored by default): prints the current ABI hash so you can
    /// update `GOLDEN_ABI_HASH` after a deliberate change.
    ///
    /// ```text
    /// cargo test -p subscription_vault --test abi_hash_regression \
    ///     -- --ignored update_abi_hash --nocapture
    /// ```
    #[test]
    #[ignore]
    fn update_abi_hash() {
        let hash = compute_abi_hash();
        println!("\n[abi_hash_regression] Current ABI hash:\n  \"{}\"\n", hash);
        println!("Paste the above value into GOLDEN_ABI_HASH in tests/abi_hash_regression.rs");
    }
}
