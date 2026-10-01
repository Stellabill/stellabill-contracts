#![cfg(test)]

//! Adversarial coverage for `SubscriptionVault::set_min_topup`
//! (`contracts/subscription_vault/src/admin.rs::do_set_min_topup`).
//!
//! The setter looks trivial — validate the caller, reject a non-positive
//! threshold, then persist — but three behaviours around it are load-bearing and
//! previously unfixtured:
//!
//! * **authorization** is checked before the value, so a wrong caller cannot
//!   learn anything about the amount and never mutates state;
//! * the **cooldown** (`admin::CONFIG_COOLDOWN_SECS`) is only armed *after* both
//!   the identity and the amount checks pass, so a rejected attempt must not
//!   lock the admin out of a legitimate change — and the cooldown is per config
//!   key label, so it must not block orthogonal keys;
//! * the **boundary** is inclusive at the bottom (`amount < min_topup` is the
//!   only rejection in `deposit_funds`), so a deposit of exactly the threshold is
//!   accepted and one unit below is not.
//!
//! Every rejection asserts the observable post-state, not merely the error code.

use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    token::StellarAssetClient as TokenAdminClient,
    Address, Env, Symbol, TryFromVal, Val, Vec,
};
use subscription_vault::{
    AdminConfigChangedEvent, Error, SubscriptionVault, SubscriptionVaultClient,
};

/// Mirrors `admin::CONFIG_COOLDOWN_SECS`.
const CONFIG_COOLDOWN_SECS: u64 = 6 * 60 * 60;

const T0: u64 = 1_000_000;
const INITIAL_MIN_TOPUP: i128 = 1_000_000;

struct Fixture {
    env: Env,
    client: SubscriptionVaultClient<'static>,
    admin: Address,
    operator: Address,
    stranger: Address,
    subscriber: Address,
    merchant: Address,
    sub_id: u32,
}

fn setup() -> Fixture {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_admin_client = TokenAdminClient::new(&env, &token_address);

    let admin = Address::generate(&env);
    let operator = Address::generate(&env);
    let stranger = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    client.init(
        &token_address,
        &7u32,
        &admin,
        &INITIAL_MIN_TOPUP,
        &(3 * 24 * 60 * 60),
    );

    token_admin_client.mint(&subscriber, &1_000_000_000);

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &5_000_000i128,
        &(30 * 24 * 60 * 60u64),
        &false,
        &None,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    Fixture {
        env,
        client,
        admin,
        operator,
        stranger,
        subscriber,
        merchant,
        sub_id,
    }
}

fn advance_seconds(env: &Env, seconds: u64) {
    let now = env.ledger().timestamp();
    env.ledger().set_timestamp(now + seconds);
}

/// Data payloads published under `topic` during the most recent invocation.
fn event_data(env: &Env, topic: &str) -> Vec<Val> {
    let want = Symbol::new(env, topic);
    let all = env.events().all();
    let mut out = Vec::new(env);
    for i in 0..all.len() {
        let (_, topics, data): (Address, Vec<Val>, Val) = all.get(i).unwrap();
        if let Some(t) = topics.get(0) {
            if let Ok(published) = Symbol::try_from_val(env, &t) {
                if published == want {
                    out.push_back(data);
                }
            }
        }
    }
    out
}

fn setup_uninitialised() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    (env, client, admin)
}

// ── Accepted values ─────────────────────────────────────────────────────────

#[test]
fn init_seeds_the_minimum_topup() {
    let f = setup();
    assert_eq!(f.client.get_min_topup(), INITIAL_MIN_TOPUP);
}

#[test]
fn set_min_topup_persists_the_new_value_and_emits_both_audit_events() {
    let f = setup();

    f.client.set_min_topup(&f.admin, &2_500_000i128);

    // NOTE: `Env::events().all()` exposes the events of the most recent
    // invocation, so the event snapshots must be taken before the next call.
    // `min_topup_updated` carries the new threshold verbatim.
    let updated = event_data(&f.env, "min_topup_updated");
    assert_eq!(updated.len(), 1);
    assert_eq!(
        i128::try_from_val(&f.env, &updated.get(0).unwrap()).unwrap(),
        2_500_000
    );

    // `admin_config_changed` carries the per-key audit trail.
    let changed = event_data(&f.env, "admin_config_changed");
    assert_eq!(changed.len(), 1);
    let parsed = AdminConfigChangedEvent::try_from_val(&f.env, &changed.get(0).unwrap()).unwrap();
    assert_eq!(
        parsed.key_label,
        soroban_sdk::String::from_str(&f.env, "MinTopup")
    );
    assert_eq!(parsed.prev_ts, 0); // first change: no previous mutation recorded
    assert_eq!(parsed.timestamp, f.env.ledger().timestamp());

    assert_eq!(f.client.get_min_topup(), 2_500_000);
}

