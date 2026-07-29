//! Tests for `export_subscription_summaries`.
//!
//! Verifies:
//!  - Empty contract returns empty vec
//!  - Exported summaries match created subscriptions
//!  - Multi-page export over ID ranges covers all subscriptions
//!  - Cancelled subscriptions still appear in export

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::StellarAssetClient as TokenAdminClient,
    Address, Env,
};
use std::vec::Vec;
use subscription_vault::{SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient};

const T0: u64 = 1_700_000_000;

fn setup() -> (
    Env,
    SubscriptionVaultClient<'static>,
    Address,
    TokenAdminClient<'static>,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = TokenAdminClient::new(&env, &token);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    client.init(&token, &7u32, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, client, admin, token_admin)
}

fn create_subs(
    client: &SubscriptionVaultClient,
    token_admin: &TokenAdminClient,
    subscriber: &Address,
    merchant: &Address,
    count: u32,
) -> Vec<u32> {
    let mut ids = Vec::new();
    token_admin.mint(subscriber, &(count as i128 * 1_000_000_000));
    for _ in 0..count {
        let id = client.create_subscription(
            subscriber,
            merchant,
            &1_000i128,
            &2_592_000u64,
            &false,
            &None::<i128>,
            &None::<u64>,
                &None::<u32>,
);
        ids.push(id);
    }
    ids
}

#[test]
fn empty_contract_returns_empty_vec() {
    let (_env, client, admin, _token_admin) = setup();
    let result = client.export_subscription_summaries(&admin, &0, &10);
    assert!(result.is_empty(), "expected no summaries on empty contract");
}

#[test]
fn single_page_export_returns_all_ids() {
    let (_env, client, admin, token_admin) = setup();
    let subscriber = Address::generate(&_env);
    let merchant = Address::generate(&_env);

    let n = 5u32;
    let created = create_subs(&client, &token_admin, &subscriber, &merchant, n);

    let result = client.export_subscription_summaries(&admin, &0, &50);
    assert_eq!(result.len() as u32, n);
    let mut exported: Vec<u32> = result.iter().map(|s| s.subscription_id).collect();
    exported.sort();
    let mut created_sorted = created.clone();
    created_sorted.sort();
    assert_eq!(
        exported, created_sorted,
        "exported ids must match created ids"
    );
}

#[test]
fn multi_page_export_covers_all_ids() {
    let (_env, client, admin, token_admin) = setup();
    let subscriber = Address::generate(&_env);
    let merchant = Address::generate(&_env);

    let n = 15u32;
    let created = create_subs(&client, &token_admin, &subscriber, &merchant, n);
    let mut all_exported: Vec<u32> = Vec::new();

    let page_size = 4u32;
    let mut start_id = 0u32;
    loop {
        let page = client.export_subscription_summaries(&admin, &start_id, &page_size);
        if page.is_empty() {
            break;
        }
        for s in page.iter() {
            all_exported.push(s.subscription_id);
        }
        start_id += page_size;
        if start_id > n {
            break;
        }
    }

    all_exported.sort();
    let mut created_sorted = created.clone();
    created_sorted.sort();
    assert_eq!(
        all_exported, created_sorted,
        "union of all pages must equal created ids"
    );
}

#[test]
fn cancelled_subscription_still_appears_in_export() {
    let (_env, client, admin, token_admin) = setup();
    let subscriber = Address::generate(&_env);
    let merchant = Address::generate(&_env);

    let created = create_subs(&client, &token_admin, &subscriber, &merchant, 10);
    let cancel_target = created[3]; // pick one in the middle

    // Cancel it
    client.cancel_subscription(&cancel_target, &subscriber);

    // Verify it still appears in export
    let result = client.export_subscription_summaries(&admin, &0, &100);
    let exported_ids: Vec<u32> = result.iter().map(|s| s.subscription_id).collect();
    assert!(
        exported_ids.contains(&cancel_target),
        "cancelled subscription {cancel_target} must still appear in export"
    );

    // Verify its status is Cancelled
    for s in result.iter() {
        if s.subscription_id == cancel_target {
            assert_eq!(s.status, SubscriptionStatus::Cancelled);
            break;
        }
    }
}

#[test]
fn partial_last_page() {
    let (_env, client, admin, token_admin) = setup();
    let subscriber = Address::generate(&_env);
    let merchant = Address::generate(&_env);

    let n = 7u32;
    let created = create_subs(&client, &token_admin, &subscriber, &merchant, n);

    let page1 = client.export_subscription_summaries(&admin, &0, &3);
    assert_eq!(page1.len() as u32, 3);

    let page2 = client.export_subscription_summaries(&admin, &3, &3);
    assert_eq!(page2.len() as u32, 3);

    let page3 = client.export_subscription_summaries(&admin, &6, &3);
    assert_eq!(page3.len() as u32, 1);

    let mut all: Vec<u32> = page1
        .iter()
        .chain(page2.iter())
        .chain(page3.iter())
        .map(|s| s.subscription_id)
        .collect();
    all.sort();

    let mut expected = created.clone();
    expected.sort();
    assert_eq!(all, expected);
}
