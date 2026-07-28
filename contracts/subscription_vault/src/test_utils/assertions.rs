use crate::{SubscriptionStatus, SubscriptionVaultClient};
use soroban_sdk::{Address, Env};

/// Assert that a subscription has the expected status.
pub fn assert_status(
    client: &SubscriptionVaultClient,
    subscription_id: &u32,
    expected: SubscriptionStatus,
) {
    let sub = client.get_subscription(subscription_id);
    assert_eq!(sub.status, expected, "Subscription status mismatch");
}

/// Assert that a subscription has the expected prepaid balance.
pub fn assert_prepaid_balance(
    client: &SubscriptionVaultClient,
    subscription_id: &u32,
    expected: i128,
) {
    let sub = client.get_subscription(subscription_id);
    assert_eq!(
        sub.prepaid_balance, expected,
        "Subscription prepaid balance mismatch"
    );
}

/// Assert that a merchant has the expected balance in a specific token.
pub fn assert_merchant_balance(
    client: &SubscriptionVaultClient,
    merchant: &Address,
    token: &Address,
    expected: i128,
) {
    let balance = client.get_merchant_balance_by_token(merchant, token);
    assert_eq!(balance, expected, "Merchant token balance mismatch");
}

/// Assert that a token balance for a specific address matches expectation.
pub fn assert_token_balance(env: &Env, token_addr: &Address, addr: &Address, expected: i128) {
    let token_client = soroban_sdk::token::Client::new(env, token_addr);
    assert_eq!(
        token_client.balance(addr),
        expected,
        "Token balance mismatch"
    );
}
