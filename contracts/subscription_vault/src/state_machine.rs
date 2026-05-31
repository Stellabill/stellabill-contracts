//! State machine: validates and applies subscription status transitions.
//!
//! Every status change MUST go through `transition_to`. The transition matrix
//! is derived from `docs/subscription_state_machine.md`.

use soroban_sdk::{Env, Vec};

use crate::types::{Error, SubscriptionStatus};

/// Returns `true` if transitioning from `from` to `to` is permitted.
///
/// Same-state (idempotent) transitions are always allowed.
pub fn can_transition(from: &SubscriptionStatus, to: &SubscriptionStatus) -> bool {
    if from == to {
        return true;
    }
    use SubscriptionStatus::*;
    matches!(
        (from, to),
        (Active, Paused)
            | (Active, Cancelled)
            | (Active, InsufficientBalance)
            | (Active, GracePeriod)
            | (Active, Expired)
            | (Paused, Active)
            | (Paused, Cancelled)
            | (Paused, Expired)
            | (InsufficientBalance, Active)
            | (InsufficientBalance, Cancelled)
            | (InsufficientBalance, Expired)
            | (GracePeriod, Active)
            | (GracePeriod, Cancelled)
            | (GracePeriod, InsufficientBalance)
            | (GracePeriod, Expired)
            | (Cancelled, Archived)
            | (Expired, Archived)
    )
}

/// Validates a transition, returning `Err(InvalidStatusTransition)` if not allowed.
pub fn validate_status_transition(
    from: &SubscriptionStatus,
    to: &SubscriptionStatus,
) -> Result<(), Error> {
    if can_transition(from, to) {
        Ok(())
    } else {
        Err(Error::InvalidStatusTransition)
    }
}

/// Validates and applies a status transition atomically.
///
/// `current` is only mutated on success; on error it is left unchanged.
pub fn transition_to(
    current: &mut SubscriptionStatus,
    next: SubscriptionStatus,
) -> Result<(), Error> {
    validate_status_transition(current, &next)?;
    *current = next;
    Ok(())
}

/// Returns the set of valid target statuses from `from`, excluding self.
///
/// Used by tests and tooling to enumerate the transition matrix.
pub fn get_allowed_transitions(from: &SubscriptionStatus) -> Vec<SubscriptionStatus> {
    let env = Env::default();
    let mut out: Vec<SubscriptionStatus> = Vec::new(&env);
    use SubscriptionStatus::*;
    let targets: &[SubscriptionStatus] = match from {
        Active => &[Paused, Cancelled, InsufficientBalance, GracePeriod, Expired],
        Paused => &[Active, Cancelled, Expired],
        Cancelled => &[Archived],
        InsufficientBalance => &[Active, Cancelled, Expired],
        GracePeriod => &[Active, Cancelled, InsufficientBalance, Expired],
        Expired => &[Archived],
        Archived => &[],
    };
    for t in targets {
        out.push_back(t.clone());
    }
    out
}
