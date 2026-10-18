//! Adversarial coverage for `coupon::compute_discount`.
//!
//! `compute_discount` is a public, `env`-free helper. `create_coupon` validates
//! `percent_off_bps <= 10_000` and `fixed_off >= 0`, but the function itself
//! accepts any `Coupon`, so the interesting cases are the ones the happy-path
//! suite does not reach:
//!
//! * the documented invariant `0 <= discount <= gross` for *every* `i128`
//!   `gross`, including `0`, negatives and `i128::MAX`/`i128::MIN`;
//! * floor-division rounding at the payable/fixed ordering boundary;
//! * out-of-band coupon fields (percentage above 100%, negative fixed amount)
//!   which must degrade deterministically instead of overflowing the
//!   intermediate `gross * bps` product.

use crate::coupon::compute_discount;
use crate::types::Coupon;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, Symbol};

fn coupon(env: &Env, percent_off_bps: u32, fixed_off: i128) -> Coupon {
    let owner = Address::generate(env);
    Coupon {
        code: Symbol::new(env, "ADVERSARIAL"),
        merchant: owner.clone(),
        token: owner,
        percent_off_bps,
        fixed_off,
        max_redemptions: 0,
        expires_at: 0,
        revoked: false,
    }
}

/// The invariant `compute_discount` advertises in its doc comment.
#[track_caller]
fn assert_invariant(gross: i128, discount: i128) {
    assert!(discount >= 0, "discount {discount} is negative");
    assert!(
        discount <= gross,
        "discount {discount} exceeds gross {gross}"
    );
}

// ════════════════════════════════════════════════════════════════════
//  Non-positive gross
// ════════════════════════════════════════════════════════════════════

#[test]
fn zero_gross_yields_no_discount_for_every_configured_coupon() {
    let env = Env::default();

    for (bps, fixed) in [(0u32, 0i128), (1, 0), (5000, 250), (10_000, 0), (10_000, i128::MAX)] {
        let c = coupon(&env, bps, fixed);
        let discount = compute_discount(0, &c);
        assert_eq!(discount, 0, "bps={bps} fixed={fixed}");
        assert_invariant(0, discount);
    }
}

#[test]
fn negative_gross_never_produces_a_negative_discount() {
    let env = Env::default();

    for bps in [0u32, 1, 2500, 10_000] {
        for gross in [-1i128, -1_000, i128::MIN] {
            let c = coupon(&env, bps, 100);
            let discount = compute_discount(gross, &c);
            assert!(discount >= 0, "bps={bps} gross={gross} -> {discount}");
        }
    }
}

// ════════════════════════════════════════════════════════════════════
//  Percentage semantics
// ════════════════════════════════════════════════════════════════════

#[test]
fn zero_percent_coupon_discounts_nothing() {
    let env = Env::default();
    let c = coupon(&env, 0, 0);

    for gross in [0i128, 1, 1_000, i128::MAX] {
        assert_eq!(compute_discount(gross, &c), 0);
    }
}

#[test]
fn one_basis_point_is_not_rounded_away_on_a_large_enough_charge() {
    let env = Env::default();
    let c = coupon(&env, 1, 0);

    assert_eq!(compute_discount(10_000, &c), 1);
    assert_eq!(compute_discount(999_999, &c), 100);
}

#[test]
fn full_percent_off_is_capped_at_the_gross_amount() {
    let env = Env::default();
    let c = coupon(&env, 10_000, 0);

    for gross in [1i128, 1_000, i128::MAX] {
        assert_eq!(compute_discount(gross, &c), gross);
    }
}

#[test]
fn percentage_discount_floors_the_payable_amount() {
    let env = Env::default();

    // 10% off 1001: payable floors to 900 (900.9), so the discount rounds up to 101.
    let c = coupon(&env, 1_000, 0);
    assert_eq!(compute_discount(1_001, &c), 101);

    // 20% off 1: payable floors to 0, the whole charge is discounted.
    let c = coupon(&env, 2_000, 0);
    assert_eq!(compute_discount(1, &c), 1);
}

// ════════════════════════════════════════════════════════════════════
//  Fixed semantics and ordering
// ════════════════════════════════════════════════════════════════════

