//! Tests for per-merchant protocol fee override (feat/merchant-fee-override).
//!
//! # Coverage
//! - `test_set_override` — admin can set a valid override; it is readable back.
//! - `test_override_applied_on_charge` — charge respects override instead of global fee.
//! - `test_clear_override_falls_back_to_global` — after clearing, global fee applies.
//! - `test_override_greater_than_global_rejected` — override > global is rejected.
//! - `test_zero_override_routes_no_fee` — zero override means no fee collected.
//! - `test_override_non_admin_rejected` — non-admin cannot set an override.
//! - `test_clear_idempotent` — clearing a non-existent override succeeds.
//! - `test_usage_charge_respects_override` — usage charge also uses override.

#[cfg(test)]
mod tests {
    use crate::test_utils::setup::TestEnv;
    use crate::types::DataKey;
    use soroban_sdk::{testutils::Address as _, Address, String};

    /// Global fee used throughout tests: 500 bps = 5 %.
    const GLOBAL_FEE_BPS: u32 = 500;
    /// Override fee: 200 bps = 2 %.
    const OVERRIDE_FEE_BPS: u32 = 200;
    /// Charge amount: 100 USDC in base units (6 decimals).
    const CHARGE_AMOUNT: i128 = 100_000_000;
    /// Ample prepaid balance.
    const PREPAID: i128 = 1_000_000_000;
    /// 30-day billing interval in seconds.
    const INTERVAL: u64 = 30 * 24 * 60 * 60;

    // ── helpers ───────────────────────────────────────────────────────────────

    /// Bootstrap: contract + global fee + treasury.
    fn setup() -> (TestEnv, Address) {
        let t = TestEnv::default();
        let treasury = Address::generate(&t.env);
        t.client
            .set_protocol_fee(&t.admin, &treasury, &GLOBAL_FEE_BPS);
        (t, treasury)
    }

    /// Create a merchant and initialize its config.
    fn make_merchant(t: &TestEnv) -> Address {
        let merchant = Address::generate(&t.env);
        t.client.initialize_merchant_config(
            &merchant,
            &merchant,
            &0i32,
            &0x1Fi32,
            &None,
            &String::from_str(&t.env, "https://example.com"),
        );
        merchant
    }

    /// Create an active subscription for `merchant` with ample prepaid balance,
    /// seeded directly into storage so no token contract is needed for the deposit.
    fn make_funded_subscription(t: &TestEnv, merchant: &Address) -> u32 {
        let subscriber = Address::generate(&t.env);
        let id = t.client.create_subscription(
            &subscriber,
            merchant,
            &CHARGE_AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
        );
        // Seed balance directly — avoids needing a live token contract.
        let mut sub = t.client.get_subscription(&id);
        sub.prepaid_balance = PREPAID;
        t.env.as_contract(&t.client.address, || {
            t.env.storage().persistent().set(&DataKey::Sub(id), &sub);
        });
        id
    }

    /// Advance ledger past one billing interval.
    fn advance_interval(t: &TestEnv) {
        t.env.ledger().with_mut(|l| l.timestamp += INTERVAL + 1);
    }

    // ── tests ─────────────────────────────────────────────────────────────────

    /// Admin can set a fee override and read it back.
    #[test]
    fn test_set_override() {
        let (t, _treasury) = setup();
        let merchant = make_merchant(&t);

        t.client
            .set_merchant_fee_override(&t.admin, &merchant, &OVERRIDE_FEE_BPS);

        let stored = t.client.get_merchant_fee_override(&merchant);
        assert_eq!(stored, Some(OVERRIDE_FEE_BPS));
    }

    /// A charge to a merchant with an override uses the override fee bps, not the global.
    #[test]
    fn test_override_applied_on_charge() {
        let (t, _treasury) = setup();
        let merchant = make_merchant(&t);
        let id = make_funded_subscription(&t, &merchant);

        // Set override to 0 so we can precisely verify no fee is taken.
        // (See test_zero_override_routes_no_fee for zero specifically;
        //  here we use OVERRIDE_FEE_BPS and check the merchant balance delta.)
        t.client
            .set_merchant_fee_override(&t.admin, &merchant, &OVERRIDE_FEE_BPS);

        advance_interval(&t);
        t.client
            .charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);

        let merchant_balance =
            t.client
                .get_merchant_balance_by_token(&merchant, &t.token);

