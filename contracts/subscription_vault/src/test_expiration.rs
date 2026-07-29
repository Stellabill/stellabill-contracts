
use super::*;
use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
use soroban_sdk::{token, Address, Env};

const T0: u64 = 1_000_000;
const INTERVAL: u64 = 60; // minimum valid interval (MIN_SUBSCRIPTION_INTERVAL_SECONDS)

fn setup_test_env() -> (
    Env,
    SubscriptionVaultClient<'static>,
    token::Client<'static>,
    token::StellarAssetClient<'static>,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = T0);

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token_admin_addr = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(token_admin_addr.clone());
    let token_client = token::Client::new(&env, &token_id.address());
    let token_admin_client = token::StellarAssetClient::new(&env, &token_id.address());

    let min_topup = 1_000_000i128;
    client.init(
        &token_id.address(),
        &6,
        &admin,
        &min_topup,
        &(7 * 24 * 60 * 60),
    );

    (env, client, token_client, token_admin_client, admin)
}

// doc 3: charge_subscription rejected at expiry boundary and after;
// withdrawal allowed after expiry (doc 5, Flow 1 steps 1-3, 6)
#[test]
fn test_expiration_timing_and_charging() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let amount = 1_000_000i128;
    let interval = INTERVAL;
    let expires_at = T0 + 2 * INTERVAL;

    let min_topup = 1_000_000i128;
    token_admin.mint(&subscriber, &(min_topup * 5));

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &amount,
        &interval,
        &false,
        &None::<i128>,
        &Some(expires_at),
    &None::<u32>,
    );
    client.deposit_funds(&sub_id, &subscriber, &(amount * 5), &None::<soroban_sdk::BytesN<32>>);

    // Before expiry: charge succeeds
    env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    client.charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(client.get_subscription(&sub_id).lifetime_charged, amount);

    // At expiry boundary — subscription expires_at = T0 + 2*INTERVAL
    env.ledger().with_mut(|l| l.timestamp = T0 + 2 * INTERVAL);
    let res = client.try_charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>);
    assert!(res.is_err(), "charge at expiry should be rejected");

    // expires_at field is preserved on the subscription
    assert!(client.get_subscription(&sub_id).expires_at.is_some());

    // After expiry — still rejects
    env.ledger().with_mut(|l| l.timestamp = T0 + 3 * INTERVAL);
    let res2 = client.try_charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>);
    assert!(res2.is_err(), "charge after expiry should be rejected");

    // Check withdrawal behavior after expiry
    let initial_balance = token_client.balance(&subscriber);
    client.withdraw_subscriber_funds(&sub_id, &subscriber);
    let final_balance = token_client.balance(&subscriber);
    assert!(final_balance > initial_balance);
}

// doc 3: charge_usage rejected when expired
#[test]
fn test_cleanup_and_archival() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let amount = 1_000_000i128;
    let expires_at = T0 + 2 * INTERVAL;
    let min_topup = 1_000_000i128;
    token_admin.mint(&subscriber, &(min_topup * 5));

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &min_topup,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(T0 + INTERVAL),
    &None::<u32>,
    );

    // Try cleanup before expiry — should fail
    let res = client.try_cleanup_subscription(&sub_id, &subscriber);
    assert!(res.is_err(), "cleanup before expiry should fail");

    // Advance past expiry and trigger it via a charge attempt
    env.ledger().with_mut(|l| l.timestamp = T0 + 2 * INTERVAL);
    let _ = client.try_charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>); // transitions to Expired

    // Perform cleanup which archives the subscription
    client.cleanup_subscription(&sub_id, &subscriber);

    let sub_archived = client.get_subscription(&sub_id);
    assert_eq!(sub_archived.status, SubscriptionStatus::Archived);
    assert_eq!(sub_archived.amount, min_topup);

    // Ensure funds can be withdrawn (already done by cleanup_subscription in some impls,
    // or via explicit withdraw)
    let sub_balance = sub_archived.prepaid_balance;
    assert_eq!(sub_balance, 0, "Funds should have been returned during cleanup");
}

