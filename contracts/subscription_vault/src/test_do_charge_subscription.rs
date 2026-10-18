//! Focused adversarial coverage for `admin::do_charge_subscription`.

use crate::test_utils::{fixtures, setup::TestEnv};
use crate::{DataKey, Error, SubscriptionStatus};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, BytesN, IntoVal};

const AMOUNT: i128 = 10_000_000;
const PREPAID: i128 = 50_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const T0: u64 = 1_000;

fn chargeable_subscription(test_env: &TestEnv) -> (u32, Address, Address) {
    let (id, subscriber, merchant) = fixtures::create_subscription_detailed(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );

    let mut subscription = test_env.client.get_subscription(&id);
    subscription.auto_renew = true;
    test_env.env.as_contract(&test_env.client.address, || {
        test_env
            .env
            .storage()
            .persistent()
            .set(&DataKey::Sub(id), &subscription);
    });
    fixtures::seed_balance(&test_env.env, &test_env.client, id, PREPAID);
    test_env.env.ledger().set_timestamp(T0 + INTERVAL + 1);

    (id, subscriber, merchant)
}

#[test]
fn do_charge_subscription_charges_a_valid_subscription() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let (id, _subscriber, merchant) = chargeable_subscription(&test_env);

    let result = test_env
        .client
        .try_charge_subscription(&id, &None::<BytesN<32>>);

    assert_eq!(result, Ok(Ok(crate::ChargeExecutionResult::Charged)));
    let subscription = test_env.client.get_subscription(&id);
    assert_eq!(subscription.prepaid_balance, PREPAID - AMOUNT);
    assert_eq!(subscription.lifetime_charged, AMOUNT);
    assert_eq!(
        test_env
            .client
            .get_merchant_balance_by_token(&merchant, &test_env.token),
        AMOUNT
    );
}

#[test]
fn do_charge_subscription_rejects_a_missing_and_maximum_id_without_state_changes() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let (id, _subscriber, _merchant) = chargeable_subscription(&test_env);
    let before = test_env.client.get_subscription(&id);

    for missing_id in [u32::MAX, id + 1] {
        assert_eq!(
            test_env
                .client
                .try_charge_subscription(&missing_id, &None::<BytesN<32>>),
            Err(Ok(Error::NotFound))
        );
        assert_eq!(test_env.client.get_subscription(&id), before);
    }
}

#[test]
fn do_charge_subscription_rejects_an_unauthorized_caller_without_mutation() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let (id, _subscriber, _merchant) = chargeable_subscription(&test_env);
    let before = test_env.client.get_subscription(&id);
    let stranger = Address::generate(&test_env.env);

    use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};
    let mut args = soroban_sdk::Vec::new(&test_env.env);
    args.push_back(id.into_val(&test_env.env));
    args.push_back(None::<BytesN<32>>.into_val(&test_env.env));
    test_env.env.mock_auths(&[MockAuth {
        address: &stranger,
        invoke: &MockAuthInvoke {
            contract: &test_env.client.address,
            fn_name: "charge_subscription",
            args,
            sub_invokes: &[],
        },
    }]);

    assert!(test_env
        .client
        .try_charge_subscription(&id, &None::<BytesN<32>>)
        .is_err());
    assert_eq!(test_env.client.get_subscription(&id), before);
}

#[test]
fn do_charge_subscription_rejects_a_second_charge_before_the_next_interval() {
    let test_env = TestEnv::default();
    test_env.env.ledger().set_timestamp(T0);
    let (id, _subscriber, _merchant) = chargeable_subscription(&test_env);

    assert_eq!(
        test_env
            .client
            .try_charge_subscription(&id, &None::<BytesN<32>>),
        Ok(Ok(crate::ChargeExecutionResult::Charged))
    );
    let before = test_env.client.get_subscription(&id);

    test_env.env.ledger().set_timestamp(T0 + INTERVAL + 1);
    let result = test_env
        .client
        .try_charge_subscription(&id, &None::<BytesN<32>>);

    assert_eq!(result, Err(Ok(Error::IntervalNotElapsed)));
    assert_eq!(test_env.client.get_subscription(&id), before);
}
