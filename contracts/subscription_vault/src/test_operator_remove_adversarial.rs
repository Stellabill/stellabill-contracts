#![cfg(test)]

//! Adversarial coverage for `operator::do_remove_operator` (`lib.rs`).
//!
//! `do_remove_operator` is the only way to revoke the least-privilege charge
//! delegate, so its failure modes matter in both directions: a rejection must
//! not revoke anything, and it must not hand an attacker a way to lock the
//! admin out of the operator slot. The behaviours pinned here are:
//!
//!   * authorization: only the *current* admin can remove, and an attacker's
//!     rejected attempts must not arm the shared config cooldown;
//!   * the config cooldown is enforced for the `Operator` key with an exact
//!     boundary (`CONFIG_COOLDOWN_SECS - 1` rejected, `CONFIG_COOLDOWN_SECS`
//!     accepted), and a `set_operator` arms it for `remove_operator`;
//!   * a rejected removal is atomic and silent: the stored operator is intact
//!     and no `operator_removed` event is published;
//!   * the cooldown is per config key, so an unrelated admin write cannot block
//!     operator changes;
//!   * removal and re-resolution requires waiting out the cooldown again;
//!   * re-removing an already empty slot is idempotent, and a no-op removal
//!     still consumes the cooldown (documented rate-limit semantics).
//!
//! `get_operator` is the storage surface under test; charge-path revocation is
//! already covered by `test_operator.rs`.

extern crate std;

use crate::admin::CONFIG_COOLDOWN_SECS;
use crate::test_utils::setup::TestEnv;
use crate::{Error, OperatorRemovedEvent, OperatorSetEvent};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    Address, IntoVal,
};

/// A non-zero base ledger time. `enforce_config_cooldown` treats a stored
/// `prev_ts` of `0` as "never changed", so every cooldown assertion has to run
/// on a clock that is not at genesis for the test to mean anything.
const T0: u64 = 1_000_000;

/// A fresh environment whose ledger clock sits at [`T0`].
fn setup() -> TestEnv {
    let te = TestEnv::default();
    te.env.ledger().with_mut(|li| li.timestamp = T0);
    te
}

fn event_count(te: &TestEnv) -> u32 {
    te.env.events().all().len()
}

/// Last published event payload decoded as an `OperatorSetEvent`.
fn last_is_operator_set(te: &TestEnv, operator: &Address) -> bool {
    match te.env.events().all().last() {
        Some(event) => {
            let payload: OperatorSetEvent = event.2.into_val(&te.env);
            payload.admin == te.admin && payload.operator == *operator
        }
        None => false,
    }
}

// ── Cooldown enforcement ─────────────────────────────────────────────────────

#[test]
fn set_operator_arms_the_cooldown_for_remove_operator() {
    let te = setup();
    let operator = Address::generate(&te.env);

    te.client.set_operator(&te.admin, &operator);

    // The `Operator` key was just written, so the removal is inside the window.
    assert_eq!(
        te.client.try_remove_operator(&te.admin),
        Err(Ok(Error::CooldownActive)),
        "remove_operator must respect the cooldown armed by set_operator"
    );
    assert_eq!(
        te.client.get_operator(),
        Some(operator),
        "a cooldown rejection must not revoke the operator"
    );
}

#[test]
fn remove_operator_boundary_is_exactly_config_cooldown_secs() {
    let te = setup();
    let operator = Address::generate(&te.env);

    te.client.set_operator(&te.admin, &operator);

    // One second short of the window: still rejected.
    te.jump(CONFIG_COOLDOWN_SECS - 1);
    assert_eq!(
        te.client.try_remove_operator(&te.admin),
        Err(Ok(Error::CooldownActive)),
        "a removal one second before the boundary must be rejected"
    );
    assert_eq!(te.client.get_operator(), Some(operator), "boundary rejection mutated storage");

    // Exactly at the boundary: accepted.
    te.jump(1);
    assert_eq!(te.client.remove_operator(&te.admin), Ok(()));
    assert_eq!(
        te.client.get_operator(),
        None,
        "a removal exactly at the cooldown boundary must be applied"
    );
}

#[test]
fn rejected_removal_is_atomic_and_silent() {
    let te = setup();
    let operator = Address::generate(&te.env);
    te.client.set_operator(&te.admin, &operator);

    let events_before = event_count(&te);
    let result = te.client.try_remove_operator(&te.admin);

    assert_eq!(result, Err(Ok(Error::CooldownActive)));
    assert_eq!(
        event_count(&te),
        events_before,
        "a rejected removal must not publish any event"
    );
    assert!(
        last_is_operator_set(&te, &operator),
        "the last event must still be the earlier operator_set"
    );
    assert_eq!(
        te.client.get_operator(),
        Some(operator),
        "the operator must survive a rejected removal"
    );

    // And the rejected call must not have refreshed the cooldown window: the
    // remaining wait is measured from the original set, not from the failure.
    te.jump(CONFIG_COOLDOWN_SECS - 1);
    assert_eq!(
        te.client.try_remove_operator(&te.admin),
        Err(Ok(Error::CooldownActive)),
        "a rejected removal must not restart the cooldown"
    );
}

// ── Authorization ────────────────────────────────────────────────────────────