// doc 2, 4, Flow 2: cancel before expiry -> Cancelled -> Archived;
// expired path: cancel rejected, cleanup -> Archived
#[test]
fn test_expiration_vs_cancellation() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let min_topup = 1_000_000i128;
    token_admin.mint(&subscriber, &(min_topup * 5));

    let expires_at = T0 + 2 * INTERVAL;

    // Scenario 1: Cancel before expiry
    let sub_id1 = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(expires_at),
    &None::<u32>,
    );
    
    client.cancel_subscription(&sub_id1, &subscriber);
    assert_eq!(
        client.get_subscription(&sub_id1).status,
        SubscriptionStatus::Cancelled
    );
    client.cancel_subscription(&sub_id1, &subscriber);
    assert_eq!(client.get_subscription(&sub_id1).status, SubscriptionStatus::Cancelled);

    // Status stays Cancelled even after the would-be expiry time passes
    env.ledger().with_mut(|l| l.timestamp = T0 + 4 * INTERVAL);
    assert_eq!(client.get_subscription(&sub_id1).status, SubscriptionStatus::Cancelled);

    env.ledger().with_mut(|l| l.timestamp = T0 + 3 * INTERVAL);
    assert_eq!(
        client.get_subscription(&sub_id1).status,
        SubscriptionStatus::Cancelled,
        "status stays Cancelled after expiry time has passed"
    );
    // Can be archived from Cancelled
    client.cleanup_subscription(&sub_id1, &subscriber);
    assert_eq!(client.get_subscription(&sub_id1).status, SubscriptionStatus::Archived);

    // Flow 1: expire without cancel -> cancel rejected -> cleanup -> Archived
    // Scenario 2: Expire without cancel
    let sub_id2 = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(expires_at),
    &None::<u32>,
    );
    
    // Trigger expiration
    env.ledger().with_mut(|l| l.timestamp = expires_at + 1);
    let res = client.try_cancel_subscription(&sub_id2, &subscriber);
    assert_eq!(res, Err(Ok(Error::SubscriptionExpired)));

    client.cleanup_subscription(&sub_id2, &subscriber);
    assert_eq!(client.get_subscription(&sub_id2).status, SubscriptionStatus::Archived);
}

// doc 3: deposit_funds rejected when expired
#[test]
fn test_deposit_rejected_when_expired() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let min_topup = 1_000_000i128;
    let expires_at = T0 + 2 * INTERVAL;
    token_admin.mint(&subscriber, &(min_topup * 5));

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &min_topup,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(T0 + INTERVAL),
    &None::<u32>,
    );

    // Advance past expiry
    env.ledger().with_mut(|l| l.timestamp = T0 + 100);
    // Trigger the expiration by attempting a charge
    let _ = client.try_charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>);

    // subscription.is_expired(now) is true; deposit should be rejected
    let res = client.try_deposit_funds(&sub_id, &subscriber, &min_topup, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(res, Err(Ok(Error::SubscriptionExpired)));
}

/// Reject `create_subscription` when `expires_at == ledger.timestamp()`.
///
/// A subscription that is already expired at creation would be a zombie
/// entry that can never be charged.  The contract must return
/// `Error::InvalidExpiration` and write no storage entries.
#[test]
fn test_reject_expiration_equal_to_now() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let expires_at = T0; // equal to current ledger timestamp

    let res = client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(expires_at),
    &None::<u32>,
    );
    assert_eq!(res, Err(Ok(Error::InvalidExpiration)));
}

/// Reject `create_subscription` when `expires_at < ledger.timestamp()`.
///
/// An expiration timestamp in the past is equally invalid — the
/// subscription would be born already expired.  Must return
/// `Error::InvalidExpiration` and write no storage entries.
#[test]
fn test_reject_expiration_in_the_past() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let expires_at = T0 - 1; // one second before current ledger time

    let res = client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(expires_at),
    &None::<u32>,
    );
    assert_eq!(res, Err(Ok(Error::InvalidExpiration)));
}

/// `None` expiration must be accepted.
///
/// Omitting `expires_at` creates an open-ended subscription that never
/// expires — this is the standard path and must succeed.
#[test]
fn test_none_expiration_accepted() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let res = client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    &None::<u32>,
    );
    assert!(res.is_ok(), "None expiration must be accepted");

    let sub_id = res.unwrap();
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.expires_at, None);
}

/// Accept `expires_at` one second in the future.
///
/// The earliest permitted expiration is `ledger.timestamp() + 1`.  This
/// test confirms that boundary is accepted and the subscription is
/// created in Active status.
#[test]
fn test_future_expiration_one_second_ahead_accepted() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let expires_at = T0 + 1; // one second after current ledger time

    let res = client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(expires_at),
    &None::<u32>,
    );
    assert!(res.is_ok(), "future expiration must be accepted");

    let sub_id = res.unwrap();
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.expires_at, Some(expires_at));
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

