use crate::{
    test_utils::{fixtures, setup::TestEnv},
    CancellationEscrow, CancellationEscrowDisputedEvent, CancellationEscrowOpenedEvent,
    CancellationEscrowReleasedEvent,
    DataKey, DisputeStatus, Error, SubscriptionStatus,
    CANCELLATION_ESCROW_WINDOW_SECS,
};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    Address, Env, FromVal, IntoVal, Symbol,
};

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const PREPAID: i128 = 50_000_000;

fn setup_cancelled_with_balance() -> (TestEnv, u32, Address, Address) {
    let test_env = TestEnv::default();
    let (id, subscriber, merchant) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env
        .client
        .deposit_funds(&id, &subscriber, &PREPAID, &None::<soroban_sdk::BytesN<32>>);
    test_env
        .client
        .cancel_subscription(&id, &subscriber);
    (test_env, id, subscriber, merchant)
}

#[test]
fn test_cancellation_creates_escrow_and_does_not_refund() {
    let (test_env, id, subscriber, merchant) = setup_cancelled_with_balance();

    let escrow = test_env.client.get_cancellation_escrow(&id);
    assert_eq!(escrow.subscription_id, id);
    assert_eq!(escrow.amount, PREPAID);
    assert_eq!(escrow.subscriber, subscriber);
    assert_eq!(escrow.merchant, merchant);
    assert_eq!(
        escrow.releases_at,
        test_env.env.ledger().timestamp() + CANCELLATION_ESCROW_WINDOW_SECS
    );

    // subscriber has NOT received the refund yet
    let token_client = soroban_sdk::token::Client::new(&test_env.env, &test_env.token);
    let sub_balance = token_client.balance(&subscriber);
    assert_eq!(sub_balance, 0);

    let sub = test_env.client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, 0);
}

#[test]
fn test_cancellation_escrow_opened_event() {
    let (test_env, id, subscriber, merchant) = setup_cancelled_with_balance();

    let events = test_env.env.events().all();
    let event = events
        .iter()
        .find(|e| {
            Symbol::from_val(&test_env.env, &e.1.get(0).unwrap())
                == Symbol::new(&test_env.env, "cancellation_escrow_opened")
        })
        .expect("missing cancellation_escrow_opened event");

    let data: CancellationEscrowOpenedEvent = event.2.clone().into_val(&test_env.env);
    assert_eq!(data.subscription_id, id);
    assert_eq!(data.subscriber, subscriber);
    assert_eq!(data.merchant, merchant);
    assert_eq!(data.amount, PREPAID);
    assert_eq!(
        data.released_at,
        test_env.env.ledger().timestamp() + CANCELLATION_ESCROW_WINDOW_SECS
    );
}

#[test]
fn test_claim_before_window_elapsed_rejected() {
    let (test_env, id, subscriber, _) = setup_cancelled_with_balance();

    let result = test_env
        .client
        .try_claim_cancellation_escrow(&subscriber, &id);
    assert_eq!(result, Err(Ok(Error::EscrowNotReleased)));
}

#[test]
fn test_claim_after_window_elapsed_succeeds() {
    let (test_env, id, subscriber, _) = setup_cancelled_with_balance();

    test_env.jump(CANCELLATION_ESCROW_WINDOW_SECS + 1);

    let claimed = test_env.client.claim_cancellation_escrow(&subscriber, &id);
    assert_eq!(claimed, PREPAID);

    let token_client = soroban_sdk::token::Client::new(&test_env.env, &test_env.token);
    assert_eq!(token_client.balance(&subscriber), PREPAID);

    let result = test_env
        .client
        .try_get_cancellation_escrow(&id);
    assert!(matches!(result, Err(Ok(_))), "Expected escrow not found error");
}

#[test]
fn test_cancellation_escrow_released_event() {
    let (test_env, id, subscriber, _) = setup_cancelled_with_balance();

    test_env.jump(CANCELLATION_ESCROW_WINDOW_SECS + 1);
    test_env.client.claim_cancellation_escrow(&subscriber, &id);

    let events = test_env.env.events().all();
    let event = events
        .iter()
        .find(|e| {
            Symbol::from_val(&test_env.env, &e.1.get(0).unwrap())
                == Symbol::new(&test_env.env, "cancellation_escrow_released")
        })
        .expect("missing cancellation_escrow_released event");

    let data: CancellationEscrowReleasedEvent = event.2.clone().into_val(&test_env.env);
    assert_eq!(data.subscription_id, id);
    assert_eq!(data.subscriber, subscriber);
    assert_eq!(data.amount, PREPAID);
}

