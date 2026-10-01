//! Merchant-managed coupon and discount system (Issue #474).
//!
//! # Lifecycle
//!
//! 1. **Merchant** calls `create_coupon` → stored under `DataKey::Coupon(code)`.
//! 2. **Subscriber** calls `apply_coupon` → binds code to subscription via
//!    `DataKey::SubCoupon(subscription_id)`; increments `DataKey::CouponRedemptions(code)`.
//! 3. `charge_one` calls `resolve_coupon_for_charge` / `validate_coupon_for_charge` /
//!    `compute_discount` to apply the discount before fee splitting.
//! 4. **Merchant** may call `revoke_coupon` at any time. Already-bound coupons that have
//!    been revoked are skipped silently at charge time (to avoid billing outages).
//!
//! # Discount Ordering
//!
//! Percentage discount is applied first, then fixed-amount:
//!
//! ```text
//! after_pct   = gross * (10_000 - percent_off_bps) / 10_000   (integer floor)
//! after_fixed = max(after_pct - fixed_off, 0)                  (clamped to zero)
//! discount    = gross - after_fixed
//! ```
//!
//! The payable amount never goes below zero.
//!
//! # Storage
//!
//! All three keys are **persistent** and get TTL extended on every write:
//!
//! | Key | Type | Purpose |
//! |---|---|---|
//! | `DataKey::Coupon(code)` | `Coupon` | Coupon record |
//! | `DataKey::CouponRedemptions(code)` | `u32` | Global bind count |
//! | `DataKey::SubCoupon(subscription_id)` | `Symbol` | Subscription → coupon code |

#![allow(dead_code)]

use crate::types::{
    Coupon, CouponAppliedEvent, CouponCreatedEvent, CouponRevokedEvent, DataKey,
    DiscountAppliedEvent, Error, SUB_TTL_EXTEND_TO, SUB_TTL_THRESHOLD, EVENT_SCHEMA_VERSION,
};
use soroban_sdk::{Address, Env, Symbol};

// ─────────────────────────────────────────────────────────────────────────────
// Storage helpers
// ─────────────────────────────────────────────────────────────────────────────

