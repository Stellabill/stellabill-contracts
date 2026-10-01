//! Adversarial coverage for `set_subscriber_create_cap` /
//! `do_set_subscriber_create_cap` (issue #987).
//!
//! `do_set_subscriber_create_cap` in
//! `contracts/subscription_vault/src/admin.rs` writes the *global* per-subscriber
//! creation-rate cap that `subscription::enforce_creation_rate_limit` reads on
//! every `create_subscription` call. It had no directly associated test
//! fixture: `test_subscriber_active_cap.rs` covers the unrelated per-subscriber
//! *concurrent* cap (`set_subscriber_active_cap`) and `test_datakey_layout.rs`
//! only asserts the storage discriminant.
//!
//! What is exercised here:
//!
//! * the default cap (50) reported before any admin call;
//! * admin-only mutation: a non-admin gets `Forbidden` and the stored cap is
//!   left untouched;
//! * the configured value round-trips through `get_subscriber_create_cap`,
//!   including the `u32::MAX` boundary and the `0` boundary;
//! * a `0` cap blocks creation *and* emits `rate_limit_tripped`, while leaving
//!   the subscription counter (`NextId`) untouched;
//! * the cap is enforced per subscriber over a rolling 24h window, resets at
//!   the exact window boundary, and applies immediately when lowered mid-window;
//! * the cap is per subscriber, not global;
//! * the documented exemption for the stored admin still bypasses the cap;
//! * every successful change emits `subscriber_create_cap_updated` carrying the
//!   new value.

use crate::test_utils::setup::TestEnv;
use crate::types::Error;
use soroban_sdk::{testutils::Address as _, testutils::Events as _, Address, IntoVal, Symbol, Val};

/// Mirrors the private `DEFAULT_CREATE_CAP` in `subscription.rs`.
const DEFAULT_CREATE_CAP: u32 = 50;
/// Mirrors the private `SECONDS_IN_DAY` in `subscription.rs`.
const SECONDS_IN_DAY: u64 = 86_400;
const INTERVAL: u64 = 24 * 60 * 60;
const AMOUNT: i128 = 1_000;
/// Comfortably above the vault minimum top-up (`TestEnv::default`).
const PREPAID: i128 = 1_000_000_000;

/// Create a subscription for `subscriber` in the `try_` form so the caller can
/// assert on the returned error instead of panicking.
fn try_create(
    te: &TestEnv,
    subscriber: &Address,
) -> Result<Result<u32, Error>, Result<Error, soroban_sdk::InvokeError>> {
    let merchant = Address::generate(&te.env);
    te.client.try_create_subscription(
        subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    )
}

/// Number of emitted events whose topics contain `name`.
fn topic_hits(te: &TestEnv, name: &str) -> u32 {
    let topic: Val = Symbol::new(&te.env, name).into_val(&te.env);
    let mut hits = 0u32;
    for (_contract, topics, _data) in te.env.events().all().iter() {
        if topics.contains(&topic) {
            hits += 1;
        }
    }
    hits
}

// ── Defaults & round-trip ───────────────────────────────────────────────────

#[test]
fn default_create_cap_is_fifty() {
    let te = TestEnv::default();

    assert_eq!(te.client.get_subscriber_create_cap(), DEFAULT_CREATE_CAP);
}

#[test]
fn admin_can_lower_and_raise_the_cap_and_the_value_round_trips() {
    let te = TestEnv::default();

    te.client.set_subscriber_create_cap(&te.admin, &3u32);
    assert_eq!(te.client.get_subscriber_create_cap(), 3);

    te.client
        .set_subscriber_create_cap(&te.admin, &(DEFAULT_CREATE_CAP * 4));
    assert_eq!(
        te.client.get_subscriber_create_cap(),
        DEFAULT_CREATE_CAP * 4
    );
}

#[test]
fn setting_the_cap_emits_an_event_carrying_the_new_value() {
    let te = TestEnv::default();

    te.client.set_subscriber_create_cap(&te.admin, &7u32);

    let topic: Val = Symbol::new(&te.env, "subscriber_create_cap_updated").into_val(&te.env);
    let want: Val = 7u32.into_val(&te.env);
    let mut seen = false;
    for (_contract, topics, data) in te.env.events().all().iter() {
        if topics.contains(&topic) {
            assert_eq!(data, want, "event payload must be the new cap");
            seen = true;
        }
    }
    assert!(seen, "expected a subscriber_create_cap_updated event");
}

#[test]
fn the_cap_can_be_rewritten_within_the_same_ledger() {
    // Unlike `set_min_topup`/`set_grace_period`, this setter does not go through
    // `enforce_config_cooldown`. Two consecutive writes must therefore both land.
    let te = TestEnv::default();

    te.client.set_subscriber_create_cap(&te.admin, &1u32);
    te.client.set_subscriber_create_cap(&te.admin, &2u32);

    assert_eq!(te.client.get_subscriber_create_cap(), 2);
}

// ── Authorization ───────────────────────────────────────────────────────────

