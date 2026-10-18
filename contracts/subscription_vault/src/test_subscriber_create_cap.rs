//! Adversarial tests for `get_subscriber_create_cap` / `set_subscriber_create_cap` (#988).
//!
//! # What is being tested
//!
//! `get_subscriber_create_cap` returns the global daily subscription-creation
//! rate limit enforced by `enforce_creation_rate_limit` inside
//! `subscription.rs`. Key facts that shape the test surface:
//!
//! * **Default** — returns `50u32` when no value has been persisted yet.
//! * **Auth** — only the stored admin may call `set_subscriber_create_cap`;
//!   any other caller must receive `Error::Forbidden`.
//! * **State isolation** — a rejected `set_subscriber_create_cap` call must
//!   not mutate the stored cap.
//! * **Round-trip** — after a successful set the getter must reflect the new
//!   value without truncation or sign-loss.
//! * **Behavioural enforcement** — the rate-limit enforcement path inside
//!   `create_subscription` obeys the cap that was written; the admin address
//!   is always exempt regardless of the cap value.
//! * **Edge / boundary values** — `0` (hard-block all non-admin creation),
//!   `1`, `u32::MAX`.
//! * **Event emission** — a successful set emits `subscriber_create_cap_updated`.

use crate::{Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    token::StellarAssetClient as TokenAdminClient,
    vec, Address, Env, IntoVal, Symbol,
};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Matches `SECONDS_IN_DAY` in `subscription.rs`.
const SECONDS_IN_DAY: u64 = 86_400;

const INTERVAL: u64 = 30 * 24 * 3600;
const AMOUNT: i128 = 10_000_000;
/// Enough prepaid balance to fund many subscription creations.
const PREPAID: i128 = 1_000_000_000;

// ── Setup helpers ─────────────────────────────────────────────────────────────

/// Fully wired test environment: real SAC token so token transfers in
/// `create_subscription` can succeed.
fn setup() -> (
    Env,
    Address,
    SubscriptionVaultClient<'static>,
    TokenAdminClient<'static>,
) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = TokenAdminClient::new(&env, &token);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    // Use 7 decimals to mirror `test_subscriber_active_cap` — keeps amounts
    // consistent across the test suite.
    client.init(&token, &7u32, &admin, &1_000_000i128, &(7 * 24 * 60 * 60u64));

    (env, admin, client, token_admin)
}

/// Create a subscription from `subscriber` to a freshly-generated merchant.
fn create_sub(
    client: &SubscriptionVaultClient,
    subscriber: &Address,
    merchant: &Address,
) -> u32 {
    client.create_subscription(
        subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    )
}

// ── Getter: default / pre-init behaviour ──────────────────────────────────────

/// Before `set_subscriber_create_cap` is ever called the getter must return
/// the hard-coded default of 50.
#[test]
fn default_cap_is_50_on_fresh_contract() {
    let (_env, _admin, client, _token_admin) = setup();
    assert_eq!(client.get_subscriber_create_cap(), 50u32);
}

/// The getter is a pure read — calling it multiple times in sequence must not
/// mutate any state or produce different results.
#[test]
fn getter_is_idempotent() {
    let (_env, _admin, client, _token_admin) = setup();
    let first = client.get_subscriber_create_cap();
    let second = client.get_subscriber_create_cap();
    assert_eq!(first, second);
    assert_eq!(first, 50u32);
}

// ── Setter: authorization ─────────────────────────────────────────────────────

/// A completely random address (not admin, not operator) attempting to set the
/// cap must receive `Error::Forbidden` and must not mutate state.
#[test]
fn non_admin_cannot_set_cap_returns_forbidden() {
    let (_env, _admin, client, _token_admin) = setup();
    let stranger = Address::generate(&_env);

    let result = client.try_set_subscriber_create_cap(&stranger, &20u32);
    assert_eq!(result, Err(Ok(Error::Forbidden)));

    // State must be unchanged — still the default.
    assert_eq!(client.get_subscriber_create_cap(), 50u32);
}

