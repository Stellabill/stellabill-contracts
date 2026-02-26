//! Subscription status state machine and transition validation.
//!
//! Full subscription lifecycle and on-chain representation are described in `docs/subscription_lifecycle.md`.
//!
//! Kept in a separate module so PRs touching state transitions do not conflict
//! with PRs touching billing, batch charge, or top-up estimation.

use crate::types::{Error, SubscriptionStatus};

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
        ),
        SubscriptionStatus::Paused => {
            matches!(
                to,
                SubscriptionStatus::Active | SubscriptionStatus::Cancelled
            )
        }
        SubscriptionStatus::Cancelled => false,
        SubscriptionStatus::InsufficientBalance => {
            matches!(
                to,
                SubscriptionStatus::Active | SubscriptionStatus::Cancelled
            )
        }
        SubscriptionStatus::GracePeriod => {
            matches!(
                to,
                SubscriptionStatus::Active
                    | SubscriptionStatus::Cancelled
                    | SubscriptionStatus::InsufficientBalance
            )
        }
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
        ],
        SubscriptionStatus::Paused => &[SubscriptionStatus::Active, SubscriptionStatus::Cancelled],
        SubscriptionStatus::Cancelled => &[],
        SubscriptionStatus::InsufficientBalance => {
            &[SubscriptionStatus::Active, SubscriptionStatus::Cancelled]
        }
        SubscriptionStatus::GracePeriod => &[
            SubscriptionStatus::Active,
            SubscriptionStatus::Cancelled,
            SubscriptionStatus::InsufficientBalance,
        ],
    }
}

/// Checks if a transition is valid without returning an error.
///
/// Convenience wrapper around [`validate_status_transition`] for boolean checks.
pub fn can_transition(from: &SubscriptionStatus, to: &SubscriptionStatus) -> bool {
    validate_status_transition(from, to).is_ok()
}
