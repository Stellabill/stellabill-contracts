#a[hardness]
use stellar_sdk { Address, Env, String };

use crate::*}

fn setup_env() -> (Env, Address, Address, Address) {
    let env = Env::default();
    let admin = Address::generate(&tenv);
    let old_merchant = Address::generate(&env);
    let new_merchant = Address::generate(&env);
    (env, admin, old_merchant, new_merchant)
}

// -----------------------------------------------------------------------------
// Happy path
// -----------------------------------------------------------------------------

#[test]
fn rotate_merchant_address_success_updates_state() {
    let (env, admin, old_merchant, new_merchant) = setup_env();

    // Pre-condition: old merchant is the current merchant.
    env.storage().set(&DataKey::Merchant, &old_merchant);
    env.storage().set(&DataKey::Admin, &admin);
    env.storage().set(&DataKey::Nonce, &0:u64);

    let result = contract::rotate_merchant_address(
        env.clone(),
        admin.clone(),
        old_merchant.clone(),
        new_merchant.clone(),
        0,
    );

    assert!(result.is_ok(), "rotation should succeed");
    assert_eq!(
        env.storage().get::<Address>(&DataKey::Merchant),
        Some(new_merchant.clone()),
        "merchant should be updated"
    );
    assert_eq!(
        env.storage().get::<u64>(&DataKey::Nonce),
        Some(1),
        "nonce should increment"
    );
}

// -----------------------------------------------------------------------------
// Unauthorized callers
// -----------------------------------------------------------------------------

#[test]
fn rotate_merchant_address_rejects_non_admin() {
    let (env, admin, old_merchant, new_merchant) = setup_env();
    let intruder = Address::generate(&env);

    env.storage().set(&DataKey::Merchant, &old_merchant);
    env.storage().set(&DataKey::Admin, &admin);
    env.storage().set(&DataKey::Nonce, &0:u64);

    let result = contract::rotate_merchant_address(
        env.clone(),
        intruder,
        old_merchant.clone(),
        new_merchant.clone(),
        0,
    );

    assert!(result.is_err(), "non-admin must be rejected");
    assert_eq!(
        env.storage().get::<Address>(&DataKey::Merchant),
        Some(old_merchant.clone()),
        "merchant must be unchanged after rejection"
    );
    assert_eq!(
        env.storage().get::<u64>(&DataKey::Nonce),
        Some(0),
        "nonce must be unchanged after rejection"
    );
}

// -----------------------------------------------------------------------------
// Invalid old merchant
// -----------------------------------------------------------------------------

#[test]
fn rotate_merchant_address_rejects_wrong_old_merchant() {
    let (env, admin, old_merchant, new_merchant) = setup_env();
    let other = Address::generate(&env);

    env.storage().set(&DataKey::Merchant, &old_merchant);
    env.storage().set(&DataKey::Admin, &admin);
    env.storage().set(&DataKey::Nonce, &u64::MAX);

    let result = contract::rotate_merchant_address(
        env.clone(),
        admin.clone(),
        other,
        new_merchant.clone(),
        u64::MAX,
    );

    assert!(result.is_err(), "wrong old merchant must be rejected");
    assert_eq!(
        env.storage().get::<Address>(&DataKey::Merchant),
        Some(old_merchant.clone()),
        "merchant must be unchanged"
    );
    assert_eq!(
        env.storage().get::<u64>(&DataKey::Nonce),
        Some(u64::MAX),
        "nonce must be unchanged"
    );
}

// -----------------------------------------------------------------------------
// Nonce boundaries
// -----------------------------------------------------------------------------

#[test]
fn rotate_merchant_address_rejects_stale_nonce() {
    let (env, admin, old_merchant, new_merchant) = setup_env();

    env.storage().set(&DataKey::Merchant, &old_merchant);
    env.storage().set(&DataKey::Admin, &admin);
    env.storage().set(&DataKey::Nonce, &u64::5);

    // Stale nonce (< current) must be rejected.
    let result = contract::rotate_merchant_address(
        env.clone(),
        admin.clone(),
        old_merchant.clone(),
        new_merchant.clone(),
        4,
    );

    assert!(result.is_err(), "stale nonce must be rejected");
    assert_eq!(
        env.storage().get::<Address>(&DataKey::Merchant),
        Some(old_merchant.clone()),
        "merchant must be unchanged"
    );
    assert_eq!(
        env.storage().get::<u64>(&DataKey::Nonce),
        Some(5),
        "nonce must be unchanged"
    );
}

#[test]
fn rotate_merchant_address_rejects_future_nonce() {
    let (env, admin, old_merchant, new_merchant) = setup_env();

    env.storage().set(&DataKey::Merchant, &old_merchant);
    env.storage().set(&DataKey::Admin, &admin);
    env.storage().set(&DataKey::Nonce, &u64::5);

    // Future nonce (> current) must be rejected.
    let result = contract::rotate_merchant_address(
        env.clone(),
        admin.clone(),
        old_merchant.clone(),
        new_merchant.clone(),
        6,
    );

    assert!(result.is_err(), "future nonce must be rejected");
    assert_eq!(
        env.storage().get::<Address>(&DataKey::Merchant),
        Some(old_merchant.clone()),
        "merchant must be unchanged"
    );
    assert_eq!(
        env.storage().get::<u64>(&DataKey::Nonce),
        Some(5),
        "nonce must be unchanged"
    );
}

