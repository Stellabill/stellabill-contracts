//! Subscription status state machine and transition validation.
//!
//! Full subscription lifecycle and on-chain representation are described in `docs/subscription_lifecycle.md`.
//!
//! Kept in a separate module so PRs touching state transitions do not conflict
//! with PRs touching billing, batch charge, or top-up estimation.
//!
//! # Design Principles
//!
//! All state transitions are:
//! - **Explicit**: No hidden implicit transitions; every transition goes through `transition_to`
//! - **Total**: Exhaustive match over all states (compiler-enforced)
//! - **Atomic**: Validation happens before any state mutation
//! - **Safe**: Invalid transitions fail without partial accounting changes

use crate::types::{Error, SubscriptionStatus};

/// Executes a validated state transition on a mutable subscription status.
///
/// This is the **single authoritative function** for all status changes in the contract.
/// Every entrypoint that modifies subscription status MUST call this function.
///
/// # Arguments
/// * `current` - Mutable reference to the subscription's current status
/// * `target` - The desired target status
///
/// # Returns
/// * `Ok(())` if transition is valid and applied
/// * `Err(Error::InvalidStatusTransition)` if transition is invalid (no mutation occurs)
///
/// # Security
///
/// Validation happens before mutation, ensuring atomic transitions.
/// If validation fails, the `current` status remains unchanged.
///
/// # Example
///
/// ```rust,ignore
/// // In an entrypoint:
/// let mut sub = get_subscription(env, id)?;
/// transition_to(&mut sub.status, SubscriptionStatus::Paused)?;
/// env.storage().instance().set(&DataKey::Sub(id), &sub);
/// ```
pub fn transition_to(
    current: &mut SubscriptionStatus,
    target: SubscriptionStatus,
) -> Result<(), Error> {
    validate_status_transition(current, &target)?;
    *current = target;
    Ok(())
}

/// Validates if a status transition is allowed by the state machine.
///
/// # State Transition Rules
///
/// | From              | To                  | Allowed |
/// |-------------------|---------------------|---------|
/// | Active            | Paused              | Yes     |
/// | Active            | Cancelled           | Yes     |
/// | Active            | InsufficientBalance | Yes     |
/// | Paused            | Active              | Yes     |
/// | Paused            | Cancelled           | Yes     |
/// | InsufficientBalance | Active            | Yes     |
/// | InsufficientBalance | Cancelled         | Yes     |
/// | Cancelled         | *any*               | No      |
/// | *any*             | Same status         | Yes (idempotent) |
///
/// # Arguments
/// * `from` - Current status
/// * `to` - Target status
///
/// # Returns
/// * `Ok(())` if transition is valid
/// * `Err(Error::InvalidStatusTransition)` if transition is invalid
pub fn validate_status_transition(
    from: &SubscriptionStatus,
    to: &SubscriptionStatus,
) -> Result<(), Error> {
    if from == to {
        return Ok(());
    }

    let valid = match from {
        SubscriptionStatus::Active => matches!(
            to,
            SubscriptionStatus::Paused
                | SubscriptionStatus::Cancelled
                | SubscriptionStatus::InsufficientBalance
                | SubscriptionStatus::GracePeriod
                | SubscriptionStatus::Expired
        ),
        SubscriptionStatus::Paused => {
            matches!(
                to,
                SubscriptionStatus::Active
                    | SubscriptionStatus::Cancelled
                    | SubscriptionStatus::Expired
            )
        }
        SubscriptionStatus::Cancelled => matches!(to, SubscriptionStatus::Archived),
        SubscriptionStatus::InsufficientBalance => {
            matches!(
                to,
                SubscriptionStatus::Active
                    | SubscriptionStatus::Cancelled
                    | SubscriptionStatus::Expired
            )
        }
        SubscriptionStatus::GracePeriod => {
            matches!(
                to,
                SubscriptionStatus::Active
                    | SubscriptionStatus::Cancelled
                    | SubscriptionStatus::InsufficientBalance
                    | SubscriptionStatus::Expired
            )
        }
        SubscriptionStatus::Expired => matches!(to, SubscriptionStatus::Archived),
        SubscriptionStatus::Archived => false,
    };

    if valid {
        Ok(())
    } else {
        Err(Error::InvalidStatusTransition)
    }
}

/// Returns all valid target statuses for a given current status.
///
/// This is useful for UI/documentation to show available actions.
pub fn get_allowed_transitions(status: &SubscriptionStatus) -> &'static [SubscriptionStatus] {
    match status {
        SubscriptionStatus::Active => &[
            SubscriptionStatus::Paused,
            SubscriptionStatus::Cancelled,
            SubscriptionStatus::InsufficientBalance,
            SubscriptionStatus::GracePeriod,
            SubscriptionStatus::Expired,
        ],
        SubscriptionStatus::Paused => &[
            SubscriptionStatus::Active,
            SubscriptionStatus::Cancelled,
            SubscriptionStatus::Expired,
        ],
        SubscriptionStatus::Cancelled => &[SubscriptionStatus::Archived],
        SubscriptionStatus::InsufficientBalance => &[
            SubscriptionStatus::Active,
            SubscriptionStatus::Cancelled,
            SubscriptionStatus::Expired,
        ],
        SubscriptionStatus::GracePeriod => &[
            SubscriptionStatus::Active,
            SubscriptionStatus::Cancelled,
            SubscriptionStatus::InsufficientBalance,
            SubscriptionStatus::Expired,
        ],
        SubscriptionStatus::Expired => &[SubscriptionStatus::Archived],
        SubscriptionStatus::Archived => &[],
    }
}

/// Checks if a transition is valid without returning an error.
///
/// Convenience wrapper around [`validate_status_transition`] for boolean checks.
pub fn can_transition(from: &SubscriptionStatus, to: &SubscriptionStatus) -> bool {
    validate_status_transition(from, to).is_ok()
}
