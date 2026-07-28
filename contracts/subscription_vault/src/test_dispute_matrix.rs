//! Exhaustive tests for the dispute lifecycle transition matrix.

use crate::{
    test_utils::{fixtures, setup::TestEnv},
    DataKey, DisputeStatus, Error, SubscriptionStatus, DISPUTE_WINDOW_SECS,
};
use soroban_sdk::{testutils::Events, FromVal, Symbol};

const DISPUTE_AMOUNT: i128 = 5_000_000;

#[derive(Clone, Copy)]
enum Action {
    Respond,
    ResolveToMerchant,
    ResolveToSubscriber,
}

fn open_dispute(test_env: &TestEnv) -> (u64, u32) {
    let (subscription_id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    let subscription = test_env.client.get_subscription(&subscription_id);

    test_env.env.as_contract(&test_env.client.address, || {
        test_env.env.storage().instance().set(
            &DataKey::MerchantBalance(subscription.merchant.clone(), subscription.token.clone()),
            &DISPUTE_AMOUNT,
        );
    });
    test_env
        .stellar_token_client()
        .mint(&test_env.client.address, &DISPUTE_AMOUNT);

    let dispute_id = test_env.client.open_dispute(
        &subscriber,
        &subscription_id,
        &DISPUTE_AMOUNT,
        &None::<soroban_sdk::BytesN<32>>,
    );

    (dispute_id, subscription_id)
}

fn prepare_status(test_env: &TestEnv, dispute_id: u64, status: DisputeStatus) {
    match status {
        DisputeStatus::Open => {}
        DisputeStatus::Responded => {
            test_env.client.respond_dispute(
                &test_env.admin,
                &dispute_id,
                &None::<soroban_sdk::BytesN<32>>,
            );
        }
        DisputeStatus::ResolvedToMerchant | DisputeStatus::ResolvedToSubscriber => {
            test_env.client.respond_dispute(
                &test_env.admin,
                &dispute_id,
                &None::<soroban_sdk::BytesN<32>>,
            );
            test_env.client.resolve_dispute(
                &test_env.admin,
                &dispute_id,
                &matches!(status, DisputeStatus::ResolvedToSubscriber),
            );
        }
    }
}

fn apply_action(test_env: &TestEnv, dispute_id: u64, action: Action) -> Result<(), Error> {
    let result = match action {
        Action::Respond => test_env.client.try_respond_dispute(
            &test_env.admin,
            &dispute_id,
            &None::<soroban_sdk::BytesN<32>>,
        ),
        Action::ResolveToMerchant => {
            test_env
                .client
                .try_resolve_dispute(&test_env.admin, &dispute_id, &false)
        }
        Action::ResolveToSubscriber => {
            test_env
                .client
                .try_resolve_dispute(&test_env.admin, &dispute_id, &true)
        }
    };

    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => panic!("failed to decode successful dispute transition"),
        Err(Ok(error)) => Err(error),
        Err(Err(_)) => panic!("failed to decode dispute transition error"),
    }
}

fn assert_matrix_case(
    from: DisputeStatus,
    action: Action,
    expected: Result<(), Error>,
    advance_window: bool,
) {
    let test_env = TestEnv::default();
    let (dispute_id, _) = open_dispute(&test_env);
    prepare_status(&test_env, dispute_id, from);

    if advance_window {
        test_env.jump(DISPUTE_WINDOW_SECS + 1);
    }

    assert_eq!(apply_action(&test_env, dispute_id, action), expected);
}

#[test]
fn test_dispute_status_transition_matrix() {
    // Open -> Responded is the only response transition.
    assert_matrix_case(DisputeStatus::Open, Action::Respond, Ok(()), false);
    assert_matrix_case(
        DisputeStatus::Responded,
        Action::Respond,
        Err(Error::DisputeAlreadyResponded),
        false,
    );

    // Open can auto-resolve to the subscriber only after the dispute window.
    assert_matrix_case(
        DisputeStatus::Open,
        Action::ResolveToSubscriber,
        Err(Error::DisputeNotResponded),
        false,
    );
    assert_matrix_case(
        DisputeStatus::Open,
        Action::ResolveToSubscriber,
        Ok(()),
        true,
    );
    assert_matrix_case(
        DisputeStatus::Open,
        Action::ResolveToMerchant,
        Err(Error::DisputeNotResponded),
        false,
    );

    // Responded disputes may be resolved in either direction.
    assert_matrix_case(
        DisputeStatus::Responded,
        Action::ResolveToMerchant,
        Ok(()),
        false,
    );
    assert_matrix_case(
        DisputeStatus::Responded,
        Action::ResolveToSubscriber,
        Ok(()),
        false,
    );

    // Resolved disputes are terminal and reject every further transition.
    for status in [
        DisputeStatus::ResolvedToMerchant,
        DisputeStatus::ResolvedToSubscriber,
    ] {
        assert_matrix_case(
            status,
            Action::Respond,
            Err(Error::DisputeAlreadyResponded),
            false,
        );
        assert_matrix_case(
            status,
            Action::ResolveToMerchant,
            Err(Error::DisputeAlreadyResolved),
            false,
        );
        assert_matrix_case(
            status,
            Action::ResolveToSubscriber,
            Err(Error::DisputeAlreadyResolved),
            false,
        );
    }
}

#[test]
fn test_dispute_transition_matrix_emits_events_for_valid_paths() {
    let test_env = TestEnv::default();
    let (dispute_id, _) = open_dispute(&test_env);

    test_env.client.respond_dispute(
        &test_env.admin,
        &dispute_id,
        &None::<soroban_sdk::BytesN<32>>,
    );
    let responded = test_env.env.events().all().iter().any(|event| {
        Symbol::from_val(&test_env.env, &event.1.get(0).unwrap())
            == Symbol::new(&test_env.env, "dispute_responded")
    });
    assert!(
        responded,
        "valid Open -> Responded transition emitted no event"
    );

    test_env
        .client
        .resolve_dispute(&test_env.admin, &dispute_id, &true);
    let resolved = test_env.env.events().all().iter().any(|event| {
        Symbol::from_val(&test_env.env, &event.1.get(0).unwrap())
            == Symbol::new(&test_env.env, "dispute_resolved")
    });
    assert!(
        resolved,
        "valid Responded -> Resolved transition emitted no event"
    );
}

#[test]
fn test_dispute_transition_matrix_rejects_missing_dispute() {
    let test_env = TestEnv::default();
    let missing_id = 999u64;

    assert_eq!(
        apply_action(&test_env, missing_id, Action::Respond),
        Err(Error::DisputeNotFound)
    );
    assert_eq!(
        apply_action(&test_env, missing_id, Action::ResolveToSubscriber),
        Err(Error::DisputeNotFound)
    );
}
