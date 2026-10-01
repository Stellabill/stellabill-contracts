//! Adversarial coverage for `admin::get_auto_pause_threshold` (issue #1019).
//!
//! `get_auto_pause_threshold` is the read side of the consecutive-failure
//! auto-pause switch. The behavioural suite in `test_auto_pause.rs` only
//! observes the stored threshold indirectly through charge outcomes; this
//! module pins the getter's own contract:
//!
//! * the default is `0` (feature disabled) on a fresh vault;
//! * the value round-trips exactly across the `u32` range, including the
//!   `0` (disable) and `u32::MAX` boundaries;
//! * a rejected (non-admin) write leaves the stored value untouched;
//! * repeated reads are pure — they mutate neither the threshold nor any
//!   other admin config;
//! * the setter is not throttled by the admin-config cooldown, unlike
//!   `set_min_topup` / `set_grace_period`.

use crate::admin::{get_auto_pause_threshold, get_grace_period, get_min_topup};
use crate::test_utils::setup::TestEnv;
use crate::types::{Error, SubscriptionStatus};
use crate::SubscriptionVaultClient;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

/// Read the stored threshold through the internal getter (there is no public
/// contract entry point for it), inside the vault's storage context.
fn stored_threshold(env: &Env, client: &SubscriptionVaultClient) -> u32 {
    env.as_contract(&client.address, || get_auto_pause_threshold(env))
}

#[test]
fn default_threshold_is_zero_before_any_set() {
    let te = TestEnv::default();
    assert_eq!(stored_threshold(&te.env, &te.client), 0);
}

#[test]
fn set_then_get_round_trips() {
    let te = TestEnv::default();
    te.client.set_auto_pause_threshold(&te.admin, &5u32);
    assert_eq!(stored_threshold(&te.env, &te.client), 5);
}

#[test]
fn zero_disables_after_a_non_zero_value() {
    let te = TestEnv::default();
    te.client.set_auto_pause_threshold(&te.admin, &3u32);
    assert_eq!(stored_threshold(&te.env, &te.client), 3);

    te.client.set_auto_pause_threshold(&te.admin, &0u32);
    assert_eq!(stored_threshold(&te.env, &te.client), 0);
}

#[test]
fn u32_max_boundary_round_trips_without_overflow() {
    let te = TestEnv::default();
    te.client.set_auto_pause_threshold(&te.admin, &u32::MAX);
    assert_eq!(stored_threshold(&te.env, &te.client), u32::MAX);
}

#[test]
fn non_admin_cannot_change_threshold_and_state_is_unchanged() {
    let te = TestEnv::default();
    te.client.set_auto_pause_threshold(&te.admin, &7u32);

    let stranger = Address::generate(&te.env);
    let res = te.client.try_set_auto_pause_threshold(&stranger, &99u32);

    assert_eq!(res, Err(Ok(Error::Forbidden)));
    assert_eq!(stored_threshold(&te.env, &te.client), 7);
}

#[test]
fn repeated_reads_are_pure() {
    let te = TestEnv::default();
    te.client.set_auto_pause_threshold(&te.admin, &11u32);

    for _ in 0..5 {
        assert_eq!(stored_threshold(&te.env, &te.client), 11);
    }
}

#[test]
fn threshold_set_does_not_disturb_other_admin_config() {
    let te = TestEnv::default();
    let min_before = te
        .env
        .as_contract(&te.client.address, || get_min_topup(&te.env).unwrap());
    let grace_before = te
        .env
        .as_contract(&te.client.address, || get_grace_period(&te.env).unwrap());

    te.client.set_auto_pause_threshold(&te.admin, &4u32);

    let min_after = te
        .env
        .as_contract(&te.client.address, || get_min_topup(&te.env).unwrap());
    let grace_after = te
        .env
        .as_contract(&te.client.address, || get_grace_period(&te.env).unwrap());

    assert_eq!(min_after, min_before);
    assert_eq!(grace_after, grace_before);
}

#[test]
fn setter_is_not_gated_by_the_admin_config_cooldown() {
    let te = TestEnv::default();
    // Two back-to-back writes with no ledger advance must both succeed: the
    // setter intentionally bypasses `enforce_config_cooldown` so operators
    // can tighten the threshold during an incident.
    te.client.set_auto_pause_threshold(&te.admin, &2u32);
    te.client.set_auto_pause_threshold(&te.admin, &1u32);
    assert_eq!(stored_threshold(&te.env, &te.client), 1);
}

#[test]
fn threshold_set_does_not_touch_subscription_state() {
    let te = TestEnv::default();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let id = te.client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &(24 * 60 * 60),
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    te.client.set_auto_pause_threshold(&te.admin, &9u32);

    let sub = te.client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(stored_threshold(&te.env, &te.client), 9);
}
