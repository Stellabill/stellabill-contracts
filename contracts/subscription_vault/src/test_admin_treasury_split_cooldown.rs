//! Adversarial coverage for the *cooldown interaction* of
//! `admin::clear_treasury_split` (issue #1013).
//!
//! `test_admin_clear_treasury_split` pins the storage outcome: clearing removes
//! the split, rejects strangers and operators, and is refused inside the config
//! cooldown. What none of those tests pin is the shape of the cooldown itself,
//! because `enforce_config_cooldown` is shared by every admin config setter and
//! returns *before* it writes:
//!
//! ```text
//! if prev_ts > 0 && now - prev_ts < CONFIG_COOLDOWN_SECS {
//!     return Err(Error::CooldownActive);   // <- returns before the write below
//! }
//! env.storage().persistent().set(&storage_key, &now);
//! ```
//!
//! Two properties follow from that ordering, and neither was asserted anywhere:
//!
//! 1. a **rejected** call must not refresh the window. If it did, an attacker who
//!    could not clear the split could still spam the entrypoint to push the
//!    cooldown forward forever — a denial of service on the legitimate admin;
//! 2. the cooldown is keyed per config label, so a `TreasurySplit` change must not
//!    lock out unrelated config setters (and vice versa).
//!
//! Both are observable through the public surface, so these tests assert them the
//! way the admin experiences them rather than by reaching into storage.

#![cfg(test)]

use crate::admin::CONFIG_COOLDOWN_SECS;
use crate::test_utils::setup::TestEnv;
use crate::types::{Error, TreasurySplitEntry};
use soroban_sdk::{testutils::Address as _, Address, Vec};

const T0: u64 = 10_000;
const NEW_MIN_TOPUP: i128 = 5_000_000;

/// A valid single-beneficiary split that satisfies the 10_000 bps sum rule.
fn split_for(te: &TestEnv, bps: u32) -> Vec<TreasurySplitEntry> {
    let mut entries = Vec::new(&te.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: Address::generate(&te.env),
        bps,
    });
    entries
}

/// Anchor the ledger clock so cooldown arithmetic is exact.
fn set_time(te: &TestEnv, ts: u64) {
    te.env.ledger().with_mut(|l| l.timestamp = ts);
}

// ════════════════════════════════════════════════════════════════════
//  A rejected call must not refresh the cooldown window
// ════════════════════════════════════════════════════════════════════

#[test]
fn a_cooldown_rejected_clear_does_not_extend_the_window() {
    let te = TestEnv::default();
    set_time(&te, T0);

    // Arms the `TreasurySplit` cooldown at T0.
    te.client.set_treasury_split(&te.admin, &split_for(&te, 10_000));

    // One second short of the deadline the clear is refused.
    set_time(&te, T0 + CONFIG_COOLDOWN_SECS - 1);
    assert_eq!(
        te.client.try_clear_treasury_split(&te.admin),
        Err(Ok(Error::CooldownActive))
    );

    // At the original deadline it must succeed. Were the rejected attempt to have
    // stamped the cooldown at `T0 + CONFIG_COOLDOWN_SECS - 1`, this would still
    // report `CooldownActive` and the admin would be locked out for another full
    // window by their own failed attempt.
    set_time(&te, T0 + CONFIG_COOLDOWN_SECS);
    assert_eq!(te.client.try_clear_treasury_split(&te.admin), Ok(()));
    assert_eq!(te.client.get_treasury_split(), None);
}

#[test]
fn repeated_rejected_clears_never_move_the_deadline() {
    let te = TestEnv::default();
    set_time(&te, T0);
    te.client.set_treasury_split(&te.admin, &split_for(&te, 10_000));

    // Hammer the entrypoint across the whole cooldown window. Each attempt is
    // refused, and none of them may consume the window.
    let mut ts = T0 + 1;
    while ts < T0 + CONFIG_COOLDOWN_SECS {
        set_time(&te, ts);
        assert_eq!(
            te.client.try_clear_treasury_split(&te.admin),
            Err(Ok(Error::CooldownActive)),
            "clear must stay refused at {ts}"
        );
        assert!(
            te.client.get_treasury_split().is_some(),
            "the split must survive every refused clear"
        );
        ts += CONFIG_COOLDOWN_SECS / 6;
    }

    // The very first instant the original window expires, the clear lands.
    set_time(&te, T0 + CONFIG_COOLDOWN_SECS);
    assert_eq!(te.client.try_clear_treasury_split(&te.admin), Ok(()));
    assert_eq!(te.client.get_treasury_split(), None);
}

