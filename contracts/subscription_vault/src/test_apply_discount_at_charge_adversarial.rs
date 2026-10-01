//! Adversarial coverage for `coupon::apply_discount_at_charge`.
//!
//! # What is tested here
//!
//! `apply_discount_at_charge(env, subscription_id, now, sub_token, gross)`
//! sits on the critical path of every charge. Its contract is:
//!
//! * **No coupon bound** → `(gross, 0)` — no storage mutation, no event.
//! * **Coupon bound but revoked** → `(gross, 0)` — billing must not be blocked.
//! * **Coupon bound but expired** (exact boundary `now == expires_at`, and
//!   `now > expires_at`) → `(gross, 0)` — silent skip.
//! * **Coupon bound but token mismatch** → `(gross, 0)` — silent skip.
//! * **Valid coupon** → `(payable, discount)` with correct arithmetic and a
//!   `discount_applied` event carrying the correct fields.
//! * **Boundary gross values** (`0`, `1`, `i128::MAX`, negative) — the
//!   invariant `0 <= discount <= max(0, gross)` must hold in all cases.
//! * **State unchanged after any skip** — the `SubCoupon` binding and the
//!   `Coupon` record itself are never mutated by `apply_discount_at_charge`.
//! * **Event emitted iff and only if the discount is actually applied.**
//!
//! The inline `#[cfg(test)] mod tests` in `coupon.rs` already covers one
//! success case and one expiry skip. Every case below is new.

#![cfg(test)]

use crate::coupon::apply_discount_at_charge;
use crate::types::{Coupon, DataKey, DiscountAppliedEvent, Subscription, SubscriptionStatus};
use crate::SubscriptionVault;
use soroban_sdk::testutils::{Address as _, Events};
use soroban_sdk::{Address, Env, FromVal, Symbol, TryFromVal};

// ─────────────────────────────────────────────────────────────────────────────
// Fixture helpers
// ─────────────────────────────────────────────────────────────────────────────

struct Fixture {
    env: Env,
    contract_id: Address,
    merchant: Address,
    subscriber: Address,
    token: Address,
    sub_id: u32,
}

fn make_subscription(subscriber: &Address, merchant: &Address, token: &Address) -> Subscription {
    Subscription {
        subscriber: subscriber.clone(),
        merchant: merchant.clone(),
        token: token.clone(),
        amount: 1_000,
        interval_seconds: 86_400,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: 10_000,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
        start_time: 0,
        expires_at: None,
        grace_start_timestamp: None,
        cancel_at: None,
        expires_at_ledger: None,
        sub_account_label: None,
        auto_renew: true,
        auto_renew_disabled_at: None,
        arrears: 0,
    }
}

fn make_coupon(merchant: &Address, code: &Symbol, token: &Address) -> Coupon {
    Coupon {
        code: code.clone(),
        merchant: merchant.clone(),
        token: token.clone(),
        percent_off_bps: 1_000, // 10 %
        fixed_off: 0,
        max_redemptions: 0,
        expires_at: 0,
        revoked: false,
    }
}

/// Persist `coupon` directly to storage (same as the private `write_coupon`
/// helper in `coupon.rs` but without the TTL bump — tests don't need it).
fn store_coupon(env: &Env, coupon: &Coupon) {
    env.storage()
        .persistent()
        .set(&DataKey::Coupon(coupon.code.clone()), coupon);
}

fn setup() -> Fixture {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());

    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let token = Address::generate(&env);
    let sub_id = 42u32;

    env.as_contract(&contract_id, || {
        let sub = make_subscription(&subscriber, &merchant, &token);
        env.storage().persistent().set(&DataKey::Sub(sub_id), &sub);
    });

    Fixture {
        env,
        contract_id,
        merchant,
        subscriber,
        token,
        sub_id,
    }
}

/// Count how many events in `env.events().all()` have the topic symbol
/// `"discount_applied"`.
fn count_discount_events(env: &Env) -> usize {
    env.events()
        .all()
        .iter()
        .filter(|ev| {
            ev.1.len() >= 1
                && Symbol::from_val(env, &ev.1.get(0).unwrap())
                    == Symbol::new(env, "discount_applied")
        })
        .count()
}

