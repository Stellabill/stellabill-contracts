#![cfg(test)]

use crate::{
    test_utils::{fixtures::create_test_subscription, fixtures::seed_balance, setup::TestEnv},
    Error, SubscriptionStatus,
};

#[test]
fn test_request_and_finalize_emergency_withdraw_after_cooldown() {
    let te = TestEnv::default();
    let (id, subscriber, _) =
        create_test_subscription(&te.env, &te.client, SubscriptionStatus::Paused);
    seed_balance(&te.env, &te.client, id, 1_000_000);

    te.client.request_emergency_withdraw(&id, &subscriber);

    te.jump(72 * 60 * 60 + 1);
    te.client.finalize_emergency_withdraw(&id, &subscriber);

    let sub = te.client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 0);
    assert_eq!(sub.status, SubscriptionStatus::Paused);
}

#[test]
fn test_finalize_before_cooldown_is_rejected() {
    let te = TestEnv::default();
    let (id, subscriber, _) =
        create_test_subscription(&te.env, &te.client, SubscriptionStatus::Paused);
    seed_balance(&te.env, &te.client, id, 1_000_000);

    te.client.request_emergency_withdraw(&id, &subscriber);

    let result = te.client.try_finalize_emergency_withdraw(&id, &subscriber);
    assert_eq!(result, Err(Ok(Error::EmergencyWithdrawCooldownActive)));
}

#[test]
fn test_finalize_rejects_when_status_changed_since_request() {
    let te = TestEnv::default();
    let (id, subscriber, _) =
        create_test_subscription(&te.env, &te.client, SubscriptionStatus::Paused);
    seed_balance(&te.env, &te.client, id, 1_000_000);

    te.client.request_emergency_withdraw(&id, &subscriber);
    te.client.cancel_subscription(&id, &subscriber);

    let result = te.client.try_finalize_emergency_withdraw(&id, &subscriber);
    assert_eq!(result, Err(Ok(Error::EmergencyWithdrawStateChanged)));
}

#[test]
fn test_double_finalize_is_rejected() {
    let te = TestEnv::default();
    let (id, subscriber, _) =
        create_test_subscription(&te.env, &te.client, SubscriptionStatus::Paused);
    seed_balance(&te.env, &te.client, id, 1_000_000);

    te.client.request_emergency_withdraw(&id, &subscriber);
    te.jump(72 * 60 * 60 + 1);
    te.client.finalize_emergency_withdraw(&id, &subscriber);

    let result = te.client.try_finalize_emergency_withdraw(&id, &subscriber);
    assert_eq!(result, Err(Ok(Error::EmergencyWithdrawNotRequested)));
}
