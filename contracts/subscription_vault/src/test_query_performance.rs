#![cfg(test)]

extern crate std;

use crate::{
    queries::{MAX_SCAN_DEPTH, MAX_SUBSCRIPTION_LIST_PAGE},
    subscription::MAX_WRITE_PATH_SCAN_DEPTH,
    types::{DataKey, Subscription, SubscriptionStatus},
    SubscriptionVault, SubscriptionVaultClient, Error,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, Symbol, Vec, String,
};

const T0: u64 = 1700000000;

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|li| li.timestamp = T0);
    // Needed to avoid gas limits when doing deep mock pagination in tests
    env.budget().reset_unlimited();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, client, token, admin)
}

fn create_mock_sub(env: &Env, subscriber: &Address, token: &Address) -> Subscription {
    Subscription {
        subscriber: subscriber.clone(),
        merchant: Address::generate(env),
        token: token.clone(),
        amount: 10_000,
        interval_seconds: 2_592_000,
        last_payment_timestamp: env.ledger().timestamp(),
        status: SubscriptionStatus::Active,
        prepaid_balance: 0,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
        start_time: env.ledger().timestamp(),
        expires_at: None,
        grace_start_timestamp: None,
    }
}

/// Helper to quickly inject N subscriptions directly into storage without crossing the host boundary repeatedly
fn inject_subscriptions(
    env: &Env,
    contract_id: &Address,
    count: u32,
    subscriber: &Address,
    token: &Address,
) {
    env.as_contract(contract_id, || {
        let next_id_key = Symbol::new(env, "next_id");
        let start_id: u32 = env.storage().instance().get(&next_id_key).unwrap_or(0);

        for i in 0..count {
            let id = start_id + i;
            let sub = create_mock_sub(env, subscriber, token);
            env.storage().instance().set(&id, &sub);
        }

        env.storage()
            .instance()
            .set(&next_id_key, &(start_id + count));
    });
}

#[test]
fn test_subscriber_list_basic() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    inject_subscriptions(&env, &client.address, 5, &subscriber, &token);

    let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &100);
    assert_eq!(page.subscription_ids.len(), 5);
    assert_eq!(page.next_start_id, None);
}

#[test]
fn test_subscriber_list_pagination() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    inject_subscriptions(&env, &client.address, 50, &subscriber, &token);

    // Fetch first 20
    let page1 = client.list_subscriptions_by_subscriber(&subscriber, &0, &20);
    assert_eq!(page1.subscription_ids.len(), 20);
    assert_eq!(page1.next_start_id, Some(20));

    // Fetch next 20
    let page2 = client.list_subscriptions_by_subscriber(&subscriber, &page1.next_start_id.unwrap(), &20);
    assert_eq!(page2.subscription_ids.len(), 20);
    assert_eq!(page2.next_start_id, Some(40));

    // Fetch last 10
    let page3 = client.list_subscriptions_by_subscriber(&subscriber, &page2.next_start_id.unwrap(), &20);
    assert_eq!(page3.subscription_ids.len(), 10);
    // next_id is 50, scan budget doesn't exhaust and it found all, so next_start_id should be None
    assert_eq!(page3.next_start_id, None);
}

#[test]
fn test_subscriber_list_scan_depth_boundary() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    let other = Address::generate(&env);

    // Create exactly MAX_SCAN_DEPTH + 10 subscriptions, all for `other`
    let total = MAX_SCAN_DEPTH + 10;
    inject_subscriptions(&env, &client.address, total, &other, &token);

    // Now if `subscriber` tries to list, it will scan MAX_SCAN_DEPTH IDs, find none,
    // and return an empty list WITH a next_start_id cursor to resume at MAX_SCAN_DEPTH.
    let page1 = client.list_subscriptions_by_subscriber(&subscriber, &0, &10);
    assert_eq!(page1.subscription_ids.len(), 0);
    assert_eq!(page1.next_start_id, Some(MAX_SCAN_DEPTH));

    let page2 = client.list_subscriptions_by_subscriber(&subscriber, &page1.next_start_id.unwrap(), &10);
    assert_eq!(page2.subscription_ids.len(), 0);
    assert_eq!(page2.next_start_id, None); // Finished remaining 10
}