fn write_coupon(env: &Env, coupon: &Coupon) {
    let key = DataKey::Coupon(coupon.code.clone());
    env.storage().persistent().set(&key, coupon);
    crate::subscription::maybe_extend_ttl(env, &key, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
}

fn read_coupon(env: &Env, code: &Symbol) -> Option<Coupon> {
    env.storage()
        .persistent()
        .get(&DataKey::Coupon(code.clone()))
}

fn read_redemptions(env: &Env, code: &Symbol) -> u32 {
    env.storage()
        .persistent()
        .get::<_, u32>(&DataKey::CouponRedemptions(code.clone()))
        .unwrap_or(0)
}

fn write_redemptions(env: &Env, code: &Symbol, count: u32) {
    let key = DataKey::CouponRedemptions(code.clone());
    env.storage().persistent().set(&key, &count);
    crate::subscription::maybe_extend_ttl(env, &key, SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO);
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Create a new coupon. Callable by the owning merchant only.
///
/// # Validation
/// - `percent_off_bps` must be in `0..=10_000`.
/// - `fixed_off` must be `>= 0`.
/// - If `expires_at > 0`, it must be strictly in the future.
/// - The coupon code must not already exist.
pub fn create_coupon(
    env: &Env,
    merchant: Address,
    code: Symbol,
    token: Address,
    percent_off_bps: u32,
    fixed_off: i128,
    max_redemptions: u32,
    expires_at: u64,
) -> Result<(), Error> {
    merchant.require_auth();

    if percent_off_bps > 10_000 {
        return Err(Error::InvalidInput);
    }
    if fixed_off < 0 {
        return Err(Error::InvalidInput);
    }
    let now = env.ledger().timestamp();
    if expires_at > 0 && expires_at <= now {
        return Err(Error::InvalidInput);
    }

    // Reject duplicate codes.
    if env
        .storage()
        .persistent()
        .has(&DataKey::Coupon(code.clone()))
    {
        return Err(Error::CouponAlreadyExists);
    }

    let coupon = Coupon {
        code: code.clone(),
        merchant: merchant.clone(),
        token: token.clone(),
        percent_off_bps,
        fixed_off,
        max_redemptions,
        expires_at,
        revoked: false,
    };
    write_coupon(env, &coupon);

    env.events().publish(
        (Symbol::new(env, "coupon_created"), merchant.clone()),
        CouponCreatedEvent {
            merchant,
            code,
            token,
            percent_off_bps,
            fixed_off,
            max_redemptions,
            expires_at,
            timestamp: now,
            schema_version: EVENT_SCHEMA_VERSION,
        },
    );
    Ok(())
}

/// Revoke an existing coupon. Only the owning merchant may revoke.
///
/// Revoked coupons cannot be applied by new subscribers. Already-bound
/// coupons are skipped silently at charge time.
pub fn revoke_coupon(env: &Env, merchant: Address, code: Symbol) -> Result<(), Error> {
    merchant.require_auth();

    let mut coupon = read_coupon(env, &code).ok_or(Error::CouponNotFound)?;
    if coupon.merchant != merchant {
        return Err(Error::Unauthorized);
    }

    coupon.revoked = true;
    write_coupon(env, &coupon);

    env.events().publish(
        (Symbol::new(env, "coupon_revoked"), merchant.clone()),
        CouponRevokedEvent {
            merchant,
            code,
            timestamp: env.ledger().timestamp(),
            schema_version: EVENT_SCHEMA_VERSION,
        },
    );
    Ok(())
}

/// Bind a coupon to a subscription. Callable by the subscription's subscriber only.
///
/// Increments the global redemption counter at bind time. A subscription can
/// hold at most one coupon (`CouponAlreadyApplied` if already bound).
pub fn apply_coupon(
    env: &Env,
    subscriber: Address,
    subscription_id: u32,
    code: Symbol,
) -> Result<(), Error> {
    subscriber.require_auth();

    // Load subscription and verify ownership.
    let sub = crate::queries::get_subscription(env, subscription_id)?;
    if sub.subscriber != subscriber {
        return Err(Error::Unauthorized);
    }

    // One coupon per subscription.
    if env
        .storage()
        .persistent()
        .has(&DataKey::SubCoupon(subscription_id))
    {
        return Err(Error::CouponAlreadyApplied);
    }

    let coupon = read_coupon(env, &code).ok_or(Error::CouponNotFound)?;

    // Coupon must be issued by the same merchant that owns the subscription.
    if coupon.merchant != sub.merchant {
        return Err(Error::Unauthorized);
    }

    if coupon.revoked {
        return Err(Error::CouponRevoked);
    }

    let now = env.ledger().timestamp();
    if coupon.expires_at > 0 && now >= coupon.expires_at {
        return Err(Error::CouponExpired);
    }

    if coupon.max_redemptions > 0 {
        let count = read_redemptions(env, &code);
        if count >= coupon.max_redemptions {
            return Err(Error::CouponRedemptionLimitReached);
        }
    }

    // Token must match the subscription's settlement token.
    if coupon.token != sub.token {
        return Err(Error::CouponTokenMismatch);
    }

    // Bind coupon to subscription.
    let sub_key = DataKey::SubCoupon(subscription_id);
    env.storage().persistent().set(&sub_key, &code);
    crate::subscription::maybe_extend_ttl(
        env,
        &sub_key,
        SUB_TTL_THRESHOLD,
        SUB_TTL_EXTEND_TO,
    );

    // Increment global redemption counter.
    increment_redemptions(env, &code);

    env.events().publish(
        (Symbol::new(env, "coupon_applied"), subscription_id),
        CouponAppliedEvent {
            subscription_id,
            subscriber,
            code,
            timestamp: now,
            schema_version: EVENT_SCHEMA_VERSION,
        },
    );
    Ok(())
}

/// Return the coupon bound to this subscription, if any.
pub fn resolve_coupon_for_charge(env: &Env, subscription_id: u32) -> Option<Coupon> {
    let code: Symbol = env
        .storage()
        .persistent()
        .get(&DataKey::SubCoupon(subscription_id))?;
    read_coupon(env, &code)
}

/// Return a coupon by code, or `None` if it does not exist.
pub fn get_coupon(env: &Env, code: Symbol) -> Option<Coupon> {
    read_coupon(env, &code)
}

/// Validate that a coupon is still usable at charge time.
///
/// Returns `Ok(())` if the coupon should be applied, or an error if it should
/// be skipped. Callers in `charge_one` catch these errors and skip the discount
/// without failing the charge, to avoid billing outages.
pub fn validate_coupon_for_charge(
    _env: &Env,
    now: u64,
    sub_token: &Address,
    coupon: &Coupon,
) -> Result<(), Error> {
    if coupon.revoked {
        return Err(Error::CouponRevoked);
    }
    if coupon.expires_at > 0 && now >= coupon.expires_at {
        return Err(Error::CouponExpired);
    }
    if coupon.token != *sub_token {
        return Err(Error::CouponTokenMismatch);
    }
    // Note: redemption limit is NOT re-checked here — the limit was already
    // enforced at apply_coupon time. Re-checking at charge time would wrongly
    // block subscribers who legitimately bound the coupon before the limit was
    // reached.
    Ok(())
}

/// Compute the discount amount for a given gross charge and coupon.
///
/// Returns the discount (non-negative). The caller subtracts this from gross.
///
/// # Discount ordering
/// 1. Percentage discount applied first.
/// 2. Fixed discount applied to the result of step 1.
/// 3. Payable amount is clamped to `[0, gross]`.
pub fn compute_discount(gross: i128, coupon: &Coupon) -> i128 {
    // A non-positive gross carries no discount. Clamping up front keeps the
    // documented invariant (`0 <= discount <= gross`) true for every i128,
    // including the negative boundary.
    let gross = gross.max(0);

    // Step 1 — percentage
    let after_pct = if coupon.percent_off_bps > 0 {
        // `percent_off_bps` is validated by `create_coupon`, but `compute_discount`
        // is public: clamp so an out-of-band value can neither overflow the
        // intermediate product nor invert the discount direction.
        let remaining_bps = (10_000i128 - coupon.percent_off_bps as i128).clamp(0, 10_000);
        // Integer floor division, expanded as
        //   floor(gross / 10_000) * bps + floor((gross % 10_000) * bps / 10_000)
        // which is algebraically identical but never materialises `gross * bps`,
        // so a large `gross` cannot overflow i128.
        let whole = gross / 10_000i128;
        let remainder = gross % 10_000i128;
        whole * remaining_bps + remainder * remaining_bps / 10_000i128
    } else {
        gross
    };

    // Step 2 — fixed (saturating subtraction, clamp to zero)
    let after_fixed = after_pct.saturating_sub(coupon.fixed_off).max(0);

    // Discount = gross - payable. Always in [0, gross].
    gross.saturating_sub(after_fixed)
}

/// Increment the global redemption counter for a coupon code.
///
/// Called once per successful `apply_coupon` binding.
pub fn increment_redemptions(env: &Env, code: &Symbol) {
    let count = read_redemptions(env, code);
    write_redemptions(env, code, count.saturating_add(1));
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal charge-path helper: apply discount + emit event
// ─────────────────────────────────────────────────────────────────────────────

/// Apply coupon discount to `gross` at charge time.
///
/// Returns `(payable, discount)`. If the coupon is revoked, expired, or token-
/// mismatched the discount is silently skipped (`(gross, 0)`).
pub fn apply_discount_at_charge(
    env: &Env,
    subscription_id: u32,
    now: u64,
    sub_token: &Address,
    gross: i128,
) -> (i128, i128) {
    let coupon = match resolve_coupon_for_charge(env, subscription_id) {
        Some(c) => c,
        None => return (gross, 0),
    };

    match validate_coupon_for_charge(env, now, sub_token, &coupon) {
        Err(_) => {
            // Expired or revoked — skip silently; billing must not be blocked.
            (gross, 0)
        }
        Ok(()) => {
            let discount = compute_discount(gross, &coupon);
            let payable = gross - discount; // discount <= gross is guaranteed

            env.events().publish(
                (Symbol::new(env, "discount_applied"), subscription_id),
                DiscountAppliedEvent {
                    subscription_id,
                    gross_amount: gross,
                    discount_amount: discount,
                    discounted_amount: payable,
                    coupon_code: coupon.code,
                    timestamp: now,
                    schema_version: EVENT_SCHEMA_VERSION,
                },
            );

            (payable, discount)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Subscription, SubscriptionStatus};
    use soroban_sdk::{
        testutils::{Address as _, Events, Ledger},
        Address, Env, FromVal, Symbol,
    };

    fn create_test_sub(subscriber: &Address, merchant: &Address, token: &Address) -> Subscription {
        Subscription {
            subscriber: subscriber.clone(),
            merchant: merchant.clone(),
            token: token.clone(),
            amount: 1_000,
            interval_seconds: 86_400,
            last_payment_timestamp: 0,
            status: SubscriptionStatus::Active,
            prepaid_balance: 5_000,
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

    fn create_test_coupon(merchant: &Address, code: &Symbol, token: &Address) -> Coupon {
        Coupon {
            code: code.clone(),
            merchant: merchant.clone(),
            token: token.clone(),
            percent_off_bps: 1_000,
            fixed_off: 50,
            max_redemptions: 10,
            expires_at: 0,
            revoked: false,
        }
    }

    struct TestFixture {
        env: Env,
        contract_id: Address,
        merchant: Address,
        subscriber: Address,
        token: Address,
        sub_id: u32,
    }

    fn setup_fixture() -> TestFixture {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(crate::SubscriptionVault, ());

        let merchant = Address::generate(&env);
        let subscriber = Address::generate(&env);
        let token = Address::generate(&env);
        let sub_id = 1u32;

        env.as_contract(&contract_id, || {
            let sub = create_test_sub(&subscriber, &merchant, &token);
            env.storage().persistent().set(&DataKey::Sub(sub_id), &sub);
        });

        TestFixture {
            env,
            contract_id,
            merchant,
            subscriber,
            token,
            sub_id,
        }
    }

    #[test]
    fn test_apply_coupon_success_updates_state_and_emits_event() {
        let f = setup_fixture();
        let code = Symbol::new(&f.env, "PROMO10");

        f.env.as_contract(&f.contract_id, || {
            let coupon = create_test_coupon(&f.merchant, &code, &f.token);
            write_coupon(&f.env, &coupon);

            assert_eq!(read_redemptions(&f.env, &code), 0);
            assert!(!f.env.storage().persistent().has(&DataKey::SubCoupon(f.sub_id)));

            let res = apply_coupon(&f.env, f.subscriber.clone(), f.sub_id, code.clone());
            assert_eq!(res, Ok(()));

            // Verify state changes
            assert_eq!(
                f.env.storage().persistent().get(&DataKey::SubCoupon(f.sub_id)),
                Some(code.clone())
            );
            assert_eq!(read_redemptions(&f.env, &code), 1);

            // Verify event emission
            let events = f.env.events().all();
            let last_event = events.last().expect("event expected");
            assert_eq!(
                Symbol::from_val(&f.env, &last_event.1.get(0).unwrap()),
                Symbol::new(&f.env, "coupon_applied")
            );
        });
    }

    #[test]
    fn test_apply_coupon_subscription_not_found_boundaries() {
        let f = setup_fixture();
        let code = Symbol::new(&f.env, "PROMO");

        f.env.as_contract(&f.contract_id, || {
            let coupon = create_test_coupon(&f.merchant, &code, &f.token);
            write_coupon(&f.env, &coupon);

            // Test non-existent boundaries: 0 (not seeded), 999, u32::MAX
            let test_ids = [0u32, 999u32, u32::MAX];
            for id in test_ids {
                assert!(crate::queries::get_subscription(&f.env, id).is_err());
                assert!(!f.env.storage().persistent().has(&DataKey::SubCoupon(id)));
            }

            let res = apply_coupon(&f.env, f.subscriber.clone(), 999, code.clone());
            assert_eq!(res, Err(Error::NotFound));

            // Verify state invariance: SubCoupon not set, redemptions unchanged
            assert!(!f.env.storage().persistent().has(&DataKey::SubCoupon(999)));
            assert_eq!(read_redemptions(&f.env, &code), 0);
        });
    }

    #[test]
    fn test_apply_coupon_unauthorized_subscriber_leaves_state_unchanged() {
        let f = setup_fixture();
        let code = Symbol::new(&f.env, "PROMO");
        let wrong_user = Address::generate(&f.env);

        f.env.as_contract(&f.contract_id, || {
            let coupon = create_test_coupon(&f.merchant, &code, &f.token);
            write_coupon(&f.env, &coupon);

            let res = apply_coupon(&f.env, wrong_user.clone(), f.sub_id, code.clone());
            assert_eq!(res, Err(Error::Unauthorized));

            // State unchanged
            assert!(!f.env.storage().persistent().has(&DataKey::SubCoupon(f.sub_id)));
            assert_eq!(read_redemptions(&f.env, &code), 0);
        });
    }

    #[test]
    fn test_apply_coupon_already_applied_preserves_original_state() {
        let f = setup_fixture();
        let code1 = Symbol::new(&f.env, "FIRST");
        let code2 = Symbol::new(&f.env, "SECOND");

        f.env.as_contract(&f.contract_id, || {
            let coupon1 = create_test_coupon(&f.merchant, &code1, &f.token);
            let coupon2 = create_test_coupon(&f.merchant, &code2, &f.token);
            write_coupon(&f.env, &coupon1);
            write_coupon(&f.env, &coupon2);

            // Pre-seed sub_id with code1
            f.env.storage().persistent().set(&DataKey::SubCoupon(f.sub_id), &code1);
            write_redemptions(&f.env, &code1, 1);

            // Attempting to apply code2 fails with CouponAlreadyApplied
            let res = apply_coupon(&f.env, f.subscriber.clone(), f.sub_id, code2.clone());
            assert_eq!(res, Err(Error::CouponAlreadyApplied));

            // Verify state invariance: original coupon remains, counters unchanged
            assert_eq!(
                f.env.storage().persistent().get(&DataKey::SubCoupon(f.sub_id)),
                Some(code1.clone())
            );
            assert_eq!(read_redemptions(&f.env, &code1), 1);
            assert_eq!(read_redemptions(&f.env, &code2), 0);
        });
    }

    #[test]
    fn test_apply_coupon_not_found_leaves_state_unchanged() {
        let f = setup_fixture();
        let non_existent_code = Symbol::new(&f.env, "GHOST");

        f.env.as_contract(&f.contract_id, || {
            let res = apply_coupon(
                &f.env,
                f.subscriber.clone(),
                f.sub_id,
                non_existent_code.clone(),
            );
            assert_eq!(res, Err(Error::CouponNotFound));

            // State unchanged
            assert!(!f.env.storage().persistent().has(&DataKey::SubCoupon(f.sub_id)));
            assert_eq!(read_redemptions(&f.env, &non_existent_code), 0);
        });
    }

    #[test]
    fn test_apply_coupon_merchant_mismatch_rejected() {
        let f = setup_fixture();
        let other_merchant = Address::generate(&f.env);
        let code = Symbol::new(&f.env, "OTHER_M");

        f.env.as_contract(&f.contract_id, || {
            let coupon = create_test_coupon(&other_merchant, &code, &f.token);
            write_coupon(&f.env, &coupon);

            let res = apply_coupon(&f.env, f.subscriber.clone(), f.sub_id, code.clone());
            assert_eq!(res, Err(Error::Unauthorized));

            // State unchanged
            assert!(!f.env.storage().persistent().has(&DataKey::SubCoupon(f.sub_id)));
            assert_eq!(read_redemptions(&f.env, &code), 0);
        });
    }

    #[test]
    fn test_apply_coupon_revoked_rejected() {
        let f = setup_fixture();
        let code = Symbol::new(&f.env, "REVOKED");

        f.env.as_contract(&f.contract_id, || {
            let mut coupon = create_test_coupon(&f.merchant, &code, &f.token);
            coupon.revoked = true;
            write_coupon(&f.env, &coupon);

            let res = apply_coupon(&f.env, f.subscriber.clone(), f.sub_id, code.clone());
            assert_eq!(res, Err(Error::CouponRevoked));

            // State unchanged
            assert!(!f.env.storage().persistent().has(&DataKey::SubCoupon(f.sub_id)));
            assert_eq!(read_redemptions(&f.env, &code), 0);
        });
    }

    #[test]
    fn test_apply_coupon_expiration_boundaries() {
        // 1. expires_at == 0 (never expires)
        let f1 = setup_fixture();
        let code_no_exp = Symbol::new(&f1.env, "NO_EXP");
        f1.env.as_contract(&f1.contract_id, || {
            let coupon = create_test_coupon(&f1.merchant, &code_no_exp, &f1.token);
            write_coupon(&f1.env, &coupon);

            // Advance ledger far into the future
            f1.env.ledger().set_timestamp(1_000_000_000);
            assert_eq!(
                apply_coupon(&f1.env, f1.subscriber.clone(), f1.sub_id, code_no_exp.clone()),
                Ok(())
            );
            assert_eq!(read_redemptions(&f1.env, &code_no_exp), 1);
        });

        // 2. expires_at = 100 boundary: now < expires_at succeeds
        let f2 = setup_fixture();
        let code_exp = Symbol::new(&f2.env, "EXP_100");
        f2.env.as_contract(&f2.contract_id, || {
            let mut coupon = create_test_coupon(&f2.merchant, &code_exp, &f2.token);
            coupon.expires_at = 100;
            write_coupon(&f2.env, &coupon);

            f2.env.ledger().set_timestamp(99);
            assert_eq!(
                apply_coupon(&f2.env, f2.subscriber.clone(), f2.sub_id, code_exp.clone()),
                Ok(())
            );
            assert_eq!(read_redemptions(&f2.env, &code_exp), 1);
        });

        // 3. expires_at = 100 boundary: now == expires_at fails
        let f3 = setup_fixture();
        f3.env.as_contract(&f3.contract_id, || {
            let mut coupon = create_test_coupon(&f3.merchant, &code_exp, &f3.token);
            coupon.expires_at = 100;
            write_coupon(&f3.env, &coupon);

            f3.env.ledger().set_timestamp(100);
            assert_eq!(
                apply_coupon(&f3.env, f3.subscriber.clone(), f3.sub_id, code_exp.clone()),
                Err(Error::CouponExpired)
            );
            assert!(!f3.env.storage().persistent().has(&DataKey::SubCoupon(f3.sub_id)));
            assert_eq!(read_redemptions(&f3.env, &code_exp), 0);
        });

        // 4. expires_at = 100 boundary: now > expires_at fails
        let f4 = setup_fixture();
        f4.env.as_contract(&f4.contract_id, || {
            let mut coupon = create_test_coupon(&f4.merchant, &code_exp, &f4.token);
            coupon.expires_at = 100;
            write_coupon(&f4.env, &coupon);

            f4.env.ledger().set_timestamp(101);
            assert_eq!(
                apply_coupon(&f4.env, f4.subscriber.clone(), f4.sub_id, code_exp.clone()),
                Err(Error::CouponExpired)
            );
            assert!(!f4.env.storage().persistent().has(&DataKey::SubCoupon(f4.sub_id)));
            assert_eq!(read_redemptions(&f4.env, &code_exp), 0);
        });
    }

    #[test]
    fn test_apply_coupon_redemption_limit_boundaries() {
        // Case 1: max_redemptions == 0 (unlimited)
        let f1 = setup_fixture();
        let code_unlimited = Symbol::new(&f1.env, "UNLIMITED");
        f1.env.as_contract(&f1.contract_id, || {
            let mut coupon = create_test_coupon(&f1.merchant, &code_unlimited, &f1.token);
            coupon.max_redemptions = 0; // unlimited
            write_coupon(&f1.env, &coupon);
            // Simulate 50 prior redemptions
            write_redemptions(&f1.env, &code_unlimited, 50);

            assert_eq!(
                apply_coupon(&f1.env, f1.subscriber.clone(), f1.sub_id, code_unlimited.clone()),
                Ok(())
            );
            assert_eq!(read_redemptions(&f1.env, &code_unlimited), 51);
        });

        // Case 2: max_redemptions == 1, current count == 0 -> succeeds
        let f2 = setup_fixture();
        let code_limit_1 = Symbol::new(&f2.env, "LIMIT1");
        f2.env.as_contract(&f2.contract_id, || {
            let mut coupon = create_test_coupon(&f2.merchant, &code_limit_1, &f2.token);
            coupon.max_redemptions = 1;
            write_coupon(&f2.env, &coupon);

            assert_eq!(
                apply_coupon(&f2.env, f2.subscriber.clone(), f2.sub_id, code_limit_1.clone()),
                Ok(())
            );
            assert_eq!(read_redemptions(&f2.env, &code_limit_1), 1);
        });

        // Case 3: max_redemptions == 1, current count == 1 (limit reached) -> fails
        let f3 = setup_fixture();
        f3.env.as_contract(&f3.contract_id, || {
            let mut coupon = create_test_coupon(&f3.merchant, &code_limit_1, &f3.token);
            coupon.max_redemptions = 1;
            write_coupon(&f3.env, &coupon);
            write_redemptions(&f3.env, &code_limit_1, 1);

            assert_eq!(
                apply_coupon(&f3.env, f3.subscriber.clone(), f3.sub_id, code_limit_1.clone()),
                Err(Error::CouponRedemptionLimitReached)
            );
            // State unchanged
            assert!(!f3.env.storage().persistent().has(&DataKey::SubCoupon(f3.sub_id)));
            assert_eq!(read_redemptions(&f3.env, &code_limit_1), 1);
        });

        // Case 4: max_redemptions == 2, current count == 2 (limit reached) -> fails
        let f4 = setup_fixture();
        let code_limit_2 = Symbol::new(&f4.env, "LIMIT2");
        f4.env.as_contract(&f4.contract_id, || {
            let mut coupon = create_test_coupon(&f4.merchant, &code_limit_2, &f4.token);
            coupon.max_redemptions = 2;
            write_coupon(&f4.env, &coupon);
            write_redemptions(&f4.env, &code_limit_2, 2);

            assert_eq!(
                apply_coupon(&f4.env, f4.subscriber.clone(), f4.sub_id, code_limit_2.clone()),
                Err(Error::CouponRedemptionLimitReached)
            );
            assert!(!f4.env.storage().persistent().has(&DataKey::SubCoupon(f4.sub_id)));
            assert_eq!(read_redemptions(&f4.env, &code_limit_2), 2);
        });
    }

    #[test]
    fn test_apply_coupon_token_mismatch_rejected() {
        let f = setup_fixture();
        let other_token = Address::generate(&f.env);
        let code = Symbol::new(&f.env, "OTHER_TOK");

        f.env.as_contract(&f.contract_id, || {
            let coupon = create_test_coupon(&f.merchant, &code, &other_token);
            write_coupon(&f.env, &coupon);

            let res = apply_coupon(&f.env, f.subscriber.clone(), f.sub_id, code.clone());
            assert_eq!(res, Err(Error::CouponTokenMismatch));

            // State unchanged
            assert!(!f.env.storage().persistent().has(&DataKey::SubCoupon(f.sub_id)));
            assert_eq!(read_redemptions(&f.env, &code), 0);
        });
    }

    #[test]
    fn test_apply_coupon_boundary_subscription_id_zero() {
        let f = setup_fixture();
        let code = Symbol::new(&f.env, "ZERO_ID");

        f.env.as_contract(&f.contract_id, || {
            let sub0 = create_test_sub(&f.subscriber, &f.merchant, &f.token);
            f.env.storage().persistent().set(&DataKey::Sub(0u32), &sub0);

            let coupon = create_test_coupon(&f.merchant, &code, &f.token);
            write_coupon(&f.env, &coupon);

            let res = apply_coupon(&f.env, f.subscriber.clone(), 0u32, code.clone());
            assert_eq!(res, Ok(()));
            assert_eq!(
                f.env.storage().persistent().get(&DataKey::SubCoupon(0u32)),
                Some(code.clone())
            );
            assert_eq!(read_redemptions(&f.env, &code), 1);
        });
    }

    #[test]
    fn test_apply_coupon_symbol_boundary_lengths() {
        // Short 1-char symbol
        let f1 = setup_fixture();
        let short_sym = Symbol::new(&f1.env, "X");
        f1.env.as_contract(&f1.contract_id, || {
            let coupon = create_test_coupon(&f1.merchant, &short_sym, &f1.token);
            write_coupon(&f1.env, &coupon);
            assert_eq!(apply_coupon(&f1.env, f1.subscriber.clone(), f1.sub_id, short_sym.clone()), Ok(()));
            assert_eq!(f1.env.storage().persistent().get(&DataKey::SubCoupon(f1.sub_id)), Some(short_sym));
        });

        // 9-char symbol (Soroban short symbol boundary)
        let f2 = setup_fixture();
        let nine_sym = Symbol::new(&f2.env, "DISCOUNT9");
        f2.env.as_contract(&f2.contract_id, || {
            let coupon = create_test_coupon(&f2.merchant, &nine_sym, &f2.token);
            write_coupon(&f2.env, &coupon);
            assert_eq!(apply_coupon(&f2.env, f2.subscriber.clone(), f2.sub_id, nine_sym.clone()), Ok(()));
            assert_eq!(f2.env.storage().persistent().get(&DataKey::SubCoupon(f2.sub_id)), Some(nine_sym));
        });

        // Symbol with numbers and underscore
        let f3 = setup_fixture();
        let num_sym = Symbol::new(&f3.env, "CPN_2026");
        f3.env.as_contract(&f3.contract_id, || {
            let coupon = create_test_coupon(&f3.merchant, &num_sym, &f3.token);
            write_coupon(&f3.env, &coupon);
            assert_eq!(apply_coupon(&f3.env, f3.subscriber.clone(), f3.sub_id, num_sym.clone()), Ok(()));
            assert_eq!(f3.env.storage().persistent().get(&DataKey::SubCoupon(f3.sub_id)), Some(num_sym));
        });
    }

    #[test]
    fn test_discount_math_and_charge_integration() {
        let f = setup_fixture();
        let code = Symbol::new(&f.env, "DISCOUNT");

        f.env.as_contract(&f.contract_id, || {
            // 20% + 50 fixed off
            let mut coupon = create_test_coupon(&f.merchant, &code, &f.token);
            coupon.percent_off_bps = 2000; // 20%
            coupon.fixed_off = 50;
            coupon.expires_at = 1000;
            write_coupon(&f.env, &coupon);

            apply_coupon(&f.env, f.subscriber.clone(), f.sub_id, code.clone()).unwrap();

            let gross = 1_000i128;
            // 1000 * 0.8 = 800; 800 - 50 = 750 payable; discount = 250
            let (payable, discount) = apply_discount_at_charge(
                &f.env,
                f.sub_id,
                500, // valid timestamp
                &f.token,
                gross,
            );
            assert_eq!(payable, 750);
            assert_eq!(discount, 250);

            // Now test silently skipping expired coupon at charge time
            let (payable_exp, discount_exp) = apply_discount_at_charge(
                &f.env,
                f.sub_id,
                1000, // expired timestamp
                &f.token,
                gross,
            );
            assert_eq!(payable_exp, gross);
            assert_eq!(discount_exp, 0);
        });
    }
}

// ═════════════════════════════════════════════════════════════════════════════
//  Adversarial coverage for `create_coupon`
//
//  The happy path, same-merchant duplicate rejection and `revoke_coupon` are
//  covered by `test_coupon.rs`.  This module pins the *validation boundaries*
//  around `create_coupon` instead: the inclusive/exclusive edges of each
//  range check, the global (not per-merchant) coupon-code namespace, the fact
//  that revocation does not free a code, and the guarantee that a rejected
//  call leaves no coupon behind.
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod create_coupon_adversarial_tests {
    use super::*;
    use crate::test_utils::{advance_ledger_by, create_test_client, setup_env};
    use crate::SubscriptionVaultClient;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::Address;

    fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
        let env = setup_env();
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        let client = create_test_client(&env, &admin, &token);
        (env, client, admin, token)
    }

    fn sym(env: &Env, text: &str) -> Symbol {
        Symbol::new(env, text)
    }

    // ── percent_off_bps boundary: 0..=10_000 inclusive ──────────────────────

    #[test]
    fn percent_off_accepts_the_inclusive_upper_bound_and_rejects_one_more() {
        let (env, client, _admin, token) = setup();
        let merchant = Address::generate(&env);

        let at_max = sym(&env, "PCT_MAX");
        client
            .mock_all_auths()
            .create_coupon(&merchant, &at_max, &token, &10_000, &0, &0, &0);
        assert_eq!(client.get_coupon(&at_max).unwrap().percent_off_bps, 10_000);

        let over_max = sym(&env, "PCT_OVER");
        let res =
            client.try_create_coupon(&merchant, &over_max, &token, &10_001, &0, &0, &0);
        assert_eq!(
            res.err().unwrap().unwrap().to_code(),
            Error::InvalidInput.to_code()
        );
        assert!(
            client.get_coupon(&over_max).is_none(),
            "a rejected percentage must not persist a coupon"
        );

        // u32::MAX is also out of range and must not wrap into validity.
        let wrapped = sym(&env, "PCT_U32MAX");
        let res = client.try_create_coupon(&merchant, &wrapped, &token, &u32::MAX, &0, &0, &0);
        assert_eq!(
            res.err().unwrap().unwrap().to_code(),
            Error::InvalidInput.to_code()
        );
        assert!(client.get_coupon(&wrapped).is_none());
    }

    #[test]
    fn percent_off_zero_is_accepted_as_no_percentage_discount() {
        let (env, client, _admin, token) = setup();
        let merchant = Address::generate(&env);
        let code = sym(&env, "PCT_ZERO");

        client
            .mock_all_auths()
            .create_coupon(&merchant, &code, &token, &0, &0, &0, &0);
        assert_eq!(client.get_coupon(&code).unwrap().percent_off_bps, 0);
    }

    // ── fixed_off boundary: >= 0 ────────────────────────────────────────────

    #[test]
    fn fixed_off_accepts_zero_and_rejects_any_negative_value() {
        let (env, client, _admin, token) = setup();
        let merchant = Address::generate(&env);

        let zero = sym(&env, "FIX_ZERO");
        client
            .mock_all_auths()
            .create_coupon(&merchant, &zero, &token, &0, &0, &0, &0);
        assert_eq!(client.get_coupon(&zero).unwrap().fixed_off, 0);

        // -1 is the closest negative value to the boundary.
        let minus_one = sym(&env, "FIX_NEG1");
        let res = client.try_create_coupon(&merchant, &minus_one, &token, &0, &-1, &0, &0);
        assert_eq!(
            res.err().unwrap().unwrap().to_code(),
            Error::InvalidInput.to_code()
        );
        assert!(client.get_coupon(&minus_one).is_none());

        // i128::MIN must also be rejected without overflowing the check.
        let min = sym(&env, "FIX_MIN");
        let res =
            client.try_create_coupon(&merchant, &min, &token, &0, &i128::MIN, &0, &0);
        assert_eq!(
            res.err().unwrap().unwrap().to_code(),
            Error::InvalidInput.to_code()
        );
        assert!(client.get_coupon(&min).is_none());
    }

    // ── expires_at boundary: 0 == never, otherwise strictly in the future ────

    #[test]
    fn expires_at_zero_means_no_expiry_and_is_accepted_at_any_ledger_time() {
        let (env, client, _admin, token) = setup();
        let merchant = Address::generate(&env);
        let code = sym(&env, "EXP_NEVER");

        advance_ledger_by(&env, 1_000_000);
        client
            .mock_all_auths()
            .create_coupon(&merchant, &code, &token, &0, &0, &0, &0);
        assert_eq!(client.get_coupon(&code).unwrap().expires_at, 0);
    }

    #[test]
    fn expires_at_must_be_strictly_after_the_current_ledger_time() {
        let (env, client, _admin, token) = setup();
        let merchant = Address::generate(&env);
        advance_ledger_by(&env, 1_000);
        // `setup_env` starts at ledger timestamp 0, so the advance above puts
        // `now` exactly at 1_000.
        let now = 1_000u64;

        // Exactly `now` is rejected: the guard is `expires_at <= now`.
        let at_now = sym(&env, "EXP_NOW");
        let res = client.try_create_coupon(&merchant, &at_now, &token, &0, &0, &0, &now);
        assert_eq!(
            res.err().unwrap().unwrap().to_code(),
            Error::InvalidInput.to_code()
        );
        assert!(client.get_coupon(&at_now).is_none());

        // One second in the past is rejected as well.
        let past = sym(&env, "EXP_PAST");
        let res = client.try_create_coupon(&merchant, &past, &token, &0, &0, &0, &(now - 1));
        assert_eq!(
            res.err().unwrap().unwrap().to_code(),
            Error::InvalidInput.to_code()
        );

        // One second in the future is the first accepted value.
        let future = sym(&env, "EXP_NEXT");
        client
            .mock_all_auths()
            .create_coupon(&merchant, &future, &token, &0, &0, &0, &(now + 1));
        assert_eq!(client.get_coupon(&future).unwrap().expires_at, now + 1);

        // u64::MAX is in the future for any realistic ledger and is accepted.
        let far = sym(&env, "EXP_MAX");
        client
            .mock_all_auths()
            .create_coupon(&merchant, &far, &token, &0, &0, &0, &u64::MAX);
        assert_eq!(client.get_coupon(&far).unwrap().expires_at, u64::MAX);
    }

    // ── max_redemptions: 0 == unlimited ─────────────────────────────────────

    #[test]
    fn max_redemptions_zero_is_accepted_as_unlimited() {
        let (env, client, _admin, token) = setup();
        let merchant = Address::generate(&env);
        let code = sym(&env, "MAX_RED0");

        client
            .mock_all_auths()
            .create_coupon(&merchant, &code, &token, &0, &0, &0, &0);
        assert_eq!(client.get_coupon(&code).unwrap().max_redemptions, 0);
    }

    #[test]
    fn all_fields_round_trip_through_storage_for_a_boundary_heavy_coupon() {
        let (env, client, _admin, token) = setup();
        let merchant = Address::generate(&env);
        let code = sym(&env, "ALL_MAX");
        advance_ledger_by(&env, 500);

        client.mock_all_auths().create_coupon(
            &merchant,
            &code,
            &token,
            &10_000,
            &i128::MAX,
            &u32::MAX,
            &u64::MAX,
        );

        let stored = client.get_coupon(&code).unwrap();
        assert_eq!(stored.code, code);
        assert_eq!(stored.merchant, merchant);
        assert_eq!(stored.token, token);
        assert_eq!(stored.percent_off_bps, 10_000);
        assert_eq!(stored.fixed_off, i128::MAX);
        assert_eq!(stored.max_redemptions, u32::MAX);
        assert_eq!(stored.expires_at, u64::MAX);
        assert!(!stored.revoked);
    }

    // ── code namespace is global, not per-merchant ──────────────────────────

    #[test]
    fn coupon_codes_are_global_so_a_second_merchant_cannot_reuse_one() {
        let (env, client, _admin, token) = setup();
        let first = Address::generate(&env);
        let second = Address::generate(&env);
        let code = sym(&env, "SHARED");

        client
            .mock_all_auths()
            .create_coupon(&first, &code, &token, &1_000, &0, &0, &0);

        let res = client.try_create_coupon(&second, &code, &token, &2_000, &0, &0, &0);
        assert_eq!(
            res.err().unwrap().unwrap().to_code(),
            Error::CouponAlreadyExists.to_code()
        );

        // The original record must be untouched by the rejected second create.
        let stored = client.get_coupon(&code).unwrap();
        assert_eq!(stored.merchant, first);
        assert_eq!(stored.percent_off_bps, 1_000);
    }

    #[test]
    fn a_revoked_code_still_occupies_the_namespace() {
        let (env, client, _admin, token) = setup();
        let merchant = Address::generate(&env);
        let code = sym(&env, "REVOKED");

        client
            .mock_all_auths()
            .create_coupon(&merchant, &code, &token, &1_000, &0, &0, &0);
        client.mock_all_auths().revoke_coupon(&merchant, &code);
        assert!(client.get_coupon(&code).unwrap().revoked);

        // Revocation flips a flag; it does not delete the record, so the code
        // cannot be recycled by the same or another merchant.
        let res = client.try_create_coupon(&merchant, &code, &token, &1_000, &0, &0, &0);
        assert_eq!(
            res.err().unwrap().unwrap().to_code(),
            Error::CouponAlreadyExists.to_code()
        );
        assert!(client.get_coupon(&code).unwrap().revoked);
    }

    // ── discount arithmetic at the validated extremes ────────────────────────

    #[test]
    fn a_full_percentage_discount_clamps_the_payable_amount_to_zero() {
        let (env, client, _admin, token) = setup();
        let merchant = Address::generate(&env);
        let code = sym(&env, "FREEBIE");

        // 100% off plus a positive fixed discount is accepted storage-wise;
        // the payable amount must still clamp at zero rather than go negative.
        client.mock_all_auths().create_coupon(
            &merchant,
            &code,
            &token,
            &10_000,
            &1_000,
            &0,
            &0,
        );
        let coupon = client.get_coupon(&code).unwrap();

        let gross = 7_777i128;
        let discount = compute_discount(gross, &coupon);
        assert_eq!(discount, gross);
        assert_eq!(gross - discount, 0);
    }

    #[test]
    fn a_zero_discount_coupon_leaves_the_gross_untouched() {
        let (env, client, _admin, token) = setup();
        let merchant = Address::generate(&env);
        let code = sym(&env, "NOOP");

        client
            .mock_all_auths()
            .create_coupon(&merchant, &code, &token, &0, &0, &0, &0);
        let coupon = client.get_coupon(&code).unwrap();

        assert_eq!(compute_discount(1_234i128, &coupon), 0);
    }

    #[test]
    fn a_fixed_discount_larger_than_the_gross_is_clamped_to_the_gross() {
        let (env, client, _admin, token) = setup();
        let merchant = Address::generate(&env);
        let code = sym(&env, "HUGE_FIX");

        client.mock_all_auths().create_coupon(
            &merchant,
            &code,
            &token,
            &0,
            &1_000_000,
            &0,
            &0,
        );
        let coupon = client.get_coupon(&code).unwrap();

        let gross = 25i128;
        assert_eq!(compute_discount(gross, &coupon), gross);
    }
}