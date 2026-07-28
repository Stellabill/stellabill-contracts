use crate::{SubscriptionStatus, SubscriptionVaultClient, types::DataKey};
use soroban_sdk::{testutils::Address as _, Address, Env, Symbol};

const DEFAULT_AMOUNT: i128 = 10_000_000;
const DEFAULT_INTERVAL: u64 = 30 * 24 * 60 * 60;

/// Create a test subscription with the given status and default amount/interval.
///
/// Alias for `create_test_subscription` to match naming in some tests.
pub fn create_subscription(
    env: &Env,
    client: &SubscriptionVaultClient,
    status: SubscriptionStatus,
) -> (u32, Address, Address) {
    create_test_subscription(env, client, status)
}

/// Create a test subscription with explicit amount and interval.
pub fn create_subscription_detailed(
    env: &Env,
    client: &SubscriptionVaultClient,
    status: SubscriptionStatus,
    amount: i128,
    interval: u64,
) -> (u32, Address, Address) {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    if status != SubscriptionStatus::Active {
        patch_status(env, client, id, status);
    }

    (id, subscriber, merchant)
}

/// Create a test subscription with a specific merchant.
pub fn create_subscription_with_merchant(
    env: &Env,
    client: &SubscriptionVaultClient,
    status: SubscriptionStatus,
    merchant: Address,
) -> (u32, Address, Address) {
    let subscriber = Address::generate(env);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &DEFAULT_AMOUNT,
        &DEFAULT_INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    if status != SubscriptionStatus::Active {
        patch_status(env, client, id, status);
    }

    (id, subscriber, merchant)
}

/// Standard test subscription helper (3 args).
pub fn create_test_subscription(
    env: &Env,
    client: &SubscriptionVaultClient,
    status: SubscriptionStatus,
) -> (u32, Address, Address) {
    create_subscription_detailed(env, client, status, DEFAULT_AMOUNT, DEFAULT_INTERVAL)
}

/// Test subscription helper with specific merchant (4 args).
pub fn create_test_subscription_with_merchant(
    env: &Env,
    client: &SubscriptionVaultClient,
    status: SubscriptionStatus,
    merchant: Address,
) -> (u32, Address, Address) {
    create_subscription_with_merchant(env, client, status, merchant)
}

/// Directly patch the status of a subscription in storage.
pub fn patch_status(
    env: &Env,
    client: &SubscriptionVaultClient,
    id: u32,
    status: SubscriptionStatus,
) {
    let mut sub = client.get_subscription(&id);
    sub.status = status;
    env.as_contract(&client.address, || {
        env.storage().instance().set(&DataKey::Sub(id), &sub);
    });
}

/// Directly seed the prepaid balance of a subscription in storage.
pub fn seed_balance(env: &Env, client: &SubscriptionVaultClient, id: u32, balance: i128) {
    let mut sub = client.get_subscription(&id);
    sub.prepaid_balance = balance;
    env.as_contract(&client.address, || {
        env.storage().instance().set(&DataKey::Sub(id), &sub);
    });
}

/// Seed the `next_id` counter to an arbitrary value.
pub fn seed_counter(env: &Env, contract_id: &Address, value: u32) {
    env.as_contract(contract_id, || {
        env.storage()
            .instance()
            .set(&DataKey::NextId, &value);
    });
}
