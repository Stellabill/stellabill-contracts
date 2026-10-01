#![cfg(test)]

//! Adversarial coverage for `state_machine::get_allowed_transitions`.
//!
//! `get_allowed_transitions` is the public source of truth for the
//! `SubscriptionStatus` transition matrix: `can_transition`,
//! `validate_status_transition` and every `transition_to` caller (notably the
//! grace-period / insufficient-balance paths in `charge_core.rs`) are derived
//! from it. A wrong or missing entry therefore either permits an illegal
//! lifecycle change or blocks a legal one.
//!
//! This suite does not sample the matrix. It enumerates all `7 * 7 = 49`
//! ordered pairs and pins each one to the matrix documented in
//! `docs/subscription_state_machine.md` — restated independently below, so the
//! contract is compared against an expectation instead of against itself.
//!
//! Adversarial cases covered:
//!
//!   - every ordered pair, allowed and rejected (no sampling);
//!   - terminal states: `Cancelled` may only archive, `Archived` is frozen;
//!   - self transitions (`X -> X`) are never advertised as a *next* state;
//!   - advertised sets contain no duplicates and no unknown variants;
//!   - `get_allowed_transitions`, `can_transition`,
//!     `validate_status_transition`, `SubscriptionStatus::can_transition_to`
//!     and `types::ALLOWED_STATUS_TRANSITIONS` agree pair-by-pair;
//!   - a rejected `transition_to` is atomic: `Err(InvalidStatusTransition)`
//!     and the caller's value is untouched;
//!   - the grace paths (`Active -> GracePeriod`,
//!     `GracePeriod -> InsufficientBalance`, `GracePeriod -> Active`) and the
//!     archive paths (`Cancelled -> Archived`, `Expired -> Archived`) stay
//!     reachable, and `InsufficientBalance -> GracePeriod` stays blocked;
//!   - the status list is exhaustive: adding a variant without revisiting the
//!     matrix breaks this file at compile time (`discriminant_of`).
//!
//! Authorization is not exercised here: `get_allowed_transitions` is a pure,
//! read-only helper that takes no `Address` and performs no storage access.
//! Authorization enters at the entrypoint layer (`do_remove_operator`, charge
//! entrypoints), which is covered by `test_operator_remove_adversarial.rs`.

extern crate std;

use crate::state_machine::{
    can_transition, get_allowed_transitions, transition_to, validate_status_transition,
};
use crate::types::ALLOWED_STATUS_TRANSITIONS;
use crate::{Error, SubscriptionStatus};
use SubscriptionStatus::*;

/// Every status variant, in discriminant order.
const ALL_STATUSES: [SubscriptionStatus; 7] = [
    Active,
    Paused,
    Cancelled,
    InsufficientBalance,
    GracePeriod,
    Expired,
    Archived,
];

/// The canonical matrix from `docs/subscription_state_machine.md`, indexed by
/// [`discriminant_of`].
///
/// * `Active`: pause, under-funded (grace first, then insufficient), expire, cancel.
/// * `Paused`: resume, expire, cancel.
/// * `Cancelled`: archive only (terminal for everything else).
/// * `InsufficientBalance`: recover via deposit, expire, cancel. Re-entering a
///   grace window is *not* documented and stays blocked.
/// * `GracePeriod`: recover, expire the window, expire, cancel.
/// * `Expired`: cancel or archive.
/// * `Archived`: fully immutable.
const DOCUMENTED: [&[SubscriptionStatus]; 7] = [
    &[Paused, InsufficientBalance, GracePeriod, Expired, Cancelled],
    &[Active, Expired, Cancelled],
    &[Archived],
    &[Active, Expired, Cancelled],
    &[Active, InsufficientBalance, Expired, Cancelled],
    &[Cancelled, Archived],
    &[],
];

/// Maps a status to its row in [`DOCUMENTED`].
///
/// There is deliberately no `_ =>` arm: adding a variant to
/// `SubscriptionStatus` without revisiting the matrix is a compile error here
/// rather than a silent `map_or(&[], …)` fallback inside
/// `get_allowed_transitions`.
fn discriminant_of(status: SubscriptionStatus) -> usize {
    match status {
        Active => 0,
        Paused => 1,
        Cancelled => 2,
        InsufficientBalance => 3,
        GracePeriod => 4,
        Expired => 5,
        Archived => 6,
    }
}