#[test]
fn one_is_the_smallest_accepted_minimum_topup() {
    let f = setup();

    f.client.set_min_topup(&f.admin, &1i128);

    assert_eq!(f.client.get_min_topup(), 1);
}

#[test]
fn i128_max_is_accepted_and_read_back_verbatim() {
    let f = setup();

    f.client.set_min_topup(&f.admin, &i128::MAX);

    assert_eq!(f.client.get_min_topup(), i128::MAX);
}

// ── Rejected values ─────────────────────────────────────────────────────────

#[test]
fn zero_is_rejected_and_the_previous_value_survives() {
    let f = setup();

    let result = f.client.try_set_min_topup(&f.admin, &0i128);

    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
    assert_eq!(f.client.get_min_topup(), INITIAL_MIN_TOPUP);
    assert_eq!(event_data(&f.env, "min_topup_updated").len(), 0);
}

#[test]
fn negative_values_are_rejected_at_both_ends_of_the_range() {
    let f = setup();

    assert_eq!(
        f.client.try_set_min_topup(&f.admin, &-1i128),
        Err(Ok(Error::InvalidAmount))
    );
    assert_eq!(f.client.get_min_topup(), INITIAL_MIN_TOPUP);

    assert_eq!(
        f.client.try_set_min_topup(&f.admin, &i128::MIN),
        Err(Ok(Error::InvalidAmount))
    );
    assert_eq!(f.client.get_min_topup(), INITIAL_MIN_TOPUP);
}

// ── Authorization ───────────────────────────────────────────────────────────

#[test]
fn a_non_admin_caller_is_rejected_before_any_state_changes() {
    let f = setup();

    let result = f.client.try_set_min_topup(&f.stranger, &9_000_000i128);

    assert_eq!(result, Err(Ok(Error::Forbidden)));
    assert_eq!(f.client.get_min_topup(), INITIAL_MIN_TOPUP);
    assert_eq!(f.client.get_admin(), f.admin);
    assert_eq!(event_data(&f.env, "min_topup_updated").len(), 0);
    assert_eq!(event_data(&f.env, "admin_config_changed").len(), 0);
}

#[test]
fn a_configured_operator_cannot_change_the_minimum_topup() {
    let f = setup();
    f.client.set_operator(&f.admin, &f.operator);

    let result = f.client.try_set_min_topup(&f.operator, &9_000_000i128);

    assert_eq!(result, Err(Ok(Error::Forbidden)));
    assert_eq!(f.client.get_min_topup(), INITIAL_MIN_TOPUP);
}

#[test]
fn the_caller_identity_is_checked_before_the_amount() {
    let f = setup();

    // A wrong caller sending an *invalid* amount still gets the identity error,
    // never `InvalidAmount`: no amount-based oracle is exposed to the caller.
    let result = f.client.try_set_min_topup(&f.stranger, &0i128);

    assert_eq!(result, Err(Ok(Error::Forbidden)));
}

#[test]
fn set_min_topup_is_rejected_before_initialisation() {
    let (_env, client, admin) = setup_uninitialised();

    let result = client.try_set_min_topup(&admin, &1_000i128);

    assert_eq!(result, Err(Ok(Error::NotInitialized)));
}

// ── Cooldown ────────────────────────────────────────────────────────────────

#[test]
fn a_second_change_within_the_cooldown_is_rejected_and_the_value_survives() {
    let f = setup();
    f.client.set_min_topup(&f.admin, &2_000_000i128);

    advance_seconds(&f.env, CONFIG_COOLDOWN_SECS - 1);
    let result = f.client.try_set_min_topup(&f.admin, &3_000_000i128);

    assert_eq!(result, Err(Ok(Error::CooldownActive)));
    assert_eq!(f.client.get_min_topup(), 2_000_000);
}

#[test]
fn a_change_exactly_at_the_cooldown_boundary_is_accepted() {
    let f = setup();
    f.client.set_min_topup(&f.admin, &2_000_000i128);

    advance_seconds(&f.env, CONFIG_COOLDOWN_SECS);
    f.client.set_min_topup(&f.admin, &3_000_000i128);

    assert_eq!(f.client.get_min_topup(), 3_000_000);
}