#[test]
fn a_cooldown_rejected_set_does_not_extend_the_window_for_the_next_set() {
    let te = TestEnv::default();
    set_time(&te, T0);
    te.client.set_treasury_split(&te.admin, &split_for(&te, 10_000));

    set_time(&te, T0 + CONFIG_COOLDOWN_SECS - 1);
    assert_eq!(
        te.client.try_set_treasury_split(&te.admin, &split_for(&te, 9_999)),
        Err(Ok(Error::CooldownActive))
    );

    // Still refused at exactly the deadline minus one, so the rejected attempt
    // neither armed nor extended anything.
    assert_eq!(
        te.client.try_set_treasury_split(&te.admin, &split_for(&te, 9_999)),
        Err(Ok(Error::CooldownActive))
    );

    // And accepted the moment the original window closes.
    set_time(&te, T0 + CONFIG_COOLDOWN_SECS);
    assert_eq!(
        te.client.try_set_treasury_split(&te.admin, &split_for(&te, 10_000)),
        Ok(())
    );
}

// ════════════════════════════════════════════════════════════════════
//  The cooldown is per config label, not global
// ════════════════════════════════════════════════════════════════════

#[test]
fn a_treasury_split_change_does_not_lock_out_unrelated_config() {
    let te = TestEnv::default();
    set_time(&te, T0);

    te.client.set_treasury_split(&te.admin, &split_for(&te, 10_000));

    // Immediately, with no cooldown elapsed: an unrelated config setter must be
    // free to run. A shared/global timestamp would refuse this.
    assert_eq!(te.client.try_set_min_topup(&te.admin, &NEW_MIN_TOPUP), Ok(()));
    assert_eq!(te.client.get_min_topup(), Ok(NEW_MIN_TOPUP));

    // And the new min top-up's own cooldown is armed, not the split's.
    assert_eq!(
        te.client.try_set_min_topup(&te.admin, &(NEW_MIN_TOPUP + 1)),
        Err(Ok(Error::CooldownActive))
    );
    assert_eq!(
        te.client.try_set_treasury_split(&te.admin, &split_for(&te, 10_000)),
        Err(Ok(Error::CooldownActive)),
        "the split's own cooldown was armed by the set at T0"
    );
}

#[test]
fn an_unrelated_config_change_does_not_arm_the_treasury_split_cooldown() {
    let te = TestEnv::default();
    set_time(&te, T0);

    // Arms `MinTopup`, not `TreasurySplit`.
    assert_eq!(te.client.try_set_min_topup(&te.admin, &NEW_MIN_TOPUP), Ok(()));

    // The split can therefore be configured straight away...
    assert_eq!(
        te.client.try_set_treasury_split(&te.admin, &split_for(&te, 10_000)),
        Ok(())
    );
    assert!(te.client.get_treasury_split().is_some());

    // ...and clearing it is then refused, because *setting* it armed its own
    // label. This is the mirror image of the previous test.
    assert_eq!(
        te.client.try_clear_treasury_split(&te.admin),
        Err(Ok(Error::CooldownActive))
    );
    assert!(te.client.get_treasury_split().is_some());
}

#[test]
fn clearing_then_reconfiguring_is_gated_by_the_split_label_only() {
    let te = TestEnv::default();
    set_time(&te, T0);
    te.client.set_treasury_split(&te.admin, &split_for(&te, 10_000));

    set_time(&te, T0 + CONFIG_COOLDOWN_SECS);
    assert_eq!(te.client.try_clear_treasury_split(&te.admin), Ok(()));

    // The clear armed `TreasurySplit` at T0 + COOLDOWN...
    assert_eq!(
        te.client.try_set_treasury_split(&te.admin, &split_for(&te, 10_000)),
        Err(Ok(Error::CooldownActive))
    );

    // ...while an unrelated config setter stays available throughout.
    assert_eq!(te.client.try_set_min_topup(&te.admin, &NEW_MIN_TOPUP), Ok(()));

    // After a further full window the split can be configured again.
    set_time(&te, T0 + 2 * CONFIG_COOLDOWN_SECS);
    assert_eq!(
        te.client.try_set_treasury_split(&te.admin, &split_for(&te, 10_000)),
        Ok(())
    );
    assert!(te.client.get_treasury_split().is_some());
}