/// `true` when `from -> to` is in the documented matrix.
fn documented_allows(from: SubscriptionStatus, to: SubscriptionStatus) -> bool {
    DOCUMENTED[discriminant_of(from)].contains(&to)
}

#[test]
fn all_forty_nine_ordered_pairs_match_the_documented_matrix() {
    let mut checked = 0usize;
    for from in ALL_STATUSES {
        for to in ALL_STATUSES {
            checked += 1;
            let expected = documented_allows(from, to);

            assert_eq!(
                can_transition(from, to),
                expected,
                "can_transition({from:?} -> {to:?}) disagrees with the documented matrix"
            );
            assert_eq!(
                get_allowed_transitions(from).contains(&to),
                expected,
                "get_allowed_transitions({from:?}) disagrees with the documented matrix for {to:?}"
            );
            assert_eq!(
                validate_status_transition(from, to).is_ok(),
                expected,
                "validate_status_transition({from:?} -> {to:?}) disagrees with the documented matrix"
            );
        }
    }
    assert_eq!(checked, 49, "the matrix must be exercised completely");
}

#[test]
fn advertised_sets_equal_the_documented_sets() {
    for status in ALL_STATUSES {
        let allowed = get_allowed_transitions(status);
        let expected = DOCUMENTED[discriminant_of(status)];

        assert_eq!(
            allowed.len(),
            expected.len(),
            "get_allowed_transitions({status:?}) has the wrong arity: {allowed:?}"
        );
        for target in expected {
            assert!(
                allowed.contains(target),
                "get_allowed_transitions({status:?}) is missing documented target {target:?}"
            );
        }
        for advertised in allowed {
            assert!(
                expected.contains(advertised),
                "get_allowed_transitions({status:?}) advertises undocumented target {advertised:?}"
            );
        }
    }
}

#[test]
fn advertised_sets_have_no_duplicates_or_unknown_variants() {
    for status in ALL_STATUSES {
        let allowed = get_allowed_transitions(status);
        for (i, a) in allowed.iter().enumerate() {
            assert!(
                ALL_STATUSES.contains(a),
                "get_allowed_transitions({status:?}) returned an unknown status"
            );
            for (j, b) in allowed.iter().enumerate() {
                if i != j {
                    assert_ne!(
                        a, b,
                        "get_allowed_transitions({status:?}) repeats {a:?}; duplicate targets make callers count transitions wrongly"
                    );
                }
            }
        }
    }
}

#[test]
fn self_transitions_are_never_advertised_as_a_next_state() {
    for status in ALL_STATUSES {
        assert!(
            !get_allowed_transitions(status).contains(&status),
            "get_allowed_transitions({status:?}) advertises a self transition"
        );
        assert!(
            !can_transition(status, status),
            "can_transition({status:?} -> {status:?}) must stay false: the table models *changes* of state"
        );
    }
}

#[test]
fn terminal_states_are_frozen_except_cancelled_archives() {
    for target in ALL_STATUSES {
        assert_eq!(
            can_transition(Cancelled, target),
            target == Archived,
            "Cancelled must only be able to archive, but {target:?} was reported as reachable"
        );
        assert!(
            !can_transition(Archived, target),
            "Archived is immutable, but {target:?} was reported as reachable"
        );
    }
}

#[test]
fn rejected_transition_is_atomic_and_deterministic() {
    let mut rejected = 0usize;
    for from in ALL_STATUSES {
        for to in ALL_STATUSES {
            if documented_allows(from, to) {
                continue;
            }
            rejected += 1;

            let mut current = from;
            assert_eq!(
                transition_to(&mut current, to),
                Err(Error::InvalidStatusTransition),
                "transition_to({from:?} -> {to:?}) must be rejected"
            );
            assert_eq!(
                current, from,
                "a rejected transition_to({from:?} -> {to:?}) mutated the caller's state"
            );
        }
    }
    assert_eq!(
        rejected, 31,
        "the rejected half of the matrix must be exercised completely"
    );
}

#[test]
fn accepted_transition_applies_exactly_the_requested_target() {
    let mut accepted = 0usize;
    for from in ALL_STATUSES {
        for to in ALL_STATUSES {
            if !documented_allows(from, to) {
                continue;
            }
            accepted += 1;

            let mut current = from;
            assert_eq!(
                transition_to(&mut current, to),
                Ok(()),
                "transition_to({from:?} -> {to:?}) must be accepted"
            );
            assert_eq!(current, to, "transition_to({from:?} -> {to:?}) wrote the wrong state");
        }
    }
    assert_eq!(
        accepted, 18,
        "the accepted half of the matrix must be exercised completely"
    );
}