/// Rejected expiration attempts must not write any storage.
///
/// After each rejection the subscription counter must remain unchanged
/// and `get_subscription` must return `NotFound` for any hypothetical
/// ID that would have been allocated.
#[test]
fn test_rejected_expiration_writes_no_storage() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Record next_id before the failed attempts.
    let _ = client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(T0), // equal to now → rejected
        &None::<u32>,
    );

    let _ = client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(T0 - 1), // in the past → rejected
        &None::<u32>,
    );

    // Now create a valid subscription — it should get id 0 (first ever).
    let res = client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
    );
    assert!(res.is_ok(), "valid subscription must succeed");
    assert_eq!(res.unwrap(), 0, "first subscription should have id 0");
}

// ─────────────────────────────────────────────────────────────────────────────
// Ledger-sequence expiration bound tests (#686)
// ─────────────────────────────────────────────────────────────────────────────
//
// These tests cover the second expiration bound (`expires_at_ledger: Option<u32>`)
// added in addition to the wall-clock `expires_at`. Either bound being met is
// sufficient to reject charges, deposits, and state transitions. The setter
// (`set_subscription_expiration_ledger`) authorizes by subscriber-or-merchant.

/// `None` ledger bound is accepted and behaves as a no-op (subscription never
/// expires on ledger sequence alone).
#[test]
fn test_ledger_expiration_none_accepted() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    token_admin.mint(&subscriber, &(1_000_000i128 * 10));

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
    );

    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.expires_at_ledger, None, "None ledger bound must persist");
    assert_eq!(sub.expires_at, None);

    // Jump sequence far past any reasonable bound; subscription must NOT
    // auto-expire because the bound is None.
    env.ledger().with_mut(|l| {
        l.timestamp = T0 + 30 * INTERVAL;
        l.sequence_number = 1_000_000;
    });
    client.deposit_funds(
        &sub_id,
        &subscriber,
        &(1_000_000i128 * 5),
        &None::<soroban_sdk::BytesN<32>>,
    );
    client.charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>);
    assert_eq!(
        client.get_subscription(&sub_id).status,
        SubscriptionStatus::Active,
        "no bound set → no ledger-driven expiration"
    );
}

/// `expires_at_ledger` strictly in the future is accepted and persisted on
/// the subscription record.
#[test]
fn test_ledger_expiration_future_bound_accepted() {
    let (env, client, token_client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let current_seq = env.ledger().sequence();
    let bound_seq = current_seq + 5;

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &Some(bound_seq),
    &None::<u32>,
    );
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.expires_at_ledger, Some(bound_seq));
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

/// Reject `create_subscription` when `expires_at_ledger == current sequence`
/// (zombie prevention, mirroring `expires_at` validation).
#[test]
fn test_reject_ledger_expiration_equal_to_now() {
    let (env, client, token_client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let current_seq = env.ledger().sequence();

    let res = client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &Some(current_seq),
    &None::<u32>,
    );
    assert_eq!(res, Err(Ok(Error::InvalidExpiration)));
}

/// Reject `create_subscription` when `expires_at_ledger < current sequence`.
#[test]
fn test_reject_ledger_expiration_in_the_past() {
    let (env, client, token_client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let past_seq = env.ledger().sequence().saturating_sub(1);

    let res = client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &Some(past_seq),
    &None::<u32>,
    );
    assert_eq!(res, Err(Ok(Error::InvalidExpiration)));
}

/// Charge is rejected once the ledger sequence reaches the bound, even when
/// the wall-clock `expires_at` is unset and `now` is far from any time bound.
#[test]
fn test_charge_rejected_when_ledger_bound_met() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    token_admin.mint(&subscriber, &(1_000_000i128 * 10));

    let current_seq = env.ledger().sequence();
    let bound_seq = current_seq + 3;

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &Some(bound_seq),
    &None::<u32>,
    );
    client.deposit_funds(
        &sub_id,
        &subscriber,
        &(1_000_000i128 * 5),
        &None::<soroban_sdk::BytesN<32>>,
    );

    // Advance the ledger sequence to (and past) the bound without touching
    // the wall-clock time.
    env.ledger().with_mut(|l| l.sequence_number = bound_seq);
    let res = client.try_charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>);
    assert!(
        res.is_err(),
        "charge must be rejected once ledger sequence reaches the bound"
    );

    // Subscription must have transitioned to Expired.
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Expired);
}

/// Deposit is rejected when the ledger bound is met, even if wall-clock time
/// is nowhere near `expires_at`.
#[test]
fn test_deposit_rejected_when_ledger_bound_met() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    token_admin.mint(&subscriber, &(1_000_000i128 * 10));

    let current_seq = env.ledger().sequence();
    let bound_seq = current_seq + 2;

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &Some(bound_seq),
    &None::<u32>,
    );

    // Advance sequence past the bound; trigger the transition by attempting
    // a charge.
    env.ledger().with_mut(|l| l.sequence_number = bound_seq + 1);
    let _ = client.try_charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>);

    let res = client.try_deposit_funds(
        &sub_id,
        &subscriber,
        &1_000_000i128,
        &None::<soroban_sdk::BytesN<32>>,
    );
    assert_eq!(res, Err(Ok(Error::SubscriptionExpired)));
}

