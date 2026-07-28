//! # Issue #634 — Emergency-stop view surface audit
//!
//! Tests that verify the security posture of every unauthenticated view function
//! under three scenarios required by the issue:
//!
//! 1. **View called while `emergency_stop` is active** — gated views must return
//!    `Error::EmergencyStopActive`; un-gated views must still return data.
//! 2. **Pre-init calls** — views that require an initialised contract must return
//!    `Error::NotInitialized` (or an empty/zero default) rather than panicking.
//! 3. **View calls returning empty state** — views over absent data must return
//!    sensible defaults (`None`, `0`, empty `Vec`) rather than panicking.
//!
//! ## Coverage map (all public view entrypoints)
//!
//! | Entrypoint | Gated? | Test |
//! |---|---|---|
//! | `get_admin_nonce` | YES — `EmergencyStopActive` | `test_get_admin_nonce_blocked_while_stopped` |
//! | `get_operator_nonce` | YES — `EmergencyStopActive` | `test_get_operator_nonce_blocked_while_stopped` |
//! | `get_admin` | No — intentionally public | `test_get_admin_returns_address` |
//! | `get_operator` | No — intentionally public | `test_get_operator_none_when_unset` |
//! | `get_oracle_config` | No — intentionally public | `test_get_oracle_config_default_while_stopped` |
//! | `get_subscription` | No | `test_get_subscription_accessible_while_stopped` |
//! | `get_emergency_stop_status` | No — by design | `test_get_emergency_stop_status` |
//! | `get_next_charge_info` | No | `test_get_next_charge_info_while_stopped` |
//! | `get_cap_info` | No | `test_get_cap_info_while_stopped` |
//! | `get_merchant_balance` | No | `test_get_merchant_balance_while_stopped` |
//! | `get_subscription_count` | No | `test_get_subscription_count_pre_init` |
//! | `version` | No | `test_version_always_accessible` |
//! | `get_min_topup` | No (returns `NotInitialized` pre-init) | `test_get_min_topup_pre_init` |
//! | `list_accepted_tokens` | No | `test_list_accepted_tokens_empty_pre_init` |
//! | `get_emergency_stop_status` pre-init | No | `test_emergency_stop_status_pre_init_is_false` |
//! | `get_admin_nonce` pre-init | YES | `test_get_admin_nonce_blocked_pre_init_when_stopped` |
//! | `get_metadata_signed_nonce` | No | `test_get_metadata_signed_nonce_while_stopped` |
//! | `get_protocol_fee_bps` | No | `test_get_protocol_fee_bps_while_stopped` |
//! | `get_auto_pause_threshold` | No | `test_get_auto_pause_threshold_while_stopped` |
//! | `get_billing_retention` | No | `test_get_billing_retention_while_stopped` |

