#![cfg(test)]

use crate::test_utils::setup::TestEnv;
use crate::{Error, SubscriptionStatus};
use soroban_sdk::{testutils::Address as _, vec, Address};

const BATCH_AMOUNT: i128 = 10_000_000;
const BATCH_INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const DEPOSIT_AMOUNT: i128 = 15_000_000; // 1.5 intervals worth

struct MultiActorSetup {
    pub env: TestEnv,
    pub merchants: [Address; 2],
    pub subscribers: [Address; 3],
    pub subscriptions: [u32; 5],
}

fn setup_multi_actor_env() -> MultiActorSetup {
    let test_env = TestEnv::default();
    let merchants = [
        Address::generate(&test_env.env),
        Address::generate(&test_env.env),
    ];
    let subscribers = [
        Address::generate(&test_env.env),
        Address::generate(&test_env.env),
        Address::generate(&test_env.env),
    ];

    let token_admin = test_env.stellar_token_client();
    for sub in &subscribers {
        token_admin.mint(sub, &(DEPOSIT_AMOUNT * 5));
    }

    let mut subscriptions = [0u32; 5];

    let topology = [(0, 0), (0, 1), (1, 0), (1, 1), (2, 0)];

    for (i, (s_idx, m_idx)) in topology.iter().enumerate() {
        let sub_id = test_env.client.create_subscription(
            &subscribers[*s_idx],
            &merchants[*m_idx],
            &BATCH_AMOUNT,
            &BATCH_INTERVAL,
            &false,
            &None,
            &None::<u64>,
        );
        test_env
            .client
            .deposit_funds(&sub_id, &subscribers[*s_idx], &DEPOSIT_AMOUNT);
        subscriptions[i] = sub_id;
    }

    MultiActorSetup {
        env: test_env,
        merchants,
        subscribers,
        subscriptions,
    }
}

#[test]
fn test_multi_actor_balances_and_statuses_after_setup() {
    let setup = setup_multi_actor_env();

    for sub_id in setup.subscriptions.iter() {
        let sub = setup.env.client.get_subscription(sub_id);
        assert_eq!(sub.status, SubscriptionStatus::Active);
        assert_eq!(sub.prepaid_balance, DEPOSIT_AMOUNT);
        assert_eq!(sub.amount, BATCH_AMOUNT);
        assert_eq!(sub.interval_seconds, BATCH_INTERVAL);
    }
}

#[test]
fn test_multi_actor_batch_charge() {
    let setup = setup_multi_actor_env();

    setup.env.jump(BATCH_INTERVAL + 1);

    let ids = vec![
        &setup.env.env,
        setup.subscriptions[0],
        setup.subscriptions[1],
        setup.subscriptions[2],
        setup.subscriptions[3],
        setup.subscriptions[4],
    ];
    let results = setup.env.client.batch_charge(&ids, &0u64);

    assert_eq!(results.len(), 5);

    for (i, result) in results.iter().enumerate() {
        assert_eq!(
            result.success, true,
            "Charge {} failed with code {}",
            i, result.error_code
        );
    }

    // Verify balances dropped and timestamps updated
    for sub_id in setup.subscriptions.iter() {
        let sub = setup.env.client.get_subscription(sub_id);
        assert_eq!(
            sub.prepaid_balance,
            DEPOSIT_AMOUNT - BATCH_AMOUNT,
            "Balance should decrease by batch amount"
        );
        assert!(sub.last_payment_timestamp > 0);
    }
}

#[test]
fn test_multi_actor_mixed_charges() {
    let setup = setup_multi_actor_env();

    let one_off_amount = 2_000_000;
    setup.env.client.charge_one_off(
        &setup.subscriptions[0],
        &setup.merchants[0],
        &one_off_amount,
    );
    setup.env.client.charge_one_off(
        &setup.subscriptions[1],
        &setup.merchants[1],
        &one_off_amount,
    );

    let sub0 = setup.env.client.get_subscription(&setup.subscriptions[0]);
    assert_eq!(sub0.prepaid_balance, DEPOSIT_AMOUNT - one_off_amount);

    setup.env.jump(BATCH_INTERVAL + 1);

    let ids = vec![
        &setup.env.env,
        setup.subscriptions[0],
        setup.subscriptions[1],
        setup.subscriptions[2],
        setup.subscriptions[3],
        setup.subscriptions[4],
    ];
    let results = setup.env.client.batch_charge(&ids, &0u64);

    assert_eq!(results.len(), 5);

    for result in results.iter() {
        assert_eq!(result.success, true);
    }

    let sub0_after = setup.env.client.get_subscription(&setup.subscriptions[0]);
    assert_eq!(
        sub0_after.prepaid_balance,
        DEPOSIT_AMOUNT - one_off_amount - BATCH_AMOUNT
    );

    let sub2 = setup.env.client.get_subscription(&setup.subscriptions[2]);
    assert_eq!(sub2.prepaid_balance, DEPOSIT_AMOUNT - BATCH_AMOUNT);
}