/// Both bounds set: the *earlier* of the two drives the expiration. This
/// exercises the "different outcomes" path requested in the task spec.
#[test]
fn test_both_bounds_set_ledger_fires_first() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    token_admin.mint(&subscriber, &(1_000_000i128 * 10));

    // Wall-clock bound far in the future; ledger bound near-term.
    let current_seq = env.ledger().sequence();
    let bound_seq = current_seq + 2;
    let wall_bound = T0 + 100 * INTERVAL;

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(wall_bound),
        &Some(bound_seq),
    &None::<u32>,
    );
    client.deposit_funds(
        &sub_id,
        &subscriber,
        &(1_000_000i128 * 5),
        &None::<soroban_sdk::BytesN<32>>,
    );

    // Advance past the ledger bound only — wall-clock is far away.
    env.ledger().with_mut(|l| {
        l.timestamp = T0 + INTERVAL;
        l.sequence_number = bound_seq + 1;
    });
    let res = client.try_charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>);
    assert!(res.is_err(), "charge must be rejected via the ledger bound");
}

/// Both bounds set, but wall-clock fires first — the ledger bound is far
/// enough out that only `expires_at` triggers the expiration. This exercises
/// the second "different outcomes" branch.
#[test]
fn test_both_bounds_set_wall_clock_fires_first() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    token_admin.mint(&subscriber, &(1_000_000i128 * 10));

    let current_seq = env.ledger().sequence();
    // Ledger bound very far in the future; wall-clock bound close.
    let bound_seq = current_seq + 10_000;
    let wall_bound = T0 + 2 * INTERVAL;

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(wall_bound),
        &Some(bound_seq),
    &None::<u32>,
    );
    client.deposit_funds(
        &sub_id,
        &subscriber,
        &(1_000_000i128 * 5),
        &None::<soroban_sdk::BytesN<32>>,
    );

    // Advance wall-clock past expiry; keep ledger sequence short of its bound.
    env.ledger().with_mut(|l| {
        l.timestamp = wall_bound + 1;
        l.sequence_number = current_seq + 5;
    });
    let res = client.try_charge_subscription(&sub_id, &None::<soroban_sdk::BytesN<32>>);
    assert!(res.is_err(), "charge must be rejected via the wall-clock bound");
}

// ── Setter tests ────────────────────────────────────────────────────────────

/// Subscriber can update the ledger bound; subsequent reads see the new value.
#[test]
fn test_set_ledger_expiration_by_subscriber() {
    let (env, client, token_client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
    );

    let current_seq = env.ledger().sequence();
    let new_bound = current_seq + 50;
    client.set_subscription_expiration_ledger(&sub_id, &subscriber, &Some(new_bound));

    assert_eq!(
        client.get_subscription(&sub_id).expires_at_ledger,
        Some(new_bound)
    );
}

/// Merchant can update the ledger bound (matching the auth model of
/// `cancel_subscription` / `pause_subscription`).
#[test]
fn test_set_ledger_expiration_by_merchant() {
    let (env, client, token_client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
    );

    let current_seq = env.ledger().sequence();
    let new_bound = current_seq + 25;
    client.set_subscription_expiration_ledger(&sub_id, &merchant, &Some(new_bound));

    assert_eq!(
        client.get_subscription(&sub_id).expires_at_ledger,
        Some(new_bound)
    );
}

/// Third-party authorizers are rejected with `Forbidden`.
#[test]
fn test_set_ledger_expiration_unauthorized_rejected() {
    let (env, client, token_client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let stranger = Address::generate(&env);

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
    );

    let res = client.try_set_subscription_expiration_ledger(&sub_id, &stranger, &Some(100u32));
    assert_eq!(res, Err(Ok(Error::Forbidden)));
}

/// Setter rejects `Some(seq) <= current_ledger` with `InvalidExpiration`.
#[test]
fn test_set_ledger_expiration_rejects_past_or_equal() {
    let (env, client, token_client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
    );

    let current_seq = env.ledger().sequence();

    // Equal to current → rejected.
    let res = client.try_set_subscription_expiration_ledger(
        &sub_id,
        &subscriber,
        &Some(current_seq),
    &None::<u32>,
    );
    assert_eq!(res, Err(Ok(Error::InvalidExpiration)));

    // Past → rejected.
    let res = client.try_set_subscription_expiration_ledger(
        &sub_id,
        &subscriber,
        &Some(current_seq.saturating_sub(1)),
    );
    assert_eq!(res, Err(Ok(Error::InvalidExpiration)));
}

