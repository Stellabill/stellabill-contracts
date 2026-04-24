use crate::test_utils::{assertions, fixtures, setup::TestEnv};
use crate::SubscriptionStatus;

#[test]
fn test_new_setup_and_fixtures() {
    let test_env = TestEnv::default();
    let (id, _subscriber, _merchant) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        10_000_000,
        30 * 24 * 60 * 60,
    );

    assertions::assert_status(&test_env.client, &id, SubscriptionStatus::Active);
}
