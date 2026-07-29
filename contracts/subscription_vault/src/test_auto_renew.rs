//! Tests for issue #562: auto-renewal disable flag with explicit renewal window.
//!
//! ## Feature summary
//!
//! * `Subscription.auto_renew` defaults to `true` on creation.
//! * `set_auto_renew(false)` halts billing at the next interval boundary.
//! * A *renewal window* (one full `interval_seconds` after `auto_renew` was
//!   disabled) allows the subscriber/merchant to re-enable without re-creating.
//! * After the window closes, re-enabling returns `Error::RenewalWindowClosed`.
//! * Emits `AutoRenewToggledEvent` on every toggle.
//! * Only the subscriber or merchant may call `set_auto_renew`.
//!
//! ## Coverage checklist
//!
//! | # | Scenario | Function |
//! |---|----------|----------|
//! | 1 | Default flag is `true` on creation | `test_auto_renew_default_is_true` |
//! | 2 | Subscriber can disable | `test_subscriber_can_disable_auto_renew` |
//! | 3 | Merchant can disable | `test_merchant_can_disable_auto_renew` |
//! | 4 | Third-party cannot disable (Forbidden) | `test_third_party_cannot_set_auto_renew` |
//! | 5 | Charge skipped when disabled and interval elapsed | `test_charge_skipped_when_auto_renew_disabled` |
//! | 6 | Charge proceeds normally when enabled | `test_charge_proceeds_when_auto_renew_enabled` |
//! | 7 | Re-enable within window succeeds | `test_reenable_within_window_succeeds` |
//! | 8 | Re-enable after window closed returns RenewalWindowClosed | `test_reenable_after_window_closed_fails` |
//! | 9 | Toggle mid-interval: flag set before interval elapsed | `test_toggle_mid_interval` |
//! | 10 | Toggle then cancel | `test_toggle_then_cancel` |
//! | 11 | Renewal after long dormancy | `test_renewal_after_long_dormancy` |
//! | 12 | Idempotent disable: second disable preserves original timestamp | `test_double_disable_preserves_timestamp` |
//! | 13 | Idempotent enable: enable when already enabled is no-op | `test_double_enable_is_noop` |
//! | 14 | Cancelled subscription rejects toggle | `test_cancelled_subscription_rejects_toggle` |
//! | 15 | Expired subscription rejects toggle | `test_expired_subscription_rejects_toggle` |
//! | 16 | NotFound when subscription doesn't exist | `test_nonexistent_subscription_returns_not_found` |
//! | 17 | AutoRenewToggledEvent emitted on disable | `test_auto_renew_event_emitted_on_disable` |
//! | 18 | AutoRenewToggledEvent emitted on re-enable | `test_auto_renew_event_emitted_on_reenable` |
//! | 19 | Batch charge skips non-renewing subscription | `test_batch_charge_skips_non_renewing` |
//! | 20 | Paused subscription can still toggle auto_renew | `test_paused_subscription_can_toggle` |

#[cfg(test)]
mod test_auto_renew {
    use crate::{
        ChargeExecutionResult, DataKey, Error, SubscriptionStatus, SubscriptionVault,
        SubscriptionVaultClient,
    };
    use soroban_sdk::{
        testutils::{Address as _, Events as _, Ledger as _},
        Address, Env, Vec,
    };

    // ── Constants ────────────────────────────────────────────────────────────

    const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days in seconds
    const AMOUNT: i128 = 10_000_000; // 10 USDC (6 decimals)
    const PREPAID: i128 = 120_000_000; // 120 USDC — covers many intervals
    const T0: u64 = 1_000_000; // arbitrary start timestamp

    // ── Test helpers ─────────────────────────────────────────────────────────

    /// Initialise a standard test environment: mock all auths, register the
    /// contract, init with a real SAC token and a 7-day grace period.
    fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();

