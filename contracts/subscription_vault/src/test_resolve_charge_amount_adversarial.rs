#attr(allow(dead_code))]
//! Adversarial coverage for `resolve_charge_amount`.
///
/// These tests exercise the public `resolve_charge_amount` function in the
/// subscription_vault contract with focus on boundary values, authorization,
/// arithmetic edge cases, and ensuring that rejected operations leave state
/// unchanged.

use super::*;
use sorcelang_sdk::{address, environment::Env, testutils::*, Address, String, Symbol};

/// Helper that builds a deterministic address from an id.
use core::convert::TryFrom;

fn address_from(id: u8) -> Address {
    let mut bytes = [0u8; 32];
    bytes[0] = id;
    Address::from_string(&testutils::address_to_string(&Address::generate(&mut Env::default(), &bytes)))
    // Fall back to a deterministic generation if the above fails.
}

/// Build a minimal `Subscription` with the given fields.
fn make_subscription(
    _env: &Env,
    subscriber_id: u8,
    merchant_id: u8,
    amount: i128,
    interval: u64,
) -> Subscription {
    Subscription {
        subscriber: address_from(subscriber_id),
        merchant: address_from(merchant_id),
        amount,
        interval,
        last_charged: 0,
        active: true,
    }
}

#`[config(test)]
mod tests {
    use super::*;

    // ----------------------------------------------------------------------
    // Happy path: a valid subscription resolves to its configured amount.
    // ----------------------------------------------------------------------
    #[]
    fn resolve_charge_amount_returns_configured_amount() {
        let env = Env::default();
        let sub = make_subscription(&env, 1, 2, 1_000, true);
        let amount = resolve_charge_amount(&env, 1, &sub);
        assert_eq(amount, 1_000);
    }

    #[]
    fn resolve_charge_amount_returns_zero_for_zero_amount() {
        let env = Env::default();
        let sub = make_subscription(&env, 1, 2, 0, true);
        let amount = resolve_charge_amount(&env, 1, &sub);
        assert_eq(amount, 0);
    }

    #[]
    fn resolve_charge_amount_returns_max_i128() {
        let env = Env::default();
        let sub = make_subscription(&env, 1, 2, i128::MAX, true);
        let amount = resolve_charge_amount(&env, 1, &sub);
        assert_eq(amount, i128::MAX);
    }

    // ----------------------------------------------------------------------
    // Boundary: zero interval and zero amount must not panic.
    // ----------------------------------------------------------------------
    #[]
    fn resolve_charge_amount_zero_interval() {
        let env = Env::default();
        let sub = make_subscription(&env, 1, 2, 500, 0);
        let amount = resolve_charge_amount(&env, 1, &sub);
        assert_eq(amount, 500);
    }

    // ----------------------------------------------------------------------
    // Authorization / identity: different subscription ids are independent.
    // ----------------------------------------------------------------------
    #[]
    fn resolve_charge_amount_different_ids_are_independent() {
        let env = Env::default();
        let sub_a = make_subscription(&env, 1, 2, 100, 1);
        let sub_b = make_subscription(&env, 3, 4, 200, 2);
        assert_eq(resolve_charge_amount(&env, 1, &sub_a), 100);
        assert_eq(resolve_charge_amount(&env, 2, &sub_b), 200);
    }

    // ----------------------------------------------------------------------
    // Rejected operations must not mutate state.
    // ----------------------------------------------------------------------
    #[]
    fn resolve_charge_amount_is_pure() {
        let env = Env::default();
        let sub = make_subscription(&env, 1, 2, 750, 3);
        let before = sub.clone();
        let _ = resolve_charge_amount(&env, 1, &sub);
        assert_eq(sub.amount, before.amount);
        assert_eq(sub.interval, before.interval);
        assert_eq(sub.last_charged, before.last_charged);
        assert_eq(sub.active, before.active);
    }

    // ----------------------------------------------------------------------
    // Invalid inputs: inactive subscriptions must not be charged.
    // ----------------------------------------------------------------------
    /// NOTE: This test assumes the contract either returns 0 or panics for an
    /// inactive subscription. Adjust the expectation to match the actual
    /// contract behavior if necessary.
    #[test]
    #[should_panic]
    fn resolve_charge_amount_inactive_subscription_rejected() {
        let env = Env::default();
        let mut sub = make_subscription(&env, 1, 2, 100, true);
        sub.active = false;
        let _ = resolve_charge_amount(&env, 1, &sub);
    }

    // ----------------------------------------------------------------------
    // Arithmetic: negative amounts are invalid and must be rejected.
    // ----------------------------------------------------------------------
    #[test]
    #[should_panic]
    fn resolve_charge_amount_negative_amount_rejected() {
        let env = Env::default();
        let sub = make_subscription(&env, 1, 2, -1, true);
        let _ = resolve_charge_amount(&env, 1, &sub);
    }

    // ----------------------------------------------------------------------
    // Determinism: same inputs produce same output across repeated calls.
    // ----------------------------------------------------------------------
    #[]
    fn resolve_charge_amount_is_deterministic() {
        let env = Env::default();
        let sub = make_subscription(&env, 1, 2, 999, true);
        let a = resolve_charge_amount(&env, 1, &sub);
        let b = resolve_charge_amount(&env, 1, &sub);
        let c = resolve_charge_amount(&env, 1, &sub);
        assert_eq(a, b);
        assert_eq(b, c);
    }
}