// ─────────────────────────────────────────────────────────────────────────────
// No coupon bound — must return (gross, 0) for every gross value
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn no_coupon_bound_returns_gross_unchanged_for_typical_amount() {
    let f = setup();
    f.env.as_contract(&f.contract_id, || {
        let (payable, discount) = apply_discount_at_charge(&f.env, f.sub_id, 0, &f.token, 1_000);
        assert_eq!(payable, 1_000);
        assert_eq!(discount, 0);
    });
}

#[test]
fn no_coupon_bound_returns_gross_zero() {
    let f = setup();
    f.env.as_contract(&f.contract_id, || {
        let (payable, discount) = apply_discount_at_charge(&f.env, f.sub_id, 0, &f.token, 0);
        assert_eq!(payable, 0);
        assert_eq!(discount, 0);
    });
}

#[test]
fn no_coupon_bound_returns_gross_max_i128() {
    let f = setup();
    f.env.as_contract(&f.contract_id, || {
        let (payable, discount) =
            apply_discount_at_charge(&f.env, f.sub_id, 0, &f.token, i128::MAX);
        assert_eq!(payable, i128::MAX);
        assert_eq!(discount, 0);
    });
}

#[test]
fn no_coupon_bound_returns_gross_negative() {
    let f = setup();
    f.env.as_contract(&f.contract_id, || {
        let (payable, discount) = apply_discount_at_charge(&f.env, f.sub_id, 0, &f.token, -1);
        assert_eq!(payable, -1);
        assert_eq!(discount, 0);
    });
}

#[test]
fn no_coupon_bound_emits_no_events() {
    let f = setup();
    f.env.as_contract(&f.contract_id, || {
        apply_discount_at_charge(&f.env, f.sub_id, 0, &f.token, 1_000);
        assert_eq!(count_discount_events(&f.env), 0);
    });
}

/// Unknown subscription_id — no coupon stored, behaves identically.
#[test]
fn unknown_subscription_id_returns_gross_unchanged() {
    let f = setup();
    let unknown_id = 99_999u32;
    f.env.as_contract(&f.contract_id, || {
        assert!(!f
            .env
            .storage()
            .persistent()
            .has(&DataKey::SubCoupon(unknown_id)));
        let (payable, discount) = apply_discount_at_charge(&f.env, unknown_id, 0, &f.token, 5_000);
        assert_eq!(payable, 5_000);
        assert_eq!(discount, 0);
    });
}

/// Subscription ID == 0 with no coupon.
#[test]
fn subscription_id_zero_no_coupon_returns_gross() {
    let f = setup();
    f.env.as_contract(&f.contract_id, || {
        let (payable, discount) = apply_discount_at_charge(&f.env, 0, 0, &f.token, 777);
        assert_eq!(payable, 777);
        assert_eq!(discount, 0);
    });
}

