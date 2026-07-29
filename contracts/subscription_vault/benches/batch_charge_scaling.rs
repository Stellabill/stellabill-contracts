#![cfg(test)]

use soroban_sdk::{testutils::{Address as _, Ledger}, Address, Env, Vec};
use subscription_vault::{SubscriptionVault, SubscriptionVaultClient, types::SubscriptionStatus};
use std::fs::OpenOptions;
use std::io::Write;

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const PREPAID: i128 = 50_000_000;

fn setup_env<'a>(env: &'a Env) -> (SubscriptionVaultClient<'a>, Address, Address) {
    env.mock_all_auths();
    env.budget().reset_unlimited();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    let admin = Address::generate(env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    (client, token, admin)
}

#[test]
fn bench_batch_charge_scaling() {
    let sizes = [1, 10, 50, 100];
    let mut results_csv = String::from("Scenario,Size,CPU_Cost,Cost_Per_Item\n");

    let scenarios = ["Success", "AllFail", "SingleId", "WithPaused"];

    for scenario in scenarios {
        let mut prev_cost_per_item = u64::MAX;

        for &size in sizes.iter() {
            let env = Env::default();
            let (client, token, _admin) = setup_env(&env);

            let mut ids = Vec::new(&env);
            let mut duplicate_id = 0;

            for i in 0..size {
                let subscriber = Address::generate(&env);
                let merchant = Address::generate(&env);
                
                let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
                token_client.mint(&subscriber, &PREPAID);

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

                if scenario == "Success" || scenario == "WithPaused" {
                    client.top_up_subscription(&subscriber, &id, &PREPAID);
                }

                if scenario == "WithPaused" && i % 2 == 0 {
                    client.pause_subscription(&subscriber, &id);
                }

                if scenario == "SingleId" {
                    if i == 0 {
                        duplicate_id = id;
                        client.top_up_subscription(&subscriber, &id, &PREPAID);
                    }
                    ids.push_back(duplicate_id);
                } else {
                    ids.push_back(id);
                }
            }

            env.ledger().set_timestamp(INTERVAL + 1);
            env.budget().reset_default();

            client.batch_charge(&ids, &0u64);

            let cpu_cost = env.budget().cpu_instruction_cost();
            let cost_per_item = cpu_cost / (size as u64);

            results_csv.push_str(&format!("{},{},{},{}\n", scenario, size, cpu_cost, cost_per_item));

            if scenario == "Success" {
                if size == 100 {
                    assert!(cost_per_item <= prev_cost_per_item || cost_per_item < prev_cost_per_item + (prev_cost_per_item / 10));
                } else {
                    assert!(cost_per_item < prev_cost_per_item, "Per item cost should decrease. Size: {}, Prev: {}, Current: {}", size, prev_cost_per_item, cost_per_item);
                }
            }

            prev_cost_per_item = cost_per_item;
        }
    }

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open("benches/batch_charge_scaling.csv")
        .unwrap();
    
    file.write_all(results_csv.as_bytes()).unwrap();
}
