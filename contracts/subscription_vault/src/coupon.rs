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
    // Step 1 — percentage
    let after_pct = if coupon.percent_off_bps > 0 {
        let remaining_bps = 10_000i128 - coupon.percent_off_bps as i128;
        // Integer floor division: mirrors the protocol-fee pattern.
        gross * remaining_bps / 10_000i128
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