#[test]
fn test_claim_unauthorized_rejected() {
    let (test_env, id, _, _) = setup_cancelled_with_balance();
    let stranger = Address::generate(&test_env.env);

    let result = test_env
        .client
        .try_claim_cancellation_escrow(&stranger, &id);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_claim_nonexistent_escrow_rejected() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    test_env
        .client
        .cancel_subscription(&id, &subscriber);

    let result = test_env
        .client
        .try_claim_cancellation_escrow(&subscriber, &id);
    assert!(matches!(result, Err(Ok(Error::EscrowNotFound))));
}

#[test]
fn test_merchant_lodge_dispute_before_window_succeeds() {
    let (test_env, id, _subscriber, merchant) = setup_cancelled_with_balance();

    let dispute_id = test_env.client.lodge_escrow_dispute(&merchant, &id);

    let dispute = test_env.client.get_dispute(&dispute_id);
    assert_eq!(dispute.subscription_id, id);
    assert_eq!(dispute.merchant, merchant);
    assert_eq!(dispute.amount, PREPAID);
    assert_eq!(dispute.status, DisputeStatus::Open);

    let result = test_env
        .client
        .try_get_cancellation_escrow(&id);
    assert!(matches!(result, Err(Ok(_))), "Expected escrow not found error");
}

#[test]
fn test_merchant_cannot_lodge_dispute_after_window() {
    let (test_env, id, _subscriber, merchant) = setup_cancelled_with_balance();

    test_env.jump(CANCELLATION_ESCROW_WINDOW_SECS + 1);

    let result = test_env
        .client
        .try_lodge_escrow_dispute(&merchant, &id);
    assert_eq!(result, Err(Ok(Error::EscrowNotReleased)));
}

#[test]
fn test_cannot_lodge_dispute_unauthorized() {
    let (test_env, id, _subscriber, _merchant) = setup_cancelled_with_balance();
    let stranger = Address::generate(&test_env.env);

    let result = test_env
        .client
        .try_lodge_escrow_dispute(&stranger, &id);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_cannot_claim_escrow_while_disputed() {
    let (test_env, id, subscriber, merchant) = setup_cancelled_with_balance();

    test_env.client.lodge_escrow_dispute(&merchant, &id);
    test_env.jump(CANCELLATION_ESCROW_WINDOW_SECS + 1);

    let result = test_env
        .client
        .try_claim_cancellation_escrow(&subscriber, &id);
    assert_eq!(result, Err(Ok(Error::DisputeAlreadyOpen)));
}

#[test]
fn test_cannot_lodge_dispute_twice() {
    let (test_env, id, _subscriber, merchant) = setup_cancelled_with_balance();

    test_env.client.lodge_escrow_dispute(&merchant, &id);

    let result = test_env
        .client
        .try_lodge_escrow_dispute(&merchant, &id);
    assert_eq!(result, Err(Ok(Error::DisputeAlreadyOpen)));
}

#[test]
fn test_lodge_dispute_events() {
    let (test_env, id, subscriber, merchant) = setup_cancelled_with_balance();

    test_env.client.lodge_escrow_dispute(&merchant, &id);

    let events = test_env.env.events().all();

    let escrow_event = events
        .iter()
        .find(|e| {
            Symbol::from_val(&test_env.env, &e.1.get(0).unwrap())
                == Symbol::new(&test_env.env, "cancellation_escrow_disputed")
        })
        .expect("missing cancellation_escrow_disputed event");

    let escrow_data: CancellationEscrowDisputedEvent =
        escrow_event.2.clone().into_val(&test_env.env);
    assert_eq!(escrow_data.subscription_id, id);
    assert_eq!(escrow_data.merchant, merchant);
    assert_eq!(escrow_data.amount, PREPAID);

    let dispute_event = events
        .iter()
        .find(|e| {
            Symbol::from_val(&test_env.env, &e.1.get(0).unwrap())
                == Symbol::new(&test_env.env, "dispute_opened")
        })
        .expect("missing dispute_opened event");
    let dispute_data: crate::DisputeOpenedEvent =
        dispute_event.2.clone().into_val(&test_env.env);
    assert_eq!(dispute_data.subscription_id, id);
    assert_eq!(dispute_data.subscriber, subscriber);
    assert_eq!(dispute_data.merchant, merchant);
    assert_eq!(dispute_data.amount, PREPAID);
}

#[test]
fn test_lodge_dispute_then_resolve_to_merchant() {
    let (test_env, id, _subscriber, merchant) = setup_cancelled_with_balance();
    let admin = test_env.admin.clone();

    let dispute_id = test_env.client.lodge_escrow_dispute(&merchant, &id);

    test_env.client.respond_dispute(
        &admin,
        &dispute_id,
        &None::<soroban_sdk::BytesN<32>>,
    );
    test_env
        .client
        .resolve_dispute(&admin, &dispute_id, &false);

    let dispute = test_env.client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, DisputeStatus::ResolvedToMerchant);

    let merchant_stored = test_env
        .client
        .get_merchant_balance_by_token(&merchant, &test_env.token);
    assert_eq!(merchant_stored, PREPAID);
}

