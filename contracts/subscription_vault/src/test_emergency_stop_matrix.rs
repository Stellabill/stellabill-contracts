#![cfg(test)]

use crate::{
    ChargeExecutionResult, Error, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
    UsageChargeResult,
};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env, String, Vec};

const T0: u64 = 1_700_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const DEPOSIT: i128 = 100_000_000;

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, client, token, admin)
}

#[test]
fn test_emergency_stop_matrix_blocks_mutations_but_allows_reads() {
    let (env, client, token, admin) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&subscriber, &DEPOSIT);

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &1_000_000i128,
        &INTERVAL,
        &true,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&sub_id, &subscriber, &10_000_000i128, &None::<soroban_sdk::BytesN<32>>);

    let plan_id = client.create_plan_template(&merchant, &1_000_000i128, &INTERVAL, &false, &None::<i128>);

    let operator = Address::generate(&env);
    client.set_operator(&admin, &operator);

    client.enable_emergency_stop(&admin);
    assert!(client.get_emergency_stop_status());

    assert_eq!(
        client.try_create_subscription(
            &subscriber,
            &merchant,
            &1_000_000i128,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
        ),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(
        client.try_create_subscription_with_token(
            &subscriber,
            &merchant,
            &token,
            &1_000_000i128,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
        ),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(client.try_create_subscription_from_plan(&subscriber, &plan_id), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_deposit_funds(&sub_id, &subscriber, &1_000_000i128, &None::<soroban_sdk::BytesN<32>>), Err(Ok(Error::EmergencyStopActive)));

    assert_eq!(client.try_charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_charge_usage(&sub_id, &100_000i128), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(
        client.try_charge_usage_with_reference(&sub_id, &100_000i128, &String::from_str(&env, "usage-ref")),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(client.try_charge_one_off(&sub_id, &merchant, &100_000i128, &None::<soroban_sdk::BytesN<32>>), Err(Ok(Error::EmergencyStopActive)));

    let ids_vec = Vec::from_array(&env, [sub_id]);
    assert_eq!(client.try_operator_batch_charge(&operator, &ids_vec, &0u64), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_operator_charge_subscription(&operator, &sub_id), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(client.try_operator_charge_usage(&operator, &sub_id, &100_000i128), Err(Ok(Error::EmergencyStopActive)));
    assert_eq!(
        client.try_operator_charge_usage_with_ref(&operator, &sub_id, &100_000i128, &String::from_str(&env, "oref")),
        Err(Ok(Error::EmergencyStopActive))
    );

    assert_eq!(client.try_partial_refund(&admin, &sub_id, &subscriber, &1_000_000i128), Err(Ok(Error::EmergencyStopActive)));

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(client.get_admin(), admin);
    assert!(client.get_emergency_stop_status());

    client.disable_emergency_stop(&admin);
    assert!(!client.get_emergency_stop_status());

    let resumed = client.create_subscription_from_plan(&subscriber, &plan_id);
    assert_eq!(client.get_subscription(&resumed).status, SubscriptionStatus::Active);
}