#[test]
fn a_non_admin_cannot_change_the_cap_and_the_stored_value_is_untouched() {
    let te = TestEnv::default();
    let stranger = Address::generate(&te.env);

    let res = te
        .client
        .try_set_subscriber_create_cap(&stranger, &1u32);

    assert_eq!(res, Err(Ok(Error::Forbidden)));
    assert_eq!(
        te.client.get_subscriber_create_cap(),
        DEFAULT_CREATE_CAP,
        "a rejected write must not change the cap"
    );
}

// ── Zero cap ────────────────────────────────────────────────────────────────

#[test]
fn a_zero_cap_blocks_creation_emits_rate_limit_tripped_and_creates_nothing() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    te.client.set_subscriber_create_cap(&te.admin, &0u32);

    let before = te.client.get_subscription_count();
    let blocked = try_create(&te, &subscriber);

    assert_eq!(blocked, Err(Ok(Error::SubscriberRateLimited)));
    assert_eq!(
        te.client.get_subscription_count(),
        before,
        "a blocked create must not allocate a subscription id"
    );
    assert_eq!(topic_hits(&te, "rate_limit_tripped"), 1);
}

#[test]
fn raising_the_cap_from_zero_unblocks_creation() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);

    te.client.set_subscriber_create_cap(&te.admin, &0u32);
    assert_eq!(try_create(&te, &subscriber), Err(Ok(Error::SubscriberRateLimited)));

    te.client.set_subscriber_create_cap(&te.admin, &1u32);
    assert!(try_create(&te, &subscriber).is_ok());
}

// ── Window enforcement ──────────────────────────────────────────────────────

#[test]
fn a_cap_of_one_allows_exactly_one_creation_per_window() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    te.stellar_token_client().mint(&subscriber, &PREPAID);
    te.client.set_subscriber_create_cap(&te.admin, &1u32);

    assert!(try_create(&te, &subscriber).is_ok());

    let blocked = try_create(&te, &subscriber);
    assert_eq!(blocked, Err(Ok(Error::SubscriberRateLimited)));
    assert_eq!(topic_hits(&te, "rate_limit_tripped"), 1);
}

#[test]
fn the_window_resets_at_exactly_one_day() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    te.stellar_token_client().mint(&subscriber, &PREPAID);
    te.client.set_subscriber_create_cap(&te.admin, &1u32);

    assert!(try_create(&te, &subscriber).is_ok());
    assert_eq!(try_create(&te, &subscriber), Err(Ok(Error::SubscriberRateLimited)));

    // One second short of the window is still blocked …
    te.jump(SECONDS_IN_DAY - 1);
    assert_eq!(try_create(&te, &subscriber), Err(Ok(Error::SubscriberRateLimited)));

    // … and the exact boundary frees a slot.
    te.jump(1);
    assert!(try_create(&te, &subscriber).is_ok());
}

#[test]
fn lowering_the_cap_mid_window_applies_to_creations_already_made() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    te.stellar_token_client().mint(&subscriber, &PREPAID);
    te.client.set_subscriber_create_cap(&te.admin, &5u32);

    assert!(try_create(&te, &subscriber).is_ok());
    assert!(try_create(&te, &subscriber).is_ok());

    // Two slots are already used; lowering the cap to 2 must take effect
    // immediately for the open window.
    te.client.set_subscriber_create_cap(&te.admin, &2u32);
    assert_eq!(try_create(&te, &subscriber), Err(Ok(Error::SubscriberRateLimited)));

    // Raising it again frees slots without waiting for the window to roll over.
    te.client.set_subscriber_create_cap(&te.admin, &3u32);
    assert!(try_create(&te, &subscriber).is_ok());
}

#[test]
fn the_cap_is_per_subscriber_not_global() {
    let te = TestEnv::default();
    let a = Address::generate(&te.env);
    let b = Address::generate(&te.env);
    te.stellar_token_client().mint(&a, &PREPAID);
    te.stellar_token_client().mint(&b, &PREPAID);
    te.client.set_subscriber_create_cap(&te.admin, &1u32);

    assert!(try_create(&te, &a).is_ok());
    assert_eq!(try_create(&te, &a), Err(Ok(Error::SubscriberRateLimited)));

    // A different subscriber has its own window and is unaffected.
    assert!(try_create(&te, &b).is_ok());
}

// ── Boundaries ──────────────────────────────────────────────────────────────

#[test]
fn the_u32_max_cap_is_accepted_and_does_not_overflow_the_window_counter() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    te.stellar_token_client().mint(&subscriber, &PREPAID);

    te.client.set_subscriber_create_cap(&te.admin, &u32::MAX);

    assert_eq!(te.client.get_subscriber_create_cap(), u32::MAX);
    // `window.count >= cap` must stay false, so creation is allowed.
    assert!(try_create(&te, &subscriber).is_ok());
    assert!(try_create(&te, &subscriber).is_ok());
}

#[test]
fn the_stored_admin_is_exempt_from_a_zero_cap() {
    let te = TestEnv::default();
    // `enforce_creation_rate_limit` returns early when the subscriber is the
    // stored admin, so an operator cannot lock the admin out of the protocol.
    te.client.set_subscriber_create_cap(&te.admin, &0u32);

    assert!(try_create(&te, &te.admin).is_ok());
}
