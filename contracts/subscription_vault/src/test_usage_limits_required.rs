#![cfg(test)]

use crate::{
    test_utils::setup::TestEnv,
    types::{Error, UsageLimits},
};
use soroban_sdk::{testutils::Address as _, Address, String, Symbol};

const AMOUNT: i128 = 10_000_000; // 1.0 USDC
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days

#[test]
fn test_usage_limits_required() {
    let TestEnv { env, client, .. } = TestEnv::default();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // 1. Try to create subscription with usage_enabled = true but no limits
    let res = client.try_create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &true,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
    );

    assert_eq!(res.err().unwrap().unwrap(), Error::UsageLimitsRequired);

    // Check that ID was NOT consumed
    let id_query: u32 = env
        .as_contract(&client.address, || {
            crate::admin::read_config(&env, &crate::types::DataKey::NextId).unwrap_or(0)
        });
    assert_eq!(id_query, 0);

    // 2. Create subscription with usage_enabled = false
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
    );
    assert_eq!(id, 0);

    // Check that ID WAS consumed
    let id_query: u32 = env
        .as_contract(&client.address, || {
            crate::admin::read_config(&env, &crate::types::DataKey::NextId).unwrap_or(0)
        });
    assert_eq!(id_query, 1);

    // 3. Set usage limits for the NextId (pre-registration)
    let next_id = id_query;
    client.configure_usage_limits(
        &merchant,
        &next_id,
        &Some(100), // rate_limit_max_calls
        &3600,      // rate_window_secs
        &10,        // burst_min_interval_secs
        &None::<i128>,
    );

    // 4. Create subscription with usage_enabled = true (now succeeds)
    let new_sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &true,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
    );
    assert_eq!(new_sub_id, 1);

    // 5. Clear limits after creation
    client.configure_usage_limits(
        &merchant,
        &new_sub_id,
        &None, // rate_limit_max_calls
        &0,    // rate_window_secs
        &0,    // burst_min_interval_secs
        &None::<i128>,
    );
    
    // Validate they were cleared
    let limits_key = crate::types::DataKey::UsageLimits(new_sub_id);
    let limits_after = env.as_contract(&client.address, || {
        env.storage().instance().get::<_, UsageLimits>(&limits_key)
    });
    
    assert!(limits_after.is_some());
    let limits_val = limits_after.unwrap();
    assert_eq!(limits_val.rate_limit_max_calls, None);
    assert_eq!(limits_val.rate_window_secs, 0);
    assert_eq!(limits_val.burst_min_interval_secs, 0);
    assert_eq!(limits_val.usage_cap_units, None);
}