/// u32::MAX subscription ID with no coupon.
#[test]
fn subscription_id_u32_max_no_coupon_returns_gross() {
    let f = setup();
    f.env.as_contract(&f.contract_id, || {
        let (payable, discount) = apply_discount_at_charge(&f.env, u32::MAX, 0, &f.token, 1_000);
        assert_eq!(payable, 1_000);
        assert_eq!(discount, 0);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Revoked coupon — must skip silently
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn revoked_coupon_returns_gross_unchanged_and_no_event() {
    let f = setup();
    let code = Symbol::new(&f.env, "REVOKED");
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.revoked = true;
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        let (payable, discount) =
            apply_discount_at_charge(&f.env, f.sub_id, 1_000, &f.token, 2_000);
        assert_eq!(payable, 2_000);
        assert_eq!(discount, 0);
        assert_eq!(count_discount_events(&f.env), 0);
    });
}

#[test]
fn revoked_coupon_does_not_mutate_coupon_record() {
    let f = setup();
    let code = Symbol::new(&f.env, "REVOKED2");
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.revoked = true;
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        apply_discount_at_charge(&f.env, f.sub_id, 500, &f.token, 3_000);

        // Coupon record is unchanged (still revoked, no extra fields flipped).
        let stored: Coupon = f
            .env
            .storage()
            .persistent()
            .get(&DataKey::Coupon(code.clone()))
            .unwrap();
        assert!(stored.revoked);
        assert_eq!(stored.code, code);

        // SubCoupon binding also unchanged.
        let binding: Symbol = f
            .env
            .storage()
            .persistent()
            .get(&DataKey::SubCoupon(f.sub_id))
            .unwrap();
        assert_eq!(binding, code);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Expired coupon — boundary cases for `now >= expires_at`
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn expired_coupon_at_exact_expiry_boundary_is_skipped() {
    let f = setup();
    let code = Symbol::new(&f.env, "EXACT_EXP");
    let expires_at: u64 = 1_000;
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.expires_at = expires_at;
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        // now == expires_at → expired
        let (payable, discount) =
            apply_discount_at_charge(&f.env, f.sub_id, expires_at, &f.token, 1_000);
        assert_eq!(payable, 1_000);
        assert_eq!(discount, 0);
        assert_eq!(count_discount_events(&f.env), 0);
    });
}

#[test]
fn expired_coupon_one_second_past_expiry_is_skipped() {
    let f = setup();
    let code = Symbol::new(&f.env, "PAST_EXP");
    let expires_at: u64 = 1_000;
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.expires_at = expires_at;
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        // now > expires_at → expired
        let (payable, discount) =
            apply_discount_at_charge(&f.env, f.sub_id, expires_at + 1, &f.token, 1_000);
        assert_eq!(payable, 1_000);
        assert_eq!(discount, 0);
        assert_eq!(count_discount_events(&f.env), 0);
    });
}

#[test]
fn expired_coupon_one_second_before_expiry_is_applied() {
    let f = setup();
    let code = Symbol::new(&f.env, "ALMOST_EXP");
    let expires_at: u64 = 1_000;
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.expires_at = expires_at;
        coupon.percent_off_bps = 1_000; // 10 %
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        // now = expires_at - 1 → valid; 10% of 1000 = 100 discount
        let (payable, discount) =
            apply_discount_at_charge(&f.env, f.sub_id, expires_at - 1, &f.token, 1_000);
        assert_eq!(payable, 900);
        assert_eq!(discount, 100);
        assert_eq!(count_discount_events(&f.env), 1);
    });
}

#[test]
fn never_expiring_coupon_expires_at_zero_is_always_applied() {
    let f = setup();
    let code = Symbol::new(&f.env, "NEVER_EXP");
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.expires_at = 0; // never expires
        coupon.percent_off_bps = 5_000; // 50 %
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        // Even at u64::MAX the coupon is still valid.
        let (payable, discount) =
            apply_discount_at_charge(&f.env, f.sub_id, u64::MAX, &f.token, 1_000);
        assert_eq!(payable, 500);
        assert_eq!(discount, 500);
        assert_eq!(count_discount_events(&f.env), 1);
    });
}

