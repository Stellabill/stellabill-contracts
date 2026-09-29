//! Adversarial coverage for `state_machine::transition_to`, the status
//! transition gate defined inline in `contracts/subscription_vault/src/lib.rs`
//! (the `pub mod state_machine { ... }` block; `transition_to` is at
//! `lib.rs:144`).
//!
//! `transition_to(current: &mut SubscriptionStatus, next: SubscriptionStatus)`
//! is the only status writer in `charge_core`, so every status a charge can
//! observe is decided by its transition matrix. This file pins the three
//! properties the contract promises:
//!
//! * **Atomicity** — `current` is mutated only on success. Every rejected
//!   call is asserted to leave the caller's value unchanged.
//! * **Determinism** — every rejection is exactly
//!   `Error::InvalidStatusTransition` (encoded as 4001; the docs quote 400);
//!   no other error escapes, and the outcome of a `(current, next)` pair never
//!   depends on call history.
//! * **Totality** — all 7 x 7 = 49 status pairs are exercised: the 15
//!   documented edges succeed and the other 34 are rejected, so no
//!   undocumented edge can be silently accepted and no documented edge
//!   rejected.
//!
//! The function is not ABI-exposed, so the authorization cases drive the real
//! charge path (`charge_subscription`, admin-only) and prove that an
//! unauthorized caller cannot reach the state machine at all.
//!
//! Note on documented divergence (pinned here, deliberately not changed):
//! the live matrix in `lib.rs` is narrower than
//! `docs/subscription_state_machine.md` and than the unused
//! `src/state_machine.rs`:
//!
//! | edge | docs / `state_machine.rs` | live `lib.rs` matrix |
//! |---|---|---|
//! | any -> same status | allowed (idempotent) | rejected |
//! | `Active -> GracePeriod` | allowed | rejected |
//! | `GracePeriod -> InsufficientBalance` | allowed | rejected |
//! | `Cancelled -> Archived` | allowed | rejected |
//! | `Expired -> Archived` | allowed | rejected |
//! | `InsufficientBalance -> Paused` | allowed in `state_machine.rs` (docs list it as blocked) | rejected |
//!
//! Consequences pinned below: `Archived` has no incoming edge at all, and
//! the `Active -> GracePeriod` edge that `charge_core` uses for the
//! grace-period flow cannot be applied.

use crate::state_machine::{can_transition, get_allowed_transitions, transition_to};
use crate::test_utils::fixtures;
use crate::types::{DataKey, Error};
use crate::{
    ChargeExecutionResult, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _, MockAuth, MockAuthInvoke},
    token, Address, BytesN, Env, IntoVal, Vec,
};

use SubscriptionStatus::{
    Active, Archived, Cancelled, Expired, GracePeriod, InsufficientBalance, Paused,
};

// ── Model ────────────────────────────────────────────────────────────────────

/// Every status, in discriminant order.
const ALL: [SubscriptionStatus; 7] = [
    Active,
    Paused,
    Cancelled,
    InsufficientBalance,
    GracePeriod,
    Expired,
    Archived,
];

/// The transition matrix as implemented by the live inline module in `lib.rs`,
/// restated here independently of `get_allowed_transitions` so the assertions
/// below compare against a model rather than against the function itself.
///
/// Note the absent `Active -> GracePeriod` edge: the live `Active` row lists
/// only `Paused`, `InsufficientBalance`, `Cancelled` and `Expired`.
const DOCUMENTED: [(SubscriptionStatus, SubscriptionStatus); 15] = [
    (Active, Paused),
    (Active, InsufficientBalance),
    (Active, Cancelled),
    (Active, Expired),
    (Paused, Active),
    (Paused, Cancelled),
    (Paused, Expired),
    (InsufficientBalance, Active),
    (InsufficientBalance, GracePeriod),
    (InsufficientBalance, Cancelled),
    (InsufficientBalance, Expired),
    (GracePeriod, Active),
    (GracePeriod, Cancelled),
    (GracePeriod, Expired),
    (Expired, Cancelled),
];

/// True when `(from, next)` is a documented edge.
fn is_documented(from: SubscriptionStatus, next: SubscriptionStatus) -> bool {
    DOCUMENTED.contains(&(from, next))
}

// ── Success paths ────────────────────────────────────────────────────────────

#[test]
fn transition_to_applies_every_documented_edge() {
    for (from, next) in DOCUMENTED {
        let mut current = from;
        assert_eq!(
            transition_to(&mut current, next),
            Ok(()),
            "{from:?} -> {next:?} is documented and must succeed"
        );
        assert_eq!(current, next, "{from:?} -> {next:?} must be applied");
    }
}