#[test]
fn fixed_discount_is_exact_and_clamped_to_the_gross() {
    let env = Env::default();

    assert_eq!(compute_discount(1_000, &coupon(&env, 0, 300)), 300);
    // Fixed amount exactly equal to the charge.
    assert_eq!(compute_discount(1_000, &coupon(&env, 0, 1_000)), 1_000);
    // Fixed amount above the charge saturates at the charge, never more.
    assert_eq!(compute_discount(1_000, &coupon(&env, 0, 1_001)), 1_000);
    assert_eq!(compute_discount(1_000, &coupon(&env, 0, i128::MAX)), 1_000);
}

#[test]
fn percentage_is_applied_before_the_fixed_amount() {
    let env = Env::default();

    // 1000 -> 800 payable, then 100 off -> 700 payable, i.e. 300 discount.
    assert_eq!(compute_discount(1_000, &coupon(&env, 2_000, 100)), 300);

    // Ordering proof: applying the fixed amount first would yield
    // (1000 - 900) * 0.8 = 80 payable, i.e. a discount of 920, not 1000.
    assert_eq!(compute_discount(1_000, &coupon(&env, 2_000, 900)), 1_000);
}

// ════════════════════════════════════════════════════════════════════
//  Out-of-band coupon fields (defence in depth)
// ════════════════════════════════════════════════════════════════════

#[test]
fn percentage_above_one_hundred_is_clamped_to_a_full_discount() {
    let env = Env::default();

    for bps in [10_001u32, 20_000, u32::MAX] {
        let c = coupon(&env, bps, 0);
        for gross in [1i128, 1_000, i128::MAX] {
            let discount = compute_discount(gross, &c);
            assert_eq!(discount, gross, "bps={bps} gross={gross}");
        }
    }
}

#[test]
fn a_negative_fixed_amount_is_clamped_to_no_discount() {
    let env = Env::default();
    let c = coupon(&env, 0, -1);

    let discount = compute_discount(1_000, &c);
    assert_eq!(discount, 0);
    assert_invariant(1_000, discount);
}

// ════════════════════════════════════════════════════════════════════
//  i128 boundaries
// ════════════════════════════════════════════════════════════════════

#[test]
fn extreme_gross_with_a_percentage_coupon_neither_overflows_nor_wraps() {
    let env = Env::default();

    for bps in [1u32, 2_500, 5_000, 9_999] {
        let c = coupon(&env, bps, 0);
        let discount = compute_discount(i128::MAX, &c);
        assert_invariant(i128::MAX, discount);
        // A 100% - bps/10000 payable must be roughly bps of the gross.
        let expected = (i128::MAX / 10_000) * bps as i128;
        assert!(
            (discount - expected).abs() <= 10_000,
            "bps={bps}: discount {discount} vs expected ~{expected}"
        );
    }
}

#[test]
fn extreme_gross_with_a_fixed_discount_saturates_at_the_gross() {
    let env = Env::default();

    for fixed in [1i128, i128::MAX / 2, i128::MAX] {
        let c = coupon(&env, 0, fixed);
        let discount = compute_discount(i128::MAX, &c);
        assert_eq!(discount, fixed.min(i128::MAX));
        assert_invariant(i128::MAX, discount);
    }
}

#[test]
fn the_invariant_holds_across_a_cross_product_of_inputs() {
    let env = Env::default();

    let grosses = [
        0i128,
        1,
        7,
        9_999,
        10_000,
        10_001,
        1_000_000,
        i128::MAX / 10_000,
        i128::MAX,
    ];
    let percents: [u32; 6] = [0, 1, 1_000, 5_000, 9_999, 10_000];
    let fixed_amounts = [0i128, 1, 500, 10_000, i128::MAX];

    for gross in grosses {
        for bps in percents {
            for fixed in fixed_amounts {
                let c = coupon(&env, bps, fixed);
                let discount = compute_discount(gross, &c);
                assert_invariant(gross, discount);
                // The discount is exactly the amount the payable drops by.
                let payable = gross - discount;
                assert!((0..=gross).contains(&payable));
            }
        }
    }
}