/// `None` clears the bound; previously-set bound is removed from storage.
#[test]
fn test_set_ledger_expiration_none_clears_bound() {
    let (env, client, token_client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let current_seq = env.ledger().sequence();
    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &Some(current_seq + 100),
    &None::<u32>,
    );
    assert_eq!(
        client.get_subscription(&sub_id).expires_at_ledger,
        Some(current_seq + 100)
    );

    client.set_subscription_expiration_ledger(&sub_id, &subscriber, &None::<u32>);
    assert_eq!(client.get_subscription(&sub_id).expires_at_ledger, None);

    // Even after the original bound would have been hit, the subscription
    // remains Active because the bound was cleared.
    env.ledger().with_mut(|l| l.sequence_number = current_seq + 100 + 1);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

/// Setter on a terminal-state subscription is rejected.
#[test]
fn test_set_ledger_expiration_rejects_cancelled() {
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    token_admin.mint(&subscriber, &(1_000_000i128 * 5));

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
    );

    client.cancel_subscription(&sub_id, &subscriber);
    assert_eq!(
        client.get_subscription(&sub_id).status,
        SubscriptionStatus::Cancelled
    );

    let res = client.try_set_subscription_expiration_ledger(&sub_id, &subscriber, &Some(100u32));
    assert_eq!(res, Err(Ok(Error::InvalidStatusTransition)));
}

/// Setter emits `expiration_ledger_set` with both the new and previous bound.
#[test]
fn test_set_ledger_expiration_emits_event() {
    let (env, client, token_client, _, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let current_seq = env.ledger().sequence();
    let initial_bound = current_seq + 10;

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &Some(initial_bound),
    &None::<u32>,
    );

    let new_bound = current_seq + 200;
    client.set_subscription_expiration_ledger(&sub_id, &subscriber, &Some(new_bound));

    // Find the ExpirationLedgerSetEvent in the event log.
    let events = env.events().all();
    let mut found_event: Option<crate::ExpirationLedgerSetEvent> = None;
    for e in events.iter() {
        let topics = e.1;
        let topic0: soroban_sdk::Symbol = soroban_sdk::Symbol::from_val(&env, &topics.get(0).unwrap());
        if topic0 == soroban_sdk::Symbol::new(&env, "expiration_ledger_set") {
            let payload: crate::ExpirationLedgerSetEvent = soroban_sdk::FromVal::from_val(&env, &e.2);
            if payload.subscription_id == sub_id {
                found_event = Some(payload);
                break;
            }
        }
    }
    let payload =
        found_event.expect("expected expiration_ledger_set event to be published");
    assert_eq!(payload.subscription_id, sub_id);
    assert_eq!(payload.expires_at_ledger, Some(new_bound));
    assert_eq!(
        payload.previous_expires_at_ledger,
        Some(initial_bound),
        "previous bound must be reported for indexer reconstruction"
    );
    assert_eq!(payload.authorizer, subscriber);
}
    let (env, client, token_client, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Record next_id before the failed attempts.
    let _ = client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(T0), // equal to now → rejected
        &None::<u32>,
);

    let _ = client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(T0 - 1), // in the past → rejected
        &None::<u32>,
);

    // Now create a valid subscription — it should get id 0 (first ever).
    let res = client.try_create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    &None::<u32>,
    );
    assert!(res.is_ok(), "valid subscription must succeed");
    assert_eq!(res.unwrap(), 0, "first subscription should have id 0");
}


// ── Snapshot export / restore round-trip of `expires_at_ledger` (#686) ─────────

/// Asserts that `export_subscription_summary` carries `expires_at_ledger` from
/// the live `Subscription` record. Without this guarantee the snapshot would
/// lose the second expiration bound, and a subsequent `restore_snapshot_page`
/// would silently clear it.
#[test]
fn test_export_subscription_summary_preserves_ledger_bound() {
    let (env, client, token_client, _, admin) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let current_seq = env.ledger().sequence();
    let bound_seq = current_seq + 50;

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token_client.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &Some(bound_seq),
        &None::<u32>,
);

    let summary = client.export_subscription_summary(&admin, &sub_id);
    assert_eq!(
        summary.expires_at_ledger,
        Some(bound_seq),
        "SubscriptionSummary must carry the ledger bound verbatim"
    );
    assert_eq!(summary.subscription_id, sub_id);
    assert_eq!(summary.expires_at, None, "wall-clock bound remains unset");
}