#[test]
fn a_rejected_amount_does_not_arm_the_cooldown() {
    let f = setup();

    // The amount check runs before `enforce_config_cooldown`, so this attempt
    // must leave no `AdminConfigLastChangedAt` entry behind.
    assert_eq!(
        f.client.try_set_min_topup(&f.admin, &0i128),
        Err(Ok(Error::InvalidAmount))
    );

    f.client.set_min_topup(&f.admin, &2_000_000i128);
    assert_eq!(f.client.get_min_topup(), 2_000_000);
}

#[test]
fn a_rejected_non_admin_attempt_does_not_arm_the_cooldown() {
    let f = setup();

    assert_eq!(
        f.client.try_set_min_topup(&f.stranger, &2_000_000i128),
        Err(Ok(Error::Forbidden))
    );

    // The legitimate admin can still make the change immediately afterwards.
    f.client.set_min_topup(&f.admin, &2_000_000i128);
    assert_eq!(f.client.get_min_topup(), 2_000_000);
}

#[test]
fn the_cooldown_is_scoped_to_the_min_topup_key_only() {
    let f = setup();

    f.client.set_min_topup(&f.admin, &2_000_000i128);

    // A different config key with its own cooldown slot is unaffected.
    f.client.set_operator(&f.admin, &f.stranger);
    assert_eq!(f.client.get_operator(), Some(f.stranger));

    // ...while the MinTopup slot is still hot.
    assert_eq!(
        f.client.try_set_min_topup(&f.admin, &3_000_000i128),
        Err(Ok(Error::CooldownActive))
    );
    assert_eq!(f.client.get_min_topup(), 2_000_000);
}

#[test]
fn a_new_value_takes_effect_for_the_next_change_window() {
    let f = setup();
    f.client.set_min_topup(&f.admin, &2_000_000i128);
    advance_seconds(&f.env, CONFIG_COOLDOWN_SECS);
    f.client.set_min_topup(&f.admin, &4_000_000i128);

    // The audit event for the second change reports the first change's timestamp
    // as `prev_ts`, so the cooldown chain is observable off-chain.
    let changed = event_data(&f.env, "admin_config_changed");
    assert_eq!(changed.len(), 1);
    let parsed = AdminConfigChangedEvent::try_from_val(&f.env, &changed.get(0).unwrap()).unwrap();
    assert_eq!(parsed.prev_ts, T0);
    assert_eq!(f.client.get_min_topup(), 4_000_000);
}

// ── Boundary interaction with `deposit_funds` ───────────────────────────────

#[test]
fn a_deposit_of_exactly_the_threshold_is_accepted_and_one_unit_below_is_not() {
    let f = setup();
    f.client.set_min_topup(&f.admin, &10_000_000i128);
    assert_eq!(f.client.get_min_topup(), 10_000_000);

    let below = f
        .client
        .try_deposit_funds(&f.sub_id, &f.subscriber, &9_999_999i128, &None);
    assert_eq!(below, Err(Ok(Error::BelowMinimumTopup)));
    assert_eq!(f.client.get_subscription(&f.sub_id).prepaid_balance, 0);

    f.client
        .deposit_funds(&f.sub_id, &f.subscriber, &10_000_000i128, &None);
    assert_eq!(
        f.client.get_subscription(&f.sub_id).prepaid_balance,
        10_000_000
    );
}

#[test]
fn lowering_the_threshold_reopens_deposits_below_the_previous_floor() {
    let f = setup();

    assert_eq!(
        f.client
            .try_deposit_funds(&f.sub_id, &f.subscriber, &500i128, &None),
        Err(Ok(Error::BelowMinimumTopup))
    );

    f.client.set_min_topup(&f.admin, &500i128);
    f.client
        .deposit_funds(&f.sub_id, &f.subscriber, &500i128, &None);

    assert_eq!(f.client.get_subscription(&f.sub_id).prepaid_balance, 500);
}

#[test]
fn changing_the_topup_threshold_does_not_disturb_the_subscription_or_merchant() {
    let f = setup();
    let before = f.client.get_subscription(&f.sub_id);

    f.client.set_min_topup(&f.admin, &7_000_000i128);

    let after = f.client.get_subscription(&f.sub_id);
    assert_eq!(after.status, before.status);
    assert_eq!(after.prepaid_balance, before.prepaid_balance);
    assert_eq!(after.merchant, before.merchant);
    assert_eq!(after.amount, before.amount);
    assert_eq!(f.client.get_merchant_balance(&f.merchant), 0);
}