#[test]
fn expired_coupon_at_u64_max_timestamp_is_skipped() {
    let f = setup();
    let code = Symbol::new(&f.env, "EXPIRED_MAX");
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.expires_at = 1; // expired for any now >= 1
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        let (payable, discount) =
            apply_discount_at_charge(&f.env, f.sub_id, u64::MAX, &f.token, 1_000);
        assert_eq!(payable, 1_000);
        assert_eq!(discount, 0);
        assert_eq!(count_discount_events(&f.env), 0);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Token mismatch — silent skip
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn token_mismatch_returns_gross_unchanged_and_no_event() {
    let f = setup();
    let code = Symbol::new(&f.env, "WRONG_TOK");
    let other_token = Address::generate(&f.env);
    f.env.as_contract(&f.contract_id, || {
        // Coupon is for `f.token`, but we charge with `other_token`.
        let coupon = make_coupon(&f.merchant, &code, &f.token);
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        let (payable, discount) =
            apply_discount_at_charge(&f.env, f.sub_id, 100, &other_token, 5_000);
        assert_eq!(payable, 5_000);
        assert_eq!(discount, 0);
        assert_eq!(count_discount_events(&f.env), 0);
    });
}

#[test]
fn token_mismatch_does_not_mutate_coupon_or_binding() {
    let f = setup();
    let code = Symbol::new(&f.env, "MISMATCH2");
    let other_token = Address::generate(&f.env);
    f.env.as_contract(&f.contract_id, || {
        let coupon = make_coupon(&f.merchant, &code, &f.token);
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        apply_discount_at_charge(&f.env, f.sub_id, 100, &other_token, 1_000);

        // Binding unchanged.
        let binding: Symbol = f
            .env
            .storage()
            .persistent()
            .get(&DataKey::SubCoupon(f.sub_id))
            .unwrap();
        assert_eq!(binding, code);

        // Coupon record unchanged — token field still points to original.
        let stored: Coupon = f
            .env
            .storage()
            .persistent()
            .get(&DataKey::Coupon(code.clone()))
            .unwrap();
        assert_eq!(stored.token, f.token);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Successful discount — arithmetic, event payload, state invariance
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn successful_10_percent_discount_returns_correct_amounts() {
    let f = setup();
    let code = Symbol::new(&f.env, "TEN_PCT");
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.percent_off_bps = 1_000; // 10 %
        coupon.fixed_off = 0;
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        // 10 % of 1_000 = 100
        let (payable, discount) = apply_discount_at_charge(&f.env, f.sub_id, 500, &f.token, 1_000);
        assert_eq!(discount, 100);
        assert_eq!(payable, 900);
        assert_eq!(payable + discount, 1_000);
    });
}

#[test]
fn successful_discount_percent_plus_fixed() {
    let f = setup();
    let code = Symbol::new(&f.env, "COMBO");
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.percent_off_bps = 2_000; // 20 %
        coupon.fixed_off = 50;
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        // 1_000 → 800 (after 20 %) → 750 (after -50) → payable 750, discount 250
        let (payable, discount) = apply_discount_at_charge(&f.env, f.sub_id, 100, &f.token, 1_000);
        assert_eq!(payable, 750);
        assert_eq!(discount, 250);
    });
}

#[test]
fn successful_discount_emits_exactly_one_discount_applied_event() {
    let f = setup();
    let code = Symbol::new(&f.env, "EVT_ONCE");
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.percent_off_bps = 5_000; // 50 %
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        apply_discount_at_charge(&f.env, f.sub_id, 1, &f.token, 2_000);
        assert_eq!(count_discount_events(&f.env), 1);
    });
}

#[test]
fn successful_discount_event_carries_correct_fields() {
    let f = setup();
    let code = Symbol::new(&f.env, "EVT_FLDS");
    let gross: i128 = 4_000;
    let now: u64 = 999;
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.percent_off_bps = 2_500; // 25 %
        coupon.fixed_off = 0;
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        // 25 % of 4_000 = 1_000 discount; payable = 3_000
        let (payable, discount) = apply_discount_at_charge(&f.env, f.sub_id, now, &f.token, gross);
        assert_eq!(discount, 1_000);
        assert_eq!(payable, 3_000);

        // Inspect the event payload.
        let events = f.env.events().all();
        let evt = events
            .iter()
            .find(|ev| {
                ev.1.len() >= 1
                    && Symbol::from_val(&f.env, &ev.1.get(0).unwrap())
                        == Symbol::new(&f.env, "discount_applied")
            })
            .expect("discount_applied event must be emitted");

        let payload = DiscountAppliedEvent::try_from_val(&f.env, &evt.2)
            .expect("event data must deserialise as DiscountAppliedEvent");

        assert_eq!(payload.subscription_id, f.sub_id);
        assert_eq!(payload.gross_amount, gross);
        assert_eq!(payload.discount_amount, 1_000);
        assert_eq!(payload.discounted_amount, 3_000);
        assert_eq!(payload.coupon_code, code);
        assert_eq!(payload.timestamp, now);
    });
}