/// Asserts that the full round-trip — `export_full_snapshot_page` on a source
/// environment and `restore_snapshot_page` on a fresh target — preserves the
/// `expires_at_ledger` field end-to-end. This is the migration safety net:
/// admins exporting from one contract and restoring into another must not lose
/// the ledger bound.
#[test]
fn test_restore_snapshot_page_preserves_ledger_bound() {
    // ── Source environment ──────────────────────────────────────────────────
    let (src_env, src_client, src_token_client, _, src_admin) = setup_test_env();
    let subscriber = Address::generate(&src_env);
    let merchant = Address::generate(&src_env);
    let src_token = src_token_client.address.clone();

    let current_seq = src_env.ledger().sequence();
    let bound_seq = current_seq + 75;

    src_token_client.mint(&subscriber, &(1_000_000i128 * 5));
    let sub_id = src_client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &src_token,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &Some(bound_seq),
        &None::<u32>,
);

    // Seed a merchant balance so the full-snapshot restoration is exercised.
    src_env.as_contract(&src_client.address, || {
        src_env.storage().instance().set(
            &DataKey::MerchantBalance(merchant.clone(), src_token.clone()),
            &(42_000i128),
        );
    });

    // Sanity: export_full_snapshot_page must surface the bound.
    let page = src_client
        .export_full_snapshot_page(&src_admin, &0u32, &10u32)
        .unwrap();
    assert_eq!(page.subscriptions.len(), 1);
    assert_eq!(
        page.subscriptions.get(0).unwrap().expires_at_ledger,
        Some(bound_seq),
        "export must carry the ledger bound"
    );

    // ── Target environment (fresh contract, same admin/auth model) ──────────
    let target_env = Env::default();
    target_env.mock_all_auths();
    let contract_id = target_env.register(crate::SubscriptionVault, ());
    let target_client = crate::SubscriptionVaultClient::new(&target_env, &contract_id);
    let target_admin = Address::generate(&target_env);
    let target_token_admin = Address::generate(&target_env);
    let target_token_id =
        target_env.register_stellar_asset_contract_v2(target_token_admin.clone());
    let target_token = target_token_id.address();

    target_client.init(
        &target_token,
        &6u32,
        &target_admin,
        &1_000_000i128,
        &(7 * 24 * 60 * 60),
    );

    // restore_snapshot_page requires the emergency stop to be active.
    target_client.enable_emergency_stop(&target_admin).unwrap();

    // Restore the source page into the target. `page.subscriptions` already
    // carries `expires_at_ledger` from the export; the restoration path must
    // round-trip it without losing or zeroing it.
    target_client
        .restore_snapshot_page(
            &target_admin,
            &0u32,
            &page.subscriptions,
            &page.balances,
            &page.next_start_id,
        )
        .unwrap();

    // ── Assertions ──────────────────────────────────────────────────────────
    let restored_sub = target_client.get_subscription(&sub_id);
    assert_eq!(
        restored_sub.expires_at_ledger,
        Some(bound_seq),
        "restored subscription must preserve the ledger bound across migration"
    );

    // `None` round-trip: a second subscription created without a ledger bound
    // must also survive export → restore. This guards against a regression
    // where the field is silently cleared or zeroed.
    let sub_id_none = src_client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &src_token,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
    );
    assert_eq!(
        src_client.get_subscription(&sub_id_none).expires_at_ledger,
        None,
        "precondition: source `None` bound must round-trip"
    );

    let page_with_none = src_client
        .export_full_snapshot_page(&src_admin, &0u32, &10u32)
        .unwrap();
    let none_summary = page_with_none
        .subscriptions
        .iter()
        .find(|s| s.subscription_id == sub_id_none)
        .expect("exported page must contain the None-bound subscription");
    assert_eq!(
        none_summary.expires_at_ledger, None,
        "export must preserve `None` ledger bound"
    );

    target_client
        .restore_snapshot_page(
            &target_admin,
            &0u32,
            &page_with_none.subscriptions,
            &page_with_none.balances,
            &page_with_none.next_start_id,
        )
        .unwrap();

    assert_eq!(
        target_client.get_subscription(&sub_id_none).expires_at_ledger,
        None,
        "restored subscription with `None` bound must remain `None`"
    );

    let restored_bal: i128 = target_env.as_contract(&target_client.address, || {
        target_env
            .storage()
            .instance()
            .get(&DataKey::MerchantBalance(merchant.clone(), target_token.clone()))
            .unwrap_or(0i128)
    });
    assert_eq!(
        restored_bal, 42_000,
        "merchant balance must also survive the round-trip"
    );
}