#[test]
fn stranger_cannot_remove_operator_when_none_is_set() {
    let te = setup();
    let stranger = Address::generate(&te.env);

    assert_eq!(
        te.client.try_remove_operator(&stranger),
        Err(Ok(Error::Unauthorized)),
        "an empty operator slot must not turn remove_operator into a public call"
    );
    assert_eq!(te.client.get_operator(), None);

    // The rejected call must not consume the admin's cooldown window.
    let operator = Address::generate(&te.env);
    assert_eq!(te.client.set_operator(&te.admin, &operator), Ok(()));
    assert_eq!(te.client.get_operator(), Some(operator));
}

#[test]
fn repeated_unauthorized_attempts_do_not_arm_the_cooldown() {
    let te = setup();
    let stranger = Address::generate(&te.env);
    let operator = Address::generate(&te.env);

    let events_before = event_count(&te);
    for attempt in 0..5 {
        assert_eq!(
            te.client.try_remove_operator(&stranger),
            Err(Ok(Error::Unauthorized)),
            "unauthorized removal attempt {attempt} must be rejected"
        );
    }
    assert_eq!(
        event_count(&te),
        events_before,
        "unauthorized attempts must not publish events"
    );

    // An attacker cannot use failed calls to lock the operator slot: the admin
    // can still install an operator at the very same ledger timestamp.
    assert_eq!(
        te.client.set_operator(&te.admin, &operator),
        Ok(()),
        "failed unauthorized removals must not block the admin"
    );
    assert_eq!(te.client.get_operator(), Some(operator));
}

#[test]
fn cooldown_is_tracked_per_config_key() {
    let te = setup();
    let operator = Address::generate(&te.env);

    // An unrelated admin write must not occupy the `Operator` cooldown slot.
    assert_eq!(te.client.set_min_topup(&te.admin, &2_000_000i128), Ok(()));
    assert_eq!(
        te.client.set_operator(&te.admin, &operator),
        Ok(()),
        "set_min_topup must not block set_operator"
    );
    assert_eq!(te.client.get_operator(), Some(operator));
}

// ── Lifecycle ────────────────────────────────────────────────────────────────

#[test]
fn set_after_remove_is_gated_by_the_same_cooldown() {
    let te = setup();
    let op1 = Address::generate(&te.env);
    let op2 = Address::generate(&te.env);

    te.client.set_operator(&te.admin, &op1);
    te.jump(CONFIG_COOLDOWN_SECS);

    assert_eq!(te.client.remove_operator(&te.admin), Ok(()));
    assert_eq!(te.client.get_operator(), None);

    // Re-installing inside the window opened by the removal is rejected.
    assert_eq!(
        te.client.try_set_operator(&te.admin, &op2),
        Err(Ok(Error::CooldownActive)),
        "set_operator must respect the cooldown armed by remove_operator"
    );
    assert_eq!(te.client.get_operator(), None);

    te.jump(CONFIG_COOLDOWN_SECS);
    assert_eq!(te.client.set_operator(&te.admin, &op2), Ok(()));
    assert_eq!(te.client.get_operator(), Some(op2));
}

#[test]
fn repeated_removal_is_idempotent_once_the_cooldown_elapses() {
    let te = setup();
    let operator = Address::generate(&te.env);

    te.client.set_operator(&te.admin, &operator);
    te.jump(CONFIG_COOLDOWN_SECS);
    assert_eq!(te.client.remove_operator(&te.admin), Ok(()));

    // Second removal inside the window: rejected, and nothing was armed twice.
    assert_eq!(
        te.client.try_remove_operator(&te.admin),
        Err(Ok(Error::CooldownActive))
    );

    te.jump(CONFIG_COOLDOWN_SECS);
    assert_eq!(
        te.client.remove_operator(&te.admin),
        (),
        "removing an already empty slot must stay a no-op success"
    );
    assert_eq!(te.client.get_operator(), None);
}

#[test]
fn noop_removal_still_consumes_the_cooldown() {
    // Characterization of the current rate-limit semantics: `remove_operator`
    // is a config *write* for the `Operator` key, so it opens a cooldown window
    // even when there was nothing to remove. Callers that need to replace an
    // operator therefore should not "clear first" — they must spend one
    // cooldown, not two.
    let te = setup();
    let operator = Address::generate(&te.env);

    assert_eq!(te.client.remove_operator(&te.admin), Ok(()));
    assert_eq!(
        te.client.try_set_operator(&te.admin, &operator),
        Err(Ok(Error::CooldownActive)),
        "a no-op removal currently opens the cooldown window"
    );

    te.jump(CONFIG_COOLDOWN_SECS);
    assert_eq!(te.client.set_operator(&te.admin, &operator), Ok(()));
    assert_eq!(te.client.get_operator(), Some(operator));
}

#[test]
fn removal_emits_exactly_one_operator_removed_event() {
    let te = setup();
    let operator = Address::generate(&te.env);

    te.client.set_operator(&te.admin, &operator);
    let before = event_count(&te);
    te.jump(CONFIG_COOLDOWN_SECS);
    te.client.remove_operator(&te.admin);

    let all = te.env.events().all();
    let payload: OperatorRemovedEvent = all.last().expect("no events").2.into_val(&te.env);

    assert_eq!(payload.admin, te.admin);
    assert_eq!(payload.timestamp, T0 + CONFIG_COOLDOWN_SECS);
    assert_eq!(te.client.get_operator(), None);
    // `enforce_config_cooldown` publishes its `admin_config_changed` event, so
    // the removal adds exactly two entries and never duplicates the removal.
    assert_eq!(event_count(&te), before + 2);
}