#[test]
fn test_multi_actor_pause_and_resume_subset() {
    let setup = setup_multi_actor_env();

    setup
        .env
        .client
        .pause_subscription(&setup.subscriptions[0], &setup.subscribers[0]);
    setup
        .env
        .client
        .pause_subscription(&setup.subscriptions[2], &setup.subscribers[1]);

    setup.env.jump(BATCH_INTERVAL + 1);

    let ids = vec![
        &setup.env.env,
        setup.subscriptions[0],
        setup.subscriptions[1],
        setup.subscriptions[2],
    ];
    let results = setup.env.client.batch_charge(&ids, &0u64);

    assert_eq!(results.get(0).unwrap().success, false);
    assert_eq!(results.get(0).unwrap().error_code, Error::NotActive as u32);

    assert_eq!(results.get(1).unwrap().success, true);

    assert_eq!(results.get(2).unwrap().success, false);
    assert_eq!(results.get(2).unwrap().error_code, Error::NotActive as u32);

    setup
        .env
        .client
        .resume_subscription(&setup.subscriptions[0], &setup.subscribers[0]);

    let sub0 = setup.env.client.get_subscription(&setup.subscriptions[0]);
    assert_eq!(sub0.status, SubscriptionStatus::Active);

    let sub2 = setup.env.client.get_subscription(&setup.subscriptions[2]);
    assert_eq!(sub2.status, SubscriptionStatus::Paused);
}

#[test]
fn test_multi_actor_cancel_isolated() {
    let setup = setup_multi_actor_env();

    setup
        .env
        .client
        .cancel_subscription(&setup.subscriptions[4], &setup.subscribers[2]);

    let sub4 = setup.env.client.get_subscription(&setup.subscriptions[4]);
    assert_eq!(sub4.status, SubscriptionStatus::Cancelled);

    for i in 0..4 {
        let sub = setup.env.client.get_subscription(&setup.subscriptions[i]);
        assert_eq!(sub.status, SubscriptionStatus::Active);
    }
}

#[test]
fn test_multi_actor_view_helpers() {
    let setup = setup_multi_actor_env();

    for sub_id in setup.subscriptions.iter() {
        let topup_0 = setup.env.client.estimate_topup_for_intervals(sub_id, &0);
        assert_eq!(topup_0, 0);

        let req_amount = 2 * BATCH_AMOUNT;
        assert!(req_amount > DEPOSIT_AMOUNT);
        let expected_shortfall = req_amount - DEPOSIT_AMOUNT;

        let topup_2 = setup.env.client.estimate_topup_for_intervals(sub_id, &2);
        assert_eq!(topup_2, expected_shortfall);
    }
}

#[test]
fn test_adversarial_merchant_pause_during_deposit() {
    let setup = setup_multi_actor_env();

    let sub_idx = 0;
    let sub_id = setup.subscriptions[sub_idx];
    let sub_addr = setup.subscribers[sub_idx].clone();
    let mer_addr = setup.merchants[0].clone();

    setup.env.client.pause_merchant(&mer_addr);

    let result = setup.env.client.try_deposit_funds(&sub_id, &sub_addr, &DEPOSIT_AMOUNT);
    assert!(result.is_err());

    setup.env.client.unpause_merchant(&mer_addr);

    setup.env.client.deposit_funds(&sub_id, &sub_addr, &DEPOSIT_AMOUNT);

    let sub = setup.env.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

#[test]
fn test_adversarial_cancel_right_before_charge() {
    let setup = setup_multi_actor_env();

    let sub_idx = 0;
    let sub_id = setup.subscriptions[sub_idx];
    let sub_addr = setup.subscribers[sub_idx].clone();

    setup.env.client.cancel_subscription(&sub_id, &sub_addr);

    setup.env.jump(BATCH_INTERVAL + 1);

    let ids = vec![&setup.env.env, sub_id];
    let results = setup.env.client.batch_charge(&ids, &0u64);

    assert_eq!(results.len(), 1);
    assert_eq!(results.get(0).unwrap().success, false);
    assert_eq!(results.get(0).unwrap().error_code, Error::NotActive as u32);
}

#[test]
fn test_adversarial_cancel_right_after_charge() {
    let setup = setup_multi_actor_env();

    let sub_idx = 0;
    let sub_id = setup.subscriptions[sub_idx];
    let sub_addr = setup.subscribers[sub_idx].clone();

    setup.env.jump(BATCH_INTERVAL + 1);

    let ids = vec![&setup.env.env, sub_id];
    let results = setup.env.client.batch_charge(&ids, &0u64);
    assert_eq!(results.len(), 1);
    assert_eq!(results.get(0).unwrap().success, true);

    let sub_before = setup.env.client.get_subscription(&sub_id);
    let balance_before = sub_before.prepaid_balance;

    setup.env.client.cancel_subscription(&sub_id, &sub_addr);

    let sub = setup.env.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(sub.prepaid_balance, balance_before);
}

#[test]
fn test_adversarial_cancel_by_merchant_vs_subscriber() {
    let setup = setup_multi_actor_env();

    let sub_idx = 0;
    let sub_id = setup.subscriptions[sub_idx];
    let _sub_addr = setup.subscribers[sub_idx].clone();
    let mer_addr = setup.merchants[0].clone();

    setup.env.client.cancel_subscription(&sub_id, &mer_addr);

    let sub = setup.env.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

#[test]
fn test_adversarial_subscriber_cannot_cancel_others() {
    let setup = setup_multi_actor_env();

    let sub_id = setup.subscriptions[0];
    let wrong_subscriber = setup.subscribers[1].clone();

    let result = setup.env.client.try_cancel_subscription(&sub_id, &wrong_subscriber);
    assert!(result.is_err());

    let sub = setup.env.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

#[test]
fn test_adversarial_merchant_cannot_cancel_others() {
    let setup = setup_multi_actor_env();

    let sub_id = setup.subscriptions[0];
    let wrong_merchant = setup.merchants[1].clone();

    let result = setup.env.client.try_cancel_subscription(&sub_id, &wrong_merchant);
    assert!(result.is_err());

    let sub = setup.env.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}