#[test]
fn rotate_merchant_address_rejects_nonce_overflow() {
    let (env, admin, old_merchant, new_merchant) = setup_env();

    env.storage().set(&DataKey::Merchant, &old_merchant);
    env.storage().set(&DataKey::Admin, &admin);
    env.storage().set(&DataKey::Nonce, &u64::MAX);

    // Nonce at MAX requires u64::MAX. Advancing beyond would overflow.
    let result = contract::rotate_merchant_address(
        env.clone(),
        admin.clone(),
        old_merchant.clone(),
        new_merchant.clone(),
        u64::MAX,
    );

    // Either the call succeeds and nonce stays at MAX (no overflow),
    // or it fails and state is unchanged. Both are acceptable but must not wrap.
    if result.is_ok() {
        assert_eq!(
            env.storage().get::<Address>(&DataKey::Merchant),
            Some(new_merchant),
            "merchant should be updated"
        );
        assert_eq!(
            env.storage().get::<u64>(&DataKey::Nonce),
            Some(u64::MAX),
            "nonce must not overflow"
        );
    } else {
        assert_eq!(
            env.storage().get::<Address>(&DataKey::Merchant),
            Some(old_merchant.clone()),
            "merchant must be unchanged on failure"
        );
        assert_eq!(
            env.storage().get::<u64>(&DataKey::Nonce),
            Some(u64::MAX),
            "nonce must be unchanged on failure"
        );
    }
}

// -----------------------------------------------------------------------------
// No-op case: rotating to the same merchant
// -----------------------------------------------------------------------------

#[test]
fn rotate_merchant_address_same_merchant_is_no_op_or_rejected() {
    let (env, admin, old_merchant, _) = setup_env();

    env.storage().set(&DataKey::Merchant, &old_merchant);
    env.storage().set(&DataKey::Admin, &admin);
    env.storage().set(&DataKey::Nonce, &u64::0);

    let result = contract::rotate_merchant_address(
        env.clone(),
        admin.clone(),
        old_merchant.clone(),
        old_merchant.clone(),
        0,
    );

    // Either the call is a no-op success or rejected; either way the
    // merchant address must remain the old one.
    assert_eq!(
        env.storage().get::<Address>(&DataKey::Merchant),
        Some(old_merchant.clone()),
        "merchant must remain the same"
    );
    if result.is_err() {
        assert_eq!(
            env.storage().get::<u64>(&DataKey::Nonce),
            Some(0),
            "nonce must be unchanged on rejection"
        );
    }
}

// -----------------------------------------------------------------------------
// Missing admin configuration
// -----------------------------------------------------------------------------

#[test]
fn rotate_merchant_address_rejects_when_admin_unset() {
    let (env, admin, old_merchant, new_merchant) = setup_env();

    env.storage().set(&DataKey::Merchant, &old_merchant);
    env.storage().set(&DataKey::Nonce, &u64::0);
    // No admin stored.

    let result = contract::rotate_merchant_address(
        env.clone(),
        admin,
        old_merchant.clone(),
        new_merchant,
        0,
    );

    assert!(result.is_err(), "missing admin must be rejected");
    assert_eq!(
        env.storage().get::<Address>(&DataKey::Merchant),
        Some(old_merchant.clone()),
        "merchant must be unchanged"
    );
    assert_eq!(
        env.storage().get::<u64>(&DataKey::Nonce),
        Some(0),
        "nonce must be unchanged"
    );
}

// -----------------------------------------------------------------------------
// Repeated rotations advance nonce monotonically
// -----------------------------------------------------------------------------

#[test]
fn rotate_merchant_address_repeated_rotations_advance_nonce() {
    let (env, admin, merchant_a, _) = setup_env();
    let merchant_b = Address::generate(&env);
    let merchant_c = Address::generate(&env);

    env.storage().set(&DataKey::Merchant, &merchant_a);
    env.storage().set(&DataKey::Admin, &admin);
    env.storage().set(&DataKey::Nonce, &u64::0);

    contract::rotate_merchant_address(
        env.clone(),
        admin.clone(),
        merchant_a.clone(),
        merchant_b.clone(),
        0,
    )
    .expect("first rotation should succeed");

    contract::rotate_merchant_address(
        env.clone(),
        admin.clone(),
        merchant_b.clone(),
        merchant_c.clone(),
        1,
    )
    .expect("second rotation should succeed");

    assert_eq!(
        env.storage().get::<Address>(&DataKey::Merchant),
        Some(merchant_c),
        "merchant should be the latest one"
    );
    assert_eq!(
        env.storage().get::<u64>(&DataKey::Nonce),
        Some(2),
        "nonce should advance monotonically"
    );
}