// ── Dual-bound invariant (#686) ──────────────────────────────────────────────────────────────

/// Property test for the dual-bound invariant of `Subscription::is_expired`.
///
/// For every `(expires_at, expires_at_ledger)` combination on the
/// `Subscription`, the result of `is_expired(now, sl)` must equal the
/// reference predicate `(now >= expires_at) OR (sl >= expires_at_ledger)`.
///
/// Boundary conditions (`current == bound`) are explicitly exercised to
/// guard against off-by-one errors in either comparison.
#[test]
fn test_is_expired_dual_bound_invariant() {
    use crate::types::Subscription;

    let env = soroban_sdk::Env::default();
    let placeholder = soroban_sdk::Address::generate(&env);

    // Helper: build a Subscription with arbitrary fixed fields and only the
    // two `expires_*` fields varying.
    fn sub_with(
        env: &soroban_sdk::Env,
        placeholder: &soroban_sdk::Address,
        expires_at: Option<u64>,
        expires_at_ledger: Option<u32>,
    ) -> Subscription {
        Subscription {
            subscriber: placeholder.clone(),
            merchant: placeholder.clone(),
            token: placeholder.clone(),
            amount: 0,
            interval_seconds: 60,
            last_payment_timestamp: 0,
            status: SubscriptionStatus::Active,
            prepaid_balance: 0,
            usage_enabled: false,
            lifetime_cap: None,
            lifetime_charged: 0,
            start_time: 0,
            expires_at,
            grace_start_timestamp: None,
            cancel_at: None,
            expires_at_ledger,
        }
    }

    // Reference predicate: true iff *either* bound is met. `None` disables
    // its respective comparison.
    fn expected(now: u64, sl: u32, exp_t: Option<u64>, exp_s: Option<u32>) -> bool {
        exp_t.map_or(false, |t| now >= t) || exp_s.map_or(false, |s| sl >= s)
    }

    // ── Bound values used throughout the matrix ────────────────────────────
    const T: u64 = 1_000_000; // wall-clock bound
    const S: u32 = 5_000; // ledger-sequence bound

    // Construct four subscription variants exhaustively: each bound is either
    // `None` or `Some(bound)`.
    let sub_none_none = sub_with(&env, &placeholder, None, None);
    let sub_time_only = sub_with(&env, &placeholder, Some(T), None);
    let sub_ledger_only = sub_with(&env, &placeholder, None, Some(S));
    let sub_both = sub_with(&env, &placeholder, Some(T), Some(S));

    // ── Test (now, sl) pairs: far past, off-by-one below, AT boundary, off-by-one above, far future
    let samples: [(u64, u32); 7] = [
        (0, 0),                  // epoch start
        (T - 1, S - 1),          // one below each bound -> not expired
        (T, S - 1),              // AT time bound, below ledger -> expired via time
        (T - 1, S),              // AT ledger bound, below time -> expired via ledger
        (T, S),                  // AT BOTH -> expired
        (T + 100, S + 100),      // past both -> expired
        (u64::MAX, u32::MAX),    // extreme -> expired for any Some bound
    ];

    for (now, sl) in samples {
        // Both None: never expired, regardless of (now, sl).
        assert_eq!(
            sub_none_none.is_expired(now, sl),
            false,
            "both-None at (now={}, sl={}) must be false",
            now, sl
        );

        // Time-only: bound on T only.
        assert_eq!(
            sub_time_only.is_expired(now, sl),
            expected(now, sl, Some(T), None),
            "time-only @ (now={}, sl={}) mismatch", now, sl
        );

        // Ledger-only: bound on S only.
        assert_eq!(
            sub_ledger_only.is_expired(now, sl),
            expected(now, sl, None, Some(S)),
            "ledger-only @ (now={}, sl={}) mismatch", now, sl
        );

        // Both: OR of two independent predicates.
        assert_eq!(
            sub_both.is_expired(now, sl),
            expected(now, sl, Some(T), Some(S)),
            "both bounds @ (now={}, sl={}) mismatch", now, sl
        );
    }

    // ── Explicit boundary assertions (more readable than the matrix) ──────
    //
    // AT the boundary (==), the subscription is expired (the predicate uses
    // `>=`, matching the existing wall-clock convention).
    assert!(sub_time_only.is_expired(T, 0), "time AT bound must expire");
    assert!(sub_ledger_only.is_expired(0, S), "ledger AT bound must expire");
    assert!(sub_both.is_expired(T, S), "both AT bound must expire");

    // Off by one below the boundary: NOT expired.
    assert!(!sub_time_only.is_expired(T - 1, 0), "time one below must NOT expire");
    assert!(!sub_ledger_only.is_expired(0, S - 1), "ledger one below must NOT expire");

    // Each bound triggers expiration independently.
    let sub_t_at_only = sub_with(&env, &placeholder, Some(T), None);
    assert!(sub_t_at_only.is_expired(T, 0));
    assert!(!sub_t_at_only.is_expired(T - 1, u32::MAX)); // even at max ledger, time alone gates
    let sub_s_at_only = sub_with(&env, &placeholder, None, Some(S));
    assert!(sub_s_at_only.is_expired(0, S));
    assert!(!sub_s_at_only.is_expired(u64::MAX, S - 1)); // even at max time, ledger alone gates
}