#[test]
fn grace_period_paths_used_by_the_charge_flow_stay_reachable() {
    // `charge_core.rs` enters grace on the first under-funded charge from
    // Active, and closes the window with GracePeriod -> InsufficientBalance.
    assert!(
        can_transition(Active, GracePeriod),
        "an under-funded charge from Active can no longer enter GracePeriod"
    );
    assert!(
        can_transition(GracePeriod, InsufficientBalance),
        "an expired grace window can no longer fall through to InsufficientBalance"
    );

    // Recovery and de-escalation paths.
    assert!(can_transition(GracePeriod, Active));
    assert!(can_transition(InsufficientBalance, Active));
    assert!(can_transition(GracePeriod, Cancelled));

    // An under-funded subscription must not be able to re-open a grace window.
    assert!(
        !can_transition(InsufficientBalance, GracePeriod),
        "InsufficientBalance -> GracePeriod is not documented and must stay blocked"
    );
}

#[test]
fn archive_paths_used_by_cleanup_stay_reachable() {
    // `do_cleanup_subscription` archives both cancelled and expired
    // subscriptions; without these entries every cleanup call reverts.
    assert!(can_transition(Cancelled, Archived));
    assert!(can_transition(Expired, Archived));
    assert!(can_transition(Expired, Cancelled));

    for target in ALL_STATUSES {
        assert!(
            !can_transition(Archived, target),
            "Archived is a terminal state but {target:?} was reported as reachable"
        );
    }
}

#[test]
fn second_public_matrix_in_types_agrees_pair_by_pair() {
    let mut checked = 0usize;
    for from in ALL_STATUSES {
        for to in ALL_STATUSES {
            checked += 1;
            assert_eq!(
                from.can_transition_to(to),
                get_allowed_transitions(from).contains(&to),
                "SubscriptionStatus::can_transition_to({from:?} -> {to:?}) drifted from get_allowed_transitions"
            );
        }
    }
    assert_eq!(checked, 49);

    // Every status with outgoing edges must have an explicit row: a missing row
    // silently degrades to `false` through `unwrap_or(false)`.
    for status in ALL_STATUSES {
        if DOCUMENTED[discriminant_of(status)].is_empty() {
            continue;
        }
        assert!(
            ALLOWED_STATUS_TRANSITIONS.iter().any(|(from, _)| *from == status),
            "types::ALLOWED_STATUS_TRANSITIONS has no row for {status:?}"
        );
    }

    for (from, allowed) in ALLOWED_STATUS_TRANSITIONS {
        let canonical = get_allowed_transitions(*from);
        // Row order is not part of the contract; compare as sets so the two
        // matrices can be written in a different order without failing.
        assert_eq!(
            allowed.len(),
            canonical.len(),
            "types::ALLOWED_STATUS_TRANSITIONS row for {from:?} has the wrong arity"
        );
        for target in allowed.iter() {
            assert!(
                canonical.contains(target),
                "types::ALLOWED_STATUS_TRANSITIONS row for {from:?} advertises {target:?}, which get_allowed_transitions rejects"
            );
        }
        for target in canonical {
            assert!(
                allowed.contains(target),
                "types::ALLOWED_STATUS_TRANSITIONS row for {from:?} is missing {target:?}, which get_allowed_transitions allows"
            );
        }
    }
}

#[test]
fn same_state_idempotency_is_explicit_at_the_type_level_only() {
    for status in ALL_STATUSES {
        // The state-machine table models changes of state, so it must not
        // advertise or accept X -> X.
        assert!(!get_allowed_transitions(status).contains(&status));
        assert!(!can_transition(status, status));
        assert!(!status.can_transition_to(status));
        assert_eq!(
            status.validate_status_transition(status),
            Err(Error::InvalidStatusTransition)
        );

        // `types` exposes the documented "setting the same status is always
        // allowed" idempotency rule for callers that re-apply a status.
        assert!(crate::types::can_transition(status, status));
        let mut current = status;
        assert_eq!(crate::types::transition_to(&mut current, status), Ok(()));
        assert_eq!(current, status);
    }
}