#[test]
fn successful_discount_does_not_mutate_coupon_record_or_binding() {
    let f = setup();
    let code = Symbol::new(&f.env, "NO_MUTATE");
    f.env.as_contract(&f.contract_id, || {
        let coupon = make_coupon(&f.merchant, &code, &f.token);
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        apply_discount_at_charge(&f.env, f.sub_id, 10, &f.token, 1_000);

        // Coupon record must be identical.
        let stored: Coupon = f
            .env
            .storage()
            .persistent()
            .get(&DataKey::Coupon(code.clone()))
            .unwrap();
        assert_eq!(stored.percent_off_bps, 1_000);
        assert!(!stored.revoked);

        // Binding must still be in place.
        let binding: Symbol = f
            .env
            .storage()
            .persistent()
            .get(&DataKey::SubCoupon(f.sub_id))
            .unwrap();
        assert_eq!(binding, code);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Boundary gross values for successful discounts
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gross_zero_with_valid_coupon_yields_zero_payable_and_zero_discount() {
    let f = setup();
    let code = Symbol::new(&f.env, "ZERO_GRS");
    f.env.as_contract(&f.contract_id, || {
        let coupon = make_coupon(&f.merchant, &code, &f.token);
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        let (payable, discount) = apply_discount_at_charge(&f.env, f.sub_id, 0, &f.token, 0);
        assert_eq!(payable, 0);
        assert_eq!(discount, 0);
        // discount_applied IS still emitted even for a zero gross (the coupon was valid).
        assert_eq!(count_discount_events(&f.env), 1);
    });
}

#[test]
fn gross_one_with_10_percent_coupon_rounds_correctly() {
    let f = setup();
    let code = Symbol::new(&f.env, "GROSS_ONE");
    f.env.as_contract(&f.contract_id, || {
        // 10 % of 1 floors to 0 payable (integer division: 1 * 9000 / 10000 = 0).
        let coupon = make_coupon(&f.merchant, &code, &f.token);
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        let (payable, discount) = apply_discount_at_charge(&f.env, f.sub_id, 0, &f.token, 1);
        // after_pct = 1 * (10000 - 1000) / 10000 = 0  → discount = 1, payable = 0
        assert_eq!(payable, 0);
        assert_eq!(discount, 1);
    });
}

#[test]
fn gross_max_i128_with_valid_coupon_does_not_overflow() {
    let f = setup();
    let code = Symbol::new(&f.env, "MAX_GROSS");
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.percent_off_bps = 5_000; // 50 %
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        let (payable, discount) =
            apply_discount_at_charge(&f.env, f.sub_id, 0, &f.token, i128::MAX);

        // Invariant: 0 <= discount <= i128::MAX and payable + discount == i128::MAX
        assert!(discount >= 0);
        assert!(discount <= i128::MAX);
        assert_eq!(payable + discount, i128::MAX);
    });
}

#[test]
fn gross_negative_with_valid_coupon_returns_gross_and_zero_discount() {
    // compute_discount clamps gross to 0 when gross < 0, so discount is 0.
    let f = setup();
    let code = Symbol::new(&f.env, "NEG_GROSS");
    f.env.as_contract(&f.contract_id, || {
        let coupon = make_coupon(&f.merchant, &code, &f.token);
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        let (payable, discount) = apply_discount_at_charge(&f.env, f.sub_id, 0, &f.token, -500);
        // gross is negative; compute_discount clamps it → discount = 0; payable = -500
        assert_eq!(discount, 0);
        assert_eq!(payable, -500);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// 100 % discount — payable becomes 0
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn full_100_percent_discount_yields_zero_payable() {
    let f = setup();
    let code = Symbol::new(&f.env, "FREE");
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.percent_off_bps = 10_000; // 100 %
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        let (payable, discount) = apply_discount_at_charge(&f.env, f.sub_id, 0, &f.token, 3_000);
        assert_eq!(payable, 0);
        assert_eq!(discount, 3_000);
        assert_eq!(count_discount_events(&f.env), 1);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Fixed-only discount larger than gross — clamped to gross
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn fixed_discount_larger_than_gross_is_clamped_to_gross() {
    let f = setup();
    let code = Symbol::new(&f.env, "BIG_FIXED");
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.percent_off_bps = 0;
        coupon.fixed_off = 10_000; // larger than charge
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        let (payable, discount) = apply_discount_at_charge(&f.env, f.sub_id, 0, &f.token, 1_000);
        assert_eq!(payable, 0);
        assert_eq!(discount, 1_000); // clamped — can't be more than gross
        assert_eq!(payable + discount, 1_000);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Multiple sequential calls — idempotent read; only one event per call
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn two_sequential_valid_charges_each_emit_their_own_event() {
    let f = setup();
    let code = Symbol::new(&f.env, "SEQUENTIAL");
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.percent_off_bps = 1_000;
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        apply_discount_at_charge(&f.env, f.sub_id, 10, &f.token, 1_000);
        apply_discount_at_charge(&f.env, f.sub_id, 20, &f.token, 2_000);

        assert_eq!(count_discount_events(&f.env), 2);
    });
}

#[test]
fn skip_followed_by_valid_charge_emits_one_event() {
    let f = setup();
    let code = Symbol::new(&f.env, "SKIP_THEN");
    let expires_at: u64 = 500;
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &f.token);
        coupon.expires_at = expires_at;
        coupon.percent_off_bps = 1_000;
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        // First call: expired → skip, no event.
        apply_discount_at_charge(&f.env, f.sub_id, expires_at + 1, &f.token, 1_000);
        assert_eq!(count_discount_events(&f.env), 0);

        // Update coupon to never expire.
        let mut updated = make_coupon(&f.merchant, &code, &f.token);
        updated.expires_at = 0;
        updated.percent_off_bps = 1_000;
        store_coupon(&f.env, &updated);

        // Second call: valid → event.
        apply_discount_at_charge(&f.env, f.sub_id, expires_at + 2, &f.token, 1_000);
        assert_eq!(count_discount_events(&f.env), 1);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// All three skip reasons in combination
// ─────────────────────────────────────────────────────────────────────────────

/// A coupon that is simultaneously revoked AND expired AND token-mismatched
/// must still produce a clean skip.
#[test]
fn triple_invalid_coupon_skips_without_panic() {
    let f = setup();
    let code = Symbol::new(&f.env, "TRPLBAD");
    let other_token = Address::generate(&f.env);
    f.env.as_contract(&f.contract_id, || {
        let mut coupon = make_coupon(&f.merchant, &code, &other_token);
        coupon.revoked = true;
        coupon.expires_at = 1; // expired for any now >= 1
        store_coupon(&f.env, &coupon);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        let (payable, discount) = apply_discount_at_charge(&f.env, f.sub_id, 999, &f.token, 4_000);
        assert_eq!(payable, 4_000);
        assert_eq!(discount, 0);
        assert_eq!(count_discount_events(&f.env), 0);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Different subscription IDs are isolated
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn coupon_bound_to_different_sub_id_does_not_affect_target_sub() {
    let f = setup();
    let code = Symbol::new(&f.env, "OTHER_SUB");
    let other_sub_id = 99u32;
    f.env.as_contract(&f.contract_id, || {
        let coupon = make_coupon(&f.merchant, &code, &f.token);
        store_coupon(&f.env, &coupon);

        // Bind coupon to `other_sub_id`, NOT `f.sub_id`.
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(other_sub_id), &code);

        // Charging `f.sub_id` must see no coupon.
        let (payable, discount) = apply_discount_at_charge(&f.env, f.sub_id, 0, &f.token, 2_000);
        assert_eq!(payable, 2_000);
        assert_eq!(discount, 0);
        assert_eq!(count_discount_events(&f.env), 0);

        // Charging `other_sub_id` must apply the discount.
        let (payable2, discount2) =
            apply_discount_at_charge(&f.env, other_sub_id, 0, &f.token, 2_000);
        assert_eq!(discount2, 200); // 10 %
        assert_eq!(payable2, 1_800);
        assert_eq!(count_discount_events(&f.env), 1);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Skip: state identity invariant (comprehensive)
// ─────────────────────────────────────────────────────────────────────────────

/// After any skip path the coupon record and SubCoupon binding are unchanged.
/// Each skip reason is tested in isolation with its own fixture to keep storage clean.
#[test]
fn state_unchanged_after_revoked_coupon_skip() {
    let f = setup();
    let code = Symbol::new(&f.env, "SKIP_REV");
    f.env.as_contract(&f.contract_id, || {
        let mut c = make_coupon(&f.merchant, &code, &f.token);
        c.revoked = true;
        store_coupon(&f.env, &c);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        let before: Coupon = f
            .env
            .storage()
            .persistent()
            .get(&DataKey::Coupon(code.clone()))
            .unwrap();

        apply_discount_at_charge(&f.env, f.sub_id, 999, &f.token, 3_000);

        let after: Coupon = f
            .env
            .storage()
            .persistent()
            .get(&DataKey::Coupon(code.clone()))
            .unwrap();
        assert_eq!(before, after, "revoked skip must not mutate coupon");

        let binding: Symbol = f
            .env
            .storage()
            .persistent()
            .get(&DataKey::SubCoupon(f.sub_id))
            .unwrap();
        assert_eq!(binding, code, "revoked skip must not mutate binding");
        assert_eq!(count_discount_events(&f.env), 0);
    });
}

#[test]
fn state_unchanged_after_expired_coupon_skip() {
    let f = setup();
    let code = Symbol::new(&f.env, "SKIP_EXP");
    f.env.as_contract(&f.contract_id, || {
        let mut c = make_coupon(&f.merchant, &code, &f.token);
        c.expires_at = 10;
        store_coupon(&f.env, &c);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        let before: Coupon = f
            .env
            .storage()
            .persistent()
            .get(&DataKey::Coupon(code.clone()))
            .unwrap();

        apply_discount_at_charge(&f.env, f.sub_id, 999, &f.token, 3_000);

        let after: Coupon = f
            .env
            .storage()
            .persistent()
            .get(&DataKey::Coupon(code.clone()))
            .unwrap();
        assert_eq!(before, after, "expired skip must not mutate coupon");

        let binding: Symbol = f
            .env
            .storage()
            .persistent()
            .get(&DataKey::SubCoupon(f.sub_id))
            .unwrap();
        assert_eq!(binding, code, "expired skip must not mutate binding");
        assert_eq!(count_discount_events(&f.env), 0);
    });
}

#[test]
fn state_unchanged_after_token_mismatch_skip() {
    let f = setup();
    let code = Symbol::new(&f.env, "SKIP_TOK");
    let other_token = Address::generate(&f.env);
    f.env.as_contract(&f.contract_id, || {
        // Coupon token != sub_token passed to apply_discount_at_charge.
        let c = make_coupon(&f.merchant, &code, &other_token);
        store_coupon(&f.env, &c);
        f.env
            .storage()
            .persistent()
            .set(&DataKey::SubCoupon(f.sub_id), &code);

        let before: Coupon = f
            .env
            .storage()
            .persistent()
            .get(&DataKey::Coupon(code.clone()))
            .unwrap();

        apply_discount_at_charge(&f.env, f.sub_id, 999, &f.token, 3_000);

        let after: Coupon = f
            .env
            .storage()
            .persistent()
            .get(&DataKey::Coupon(code.clone()))
            .unwrap();
        assert_eq!(before, after, "token-mismatch skip must not mutate coupon");

        let binding: Symbol = f
            .env
            .storage()
            .persistent()
            .get(&DataKey::SubCoupon(f.sub_id))
            .unwrap();
        assert_eq!(binding, code, "token-mismatch skip must not mutate binding");
        assert_eq!(count_discount_events(&f.env), 0);
    });
}
