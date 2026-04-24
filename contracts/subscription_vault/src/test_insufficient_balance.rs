use crate::{ChargeExecutionResult, Error, SubscriptionStatus};
use crate::test_utils::{fixtures, setup::TestEnv};

const T0: u64 = 1_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const AMOUNT: i128 = 10_000_000;
const GRACE_PERIOD: u64 = 7 * 24 * 60 * 60;

#[test]
fn repeated_failed_charges_preserve_financial_state() {
    let test_env = TestEnv::with_min_topup(1_000_000);
    test_env.set_timestamp(T0);

    let (id, _subscriber, merchant) =
        fixtures::create_test_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    test_env.set_timestamp(T0 + INTERVAL + 1);
    assert_eq!(
        test_env.client.try_charge_subscription(&id),
        Ok(Ok(ChargeExecutionResult::InsufficientBalance))
    );

    let first = test_env.client.get_subscription(&id);
    assert_eq!(first.status, SubscriptionStatus::GracePeriod);
    assert_eq!(first.prepaid_balance, 0);
    assert_eq!(first.last_payment_timestamp, T0);
    assert_eq!(first.lifetime_charged, 0);
    assert_eq!(test_env.client.get_merchant_balance(&merchant), 0);
    assert_eq!(
        test_env.client.get_sub_statements_offset(&id, &0, &10, &true).total,
        0
    );

    test_env.set_timestamp(T0 + INTERVAL + 2);
    assert_eq!(
        test_env.client.try_charge_subscription(&id),
        Ok(Ok(ChargeExecutionResult::InsufficientBalance))
    );

    let second = test_env.client.get_subscription(&id);
    assert_eq!(second.status, SubscriptionStatus::GracePeriod);
    assert_eq!(second.prepaid_balance, 0);
    assert_eq!(second.last_payment_timestamp, T0);
    assert_eq!(second.lifetime_charged, 0);
    assert_eq!(test_env.client.get_merchant_balance(&merchant), 0);
    assert_eq!(
        test_env.client.get_sub_statements_offset(&id, &0, &10, &true).total,
        0
    );

    test_env.set_timestamp(T0 + INTERVAL + GRACE_PERIOD + 1);
    assert_eq!(
        test_env.client.try_charge_subscription(&id),
        Ok(Ok(ChargeExecutionResult::InsufficientBalance))
    );

    let after = test_env.client.get_subscription(&id);
    assert_eq!(after.status, SubscriptionStatus::InsufficientBalance);
    assert_eq!(after.prepaid_balance, 0);
    assert_eq!(after.last_payment_timestamp, T0);
    assert_eq!(after.lifetime_charged, 0);
    assert_eq!(test_env.client.get_merchant_balance(&merchant), 0);
    assert_eq!(
        test_env.client.get_sub_statements_offset(&id, &0, &10, &true).total,
        0
    );
}

#[test]
fn resume_from_underfunded_state_requires_sufficient_topup() {
    let test_env = TestEnv::with_min_topup(1_000_000);
    test_env.set_timestamp(T0);

    let (id, subscriber, _merchant) =
        fixtures::create_test_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.stellar_token_client().mint(&subscriber, &20_000_000i128);

    test_env.set_timestamp(T0 + INTERVAL + GRACE_PERIOD + 1);
    assert_eq!(
        test_env.client.try_charge_subscription(&id),
        Ok(Ok(ChargeExecutionResult::InsufficientBalance))
    );
    assert_eq!(
        test_env.client.get_subscription(&id).status,
        SubscriptionStatus::InsufficientBalance
    );

    test_env.client.deposit_funds(&id, &subscriber, &5_000_000i128);
    assert_eq!(
        test_env.client.try_resume_subscription(&id, &subscriber),
        Err(Ok(Error::InsufficientBalance))
    );

    test_env.client.deposit_funds(&id, &subscriber, &5_000_000i128);
    test_env.client.resume_subscription(&id, &subscriber);

    let resumed = test_env.client.get_subscription(&id);
    assert_eq!(resumed.status, SubscriptionStatus::Active);
    assert_eq!(resumed.prepaid_balance, AMOUNT);
}

#[test]
fn cancel_from_insufficient_balance_succeeds() {
    let test_env = TestEnv::with_min_topup(1_000_000);
    test_env.set_timestamp(T0);

    let (id, subscriber, _merchant) =
        fixtures::create_test_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    test_env.set_timestamp(T0 + INTERVAL + GRACE_PERIOD + 1);
    assert_eq!(
        test_env.client.try_charge_subscription(&id),
        Ok(Ok(ChargeExecutionResult::InsufficientBalance))
    );
    assert_eq!(
        test_env.client.get_subscription(&id).status,
        SubscriptionStatus::InsufficientBalance
    );

    test_env.client.cancel_subscription(&id, &subscriber);
    assert_eq!(
        test_env.client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}
