// Standalone state machine test - verifies transition_to() works correctly
// This file isolates the state machine tests from other pre-existing compilation errors

#[cfg(test)]
mod state_machine_standalone_tests {
    use crate::state_machine::transition_to;
    use crate::types::{Error, SubscriptionStatus};

    #[test]
    fn test_basic_valid_transitions() {
        // Test Active -> Paused
        let mut status = SubscriptionStatus::Active;
        assert!(transition_to(&mut status, SubscriptionStatus::Paused).is_ok());
        assert_eq!(status, SubscriptionStatus::Paused);

        // Test Paused -> Active
        let mut status = SubscriptionStatus::Paused;
        assert!(transition_to(&mut status, SubscriptionStatus::Active).is_ok());
        assert_eq!(status, SubscriptionStatus::Active);

        // Test Active -> Cancelled
        let mut status = SubscriptionStatus::Active;
        assert!(transition_to(&mut status, SubscriptionStatus::Cancelled).is_ok());
        assert_eq!(status, SubscriptionStatus::Cancelled);
    }

    #[test]
    fn test_basic_invalid_transitions() {
        // Test Cancelled -> Active (should fail)
        let mut status = SubscriptionStatus::Cancelled;
        let result = transition_to(&mut status, SubscriptionStatus::Active);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), Error::InvalidStatusTransition);
        // Verify status didn't change
        assert_eq!(status, SubscriptionStatus::Cancelled);

        // Test Archived -> anything (should fail)
        let mut status = SubscriptionStatus::Archived;
        let result = transition_to(&mut status, SubscriptionStatus::Active);
        assert!(result.is_err());
        assert_eq!(status, SubscriptionStatus::Archived);
    }

    #[test]
    fn test_idempotent_transitions() {
        // Same-state transitions should be allowed (idempotent)
        let states = [
            SubscriptionStatus::Active,
            SubscriptionStatus::Paused,
            SubscriptionStatus::Cancelled,
            SubscriptionStatus::GracePeriod,
            SubscriptionStatus::InsufficientBalance,
            SubscriptionStatus::Expired,
            SubscriptionStatus::Archived,
        ];

        for state in states {
            let mut status = state.clone();
            assert!(
                transition_to(&mut status, state.clone()).is_ok(),
                "Same-state transition should be allowed for {:?}",
                state
            );
            assert_eq!(status, state);
        }
    }

    #[test]
    fn test_grace_period_transitions() {
        // Active -> GracePeriod
        let mut status = SubscriptionStatus::Active;
        assert!(transition_to(&mut status, SubscriptionStatus::GracePeriod).is_ok());
        assert_eq!(status, SubscriptionStatus::GracePeriod);

        // GracePeriod -> Active (recovery)
        let mut status = SubscriptionStatus::GracePeriod;
        assert!(transition_to(&mut status, SubscriptionStatus::Active).is_ok());
        assert_eq!(status, SubscriptionStatus::Active);

        // GracePeriod -> InsufficientBalance
        let mut status = SubscriptionStatus::GracePeriod;
        assert!(transition_to(&mut status, SubscriptionStatus::InsufficientBalance).is_ok());
        assert_eq!(status, SubscriptionStatus::InsufficientBalance);
    }

    #[test]
    fn test_insufficient_balance_transitions() {
        // GracePeriod -> InsufficientBalance
        let mut status = SubscriptionStatus::GracePeriod;
        assert!(transition_to(&mut status, SubscriptionStatus::InsufficientBalance).is_ok());

        // InsufficientBalance -> Active (recovery)
        let mut status = SubscriptionStatus::InsufficientBalance;
        assert!(transition_to(&mut status, SubscriptionStatus::Active).is_ok());

        // InsufficientBalance -> Cancelled
        let mut status = SubscriptionStatus::InsufficientBalance;
        assert!(transition_to(&mut status, SubscriptionStatus::Cancelled).is_ok());
    }

    #[test]
    fn test_expiration_transitions() {
        // Active -> Expired
        let mut status = SubscriptionStatus::Active;
        assert!(transition_to(&mut status, SubscriptionStatus::Expired).is_ok());

        // Expired -> Archived
        let mut status = SubscriptionStatus::Expired;
        assert!(transition_to(&mut status, SubscriptionStatus::Archived).is_ok());

        // Paused -> Expired
        let mut status = SubscriptionStatus::Paused;
        assert!(transition_to(&mut status, SubscriptionStatus::Expired).is_ok());
    }

    #[test]
    fn test_terminal_states() {
        // Cancelled -> Archived
        let mut status = SubscriptionStatus::Cancelled;
        assert!(transition_to(&mut status, SubscriptionStatus::Archived).is_ok());

        // Cancelled cannot go back to Active
        let mut status = SubscriptionStatus::Cancelled;
        assert!(transition_to(&mut status, SubscriptionStatus::Active).is_err());

        // Archived is final - no outgoing transitions
        let mut status = SubscriptionStatus::Archived;
        assert!(transition_to(&mut status, SubscriptionStatus::Active).is_err());
        assert!(transition_to(&mut status, SubscriptionStatus::Cancelled).is_err());
    }

    #[test]
    fn test_complex_lifecycle_sequence() {
        // Simulate: Active -> Paused -> Active -> GracePeriod -> Active -> Cancelled -> Archived
        let mut status = SubscriptionStatus::Active;

        // Pause
        transition_to(&mut status, SubscriptionStatus::Paused).unwrap();
        assert_eq!(status, SubscriptionStatus::Paused);

        // Resume
        transition_to(&mut status, SubscriptionStatus::Active).unwrap();
        assert_eq!(status, SubscriptionStatus::Active);

        // Underfunded -> GracePeriod
        transition_to(&mut status, SubscriptionStatus::GracePeriod).unwrap();
        assert_eq!(status, SubscriptionStatus::GracePeriod);

        // Recover
        transition_to(&mut status, SubscriptionStatus::Active).unwrap();
        assert_eq!(status, SubscriptionStatus::Active);

        // Cancel
        transition_to(&mut status, SubscriptionStatus::Cancelled).unwrap();
        assert_eq!(status, SubscriptionStatus::Cancelled);

        // Archive
        transition_to(&mut status, SubscriptionStatus::Archived).unwrap();
        assert_eq!(status, SubscriptionStatus::Archived);

        // Try to reactivate (should fail)
        assert!(transition_to(&mut status, SubscriptionStatus::Active).is_err());
    }
}