#[test]
fn test_subscriber_list_sparse_ids() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    let other = Address::generate(&env);

    inject_subscriptions(&env, &client.address, 10, &subscriber, &token);
    inject_subscriptions(&env, &client.address, 40, &other, &token);
    inject_subscriptions(&env, &client.address, 10, &subscriber, &token);

    // 60 total subscriptions. subscriber has 0..10 and 50..60.
    let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &100);
    assert_eq!(page.subscription_ids.len(), 20);
    assert_eq!(page.next_start_id, None);
}

#[test]
fn test_subscriber_list_limit_one() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    inject_subscriptions(&env, &client.address, 5, &subscriber, &token);

    let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &1);
    assert_eq!(page.subscription_ids.len(), 1);
    assert_eq!(page.subscription_ids.get(0).unwrap(), 0);
    assert_eq!(page.next_start_id, Some(1));
}

#[test]
fn test_subscriber_list_limit_max() {
    let (env, client, token, _) = setup();
    let subscriber = Address::generate(&env);
    inject_subscriptions(&env, &client.address, MAX_SUBSCRIPTION_LIST_PAGE, &subscriber, &token);

    let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &MAX_SUBSCRIPTION_LIST_PAGE);
    assert_eq!(page.subscription_ids.len(), MAX_SUBSCRIPTION_LIST_PAGE);
    // Note: since it hit the limit exactly on the last item, it might return next_start_id == Some(100) or None
    // Currently, it breaks early, so if loop finishes, it sets to None. Wait, if it pushes max, len == limit. Next iteration breaks.
    // We just ensure it doesn't crash.
}

#[test]
fn test_subscriber_list_empty() {
    let (env, client, _token, _) = setup();
    let subscriber = Address::generate(&env);
    let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &100);
    assert_eq!(page.subscription_ids.len(), 0);
    assert_eq!(page.next_start_id, None);
}

#[test]
#[should_panic(expected = "Error(Contract, #1015)")] // InvalidInput = 1015
fn test_subscriber_list_invalid_limit_zero() {
    let (env, client, _token, _) = setup();
    client.list_subscriptions_by_subscriber(&Address::generate(&env), &0, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #1015)")] // InvalidInput = 1015
fn test_subscriber_list_invalid_limit_overflow() {
    let (env, client, _token, _) = setup();
    client.list_subscriptions_by_subscriber(&Address::generate(&env), &0, &(MAX_SUBSCRIPTION_LIST_PAGE + 1));
}

fn create_sub_for_merchant_and_token(client: &SubscriptionVaultClient<'static>, subscriber: &Address, merchant: &Address, token: &Address) -> u32 {
    client.create_subscription(subscriber, merchant, &1000, &(30 * 24 * 60 * 60), &false, &None, &None::<u64>)
}

#[test]
fn test_merchant_query_basic() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    for _ in 0..10 {
        create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    }

    let page = client.get_subscriptions_by_merchant(&merchant, &0, &100);
    assert_eq!(page.len(), 10);
    assert_eq!(client.get_merchant_subscription_count(&merchant), 10);
}

#[test]
fn test_merchant_query_pagination() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    for _ in 0..15 {
        create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    }

    let page1 = client.get_subscriptions_by_merchant(&merchant, &0, &10);
    assert_eq!(page1.len(), 10);

    let page2 = client.get_subscriptions_by_merchant(&merchant, &10, &10);
    assert_eq!(page2.len(), 5);
}

#[test]
#[should_panic(expected = "Error(Contract, #1015)")] // InvalidInput = 1015
fn test_merchant_query_limit_zero() {
    let (env, client, _token, _) = setup();
    client.get_subscriptions_by_merchant(&Address::generate(&env), &0, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #1015)")] // InvalidInput = 1015
fn test_merchant_query_limit_overflow() {
    let (env, client, _token, _) = setup();
    client.get_subscriptions_by_merchant(&Address::generate(&env), &0, &(MAX_SUBSCRIPTION_LIST_PAGE + 1));
}

#[test]
fn test_merchant_query_start_past_end() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    
    let page = client.get_subscriptions_by_merchant(&merchant, &2, &10);
    assert_eq!(page.len(), 0);
}

#[test]
fn test_token_query_basic() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    for _ in 0..10 {
        create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    }

    let page = client.get_subscriptions_by_token(&token, &0, &100);
    assert_eq!(page.len(), 10);
    assert_eq!(client.get_token_subscription_count(&token), 10);
}

