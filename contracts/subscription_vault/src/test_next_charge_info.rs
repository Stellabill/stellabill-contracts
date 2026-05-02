use crate::test_utils::setup::TestEnv;
use crate::types::{SubscriptionStatus};
use soroban_sdk::{testutils::Address as _, Address, symbol_short};

#[test]
fn test_get_next_charge_info_active_funded() {
    let setup = TestEnv::default();
    let subscriber = Address::generate(&setup.env);
    let merchant = Address::generate(&setup.env);
    let amount = 10_000_000i128; // 10 USDC
    let interval = 86400; // 1 day

    let sub_id = setup.client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    // Deposit enough for 1 charge
    setup.stellar_token_client().mint(&subscriber, &amount);
    setup.client.deposit_funds(&sub_id, &subscriber, &amount);

    let info = setup.client.get_next_charge_info(&sub_id);
    assert_eq!(info.is_charge_expected, true);
    assert_eq!(info.status, SubscriptionStatus::Active);
    assert_eq!(info.reason, symbol_short!("active"));
    assert_eq!(info.amount, amount);
    assert_eq!(info.token, setup.token);
}

#[test]
fn test_get_next_charge_info_active_unfunded() {
    let setup = TestEnv::default();
    let subscriber = Address::generate(&setup.env);
    let merchant = Address::generate(&setup.env);
    let amount = 10_000_000i128;
    let interval = 86400;

    let sub_id = setup.client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    let info = setup.client.get_next_charge_info(&sub_id);
    assert_eq!(info.is_charge_expected, true);
    assert_eq!(info.status, SubscriptionStatus::Active);
    assert_eq!(info.reason, symbol_short!("funds_low"));
}

#[test]
fn test_get_next_charge_info_paused() {
    let setup = TestEnv::default();
    let subscriber = Address::generate(&setup.env);
    let merchant = Address::generate(&setup.env);
    
    let sub_id = setup.client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000,
        &86400,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    setup.client.pause_subscription(&sub_id, &subscriber);

    let info = setup.client.get_next_charge_info(&sub_id);
    assert_eq!(info.is_charge_expected, false);
    assert_eq!(info.status, SubscriptionStatus::Paused);
    assert_eq!(info.reason, symbol_short!("paused"));
}

#[test]
fn test_get_next_charge_info_cancelled() {
    let setup = TestEnv::default();
    let subscriber = Address::generate(&setup.env);
    let merchant = Address::generate(&setup.env);
    
    let sub_id = setup.client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000,
        &86400,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    setup.client.cancel_subscription(&sub_id, &subscriber);

    let info = setup.client.get_next_charge_info(&sub_id);
    assert_eq!(info.is_charge_expected, false);
    assert_eq!(info.status, SubscriptionStatus::Cancelled);
    assert_eq!(info.reason, symbol_short!("cancel"));
}

#[test]
fn test_get_next_charge_info_expired() {
    let setup = TestEnv::default();
    let subscriber = Address::generate(&setup.env);
    let merchant = Address::generate(&setup.env);
    let now = setup.env.ledger().timestamp();
    let expires_at = now + 100;
    
    let sub_id = setup.client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000,
        &86400,
        &false,
        &None::<i128>,
        &Some(expires_at),
    );

    setup.jump(200);

    let info = setup.client.get_next_charge_info(&sub_id);
    assert_eq!(info.is_charge_expected, false);
    // Note: status might still be Active in storage until someone tries to interact with it,
    // but the query logic should report it as expired.
    // Wait, the query returns sub.status. 
    // In my implementation: 
    // if subscription.is_expired(now) { reason = symbol_short!("expired"); }
    // status: subscription.status
    // This is correct as it shows the stored status but the reason explains why no charge.
    assert_eq!(info.reason, symbol_short!("expired"));
}

#[test]
fn test_get_next_charge_info_cap_reached() {
    let setup = TestEnv::default();
    let subscriber = Address::generate(&setup.env);
    let merchant = Address::generate(&setup.env);
    let amount = 10_000_000i128;
    let cap = 15_000_000i128;
    
    let sub_id = setup.client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &86400,
        &false,
        &Some(cap),
        &None::<u64>,
    );

    // Charge once
    setup.stellar_token_client().mint(&subscriber, &amount);
    setup.client.deposit_funds(&sub_id, &subscriber, &amount);
    setup.jump(86401);
    setup.client.batch_charge(&soroban_sdk::vec![&setup.env, sub_id]);

    // Now charged 10M, cap is 15M. Next charge of 10M would exceed cap.
    let info = setup.client.get_next_charge_info(&sub_id);
    assert_eq!(info.is_charge_expected, true); // It's still expected to be ATTEMPTED
    assert_eq!(info.reason, symbol_short!("cap_near"));
}