/// Passing a *different* address that looks like an admin (but isn't stored as
/// one) must also be rejected — verifying that identity comparison is strict.
#[test]
fn wrong_admin_address_is_rejected() {
    let (env, _real_admin, client, _token_admin) = setup();
    let impersonator = Address::generate(&env);

    let result = client.try_set_subscriber_create_cap(&impersonator, &99u32);
    assert_eq!(result, Err(Ok(Error::Forbidden)));

    // The cap must be unchanged.
    assert_eq!(client.get_subscriber_create_cap(), 50u32);
}

// ── Setter: valid mutations ───────────────────────────────────────────────────

/// Admin sets an explicit cap; the getter reflects the update.
#[test]
fn admin_can_set_and_read_back_cap() {
    let (_env, admin, client, _token_admin) = setup();

    client.set_subscriber_create_cap(&admin, &10u32);
    assert_eq!(client.get_subscriber_create_cap(), 10u32);
}

/// Round-trip with value 1 — boundary between "one creation allowed" and
/// "zero, which hard-blocks all creation".
#[test]
fn round_trip_cap_value_1() {
    let (_env, admin, client, _token_admin) = setup();

    client.set_subscriber_create_cap(&admin, &1u32);
    assert_eq!(client.get_subscriber_create_cap(), 1u32);
}

/// Round-trip with `u32::MAX` — ensures no truncation or overflow in storage.
#[test]
fn round_trip_cap_value_u32_max() {
    let (_env, admin, client, _token_admin) = setup();

    client.set_subscriber_create_cap(&admin, &u32::MAX);
    assert_eq!(client.get_subscriber_create_cap(), u32::MAX);
}

/// Overwriting the cap multiple times always converges on the last value.
#[test]
fn successive_overwrites_converge_on_last_value() {
    let (_env, admin, client, _token_admin) = setup();

    client.set_subscriber_create_cap(&admin, &100u32);
    assert_eq!(client.get_subscriber_create_cap(), 100u32);

    client.set_subscriber_create_cap(&admin, &7u32);
    assert_eq!(client.get_subscriber_create_cap(), 7u32);

    client.set_subscriber_create_cap(&admin, &200u32);
    assert_eq!(client.get_subscriber_create_cap(), 200u32);
}

// ── Event emission ────────────────────────────────────────────────────────────

/// A successful `set_subscriber_create_cap` must emit the
/// `subscriber_create_cap_updated` event carrying the new cap value.
#[test]
fn set_cap_emits_event() {
    let (env, admin, client, _token_admin) = setup();

    client.set_subscriber_create_cap(&admin, &25u32);

    let events = env.events().all();
    // At least one event must have the expected topic symbol.
    let found = events.iter().any(|(_, topics, _data)| {
        // The event topics vec encodes the symbol as the first element.
        let topic_symbol = Symbol::new(&env, "subscriber_create_cap_updated");
        let encoded: soroban_sdk::Val = (topic_symbol,).into_val(&env);
        topics == vec![&env, encoded]
    });
    assert!(found, "subscriber_create_cap_updated event was not emitted");
}

/// A rejected call (non-admin) must not emit the event — no silent partial
/// side-effects.
#[test]
fn rejected_set_does_not_emit_event() {
    let (env, _admin, client, _token_admin) = setup();
    let stranger = Address::generate(&env);

    let _ = client.try_set_subscriber_create_cap(&stranger, &5u32);

    let events = env.events().all();
    let found = events.iter().any(|(_, topics, _data)| {
        let topic_symbol = Symbol::new(&env, "subscriber_create_cap_updated");
        let encoded: soroban_sdk::Val = (topic_symbol,).into_val(&env);
        topics == vec![&env, encoded]
    });
    assert!(!found, "event must not be emitted after a rejected call");
}

// ── Behavioural enforcement: cap = 0 (hard block) ────────────────────────────

/// When the cap is set to `0` every non-admin subscriber must be blocked from
/// creating a new subscription, and the error must be `SubscriberRateLimited`.
#[test]
fn cap_zero_blocks_all_non_admin_creation() {
    let (env, admin, client, token_admin) = setup();

    client.set_subscriber_create_cap(&admin, &0u32);

    let subscriber = Address::generate(&env);
    token_admin.mint(&subscriber, &PREPAID);
    let merchant = Address::generate(&env);

    let result = client.try_create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert_eq!(result, Err(Ok(Error::SubscriberRateLimited)));
}

