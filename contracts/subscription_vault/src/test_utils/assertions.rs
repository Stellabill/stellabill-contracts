use crate::{SubscriptionStatus, SubscriptionVaultClient};
use soroban_sdk::Env;

/// Assert that the subscription at `id` has the expected status.
pub fn assert_status(
    client: &SubscriptionVaultClient,
    id: &u32,
    expected: SubscriptionStatus,
) {
    let sub = client.get_subscription(id);
    assert_eq!(
        sub.status, expected,
        "subscription {} status: expected {:?}, got {:?}",
        id, expected, sub.status
    );
}

/// Assert that the subscription at `id` has the expected prepaid balance.
pub fn assert_prepaid_balance(
    client: &SubscriptionVaultClient,
    id: &u32,
    expected: i128,
) {
    let sub = client.get_subscription(id);
    assert_eq!(
        sub.prepaid_balance, expected,
        "subscription {} prepaid_balance: expected {}, got {}",
        id, expected, sub.prepaid_balance
    );
}
