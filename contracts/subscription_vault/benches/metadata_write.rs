//! Metadata write cost benchmark for `set_metadata` (#615).
//!
//! Measures the CPU-instruction cost of writing a metadata key at 1, 5, and
//! `MAX_METADATA_KEYS` (10) key counts, to detect superlinear growth in the
//! metadata storage layout. Each `set_metadata` call rewrites the
//! subscription's `DataKey::MetadataKeys` `Vec<String>` (see
//! `crate::metadata::apply_metadata_value`), so the cost of the n-th write is
//! expected to grow roughly linearly with n. This test fails if the cost at
//! `MAX_METADATA_KEYS` grows superlinearly (beyond 2x the linear projection),
//! acting as a canary for accidental storage churn in the metadata path.
//!
//! # Scenarios
//! 1. **1 key**  - first metadata write on a fresh subscription.
//! 2. **5 keys** - fifth write (4 keys already present).
//! 3. **MAX_METADATA_KEYS** - tenth write (9 keys already present, still under the cap).

#![cfg(test)]

use soroban_sdk::{
    testutils::Address as _,
    Address, Env, String, Symbol,
};
use subscription_vault::{types::MAX_METADATA_KEYS, SubscriptionVault, SubscriptionVaultClient};

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;

fn setup() -> (Env, SubscriptionVaultClient<'static>, u32, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6u32, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<Symbol>,
    );

    (env, client, sub_id, subscriber)
}

fn measure_nth_write(n: u32) -> u64 {
    let (env, client, sub_id, authorizer) = setup();

    for i in 1..n {
        let key = String::from_str(&env, &format!("key_{}", i));
        let value = String::from_str(&env, "benchmark-value");
        client.set_metadata(&sub_id, &authorizer, &key, &value);
    }

    env.cost_estimate().budget().reset_unlimited();
    let key = String::from_str(&env, &format!("key_{}", n));
    let value = String::from_str(&env, "benchmark-value");
    client.set_metadata(&sub_id, &authorizer, &key, &value);
    env.cost_estimate().resources().instructions.max(0) as u64
}

#[test]
fn bench_metadata_write_scaling() {
    let cost_1 = measure_nth_write(1);
    let cost_5 = measure_nth_write(5);
    let cost_max = measure_nth_write(MAX_METADATA_KEYS);

    std::println!(
        "[metadata_write] cost@1={} cost@5={} cost@MAX={}",
        cost_1,
        cost_5,
        cost_max
    );

    assert!(
        cost_max >= cost_5 && cost_5 >= cost_1,
        "metadata write cost must grow monotonically with key count (cost@1={}, cost@5={}, cost@MAX={})",
        cost_1,
        cost_5,
        cost_max
    );

    let linear_projection = cost_1.saturating_mul(MAX_METADATA_KEYS as u64);
    assert!(
        cost_max <= linear_projection.saturating_mul(2),
        "metadata write cost is superlinear: cost@MAX={} exceeds 2x linear projection {}",
        cost_max,
        linear_projection
    );
}