        // Advance ledger to a known starting timestamp so relative arithmetic is
        // deterministic and independent of wall-clock drift.
        env.ledger().with_mut(|l| {
            l.timestamp = T0;
            l.sequence_number = 1;
        });

        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        // min_topup = 1 USDC, grace_period = 7 days
        client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

        (env, client, token, admin)
    }

    /// Create a subscription and fund it with `PREPAID` balance directly in
    /// storage so tests can charge without going through the token transfer path.
    fn create_funded_subscription(
        env: &Env,
        client: &SubscriptionVaultClient,
    ) -> (u32, Address, Address) {
        let subscriber = Address::generate(env);
        let merchant = Address::generate(env);
        let id = client.create_subscription(
            &subscriber,
            &merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
            &None::<Address>,
        );

        // Seed balance directly so we do not need a live token transfer.
        let mut sub = client.get_subscription(&id);
        sub.prepaid_balance = PREPAID;
        env.as_contract(&client.address, || {
            env.storage().persistent().set(&DataKey::Sub(id), &sub);
        });

        // Also seed a matching merchant balance for withdrawal tests.
        env.as_contract(&client.address, || {
            env.storage().instance().set(
                &DataKey::MerchantBalance(merchant.clone(), sub.token.clone()),
                &0i128,
            );
        });

        (id, subscriber, merchant)
    }

    /// Advance the ledger timestamp by `delta` seconds.
    fn advance(env: &Env, delta: u64) {
        env.ledger().with_mut(|l| {
            l.timestamp += delta;
        });
    }

    // ── Test 1: default auto_renew = true ────────────────────────────────────

    #[test]
    fn test_auto_renew_default_is_true() {
        let (env, client, _token, _admin) = setup();
        let (id, _subscriber, _merchant) = create_funded_subscription(&env, &client);

        let sub = client.get_subscription(&id);
        assert!(
            sub.auto_renew,
            "auto_renew must default to true on creation"
        );
        assert!(
            sub.auto_renew_disabled_at.is_none(),
            "auto_renew_disabled_at must be None on creation"
        );
    }

    // ── Test 2: subscriber can disable ───────────────────────────────────────

    #[test]
    fn test_subscriber_can_disable_auto_renew() {
        let (env, client, _token, _admin) = setup();
        let (id, subscriber, _merchant) = create_funded_subscription(&env, &client);

        let result = client.try_set_auto_renew(&id, &subscriber, &false);
        assert!(result.is_ok(), "subscriber should be allowed to disable auto_renew");

        let sub = client.get_subscription(&id);
        assert!(!sub.auto_renew, "auto_renew must be false after subscriber disables");
        assert!(
            sub.auto_renew_disabled_at.is_some(),
            "auto_renew_disabled_at must be set after disabling"
        );
        assert_eq!(
            sub.auto_renew_disabled_at.unwrap(),
            T0,
            "disabled_at should equal ledger timestamp at time of disable"
        );
    }

    // ── Test 3: merchant can disable ─────────────────────────────────────────

    #[test]
    fn test_merchant_can_disable_auto_renew() {
        let (env, client, _token, _admin) = setup();
        let (id, _subscriber, merchant) = create_funded_subscription(&env, &client);

        let result = client.try_set_auto_renew(&id, &merchant, &false);
        assert!(result.is_ok(), "merchant should be allowed to disable auto_renew");

        let sub = client.get_subscription(&id);
        assert!(!sub.auto_renew, "auto_renew must be false after merchant disables");
    }

    // ── Test 4: third party cannot toggle ────────────────────────────────────

    #[test]
    fn test_third_party_cannot_set_auto_renew() {
        let (env, client, _token, _admin) = setup();
        let (id, _subscriber, _merchant) = create_funded_subscription(&env, &client);

        let attacker = Address::generate(&env);
        let result = client.try_set_auto_renew(&id, &attacker, &false);

        assert_eq!(
            result,
            Err(Ok(Error::Forbidden)),
            "third party must receive Forbidden"
        );
    }

    // ── Test 5: charge skipped when disabled and interval elapsed ────────────

    #[test]
    fn test_charge_skipped_when_auto_renew_disabled() {
        let (env, client, _token, _admin) = setup();
        let (id, subscriber, _merchant) = create_funded_subscription(&env, &client);

        // Disable auto-renewal.
        client.set_auto_renew(&id, &subscriber, &false);

        // Advance past one full interval.
        advance(&env, INTERVAL + 1);

        // Charge should return Skipped, not an error.
        let result = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
        assert_eq!(
            result,
            Ok(Ok(ChargeExecutionResult::Skipped)),
            "charge should be skipped when auto_renew=false and interval has elapsed"
        );

        // Balance must be unchanged.
        let sub = client.get_subscription(&id);
        assert_eq!(
            sub.prepaid_balance, PREPAID,
            "prepaid balance must not change on a skipped charge"
        );
    }

    // ── Test 6: charge proceeds when enabled ─────────────────────────────────

    #[test]
    fn test_charge_proceeds_when_auto_renew_enabled() {
        let (env, client, _token, _admin) = setup();
        let (id, _subscriber, _merchant) = create_funded_subscription(&env, &client);

        // auto_renew defaults to true; just advance and charge.
        advance(&env, INTERVAL + 1);

        let result = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
        // Should succeed (Ok(Ok(Charged)) or similar non-Skipped result).
        assert!(
            result.is_ok(),
            "charge must not error when auto_renew=true and interval elapsed"
        );
        let charge_result = result.unwrap().unwrap();
        assert_ne!(
            charge_result,
            ChargeExecutionResult::Skipped,
            "charge result must not be Skipped when auto_renew=true"
        );

        let sub = client.get_subscription(&id);
        assert!(
            sub.prepaid_balance < PREPAID,
            "balance must decrease after a successful charge"
        );
    }

    // ── Test 7: re-enable within window succeeds ──────────────────────────────

    #[test]
    fn test_reenable_within_window_succeeds() {
        let (env, client, _token, _admin) = setup();
        let (id, subscriber, _merchant) = create_funded_subscription(&env, &client);

        // Disable auto-renewal at T0.
        client.set_auto_renew(&id, &subscriber, &false);

        // Advance to just before the window closes (window = 1 interval).
        advance(&env, INTERVAL - 1);

        // Re-enable: should succeed.
        let result = client.try_set_auto_renew(&id, &subscriber, &true);
        assert!(result.is_ok(), "re-enable within renewal window must succeed");

        let sub = client.get_subscription(&id);
        assert!(sub.auto_renew, "auto_renew must be true after re-enable");
        assert!(
            sub.auto_renew_disabled_at.is_none(),
            "auto_renew_disabled_at must be cleared after re-enable"
        );
    }

    // ── Test 8: re-enable after window closed returns RenewalWindowClosed ────

    #[test]
    fn test_reenable_after_window_closed_fails() {
        let (env, client, _token, _admin) = setup();
        let (id, subscriber, _merchant) = create_funded_subscription(&env, &client);

        // Disable auto-renewal at T0.
        client.set_auto_renew(&id, &subscriber, &false);

        // Advance past the full window (disabled_at + interval).
        advance(&env, INTERVAL + 1);

        // Re-enable: must fail with RenewalWindowClosed.
        let result = client.try_set_auto_renew(&id, &subscriber, &true);
        assert_eq!(
            result,
            Err(Ok(Error::RenewalWindowClosed)),
            "re-enable after window must return RenewalWindowClosed"
        );

        // Subscription must still have auto_renew=false.
        let sub = client.get_subscription(&id);
        assert!(
            !sub.auto_renew,
            "auto_renew must remain false after failed re-enable"
        );
    }

    // ── Test 9: toggle mid-interval ───────────────────────────────────────────

    #[test]
    fn test_toggle_mid_interval() {
        let (env, client, _token, _admin) = setup();
        let (id, subscriber, _merchant) = create_funded_subscription(&env, &client);

        // Advance halfway through the interval, then disable.
        advance(&env, INTERVAL / 2);
        client.set_auto_renew(&id, &subscriber, &false);

        // Interval hasn't elapsed yet — charge should return IntervalNotElapsed
        // (not Skipped, since the interval guard fires before the auto_renew skip).
        let result = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
        assert_eq!(
            result,
            Err(Ok(Error::IntervalNotElapsed)),
            "charge before interval elapsed must return IntervalNotElapsed even with auto_renew=false"
        );

        // Advance past the full interval (total = T0 + INTERVAL + 1).
        advance(&env, INTERVAL / 2 + 1);

        // Now charge should be skipped.
        let result2 = client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
        assert_eq!(
            result2,
            Ok(Ok(ChargeExecutionResult::Skipped)),
            "charge after interval elapsed with auto_renew=false must be Skipped"
        );
    }

    // ── Test 10: toggle then cancel ───────────────────────────────────────────

    #[test]
    fn test_toggle_then_cancel() {
        let (env, client, _token, _admin) = setup();
        let (id, subscriber, _merchant) = create_funded_subscription(&env, &client);

        // Disable auto-renewal.
        client.set_auto_renew(&id, &subscriber, &false);

        // Cancel — should succeed regardless of auto_renew state.
        let result = client.try_cancel_subscription(&id, &subscriber);
        assert!(result.is_ok(), "cancel after disabling auto_renew must succeed");

        let sub = client.get_subscription(&id);
        assert_eq!(
            sub.status,
            SubscriptionStatus::Cancelled,
            "subscription must be cancelled"
        );

        // Attempting to toggle on a cancelled subscription must fail.
        let toggle_on_cancelled = client.try_set_auto_renew(&id, &subscriber, &true);
        assert_eq!(
            toggle_on_cancelled,
            Err(Ok(Error::InvalidStatusTransition)),
            "toggle on cancelled subscription must return InvalidStatusTransition"
        );
    }

    // ── Test 11: renewal after long dormancy ──────────────────────────────────

    #[test]
    fn test_renewal_after_long_dormancy() {
        let (env, client, _token, _admin) = setup();
        let (id, subscriber, _merchant) = create_funded_subscription(&env, &client);

        // Disable auto-renewal.
        client.set_auto_renew(&id, &subscriber, &false);

        // Advance many intervals (simulate months of inactivity).
        advance(&env, INTERVAL * 12);

        // Re-enable must fail — window has long since closed.
        let result = client.try_set_auto_renew(&id, &subscriber, &true);
        assert_eq!(
            result,
            Err(Ok(Error::RenewalWindowClosed)),
            "re-enable after long dormancy must return RenewalWindowClosed"
        );

        // Charge must still be skipped — billing halted correctly.
        let charge_result =
            client.try_charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
        assert_eq!(
            charge_result,
            Ok(Ok(ChargeExecutionResult::Skipped)),
            "charge during long dormancy with auto_renew=false must be Skipped"
        );
    }

    // ── Test 12: double disable preserves original timestamp ─────────────────

    #[test]
    fn test_double_disable_preserves_timestamp() {
        let (env, client, _token, _admin) = setup();
        let (id, subscriber, _merchant) = create_funded_subscription(&env, &client);

        // First disable at T0.
        client.set_auto_renew(&id, &subscriber, &false);
        let first_disabled_at = client.get_subscription(&id).auto_renew_disabled_at;

        // Advance a bit and disable again.
        advance(&env, 1000);
        client.set_auto_renew(&id, &subscriber, &false);
        let second_disabled_at = client.get_subscription(&id).auto_renew_disabled_at;

        assert_eq!(
            first_disabled_at, second_disabled_at,
            "second disable must preserve the original disable timestamp, not overwrite it"
        );
    }

    // ── Test 13: double enable is no-op ──────────────────────────────────────

    #[test]
    fn test_double_enable_is_noop() {
        let (env, client, _token, _admin) = setup();
        let (id, subscriber, _merchant) = create_funded_subscription(&env, &client);

        // Already enabled by default; enabling again is idempotent.
        let result = client.try_set_auto_renew(&id, &subscriber, &true);
        assert!(result.is_ok(), "enabling when already enabled must succeed (no-op)");

        let sub = client.get_subscription(&id);
        assert!(sub.auto_renew, "auto_renew must remain true");
        assert!(
            sub.auto_renew_disabled_at.is_none(),
            "disabled_at must remain None"
        );
    }

    // ── Test 14: cancelled subscription rejects toggle ────────────────────────

    #[test]
    fn test_cancelled_subscription_rejects_toggle() {
        let (env, client, _token, _admin) = setup();
        let (id, subscriber, _merchant) = create_funded_subscription(&env, &client);

        client.cancel_subscription(&id, &subscriber);

        let result = client.try_set_auto_renew(&id, &subscriber, &false);
        assert_eq!(
            result,
            Err(Ok(Error::InvalidStatusTransition)),
            "toggle on cancelled subscription must return InvalidStatusTransition"
        );
    }

    // ── Test 15: expired subscription rejects toggle ──────────────────────────

    #[test]
    fn test_expired_subscription_rejects_toggle() {
        let (env, client, _token, _admin) = setup();
        let (id, subscriber, _merchant) = {
            let subscriber = Address::generate(&env);
            let merchant = Address::generate(&env);
            // expires_at = T0 + 1000 seconds
            let expires_at: u64 = T0 + 1000;
            let id = client.create_subscription(
                &subscriber,
                &merchant,
                &AMOUNT,
                &INTERVAL,
                &false,
                &None::<i128>,
                &Some(expires_at),
                &None::<Address>,
            );
            let mut sub = client.get_subscription(&id);
            sub.prepaid_balance = PREPAID;
            env.as_contract(&client.address, || {
                env.storage().persistent().set(&DataKey::Sub(id), &sub);
            });
            (id, subscriber, merchant)
        };

        // Advance past expiration.
        advance(&env, 1001);

        let result = client.try_set_auto_renew(&id, &subscriber, &false);
        assert_eq!(
            result,
            Err(Ok(Error::SubscriptionExpired)),
            "toggle on expired subscription must return SubscriptionExpired"
        );
    }

    // ── Test 16: non-existent subscription returns NotFound ───────────────────

    #[test]
    fn test_nonexistent_subscription_returns_not_found() {
        let (env, client, _token, _admin) = setup();
        let caller = Address::generate(&env);

        let result = client.try_set_auto_renew(&9999, &caller, &false);
        assert_eq!(
            result,
            Err(Ok(Error::NotFound)),
            "non-existent subscription must return NotFound"
        );
    }

    // ── Test 17: AutoRenewToggledEvent emitted on disable ────────────────────

    #[test]
    fn test_auto_renew_event_emitted_on_disable() {
        let (env, client, _token, _admin) = setup();
        let (id, subscriber, merchant) = create_funded_subscription(&env, &client);

        // Snapshot event count before the call.
        let events_before = env.events().all().len();

        client.set_auto_renew(&id, &subscriber, &false);

        // At least one event must have been published.
        let events_after = env.events().all().len();
        assert!(
            events_after > events_before,
            "at least one event must be emitted when auto_renew is disabled"
        );

        // Subscription state must reflect the change.
        let sub = client.get_subscription(&id);
        assert!(!sub.auto_renew);
        assert_eq!(sub.subscriber, subscriber);
        assert_eq!(sub.merchant, merchant);
    }

    // ── Test 18: AutoRenewToggledEvent emitted on re-enable ───────────────────

    #[test]
    fn test_auto_renew_event_emitted_on_reenable() {
        let (env, client, _token, _admin) = setup();
        let (id, subscriber, _merchant) = create_funded_subscription(&env, &client);

        client.set_auto_renew(&id, &subscriber, &false);
        advance(&env, INTERVAL / 2); // within window

        let events_before = env.events().all().len();
        client.set_auto_renew(&id, &subscriber, &true);
        let events_after = env.events().all().len();

        assert!(
            events_after > events_before,
            "re-enable must also emit an event"
        );

        let sub = client.get_subscription(&id);
        assert!(sub.auto_renew, "auto_renew must be true after re-enable");
        assert!(
            sub.auto_renew_disabled_at.is_none(),
            "disabled_at must be cleared after re-enable"
        );
    }

    // ── Test 19: batch charge skips non-renewing subscription ─────────────────

    #[test]
    fn test_batch_charge_skips_non_renewing() {
        let (env, client, _token, admin) = setup();
        let (id1, subscriber1, _merchant1) = create_funded_subscription(&env, &client);
        let (id2, _subscriber2, _merchant2) = create_funded_subscription(&env, &client);

        // Disable auto-renewal on id1 only.
        client.set_auto_renew(&id1, &subscriber1, &false);

        // Advance past one interval.
        advance(&env, INTERVAL + 1);

        // Run batch charge; get the admin nonce first.
        let nonce = client.get_admin_nonce(&admin, &0u32);
        let mut ids = Vec::<u32>::new(&env);
        ids.push_back(id1);
        ids.push_back(id2);

        let results = client.batch_charge(&ids, &nonce);
        assert_eq!(results.len(), 2, "batch must return one result per id");

        // id1 must be reported as success=true (skipped without error).
        let r1 = results.get(0).unwrap();
        assert!(
            r1.success,
            "batch charge of non-renewing subscription must report success (skipped gracefully)"
        );
        assert_eq!(
            r1.error_code, 0,
            "batch charge of non-renewing subscription must have no error code"
        );

        // id2 must have been charged (success, error_code=0).
        let r2 = results.get(1).unwrap();
        assert!(r2.success, "batch charge of active subscription must succeed");
        assert_eq!(r2.error_code, 0);

        // id1 balance unchanged, id2 balance decreased.
        assert_eq!(
            client.get_subscription(&id1).prepaid_balance,
            PREPAID,
            "non-renewing subscription balance must be unchanged"
        );
        assert!(
            client.get_subscription(&id2).prepaid_balance < PREPAID,
            "active subscription balance must decrease after charge"
        );
    }

    // ── Test 20: paused subscription can toggle auto_renew ───────────────────

    #[test]
    fn test_paused_subscription_can_toggle() {
        let (env, client, _token, _admin) = setup();
        let (id, subscriber, _merchant) = create_funded_subscription(&env, &client);

        // Pause the subscription.
        client.pause_subscription(&id, &subscriber);
        let sub = client.get_subscription(&id);
        assert_eq!(sub.status, SubscriptionStatus::Paused);

        // Disabling auto_renew on a paused subscription must succeed.
        let result = client.try_set_auto_renew(&id, &subscriber, &false);
        assert!(
            result.is_ok(),
            "disabling auto_renew on a paused subscription must succeed"
        );

        let sub = client.get_subscription(&id);
        assert!(!sub.auto_renew, "auto_renew must be false");

        // Re-enable within window.
        advance(&env, INTERVAL / 2);
        let result2 = client.try_set_auto_renew(&id, &subscriber, &true);
        assert!(
            result2.is_ok(),
            "re-enabling within window on a paused subscription must succeed"
        );
        let sub2 = client.get_subscription(&id);
        assert!(sub2.auto_renew, "auto_renew must be true after re-enable");
    }
}