        // Expected net = CHARGE_AMOUNT * (10_000 - OVERRIDE_FEE_BPS) / 10_000
        let expected_net =
            CHARGE_AMOUNT * (10_000 - OVERRIDE_FEE_BPS as i128) / 10_000;
        assert_eq!(
            merchant_balance, expected_net,
            "merchant should receive net after override fee, got {merchant_balance}, expected {expected_net}"
        );
    }

    /// After clearing an override the global fee applies again.
    #[test]
    fn test_clear_override_falls_back_to_global() {
        let (t, _treasury) = setup();
        let merchant = make_merchant(&t);

        // Set then clear the override.
        t.client
            .set_merchant_fee_override(&t.admin, &merchant, &OVERRIDE_FEE_BPS);
        t.client
            .clear_merchant_fee_override(&t.admin, &merchant);

        // Override should no longer be stored.
        assert_eq!(t.client.get_merchant_fee_override(&merchant), None);

        // Charge should use global fee.
        let id = make_funded_subscription(&t, &merchant);
        advance_interval(&t);
        t.client
            .charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);

        let merchant_balance =
            t.client
                .get_merchant_balance_by_token(&merchant, &t.token);

        let expected_net =
            CHARGE_AMOUNT * (10_000 - GLOBAL_FEE_BPS as i128) / 10_000;
        assert_eq!(
            merchant_balance, expected_net,
            "after clearing override, merchant should receive global-fee net: got {merchant_balance}, expected {expected_net}"
        );
    }

    /// An override greater than the global fee must be rejected with InvalidFeeBips.
    #[test]
    fn test_override_greater_than_global_rejected() {
        let (t, _treasury) = setup();
        let merchant = make_merchant(&t);

        // GLOBAL_FEE_BPS = 500; try to set override to 501.
        let result = t
            .client
            .try_set_merchant_fee_override(&t.admin, &merchant, &(GLOBAL_FEE_BPS + 1));

        assert!(
            result.is_err(),
            "override > global fee should be rejected"
        );
    }

    /// An override equal to MAX_FEE_BIPS (10_000) when global = 10_000 is accepted;
    /// values above MAX_FEE_BIPS are always rejected regardless of global.
    #[test]
    fn test_override_above_max_fee_bips_rejected() {
        let t = TestEnv::default();
        let treasury = Address::generate(&t.env);
        // Set global fee to max so the only way to trip the MAX check is to go over it.
        t.client.set_protocol_fee(&t.admin, &treasury, &10_000);
        let merchant = make_merchant(&t);

        // 10_001 should always fail.
        let result =
            t.client
                .try_set_merchant_fee_override(&t.admin, &merchant, &10_001u32);
        assert!(result.is_err(), "fee_bps > MAX_FEE_BIPS must be rejected");
    }

    /// A zero override means no fee is collected — the merchant receives the full charge.
    #[test]
    fn test_zero_override_routes_no_fee() {
        let (t, _treasury) = setup();
        let merchant = make_merchant(&t);
        let id = make_funded_subscription(&t, &merchant);

        t.client
            .set_merchant_fee_override(&t.admin, &merchant, &0u32);

        advance_interval(&t);
        t.client
            .charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);

        let merchant_balance =
            t.client
                .get_merchant_balance_by_token(&merchant, &t.token);

        // Zero fee: merchant should receive the full charge amount.
        assert_eq!(
            merchant_balance, CHARGE_AMOUNT,
            "zero override: merchant must receive full charge amount"
        );
    }

    /// Non-admin cannot set an override.
    #[test]
    fn test_override_non_admin_rejected() {
        let (t, _treasury) = setup();
        let merchant = make_merchant(&t);
        let stranger = Address::generate(&t.env);

        let result = t
            .client
            .try_set_merchant_fee_override(&stranger, &merchant, &OVERRIDE_FEE_BPS);

        assert!(
            result.is_err(),
            "non-admin must not be able to set a fee override"
        );
    }

    /// Non-admin cannot clear an override.
    #[test]
    fn test_clear_override_non_admin_rejected() {
        let (t, _treasury) = setup();
        let merchant = make_merchant(&t);
        t.client
            .set_merchant_fee_override(&t.admin, &merchant, &OVERRIDE_FEE_BPS);

        let stranger = Address::generate(&t.env);
        let result = t
            .client
            .try_clear_merchant_fee_override(&stranger, &merchant);

        assert!(result.is_err(), "non-admin must not clear a fee override");
    }

    /// Clearing a non-existent override is idempotent (no error).
    #[test]
    fn test_clear_idempotent() {
        let (t, _treasury) = setup();
        let merchant = make_merchant(&t);

        // No override set yet — clearing should succeed silently.
        t.client.clear_merchant_fee_override(&t.admin, &merchant);
        assert_eq!(t.client.get_merchant_fee_override(&merchant), None);
    }

    /// Usage charges also respect the per-merchant override.
    #[test]
    fn test_usage_charge_respects_override() {
        let (t, _treasury) = setup();
        let merchant = make_merchant(&t);

        // Create a usage-enabled subscription.
        let subscriber = Address::generate(&t.env);
        let id = t.client.create_subscription(
            &subscriber,
            &merchant,
            &CHARGE_AMOUNT,
            &INTERVAL,
            &true, // usage_enabled
            &None::<i128>,
            &None::<u64>,
        );

        // Configure minimal usage limits (no rate/burst/cap limits).
        t.client.configure_usage_limits(
            &merchant,
            &id,
            &None::<u32>,
            &0u64,
            &0u64,
            &None::<i128>,
        );

        // Seed balance.
        let mut sub = t.client.get_subscription(&id);
        sub.prepaid_balance = PREPAID;
        t.env.as_contract(&t.client.address, || {
            t.env.storage().persistent().set(&DataKey::Sub(id), &sub);
        });

        // Set the override.
        t.client
            .set_merchant_fee_override(&t.admin, &merchant, &OVERRIDE_FEE_BPS);

        let usage_amount: i128 = 50_000_000; // 50 USDC
        t.client.charge_usage_with_reference(
            &id,
            &usage_amount,
            &String::from_str(&t.env, "ref-001"),
        );

        let merchant_balance =
            t.client
                .get_merchant_balance_by_token(&merchant, &t.token);

        let expected_net =
            usage_amount * (10_000 - OVERRIDE_FEE_BPS as i128) / 10_000;
        assert_eq!(
            merchant_balance, expected_net,
            "usage charge: merchant net should reflect override fee"
        );
    }
}
