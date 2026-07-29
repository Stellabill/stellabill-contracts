use crate::test_utils::setup::TestEnv;
use crate::types::Error;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::Address;

const AMOUNT: i128 = 1_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;

/// Helper: initialise merchant config and return the merchant address.
fn init_merchant(test_env: &TestEnv) -> Address {
    let merchant = Address::generate(&test_env.env);
    test_env.client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0i32,
        &0x1Fi32,
        &None,
        &soroban_sdk::String::from_str(&test_env.env, "https://example.com"),
    );
    merchant
}

/// Helper: create an active subscription for a given subscriber and merchant.
fn create_sub(
    test_env: &TestEnv,
    subscriber: &Address,
    merchant: &Address,
) -> u32 {
    test_env.client.create_subscription_full(
        subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    )
}

/// Helper: create a subscription with a specific sub_account_label too.
#[allow(dead_code)]
fn create_sub_with_label(
    test_env: &TestEnv,
    subscriber: &Address,
    merchant: &Address,
    label: &soroban_sdk::Symbol,
) -> u32 {
    test_env.client.create_subscription_full(
        subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &Some(label.clone()),
    )
}

// ── Boundary: exactly cap creations succeed, cap + 1 fails ──────────────────

#[test]
fn cap_accepts_exactly_cap_creations_and_rejects_cap_plus_one() {
    let test_env = TestEnv::default();
    let admin = test_env.admin.clone();
    let merchant = init_merchant(&test_env);
    let subscriber = Address::generate(&test_env.env);

    // Admin sets per-merchant max to 3.
    test_env.client.set_merchant_max_subs(&admin, &merchant, &3u32);
    assert_eq!(test_env.client.get_merchant_max_subs(&merchant), 3);

    // Create 3 subscriptions — all should succeed.
    let id1 = create_sub(&test_env, &subscriber, &merchant);
    let id2 = create_sub(&test_env, &subscriber, &merchant);
    let id3 = create_sub(&test_env, &subscriber, &merchant);

    // Verify the merchant's subscription count.
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant),
        3
    );

    // 4th subscription should be rejected.
    let result = test_env.client.try_create_subscription_full(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert_eq!(result, Err(Ok(Error::MaxConcurrentSubscriptionsReached)));

    // Sanity — the three existing subs are untouched.
    let _sub1 = test_env.client.get_subscription(&id1);
    let _sub2 = test_env.client.get_subscription(&id2);
    let _sub3 = test_env.client.get_subscription(&id3);
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant),
        3
    );
}

// ── Edge case: cap increase after saturation ─────────────────────────────────

#[test]
fn cap_increase_after_saturation_allows_further_creations() {
    let test_env = TestEnv::default();
    let admin = test_env.admin.clone();
    let merchant = init_merchant(&test_env);
    let subscriber = Address::generate(&test_env.env);

    // Cap at 2.
    test_env.client.set_merchant_max_subs(&admin, &merchant, &2u32);

    // Fill both slots.
    let _id1 = create_sub(&test_env, &subscriber, &merchant);
    let _id2 = create_sub(&test_env, &subscriber, &merchant);

    // 3rd is blocked.
    let blocked = test_env.client.try_create_subscription_full(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert_eq!(blocked, Err(Ok(Error::MaxConcurrentSubscriptionsReached)));

    // Admin raises cap to 5.
    test_env.client.set_merchant_max_subs(&admin, &merchant, &5u32);
    assert_eq!(test_env.client.get_merchant_max_subs(&merchant), 5);

    // Now the 3rd subscription succeeds.
    let id3 = create_sub(&test_env, &subscriber, &merchant);
    let _sub3 = test_env.client.get_subscription(&id3);
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant),
        3
    );
}

// ── Edge case: cap decrease below current count ──────────────────────────────