#[test]
fn transition_to_returns_unit_on_success() {
    // The return type is `Result<(), Error>`: callers can only observe success
    // or the transition error, never a payload.
    let mut current = Active;
    let result: Result<(), Error> = transition_to(&mut current, Paused);
    assert_eq!(result, Ok(()));
    assert_eq!(current, Paused);
}

#[test]
fn transition_to_successive_documented_edges_track_the_applied_value() {
    // Active -> InsufficientBalance -> GracePeriod -> Expired -> Cancelled,
    // then the terminal row.
    let mut current = Active;
    for next in [InsufficientBalance, GracePeriod, Expired, Cancelled] {
        assert_eq!(transition_to(&mut current, next), Ok(()));
    }
    assert_eq!(current, Cancelled);

    for next in ALL {
        assert_eq!(
            transition_to(&mut current, next),
            Err(Error::InvalidStatusTransition),
            "Cancelled -> {next:?} must be rejected"
        );
    }
    assert_eq!(current, Cancelled);
}

// ── Failure paths: rejected pairs never mutate ───────────────────────────────

#[test]
fn transition_to_rejects_every_undocumented_pair_without_mutating() {
    let mut checked = 0usize;
    for from in ALL {
        for next in ALL {
            if is_documented(from, next) {
                continue;
            }
            let mut current = from;
            assert_eq!(
                transition_to(&mut current, next),
                Err(Error::InvalidStatusTransition),
                "{from:?} -> {next:?} is undocumented and must be rejected"
            );
            assert_eq!(
                current, from,
                "{from:?} -> {next:?} must leave the status unchanged"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 34, "49 pairs minus the 15 documented edges");
}

#[test]
fn transition_to_rejection_error_is_always_invalid_status_transition() {
    for from in ALL {
        for next in ALL {
            if is_documented(from, next) {
                continue;
            }
            let mut current = from;
            let err = transition_to(&mut current, next).unwrap_err();
            assert_eq!(err, Error::InvalidStatusTransition);
            // Pinned verbatim so a change to the wire code is a deliberate edit.
            // (The docs quote 400; the enum currently encodes 4001.)
            assert_eq!(err.to_code(), 4001, "the error code is part of the ABI");
            assert_eq!(current, from);
        }
    }
}

#[test]
fn transition_to_rejects_self_transition_for_every_status() {
    // The live matrix contains no self-edges, so `next == current` is a
    // rejection, not an idempotent no-op. Divergence: the docs allow it and
    // `types::transition_to` (a different, unused function) short-circuits it
    // to `Ok(())`. Pinned here so the difference stays observable.
    for status in ALL {
        let mut current = status;
        assert_eq!(
            transition_to(&mut current, status),
            Err(Error::InvalidStatusTransition),
            "{status:?} -> {status:?} must be rejected by the live matrix"
        );
        assert_eq!(current, status, "a rejected self-transition must not write");
    }
}

#[test]
fn transition_to_rejects_every_target_from_terminal_states() {
    for terminal in [Cancelled, Archived] {
        assert!(
            get_allowed_transitions(terminal).is_empty(),
            "{terminal:?} must have no outgoing edges"
        );
        for next in ALL {
            let mut current = terminal;
            assert_eq!(
                transition_to(&mut current, next),
                Err(Error::InvalidStatusTransition),
                "{terminal:?} -> {next:?} must be rejected"
            );
            assert_eq!(current, terminal);
        }
    }
}

#[test]
fn transition_to_never_enters_archived_from_any_status() {
    // `Archived` is unreachable through this gate: no matrix row lists it as a
    // target, so the `Cancelled -> Archived` / `Expired -> Archived` cleanup
    // edges in the docs are not implemented in the live matrix.
    for from in ALL {
        let mut current = from;
        assert_eq!(
            transition_to(&mut current, Archived),
            Err(Error::InvalidStatusTransition),
            "{from:?} -> Archived must be rejected"
        );
        assert_eq!(current, from);
    }
}

#[test]
fn transition_to_expired_row_only_reaches_cancelled() {
    // Boundary row: `Expired` is the last non-terminal state and its single
    // outgoing edge is `Cancelled`, not `Archived`.
    assert_eq!(get_allowed_transitions(Expired), &[Cancelled]);
    for next in ALL {
        let mut current = Expired;
        let expected = if next == Cancelled {
            Ok(())
        } else {
            Err(Error::InvalidStatusTransition)
        };
        assert_eq!(transition_to(&mut current, next), expected);
        assert_eq!(
            current,
            if next == Cancelled {
                Cancelled
            } else {
                Expired
            }
        );
    }
}

#[test]
fn transition_to_rejects_active_to_grace_period() {
    // The live `Active` row has no `GracePeriod` edge, so the grace-period
    // recovery flow documented in `docs/subscription_state_machine.md` and
    // implemented in the unused `src/state_machine.rs` cannot start from
    // `Active`. `GracePeriod` is only reachable from `InsufficientBalance`.
    let mut current = Active;
    assert_eq!(
        transition_to(&mut current, GracePeriod),
        Err(Error::InvalidStatusTransition)
    );
    assert_eq!(current, Active);
    assert!(!get_allowed_transitions(Active).contains(&GracePeriod));

    // The neighbouring row does allow it.
    let mut from_underfunded = InsufficientBalance;
    assert_eq!(transition_to(&mut from_underfunded, GracePeriod), Ok(()));
    assert_eq!(from_underfunded, GracePeriod);
}

#[test]
fn transition_to_rejects_insufficient_balance_to_paused() {
    // `charge_core` attempts `InsufficientBalance -> Paused` for the
    // admin-configured auto-pause and silently ignores the result (`.is_ok()`).
    // The live matrix has no such edge, so that branch can never fire. The
    // unused `src/state_machine.rs` allows it; the docs list it as blocked;
    // `test_auto_pause.rs` asserts the opposite behaviour and is one of the
    // pre-existing suite failures. Pinned here (not fixed) so the dead path
    // stays visible.
    let mut current = InsufficientBalance;
    assert_eq!(
        transition_to(&mut current, Paused),
        Err(Error::InvalidStatusTransition)
    );
    assert_eq!(current, InsufficientBalance);
    assert!(!get_allowed_transitions(InsufficientBalance).contains(&Paused));
}

#[test]
fn transition_to_rejected_calls_do_not_mutate_across_a_full_rejection_sweep() {
    // Same-status rejections are the cheapest way to hammer the failure path:
    // seven rejected calls against one variable must leave it alone.
    for from in ALL {
        let mut current = from;
        for _ in 0..7 {
            assert!(transition_to(&mut current, from).is_err());
        }
        assert_eq!(current, from);
    }
}

#[test]
fn transition_to_rejection_does_not_disturb_a_previously_applied_transition() {
    // A rejected call after a successful one keeps the newly applied value; it
    // must not roll back to the original status.
    let mut current = Active;
    assert_eq!(transition_to(&mut current, Paused), Ok(()));
    assert_eq!(
        transition_to(&mut current, GracePeriod),
        Err(Error::InvalidStatusTransition)
    );
    assert_eq!(current, Paused);
}

// ── Consistency with the sibling helpers ─────────────────────────────────────

#[test]
fn transition_to_agrees_with_can_transition_and_allowed_list_for_every_pair() {
    for from in ALL {
        for next in ALL {
            let mut current = from;
            let accepted = transition_to(&mut current, next).is_ok();
            assert_eq!(
                accepted,
                can_transition(from, next),
                "{from:?} -> {next:?}: transition_to disagrees with can_transition"
            );
            assert_eq!(
                accepted,
                get_allowed_transitions(from).contains(&next),
                "{from:?} -> {next:?}: transition_to disagrees with get_allowed_transitions"
            );
            assert_eq!(current, if accepted { next } else { from });
        }
    }
}

#[test]
fn get_allowed_transitions_lists_exactly_the_documented_edges() {
    let mut total = 0usize;
    for from in ALL {
        total += get_allowed_transitions(from).len();
        for next in get_allowed_transitions(from) {
            assert!(
                is_documented(from, *next),
                "undocumented edge {from:?} -> {next:?}"
            );
            assert!(can_transition(from, *next));
        }
    }
    assert_eq!(
        total,
        DOCUMENTED.len(),
        "no undocumented and no duplicated edges"
    );
}

// ── Entry-point coverage: reaching the gate through `charge_subscription` ────

const T0: u64 = 1_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const AMOUNT: i128 = 10_000_000;

/// A contract initialized with `grace_period` and one `Active` subscription
/// with a zero prepaid balance, one interval in the future.
struct Fixture {
    env: Env,
    client: SubscriptionVaultClient<'static>,
    id: u32,
}

fn setup_underfunded(grace_period: u64) -> Fixture {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = T0);

    let admin = Address::generate(&env);
    let token_client = token::StellarAssetClient::new(
        &env,
        &env.register_stellar_asset_contract_v2(Address::generate(&env))
            .address(),
    );
    let client = SubscriptionVaultClient::new(&env, &env.register(SubscriptionVault, ()));
    client.init(
        &token_client.address,
        &6,
        &admin,
        &1_000_000i128,
        &grace_period,
    );

    let (id, _subscriber, _merchant) =
        fixtures::create_subscription_detailed(&env, &client, Active, AMOUNT, INTERVAL);

    // `auto_renew` defaults to false, and the charge engine silently skips
    // non-renewing subscriptions; the underfunded branch under test only runs
    // for auto-renewing ones.
    let mut sub = client.get_subscription(&id);
    sub.auto_renew = true;
    env.as_contract(&client.address, || {
        env.storage().persistent().set(&DataKey::Sub(id), &sub);
    });

    let stored = client.get_subscription(&id);
    assert_eq!(stored.status, Active);
    assert_eq!(stored.prepaid_balance, 0);

    Fixture { env, client, id }
}

impl Fixture {
    /// Move past the first billing interval.
    fn advance_one_interval(&self) -> u64 {
        self.env.ledger().with_mut(|l| l.timestamp += INTERVAL + 1);
        self.env.ledger().timestamp()
    }

    fn status(&self) -> SubscriptionStatus {
        self.client.get_subscription(&self.id).status
    }
}

#[test]
fn admin_charge_persists_the_state_machine_transition_to_insufficient_balance() {
    // Success path end to end: `charge_core` applies `Active ->
    // InsufficientBalance` through this exact gate, so the applied value is
    // observable in storage. `grace_period = 0` selects the direct edge.
    let f = setup_underfunded(0);
    let now = f.advance_one_interval();

    assert_eq!(
        f.client.try_charge_subscription(&f.id, &None::<BytesN<32>>),
        Ok(Ok(ChargeExecutionResult::InsufficientBalance))
    );
    assert_eq!(f.status(), InsufficientBalance);

    let sub = f.client.get_subscription(&f.id);
    assert_eq!(sub.prepaid_balance, 0, "no funds move on a failed charge");
    assert_eq!(
        sub.last_payment_timestamp, T0,
        "a failed charge must not advance the billing clock"
    );
    assert!(
        sub.grace_start_timestamp.is_none(),
        "the direct edge does not start a grace clock (now = {now})"
    );
}

#[test]
fn admin_charge_cannot_enter_grace_period_from_active() {
    // With a configured grace period `charge_core` takes the
    // `Active -> GracePeriod` branch, but the live matrix has no such edge, so
    // `transition_to` rejects it and the `?` aborts the charge. The stored
    // status is left untouched, exactly as the atomicity contract requires.
    // Divergence: the docs and the unused `src/state_machine.rs` both allow
    // `Active -> GracePeriod`. Pinned, not fixed.
    let f = setup_underfunded(7 * 24 * 60 * 60);
    let now = f.advance_one_interval();

    assert_eq!(
        f.client.try_charge_subscription(&f.id, &None::<BytesN<32>>),
        Err(Ok(Error::InvalidStatusTransition))
    );
    assert_eq!(
        f.status(),
        Active,
        "the rejected transition must not persist"
    );
    let sub = f.client.get_subscription(&f.id);
    assert_eq!(sub.prepaid_balance, 0);
    assert_eq!(
        sub.last_payment_timestamp, T0,
        "no clock advance at t = {now}"
    );
    assert!(sub.grace_start_timestamp.is_none());
}

#[test]
fn unauthorized_charge_caller_cannot_reach_the_state_machine() {
    // `charge_subscription` is admin-only: `require_stored_admin_auth` runs
    // before any state is read, so an unauthenticated caller is rejected by the
    // host and no status transition is attempted.
    let f = setup_underfunded(0);
    f.advance_one_interval();
    f.env.mock_auths(&[]);

    assert!(
        f.client
            .try_charge_subscription(&f.id, &None::<BytesN<32>>)
            .is_err(),
        "an unsigned charge must be rejected"
    );
    assert_eq!(f.status(), Active, "status must be untouched");
    assert_eq!(f.client.get_subscription(&f.id).prepaid_balance, 0);
}

#[test]
fn charge_signed_by_a_non_admin_cannot_reach_the_state_machine() {
    // A valid signature from the wrong account is not enough: the stored admin
    // is loaded from storage and required to sign, so a stranger's signature
    // cannot authorize a charge.
    let f = setup_underfunded(0);
    let now = f.advance_one_interval();

    let mut args = Vec::new(&f.env);
    args.push_back(f.id.into_val(&f.env));
    args.push_back(None::<BytesN<32>>.into_val(&f.env));
    let stranger = Address::generate(&f.env);
    let invoke = MockAuthInvoke {
        contract: &f.client.address,
        fn_name: "charge_subscription",
        args,
        sub_invokes: &[],
    };
    f.env.mock_auths(&[MockAuth {
        address: &stranger,
        invoke: &invoke,
    }]);

    assert!(
        f.client
            .try_charge_subscription(&f.id, &None::<BytesN<32>>)
            .is_err(),
        "a charge signed by a non-admin must be rejected"
    );
    assert_eq!(f.status(), Active, "status must be untouched");
    assert_eq!(
        f.client.get_subscription(&f.id).last_payment_timestamp,
        T0,
        "no charge may be recorded at t = {now}"
    );
}
