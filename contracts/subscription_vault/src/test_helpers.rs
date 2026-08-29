// Helper functions for emergency stop tests

use crate::{Error, SubscriptionVaultClient};
use soroban_sdk::{Address, Env, String, Vec};

/// Runs all mutating entrypoints on the contract and returns a vector of their results.
/// The caller can assert on the errors or successes as needed.
pub fn run_all_mutation_calls(
    env: &Env,
    client: &SubscriptionVaultClient,
    sub_id: &Address,
    subscriber: &Address,
    merchant: &Address,
    operator: &Address,
    token: &Address,
) -> Vec<Result<(), Error>> {
    let mut results = Vec::new();

    // try_create_subscription (should be blocked when emergency stop is active)
    results.push(client.try_create_subscription(
        subscriber,
        merchant,
        &1_000_000i128,
        &30 * 24 * 60 * 60,
        &false,
        &None::<i128>,
        &None::<u64>,
    ).map(|_| ( )));

    // try_create_subscription_with_token
    results.push(client.try_create_subscription_with_token(
        subscriber,
        merchant,
        token,
        &1_000_000i128,
        &30 * 24 * 60 * 60,
        &false,
        &None::<i128>,
        &None::<u64>,
    ).map(|_| ( )));

    // try_create_subscription_from_plan (requires a plan_id, but we can reuse a dummy)
    results.push(client.try_create_subscription_from_plan(subscriber, sub_id).map(|_| ( )));

    // try_deposit_funds
    results.push(client.try_deposit_funds(
        sub_id,
        subscriber,
        &1_000_000i128,
        &None::<soroban_sdk::BytesN<32>>,
    ).map(|_| ( )));

    // try_charge_subscription
    results.push(client.try_charge_subscription(sub_id, &None::<soroban_sdk::BytesN<32>>).map(|_| ( )));

    // try_charge_usage
    results.push(client.try_charge_usage(sub_id, &100_000i128).map(|_| ( )));

    // try_charge_usage_with_reference
    results.push(client.try_charge_usage_with_reference(
        sub_id,
        &100_000i128,
        &String::from_str(env, "usage-ref"),
    ).map(|_| ( )));

    // try_charge_one_off
    results.push(client.try_charge_one_off(
        sub_id,
        merchant,
        &100_000i128,
        &None::<soroban_sdk::BytesN<32>>,
    ).map(|_| ( )));

    // operator batch charge
    let ids_vec = Vec::from_array(env, [*sub_id]);
    results.push(client.try_operator_batch_charge(operator, &ids_vec, &0u64).map(|_| ( )));
    results.push(client.try_operator_charge_subscription(operator, sub_id).map(|_| ( )));
    results.push(client.try_operator_charge_usage(operator, sub_id, &100_000i128).map(|_| ( )));
    results.push(client.try_operator_charge_usage_with_ref(
        operator,
        sub_id,
        &100_000i128,
        &String::from_str(env, "oref"),
    ).map(|_| ( )));

    // partial refund
    results.push(client.try_partial_refund(admin, sub_id, subscriber, &1_000_000i128).map(|_| ( )));

    results
}