#[test]
fn cap_decrease_below_current_count_does_not_evict_existing_subs() {
    let test_env = TestEnv::default();
    let admin = test_env.admin.clone();
    let merchant = init_merchant(&test_env);
    let subscriber = Address::generate(&test_env.env);

    // Cap at 5.
    test_env.client.set_merchant_max_subs(&admin, &merchant, &5u32);

    // Create 4 subscriptions.
    let id1 = create_sub(&test_env, &subscriber, &merchant);
    let id2 = create_sub(&test_env, &subscriber, &merchant);
    let id3 = create_sub(&test_env, &subscriber, &merchant);
    let id4 = create_sub(&test_env, &subscriber, &merchant);
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant),
        4
    );

    // Admin lowers cap to 2 (below current count of 4).
    test_env.client.set_merchant_max_subs(&admin, &merchant, &2u32);
    assert_eq!(test_env.client.get_merchant_max_subs(&merchant), 2);

    // Existing subscriptions are NOT evicted — they remain active.
    let _sub1 = test_env.client.get_subscription(&id1);
    let _sub2 = test_env.client.get_subscription(&id2);
    let _sub3 = test_env.client.get_subscription(&id3);
    let _sub4 = test_env.client.get_subscription(&id4);
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant),
        4
    );

    // New creation is blocked because count (4) >= cap (2).
    let blocked = test_env.client.try_create_subscription_full(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert_eq!(blocked, Err(Ok(Error::MaxConcurrentSubscriptionsReached)));
}

// ── Edge case: cancellation frees a slot ─────────────────────────────────────

#[test]
fn cancellation_frees_slot_within_cap() {
    let test_env = TestEnv::default();
    let admin = test_env.admin.clone();
    let merchant = init_merchant(&test_env);
    let subscriber = Address::generate(&test_env.env);

    // Cap at 1.
    test_env.client.set_merchant_max_subs(&admin, &merchant, &1u32);

    let id = create_sub(&test_env, &subscriber, &merchant);
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant),
        1
    );

    // Cancel the subscription.
    test_env.client.cancel_subscription(&id, &subscriber);

    // Count should drop to 0, freeing a slot.
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant),
        0
    );

    // New creation succeeds.
    let id2 = create_sub(&test_env, &subscriber, &merchant);
    let _sub2 = test_env.client.get_subscription(&id2);
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant),
        1
    );
}

// ── Edge case: cap = 0 blocks all creations ──────────────────────────────────

#[test]
fn cap_zero_blocks_all_creations() {
    let test_env = TestEnv::default();
    let admin = test_env.admin.clone();
    let merchant = init_merchant(&test_env);
    let subscriber = Address::generate(&test_env.env);

    // Cap at 0.
    test_env.client.set_merchant_max_subs(&admin, &merchant, &0u32);
    assert_eq!(test_env.client.get_merchant_max_subs(&merchant), 0);

    let blocked = test_env.client.try_create_subscription_full(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert_eq!(blocked, Err(Ok(Error::MaxConcurrentSubscriptionsReached)));
}

// ── Edge case: default (u32::MAX) allows many creations ──────────────────────

#[test]
fn default_cap_is_max_and_allows_creations() {
    let test_env = TestEnv::default();
    let merchant = init_merchant(&test_env);
    let subscriber = Address::generate(&test_env.env);

    // Default is u32::MAX.
    assert_eq!(
        test_env.client.get_merchant_max_subs(&merchant),
        u32::MAX
    );

    // Creating several subscriptions must always succeed.
    for _ in 0..5 {
        create_sub(&test_env, &subscriber, &merchant);
    }
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant),
        5
    );
}

// ── Edge case: cap of 1 (exact boundary) ─────────────────────────────────────