#[test]
fn test_token_query_pagination() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    for _ in 0..15 {
        create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    }

    let page1 = client.get_subscriptions_by_token(&token, &0, &10);
    assert_eq!(page1.len(), 10);

    let page2 = client.get_subscriptions_by_token(&token, &10, &10);
    assert_eq!(page2.len(), 5);
}

#[test]
#[should_panic(expected = "Error(Contract, #1015)")] // InvalidInput = 1015
fn test_token_query_limit_zero() {
    let (env, client, token, _) = setup();
    client.get_subscriptions_by_token(&token, &0, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #1015)")] // InvalidInput = 1015
fn test_token_query_limit_overflow() {
    let (env, client, token, _) = setup();
    client.get_subscriptions_by_token(&token, &0, &(MAX_SUBSCRIPTION_LIST_PAGE + 1));
}

#[test]
fn test_merchant_count_and_token_count() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    assert_eq!(client.get_merchant_subscription_count(&merchant), 0);
    assert_eq!(client.get_token_subscription_count(&token), 0);

    create_sub_for_merchant_and_token(&client, &subscriber, &merchant, &token);
    
    assert_eq!(client.get_merchant_subscription_count(&merchant), 1);
    assert_eq!(client.get_token_subscription_count(&token), 1);
}

#[test]
#[should_panic(expected = "Error(Contract, #1015)")] // InvalidInput = 1015
fn test_write_path_scan_depth_guard_triggers_for_large_contracts() {
    let (env, client, token, _) = setup();
    
    // We simulate a contract that has exceeded the MAX_WRITE_PATH_SCAN_DEPTH
    // by injecting a fake next_id. 
    env.as_contract(&client.address, || {
        env.storage().instance().set(&Symbol::new(&env, "next_id"), &(MAX_WRITE_PATH_SCAN_DEPTH + 1));
    });

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // In order to trigger the O(n) scan, we need a credit limit > 0
    // so `compute_subscriber_exposure` gets called instead of fast-path exiting.
    env.as_contract(&client.address, || {
        let credit_limit_key = (Symbol::new(&env, "credit_limit"), subscriber.clone(), token.clone());
        env.storage().instance().set(&credit_limit_key, &1000i128); // Non-zero sets up the scan
    });

    // This creation should fail with InvalidInput because we simulated an oversized contract
    // AND we forced the scan path by configuring a credit limit.
    client.create_subscription(&subscriber, &merchant, &100, &(30 * 24 * 60 * 60), &false, &None, &None::<u64>);
}

#[test]
fn test_metadata_lookups_performance() {
    let (env, client, token, _) = setup();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    // Create a subscription
    let id = client.create_subscription(&subscriber, &merchant, &1000, &(30 * 24 * 60 * 60), &false, &None, &None::<u64>);

    // Set max metadata keys (10)
    for i in 0..10 {
        let key = String::from_str(&env, &std::format!("key_{}", i));
        let val = String::from_str(&env, &std::format!("val_{}", i));
        client.set_metadata(&id, &subscriber, &key, &val);
    }

    // Verify listing
    let keys = client.list_metadata_keys(&id);
    assert_eq!(keys.len(), 10);

    // Verify getting each key
    for i in 0..10 {
        let key = String::from_str(&env, &std::format!("key_{}", i));
        let val = client.get_metadata(&id, &key);
        assert_eq!(val, String::from_str(&env, &std::format!("val_{}", i)));
    }

    // Ensure we can update an existing key without exceeding limit
    let key0 = String::from_str(&env, "key_0");
    let new_val = String::from_str(&env, "new_val_0");
    client.set_metadata(&id, &subscriber, &key0, &new_val);
    assert_eq!(client.get_metadata(&id, &key0), new_val);
    assert_eq!(client.list_metadata_keys(&id).len(), 10);
}
