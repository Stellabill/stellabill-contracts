//! Metadata write/read cost benchmark by key count (#615).
//!
//! Measures the CPU-instruction cost of `set_metadata` (write) and
//! `get_metadata` (read) at 1, 5 and `MAX_METADATA_KEYS` key counts,
//! recording read and write costs separately, and asserts the per-key write
//! cost does not grow superlinearly as the key count grows — a canary for an
//! accidental superlinear metadata storage layout.
//!
//! Results are written to `benches/metadata_write.csv` so CI logs can be used
//! for regression tracking. The test itself fails (and therefore the CI run)
//! when the per-key write cost at `MAX_METADATA_KEYS` exceeds the single-key
//! write cost by more than `SUPERLINEAR_TOLERANCE`, i.e. when the metadata
//! layout degrades from ~O(1) to superlinear.
//!
//! Edge cases covered: key at `MAX_METADATA_KEY_LENGTH`, value at
//! `MAX_METADATA_VALUE_LENGTH`, and idempotent overwrite of an existing key.
#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env,
};
use subscription_vault::{
    SubscriptionVault, SubscriptionVaultClient, MAX_METADATA_KEYS, MAX_METADATA_KEY_LENGTH,
    MAX_METADATA_VALUE_LENGTH,
};
use std::fs::OpenOptions;
use std::io::Write;

/// Deposit amount used to create the subscription under test (USDC stroops).
const AMOUNT: i128 = 10_000_000;

/// Billing interval used to create the subscription under test.
const INTERVAL: u64 = 30 * 24 * 60 * 60;

/// Per-key write cost at `MAX_METADATA_KEYS` may exceed the single-key cost by
/// at most this factor before the benchmark flags superlinear growth.
const SUPERLINEAR_TOLERANCE: u64 = 3;

fn setup_env<'a>(env: &'a Env) -> (SubscriptionVaultClient<'a>, Address) {
    env.mock_all_auths();
    env.budget().reset_unlimited();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    let admin = Address::generate(env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    (client, admin)
}

/// Deterministic key for index `i` (well under `MAX_METADATA_KEY_LENGTH`).
fn key_for(env: &Env, i: u32) -> soroban_sdk::String {
    soroban_sdk::String::from_str(env, &format!("metadata_key_{:02}", i))
}

/// Key at the maximum permitted length (`MAX_METADATA_KEY_LENGTH`).
fn max_length_key(env: &Env) -> soroban_sdk::String {
    soroban_sdk::String::from_str(env, &"k".repeat(MAX_METADATA_KEY_LENGTH as usize))
}

/// Value at the maximum permitted length (`MAX_METADATA_VALUE_LENGTH`).
fn max_length_value(env: &Env) -> soroban_sdk::String {
    soroban_sdk::String::from_str(env, &"v".repeat(MAX_METADATA_VALUE_LENGTH as usize))
}

/// Runs `f` and returns the CPU-instruction cost it incurred.
fn measure(env: &Env, f: impl FnOnce()) -> u64 {
    let before = env.budget().cpu_instruction_cost();
    f();
    env.budget().cpu_instruction_cost() - before
}

#[test]
fn bench_metadata_write_cost_by_key_count() {
    let mut csv =
        String::from("key_count,write_cpu,write_cpu_per_key,read_cpu,read_cpu_per_key\n");

    let mut single_key_write_per_key = 0u64;
    let mut max_write_per_key = 0u64;

    for key_count in [1u32, 5u32, MAX_METADATA_KEYS] {
        let env = Env::default();
        let (client, _admin) = setup_env(&env);

        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);
        let id = client.create_subscription(
            &subscriber,
            &merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
            &None::<Address>,
        );

        // Write phase: set `key_count` distinct keys at max value length, then
        // overwrite the first key (idempotent overwrite edge case).
        let write_cost = measure(&env, || {
            for i in 0..key_count {
                let _ = client.set_metadata(
                    &id,
                    &subscriber,
                    &key_for(&env, i),
                    &max_length_value(&env),
                );
            }
            let _ = client.set_metadata(
                &id,
                &subscriber,
                &key_for(&env, 0),
                &max_length_value(&env),
            );
        });
        let write_per_key = write_cost / key_count as u64;

        // Read phase: read every key back.
        let read_cost = measure(&env, || {
            for i in 0..key_count {
                let _ = client.get_metadata(&id, &key_for(&env, i));
            }
        });
        let read_per_key = read_cost / key_count as u64;

        if key_count == 1 {
            single_key_write_per_key = write_per_key;
        }
        if key_count == MAX_METADATA_KEYS {
            max_write_per_key = write_per_key;
        }

        csv.push_str(&format!(
            "{},{},{},{},{}\n",
            key_count, write_cost, write_per_key, read_cost, read_per_key
        ));
    }

    // Superlinear-growth canary: per-key write cost must not blow up as the
    // number of stored keys grows. A linear layout stays ~constant per key; a
    // superlinear layout would see per-key cost rise with key count.
    assert!(
        max_write_per_key <= single_key_write_per_key.saturating_mul(SUPERLINEAR_TOLERANCE),
        "metadata write cost grows superlinearly: per-key cost at MAX_METADATA_KEYS ({} keys) = {} vs 1 key = {}",
        MAX_METADATA_KEYS,
        max_write_per_key,
        single_key_write_per_key
    );

    // Edge case: max-length key and max-length value are accepted.
    {
        let env = Env::default();
        let (client, _admin) = setup_env(&env);
        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);
        let id = client.create_subscription(
            &subscriber,
            &merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
            &None::<Address>,
        );
        assert!(client
            .set_metadata(&id, &subscriber, &max_length_key(&env), &max_length_value(&env))
            .is_ok());
    }

    // Publish results for CI regression tracking.
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open("benches/metadata_write.csv")
        .unwrap();
    file.write_all(csv.as_bytes()).unwrap();
}