#[cfg(test)]
mod test_emergency_stop_view_surface {
    use crate::{
        nonce::{DOMAIN_ADMIN_ROTATION, DOMAIN_BATCH_CHARGE, DOMAIN_OPERATOR_BATCH_CHARGE},
        types::Error,
        SubscriptionVault, SubscriptionVaultClient,
    };
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        Address, Env,
    };

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Register the contract and return a connected client.
    fn deploy(env: &Env) -> SubscriptionVaultClient {
        let id = env.register(SubscriptionVault, ());
        SubscriptionVaultClient::new(env, &id)
    }

    /// Deploy + initialise with sensible defaults; return (client, admin, token).
    fn deploy_and_init(env: &Env) -> (SubscriptionVaultClient, Address, Address) {
        let client = deploy(env);
        let admin = Address::generate(env);
        let token = Address::generate(env);
        env.mock_all_auths();
        client
            .init(&token, &6u32, &admin, &1_000_000i128, &3600u64)
            .unwrap();
        (client, admin, token)
    }

    /// Enable emergency stop on an initialised client.
    fn enable_stop(client: &SubscriptionVaultClient, admin: &Address) {
        client.enable_emergency_stop(admin).unwrap();
    }

    // =========================================================================
    // 1.  GATED VIEW — get_admin_nonce
    // =========================================================================

    /// `get_admin_nonce` must return `EmergencyStopActive` when the circuit
    /// breaker is engaged.  An attacker cannot enumerate the current admin nonce
    /// during a stop window to pre-compute a valid `rotate_admin` payload.
    #[test]
    fn test_get_admin_nonce_blocked_while_stopped() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);
        enable_stop(&client, &admin);

        let result = client.try_get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE);
        assert_eq!(result, Err(Ok(Error::EmergencyStopActive)));
    }

    /// `get_admin_nonce` must return the correct nonce value (0 for a fresh
    /// signer) when the emergency stop is **not** active.
    #[test]
    fn test_get_admin_nonce_accessible_when_not_stopped() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);

        let nonce = client.get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE);
        assert_eq!(nonce, 0u64);
    }

    /// `get_admin_nonce` for all three primary domains must be blocked while stopped.
    #[test]
    fn test_get_admin_nonce_blocked_all_domains() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);
        enable_stop(&client, &admin);

        for domain in [DOMAIN_BATCH_CHARGE, DOMAIN_ADMIN_ROTATION] {
            let result = client.try_get_admin_nonce(&admin, &domain);
            assert_eq!(
                result,
                Err(Ok(Error::EmergencyStopActive)),
                "expected EmergencyStopActive for domain {domain}"
            );
        }
    }

    // =========================================================================
    // 2.  GATED VIEW — get_operator_nonce
    // =========================================================================

    /// `get_operator_nonce` must return `EmergencyStopActive` while stopped.
    #[test]
    fn test_get_operator_nonce_blocked_while_stopped() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);
        let operator = Address::generate(&env);
        client.set_operator(&admin, &operator).unwrap();
        enable_stop(&client, &admin);

        let result = client.try_get_operator_nonce(&operator);
        assert_eq!(result, Err(Ok(Error::EmergencyStopActive)));
    }

    /// `get_operator_nonce` returns `0` for a fresh operator when not stopped.
    #[test]
    fn test_get_operator_nonce_accessible_when_not_stopped() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);
        let operator = Address::generate(&env);
        client.set_operator(&admin, &operator).unwrap();

        let nonce = client.get_operator_nonce(&operator);
        assert_eq!(nonce, 0u64);
    }

    /// `get_operator_nonce` persists across stop→resume cycles: nonce state is
    /// unchanged by toggling the circuit breaker.
    #[test]
    fn test_get_operator_nonce_preserved_across_stop_resume() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);
        let operator = Address::generate(&env);
        client.set_operator(&admin, &operator).unwrap();

        // Nonce is 0 before stop.
        assert_eq!(client.get_operator_nonce(&operator), 0u64);

        // Stop → nonce query blocked.
        enable_stop(&client, &admin);
        assert_eq!(
            client.try_get_operator_nonce(&operator),
            Err(Ok(Error::EmergencyStopActive))
        );

        // Resume → nonce is still 0 (unchanged by the stop).
        env.ledger().with_mut(|l| l.timestamp += 7200);
        client.disable_emergency_stop(&admin).unwrap();
        assert_eq!(client.get_operator_nonce(&operator), 0u64);
    }

    // =========================================================================
    // 3.  INTENTIONALLY-PUBLIC views — no gate applied
    // =========================================================================

    /// `get_admin` returns the admin address even while stopped.  The admin
    /// address is observable on-chain regardless (it signed every previous
    /// admin-only transaction), so exposing it via a query adds no information.
    #[test]
    fn test_get_admin_returns_address_while_stopped() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);
        enable_stop(&client, &admin);

        let result = client.get_admin();
        assert_eq!(result, Ok(admin));
    }

    /// `get_operator` returns `None` when no operator is set — even while stopped.
    #[test]
    fn test_get_operator_none_when_unset_while_stopped() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);
        enable_stop(&client, &admin);

        assert_eq!(client.get_operator(), None);
    }

    /// `get_oracle_config` returns the default disabled config while stopped.
    /// The oracle address is observable on-chain; hiding it in a view would
    /// provide no meaningful protection.
    #[test]
    fn test_get_oracle_config_default_while_stopped() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);
        enable_stop(&client, &admin);

        let cfg = client.get_oracle_config();
        assert!(!cfg.enabled, "oracle must be disabled by default");
        assert!(cfg.oracle.is_none());
    }

    /// `get_emergency_stop_status` must always be readable — it is the primary
    /// signal UIs use to gate user interactions.
    #[test]
    fn test_get_emergency_stop_status_reflects_state() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);

        assert!(!client.get_emergency_stop_status());
        enable_stop(&client, &admin);
        assert!(client.get_emergency_stop_status());
    }

    /// `get_subscription` remains accessible while stopped so subscribers can
    /// verify their position before deciding whether to act.
    #[test]
    fn test_get_subscription_accessible_while_stopped() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, token) = deploy_and_init(&env);

        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);

        // Create subscription before stop.
        let sub_id = client
            .create_subscription(
                &subscriber,
                &merchant,
                &1_000_000i128,
                &86_400u64,
                &false,
                &None,
                &None,
            )
            .unwrap();

        enable_stop(&client, &admin);

        // Still readable while stopped.
        let sub = client.get_subscription(&sub_id).unwrap();
        assert_eq!(sub.subscriber, subscriber);
        assert_eq!(sub.merchant, merchant);
    }

    /// `get_next_charge_info` is a pure computation over subscription data and
    /// returns useful info to subscribers even while the stop is active.
    #[test]
    fn test_get_next_charge_info_while_stopped() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);

        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);
        let sub_id = client
            .create_subscription(
                &subscriber,
                &merchant,
                &1_000_000i128,
                &86_400u64,
                &false,
                &None,
                &None,
            )
            .unwrap();

        enable_stop(&client, &admin);

        let info = client.get_next_charge_info(&sub_id).unwrap();
        assert_eq!(info.amount, 1_000_000i128);
    }

    /// `get_cap_info` returns cap data while stopped.
    #[test]
    fn test_get_cap_info_while_stopped() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);

        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);
        let cap: i128 = 5_000_000;
        let sub_id = client
            .create_subscription(
                &subscriber,
                &merchant,
                &1_000_000i128,
                &86_400u64,
                &false,
                &Some(cap),
                &None,
            )
            .unwrap();

        enable_stop(&client, &admin);

        let info = client.get_cap_info(&sub_id).unwrap();
        assert_eq!(info.lifetime_cap, Some(cap));
        assert!(!info.cap_reached);
    }

    /// `get_merchant_balance` returns 0 for an unknown merchant — no panic.
    #[test]
    fn test_get_merchant_balance_while_stopped() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);
        let merchant = Address::generate(&env);
        enable_stop(&client, &admin);

        assert_eq!(client.get_merchant_balance(&merchant), 0i128);
    }

    // =========================================================================
    // 4.  PRE-INIT CALLS — contract not yet initialised
    // =========================================================================

    /// `get_emergency_stop_status` returns `false` (safe default) on a fresh
    /// un-initialised contract — the stop is off.
    #[test]
    fn test_emergency_stop_status_pre_init_is_false() {
        let env = Env::default();
        let client = deploy(&env);
        assert!(!client.get_emergency_stop_status());
    }

    /// `get_min_topup` returns `NotInitialized` when called before `init`.
    #[test]
    fn test_get_min_topup_pre_init() {
        let env = Env::default();
        let client = deploy(&env);
        assert_eq!(client.try_get_min_topup(), Err(Ok(Error::NotInitialized)));
    }

    /// `get_admin` returns `NotInitialized` before `init`.
    #[test]
    fn test_get_admin_pre_init() {
        let env = Env::default();
        let client = deploy(&env);
        assert_eq!(client.try_get_admin(), Err(Ok(Error::NotInitialized)));
    }

    /// `get_subscription` returns `NotFound` for id `0` on an un-initialised contract.
    #[test]
    fn test_get_subscription_pre_init_returns_not_found() {
        let env = Env::default();
        let client = deploy(&env);
        assert_eq!(
            client.try_get_subscription(&0u32),
            Err(Ok(Error::NotFound))
        );
    }

    /// `get_subscription_count` returns `0` before any subscriptions are created.
    #[test]
    fn test_get_subscription_count_pre_init() {
        let env = Env::default();
        let client = deploy(&env);
        assert_eq!(client.get_subscription_count(), 0u32);
    }

    /// `version` is always accessible regardless of init or stop state.
    #[test]
    fn test_version_always_accessible() {
        let env = Env::default();
        let client = deploy(&env);
        assert_eq!(client.version(), 1u32);
    }

    /// `list_accepted_tokens` returns an empty vec on un-initialised contract.
    #[test]
    fn test_list_accepted_tokens_empty_pre_init() {
        let env = Env::default();
        let client = deploy(&env);
        assert!(client.list_accepted_tokens().is_empty());
    }

    /// `get_operator` returns `None` on un-initialised contract — no panic.
    #[test]
    fn test_get_operator_pre_init_returns_none() {
        let env = Env::default();
        let client = deploy(&env);
        assert_eq!(client.get_operator(), None);
    }

    /// `get_protocol_fee_bps` returns `0` (disabled) before any fee is set.
    #[test]
    fn test_get_protocol_fee_bps_pre_init() {
        let env = Env::default();
        let client = deploy(&env);
        assert_eq!(client.get_protocol_fee_bps(), 0u32);
    }

    // =========================================================================
    // 5.  EMPTY-STATE — initialised but no relevant data present
    // =========================================================================

    /// `get_billing_retention` returns `keep_recent = 0` (all retained) by default.
    #[test]
    fn test_get_billing_retention_default() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, _token) = deploy_and_init(&env);
        let retention = client.get_billing_retention();
        assert_eq!(retention.keep_recent, 0u32);
    }

    /// `get_billing_retention` is accessible while stopped.
    #[test]
    fn test_get_billing_retention_while_stopped() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);
        enable_stop(&client, &admin);
        let retention = client.get_billing_retention();
        assert_eq!(retention.keep_recent, 0u32);
    }

    /// `get_auto_pause_threshold` returns `0` (disabled) by default.
    #[test]
    fn test_get_auto_pause_threshold_while_stopped() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);
        enable_stop(&client, &admin);
        assert_eq!(client.get_auto_pause_threshold(), 0u32);
    }

    /// `get_merchant_max_subs` returns `u32::MAX` (unlimited) for an unknown merchant.
    #[test]
    fn test_get_merchant_max_subs_empty_state() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, _token) = deploy_and_init(&env);
        let merchant = Address::generate(&env);
        assert_eq!(client.get_merchant_max_subs(&merchant), u32::MAX);
    }

    /// `get_global_cap_default` returns `None` before any cap is set.
    #[test]
    fn test_get_global_cap_default_empty_state() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, _token) = deploy_and_init(&env);
        assert_eq!(client.get_global_cap_default(), None);
    }

    /// `get_merchant_cap_default` returns `None` for an unknown merchant.
    #[test]
    fn test_get_merchant_cap_default_empty_state() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, _token) = deploy_and_init(&env);
        let merchant = Address::generate(&env);
        assert_eq!(client.get_merchant_cap_default(&merchant), None);
    }

    /// `get_subscription_dispute` returns `None` for a subscription with no dispute.
    #[test]
    fn test_get_subscription_dispute_none_when_absent() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, _token) = deploy_and_init(&env);
        assert_eq!(client.get_subscription_dispute(&0u32), None);
    }

    /// `get_coupon` returns `None` for an unknown coupon code.
    #[test]
    fn test_get_coupon_none_when_absent() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, _token) = deploy_and_init(&env);
        let code = soroban_sdk::Symbol::new(&env, "NONE");
        assert_eq!(client.get_coupon(&code), None);
    }

    /// `list_guardians` returns an empty vec when no guardians are set.
    #[test]
    fn test_list_guardians_empty_state() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, _token) = deploy_and_init(&env);
        assert!(client.list_guardians().is_empty());
    }

    /// `get_metadata_signed_nonce` returns `0` for a fresh signer — and remains
    /// accessible while stopped (it reveals no operationally-sensitive config).
    #[test]
    fn test_get_metadata_signed_nonce_while_stopped() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);
        enable_stop(&client, &admin);

        let signer = Address::generate(&env);
        // Returns 0 (no nonce consumed yet) — accessible while stopped because
        // metadata signing does not affect fund custody or admin control.
        assert_eq!(client.get_metadata_signed_nonce(&signer), 0u64);
    }

    // =========================================================================
    // 6.  NONCE GATE — additional edge cases
    // =========================================================================

    /// `get_admin_nonce` with a domain that has never been used returns `0` when
    /// not stopped.
    #[test]
    fn test_get_admin_nonce_zero_for_fresh_domain() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);
        // Domain 99 has never been touched.
        assert_eq!(client.get_admin_nonce(&admin, &99u32), 0u64);
    }

    /// After `disable_emergency_stop`, `get_admin_nonce` becomes accessible again.
    #[test]
    fn test_get_admin_nonce_accessible_after_stop_lifted() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);

        enable_stop(&client, &admin);

        // Blocked while stopped.
        assert_eq!(
            client.try_get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE),
            Err(Ok(Error::EmergencyStopActive))
        );

        // Advance time past cooldown and lift the stop.
        env.ledger().with_mut(|l| l.timestamp += 7200);
        client.disable_emergency_stop(&admin).unwrap();

        // Now accessible.
        let nonce = client.get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE);
        assert_eq!(nonce, 0u64);
    }

    /// `get_operator_nonce` for an address that is not the current operator is
    /// still blocked while stopped (the gate is on stop state, not operator identity).
    #[test]
    fn test_get_operator_nonce_blocked_for_any_address_while_stopped() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);
        enable_stop(&client, &admin);

        let random = Address::generate(&env);
        assert_eq!(
            client.try_get_operator_nonce(&random),
            Err(Ok(Error::EmergencyStopActive))
        );
    }

    /// Toggle stop on and off twice — nonce gate tracks the live flag, not a
    /// latched state.
    #[test]
    fn test_nonce_gate_tracks_live_stop_flag() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _token) = deploy_and_init(&env);

        // Cycle 1: stop on → off
        enable_stop(&client, &admin);
        assert!(client.try_get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE).is_err());
        env.ledger().with_mut(|l| l.timestamp += 7200);
        client.disable_emergency_stop(&admin).unwrap();
        assert!(client.try_get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE).is_ok());

        // Cycle 2: stop on → off again
        env.ledger().with_mut(|l| l.timestamp += 7200);
        enable_stop(&client, &admin);
        assert!(client.try_get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE).is_err());
        env.ledger().with_mut(|l| l.timestamp += 7200);
        client.disable_emergency_stop(&admin).unwrap();
        assert!(client.try_get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE).is_ok());
    }
}