#[test]
fn test_lodge_dispute_then_resolve_to_subscriber() {
    let (test_env, id, subscriber, merchant) = setup_cancelled_with_balance();
    let admin = test_env.admin.clone();

    let dispute_id = test_env.client.lodge_escrow_dispute(&merchant, &id);

    test_env.client.respond_dispute(
        &admin,
        &dispute_id,
        &None::<soroban_sdk::BytesN<32>>,
    );
    test_env
        .client
        .resolve_dispute(&admin, &dispute_id, &true);

    let dispute = test_env.client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, DisputeStatus::ResolvedToSubscriber);

    let token_client = soroban_sdk::token::Client::new(&test_env.env, &test_env.token);
    assert_eq!(token_client.balance(&subscriber), PREPAID);
}

#[test]
fn test_no_escrow_when_zero_balance() {
    let test_env = TestEnv::default();
    let (id, subscriber, _) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);

    test_env
        .client
        .cancel_subscription(&id, &subscriber);

    let result = test_env
        .client
        .try_get_cancellation_escrow(&id);
    assert!(matches!(result, Err(Ok(_))), "Expected escrow not found error");
}

#[test]
fn test_merchant_cancel_also_creates_escrow() {
    let test_env = TestEnv::default();
    let (id, subscriber, merchant) =
        fixtures::create_subscription(&test_env.env, &test_env.client, SubscriptionStatus::Active);
    test_env.stellar_token_client().mint(&subscriber, &PREPAID);
    test_env
        .client
        .deposit_funds(&id, &subscriber, &PREPAID, &None::<soroban_sdk::BytesN<32>>);

    test_env
        .client
        .cancel_subscription(&id, &merchant);

    let escrow = test_env.client.get_cancellation_escrow(&id);
    assert_eq!(escrow.amount, PREPAID);
    assert_eq!(escrow.merchant, merchant);

    let dispute_id = test_env.client.lodge_escrow_dispute(&merchant, &id);
    assert!(dispute_id == 0);
}

#[test]
fn test_claim_after_window_can_only_be_done_once() {
    let (test_env, id, subscriber, _) = setup_cancelled_with_balance();

    test_env.jump(CANCELLATION_ESCROW_WINDOW_SECS + 1);

    test_env.client.claim_cancellation_escrow(&subscriber, &id);

    let result = test_env
        .client
        .try_claim_cancellation_escrow(&subscriber, &id);
    assert!(matches!(result, Err(Ok(Error::EscrowNotFound))));
}

#[test]
fn test_escrow_contains_released_at_field() {
    let (test_env, id, _subscriber, _merchant) = setup_cancelled_with_balance();

    let escrow = test_env.client.get_cancellation_escrow(&id);

    let now = test_env.env.ledger().timestamp();
    assert_eq!(escrow.releases_at, now + CANCELLATION_ESCROW_WINDOW_SECS);
}

#[test]
fn test_claim_exactly_at_window_edge_rejected() {
    let (test_env, id, subscriber, _) = setup_cancelled_with_balance();

    test_env.jump(CANCELLATION_ESCROW_WINDOW_SECS);

    let result = test_env
        .client
        .try_claim_cancellation_escrow(&subscriber, &id);
    assert_eq!(result, Err(Ok(Error::EscrowNotReleased)));
}

#[test]
fn test_lodge_dispute_exactly_at_window_edge_rejected() {
    let (test_env, id, _subscriber, merchant) = setup_cancelled_with_balance();

    test_env.jump(CANCELLATION_ESCROW_WINDOW_SECS);

    let result = test_env
        .client
        .try_lodge_escrow_dispute(&merchant, &id);
    assert_eq!(result, Err(Ok(Error::EscrowNotReleased)));
}