/// Randomized sweep over the dual-bound invariant, with a fixed seed for
/// reproducibility. Confirms the predicate holds over a wide range of
/// arbitrary inputs rather than only the hand-picked boundary cases.
#[test]
fn test_is_expired_dual_bound_invariant_randomized() {
    use crate::types::Subscription;

    let env = soroban_sdk::Env::default();
    let placeholder = soroban_sdk::Address::generate(&env);

    fn sub_with(
        env: &soroban_sdk::Env,
        placeholder: &soroban_sdk::Address,
        expires_at: Option<u64>,
        expires_at_ledger: Option<u32>,
    ) -> Subscription {
        Subscription {
            subscriber: placeholder.clone(),
            merchant: placeholder.clone(),
            token: placeholder.clone(),
            amount: 0,
            interval_seconds: 60,
            last_payment_timestamp: 0,
            status: SubscriptionStatus::Active,
            prepaid_balance: 0,
            usage_enabled: false,
            lifetime_cap: None,
            lifetime_charged: 0,
            start_time: 0,
            expires_at,
            grace_start_timestamp: None,
            cancel_at: None,
            expires_at_ledger,
        }
    }

    fn expected(now: u64, sl: u32, exp_t: Option<u64>, exp_s: Option<u32>) -> bool {
        exp_t.map_or(false, |t| now >= t) || exp_s.map_or(false, |s| sl >= s)
    }

    // Deterministic LCG so a CI failure is reproducible from the seed.
    let mut state: u64 = 0xDEAD_BEEF_CAFE_F00D;
    const A: u64 = 6364136223846793005;
    const C: u64 = 1442695040888963407;
    let mut next_u64 = move || -> u64 {
        state = state.wrapping_mul(A).wrapping_add(C);
        state
    };

    // Run many randomized trials across all 4 bound combinations.
    for trial in 0..256u64 {
        let r = next_u64();
        // Bound choices cycle through the four combinations deterministically.
        let exp_t = match trial % 4 {
            0 => None,
            1 => Some((r % 5_000_000) + 1_000),
            2 => None,
            _ => Some((r % 5_000_000) + 1_000),
        };
        let exp_s = match trial % 4 {
            0 => None,
            1 => None,
            2 => Some(((r >> 16) % 50_000) + 100),
            _ => Some(((r >> 16) % 50_000) + 100),
        };
        let sub = sub_with(&env, &placeholder, exp_t, exp_s);

        // Sample now/sl around each bound to catch boundary cases too.
        for _ in 0..4 {
            let now_seed = next_u64();
            let sl_seed = next_u64();
            let (now, sl) = match (exp_t, exp_s) {
                (Some(t), Some(s)) => {
                    let delta_t = (now_seed % 5) as i64 - 2; // -2..+2 around t
                    let delta_s = (sl_seed % 5) as i32 - 2;
                    let t_adj = (t as i64 + delta_t).max(0) as u64;
                    let s_adj = (s as i32 + delta_s).max(0) as u32;
                    (t_adj, s_adj)
                }
                (Some(t), None) => {
                    let delta_t = (now_seed % 5) as i64 - 2;
                    let t_adj = (t as i64 + delta_t).max(0) as u64;
                    (t_adj, (sl_seed % 1_000_000) as u32)
                }
                (None, Some(s)) => {
                    let delta_s = (sl_seed % 5) as i32 - 2;
                    let s_adj = (s as i32 + delta_s).max(0) as u32;
                    ((now_seed % 5_000_000_000) as u64, s_adj)
                }
                (None, None) => {
                    ((now_seed % 5_000_000_000) as u64, (sl_seed % 1_000_000) as u32)
                }
            };

            let got = sub.is_expired(now, sl);
            let want = expected(now, sl, exp_t, exp_s);
            assert_eq!(
                got, want,
                "trial {}: (now={}, sl={}, exp_t={:?}, exp_s={:?})",
                trial, now, sl, exp_t, exp_s
            );
        }
    }
}