#[test]
fn cap_of_one_accepts_one_and_rejects_second() {
    let test_env = TestEnv::default();
    let admin = test_env.admin.clone();
    let merchant = init_merchant(&test_env);
    let subscriber = Address::generate(&test_env.env);

    test_env.client.set_merchant_max_subs(&admin, &merchant, &1u32);

    let id = create_sub(&test_env, &subscriber, &merchant);
    let _sub = test_env.client.get_subscription(&id);

    let blocked = test_env.client.try_create_subscription_full(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert_eq!(blocked, Err(Ok(Error::MaxConcurrentSubscriptionsReached)));
}

// ── Edge case: pause does NOT free a slot, cancel does ───────────────────────

#[test]
fn pause_does_not_free_slot_cancel_does() {
    let test_env = TestEnv::default();
    let admin = test_env.admin.clone();
    let merchant = init_merchant(&test_env);
    let subscriber = Address::generate(&test_env.env);

    test_env.client.set_merchant_max_subs(&admin, &merchant, &1u32);

    let id = create_sub(&test_env, &subscriber, &merchant);
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant),
        1
    );

    // Pause the subscription — count should remain at 1 (slot not freed).
    test_env.client.pause_subscription(&id, &subscriber);
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant),
        1
    );

    // Creation still blocked.
    let blocked = test_env.client.try_create_subscription_full(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert_eq!(blocked, Err(Ok(Error::MaxConcurrentSubscriptionsReached)));

    // Resume + Cancel — count should drop to 0.
    test_env.client.resume_subscription(&id, &subscriber);
    test_env.client.cancel_subscription(&id, &subscriber);
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant),
        0
    );

    // Now creation succeeds.
    let id2 = create_sub(&test_env, &subscriber, &merchant);
    let _sub2 = test_env.client.get_subscription(&id2);
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant),
        1
    );
}

// ── Edge case: multiple merchants with independent caps ──────────────────────

#[test]
fn independent_merchant_caps_do_not_interfere() {
    let test_env = TestEnv::default();
    let admin = test_env.admin.clone();
    let merchant_a = init_merchant(&test_env);
    let merchant_b = init_merchant(&test_env);
    let subscriber = Address::generate(&test_env.env);

    // Merchant A: cap = 1, Merchant B: cap = 3
    test_env.client.set_merchant_max_subs(&admin, &merchant_a, &1u32);
    test_env.client.set_merchant_max_subs(&admin, &merchant_b, &3u32);

    // Fill merchant A's cap.
    let _ida = create_sub(&test_env, &subscriber, &merchant_a);
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant_a),
        1
    );

    // Merchant A over-cap blocked.
    let blocked = test_env.client.try_create_subscription_full(
        &subscriber,
        &merchant_a,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert_eq!(blocked, Err(Ok(Error::MaxConcurrentSubscriptionsReached)));

    // Merchant B can still create; its cap is independent.
    let _idb1 = create_sub(&test_env, &subscriber, &merchant_b);
    let _idb2 = create_sub(&test_env, &subscriber, &merchant_b);
    let _idb3 = create_sub(&test_env, &subscriber, &merchant_b);
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant_b),
        3
    );

    // Merchant B over its cap of 3.
    let blocked_b = test_env.client.try_create_subscription_full(
        &subscriber,
        &merchant_b,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert_eq!(blocked_b, Err(Ok(Error::MaxConcurrentSubscriptionsReached)));
}

// ── Edge case: admin override — set and clear ────────────────────────────────

#[test]
fn admin_can_clear_merchant_max_subs_to_restore_default() {
    let test_env = TestEnv::default();
    let admin = test_env.admin.clone();
    let merchant = init_merchant(&test_env);
    let subscriber = Address::generate(&test_env.env);

    // Set restrictive cap.
    test_env.client.set_merchant_max_subs(&admin, &merchant, &1u32);
    assert_eq!(test_env.client.get_merchant_max_subs(&merchant), 1);

    let _id = create_sub(&test_env, &subscriber, &merchant);

    let blocked = test_env.client.try_create_subscription_full(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert_eq!(blocked, Err(Ok(Error::MaxConcurrentSubscriptionsReached)));

    // Clear the cap (back to u32::MAX default).
    test_env.client.set_merchant_max_subs(&admin, &merchant, &u32::MAX);
    assert_eq!(
        test_env.client.get_merchant_max_subs(&merchant),
        u32::MAX
    );

    // Now creation succeeds again.
    let id2 = create_sub(&test_env, &subscriber, &merchant);
    let _sub2 = test_env.client.get_subscription(&id2);
    assert_eq!(
        test_env.client.get_merchant_subscription_count(&merchant),
        2
    );
}