/// When the cap is `0`, the *admin address* must still be able to create
/// subscriptions because admins are exempt from the rate limit.
#[test]
fn cap_zero_does_not_block_admin() {
    let (env, admin, client, token_admin) = setup();

    client.set_subscriber_create_cap(&admin, &0u32);

    // The admin acts as both subscriber and merchant here.
    token_admin.mint(&admin, &PREPAID);
    let merchant = Address::generate(&env);

    // Should succeed — admin bypasses the rate limit entirely.
    let id = create_sub(&client, &admin, &merchant);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.subscriber, admin);
}

// ── Behavioural enforcement: cap = N allows exactly N per day ────────────────

/// Setting the cap to N must allow exactly N creates within one day-window
/// and reject the (N+1)th attempt with `SubscriberRateLimited`.
#[test]
fn cap_n_allows_n_creates_and_blocks_n_plus_one() {
    let (env, admin, client, token_admin) = setup();

    const N: u32 = 3;
    client.set_subscriber_create_cap(&admin, &N);

    let subscriber = Address::generate(&env);
    token_admin.mint(&subscriber, &PREPAID);
    let merchant = Address::generate(&env);

    for _ in 0..N {
        create_sub(&client, &subscriber, &merchant);
    }

    // (N+1)th must be rejected.
    let result = client.try_create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert_eq!(result, Err(Ok(Error::SubscriberRateLimited)));
}

/// After a block, advancing the ledger clock past one day resets the window
/// so the subscriber may create again up to the cap.
#[test]
fn window_resets_after_one_day() {
    let (env, admin, client, token_admin) = setup();

    client.set_subscriber_create_cap(&admin, &1u32);

    let subscriber = Address::generate(&env);
    token_admin.mint(&subscriber, &PREPAID);
    let merchant = Address::generate(&env);

    // First create succeeds.
    create_sub(&client, &subscriber, &merchant);

    // Second create in the same window is rejected.
    let blocked = client.try_create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert_eq!(blocked, Err(Ok(Error::SubscriberRateLimited)));

    // Advance ledger by exactly SECONDS_IN_DAY to open a fresh window.
    env.ledger().with_mut(|l| l.timestamp += SECONDS_IN_DAY);

    // Now creation must succeed again.
    create_sub(&client, &subscriber, &merchant);
}

// ── State isolation: rejected set leaves state unchanged ─────────────────────

/// A failed `set_subscriber_create_cap` (wrong admin) must leave the stored
/// cap byte-for-byte identical to what it was before the call.
#[test]
fn rejected_set_does_not_mutate_stored_cap() {
    let (env, admin, client, _token_admin) = setup();

    // First establish a known non-default cap.
    client.set_subscriber_create_cap(&admin, &42u32);
    assert_eq!(client.get_subscriber_create_cap(), 42u32);

    // Attempt a mutation from a stranger.
    let stranger = Address::generate(&env);
    let _ = client.try_set_subscriber_create_cap(&stranger, &99u32);

    // State must still be 42.
    assert_eq!(client.get_subscriber_create_cap(), 42u32);
}

// ── Rate-limit windows are per-subscriber, not global ─────────────────────────

/// Exhausting one subscriber's daily window must not affect a different
/// subscriber's window.
#[test]
fn rate_limit_windows_are_per_subscriber() {
    let (env, admin, client, token_admin) = setup();

    client.set_subscriber_create_cap(&admin, &1u32);

    let alice = Address::generate(&env);
    let bob = Address::generate(&env);
    token_admin.mint(&alice, &PREPAID);
    token_admin.mint(&bob, &PREPAID);

    let merchant = Address::generate(&env);

    // Alice uses up her slot.
    create_sub(&client, &alice, &merchant);

    // Alice is blocked on the 2nd attempt.
    let alice_blocked = client.try_create_subscription(
        &alice,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    assert_eq!(alice_blocked, Err(Ok(Error::SubscriberRateLimited)));

    // Bob's window is independent — his first create still succeeds.
    create_sub(&client, &bob, &merchant);
}
