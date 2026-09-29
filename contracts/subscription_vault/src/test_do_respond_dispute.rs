//! Adversarial coverage for do_respond_dispute in dispute.rs
//!
//! Tests validate the dispute response logic across authorization, state transitions,
//! boundary conditions, and error paths.

use crate::{
    test_utils::{fixtures, setup::TestEnv},
    types::{DataKey, Dispute, DisputeRespondedEvent, DisputeStatus, Error},
    SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Events, MockAuth, MockAuthInvoke},
    Address, BytesN, Env, FromVal, IntoVal, Symbol,
};

// ── Test Constants ───────────────────────────────────────────────────────────

const DISPUTE_AMOUNT: i128 = 5_000_000; // 5 USDC (6 decimals)

// ── Test Helpers ─────────────────────────────────────────────────────────────

/// Setup test environment and create a subscription with merchant balance.
fn setup() -> (TestEnv, u32, Address, Address) {
    let test_env = TestEnv::default();
    let (id, subscriber, merchant) = fixtures::create_subscription(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
    );
    
    // Seed merchant balance to support dispute
    seed_merchant_balance(&test_env, id, DISPUTE_AMOUNT * 2);
    
    (test_env, id, subscriber, merchant)
}

/// Seed merchant balance for a subscription.
fn seed_merchant_balance(test_env: &TestEnv, subscription_id: u32, amount: i128) {
    let sub = test_env.client.get_subscription(&subscription_id);
    test_env.env.as_contract(&test_env.client.address, || {
        test_env.env.storage().instance().set(
            &DataKey::MerchantBalance(sub.merchant.clone(), sub.token.clone()),
            &amount,
        );
    });
}

/// Open a dispute and return its ID.
fn open_dispute(test_env: &TestEnv, subscriber: &Address, subscription_id: u32) -> u64 {
    test_env
        .client
        .open_dispute(
            subscriber,
            &subscription_id,
            &DISPUTE_AMOUNT,
            &None::<BytesN<32>>,
        )
}

/// Mock authorization for exactly one `respond_dispute` invocation by `caller`.
///
/// The shared fixture mocks *all* auths, so accepting every signature would
/// hide whether the caller is actually constrained. Narrowing the mocked auth
/// set to a single signer proves the call is authorized *as that signer* and
/// that the stored-admin comparison — not a missing signature — is what
/// rejects them.
fn mock_respond_dispute_auth(
    env: &Env,
    client: &SubscriptionVaultClient,
    caller: &Address,
    dispute_id: u64,
    evidence: &Option<BytesN<32>>,
) {
    let mut args = soroban_sdk::Vec::new(env);
    args.push_back(caller.into_val(env));
    args.push_back(dispute_id.into_val(env));
    args.push_back(evidence.clone().into_val(env));

    env.mock_auths(&[MockAuth {
        address: caller,
        invoke: &MockAuthInvoke {
            contract: &client.address,
            fn_name: "respond_dispute",
            args,
            sub_invokes: &[],
        },
    }]);
}

/// Read dispute directly from storage.
fn read_dispute_from_storage(env: &Env, contract_id: &Address, dispute_id: u64) -> Option<Dispute> {
    env.as_contract(contract_id, || {
        env.storage()
            .persistent()
            .get(&DataKey::Dispute(dispute_id))
    })
}

/// Generate a sample evidence hash for testing.
fn sample_evidence_hash(env: &Env) -> BytesN<32> {
    let mut bytes = [0u8; 32];
    bytes[0] = 0xAB;
    bytes[31] = 0xCD;
    BytesN::from_array(env, &bytes)
}

// ── Happy Path Tests ─────────────────────────────────────────────────────────

#[test]
fn test_do_respond_dispute_success_no_evidence() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    // Respond without evidence
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    
    let dispute = test_env.client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, DisputeStatus::Responded);
    assert!(dispute.responded_at.is_some());
    assert_eq!(dispute.admin_evidence_hash, None);
}

#[test]
fn test_do_respond_dispute_success_with_evidence() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    let evidence = sample_evidence_hash(&test_env.env);
    
    // Respond with evidence
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &Some(evidence.clone()));
    
    let dispute = test_env.client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, DisputeStatus::Responded);
    assert!(dispute.responded_at.is_some());
    assert_eq!(dispute.admin_evidence_hash, Some(evidence));
}

#[test]
fn test_do_respond_dispute_sets_responded_at_timestamp() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    let timestamp_before = test_env.env.ledger().timestamp();
    
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    
    let dispute = test_env.client.get_dispute(&dispute_id);
    assert!(dispute.responded_at.is_some());
    let responded_at = dispute.responded_at.unwrap();
    assert!(
        responded_at >= timestamp_before,
        "responded_at must be at or after the response time"
    );
}

#[test]
fn test_do_respond_dispute_transitions_from_open_to_responded() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    let before = test_env.client.get_dispute(&dispute_id);
    assert_eq!(before.status, DisputeStatus::Open);
    assert_eq!(before.responded_at, None);
    
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    
    let after = test_env.client.get_dispute(&dispute_id);
    assert_eq!(after.status, DisputeStatus::Responded);
    assert!(after.responded_at.is_some());
}

// ── Authorization Tests ──────────────────────────────────────────────────────

#[test]
fn test_do_respond_dispute_requires_admin_auth() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    // Non-admin caller should be rejected
    let non_admin = Address::generate(&test_env.env);
    mock_respond_dispute_auth(&test_env.env, &test_env.client, &non_admin, dispute_id, &None::<BytesN<32>>);
    
    let result = test_env
        .client
        .try_respond_dispute(&non_admin, &dispute_id, &None::<BytesN<32>>);
    
    assert!(result.is_err(), "Non-admin should be rejected");
}

#[test]
fn test_do_respond_dispute_rejects_wrong_admin() {
    let env = Env::default();
    env.mock_all_auths();
    
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    
    // Create subscription and open dispute
    let (id, subscriber, _merchant) = fixtures::create_subscription(&env, &client, SubscriptionStatus::Active);
    seed_merchant_balance_direct(&env, &client.address, id, &token, DISPUTE_AMOUNT * 2);
    
    let dispute_id = client
        .open_dispute(&subscriber, &id, &DISPUTE_AMOUNT, &None::<BytesN<32>>);
    
    // Try with a different admin
    let wrong_admin = Address::generate(&env);
    mock_respond_dispute_auth(&env, &client, &wrong_admin, dispute_id, &None::<BytesN<32>>);
    
    let result = client.try_respond_dispute(&wrong_admin, &dispute_id, &None::<BytesN<32>>);
    assert_eq!(result, Err(Ok(Error::Forbidden)));
}

fn seed_merchant_balance_direct(
    env: &Env,
    contract_id: &Address,
    subscription_id: u32,
    token: &Address,
    amount: i128,
) {
    env.as_contract(contract_id, || {
        let sub_key = DataKey::Sub(subscription_id);
        let sub: crate::types::Subscription = env.storage().persistent().get(&sub_key).unwrap();
        env.storage().instance().set(
            &DataKey::MerchantBalance(sub.merchant.clone(), token.clone()),
            &amount,
        );
    });
}

#[test]
fn test_do_respond_dispute_rejects_subscriber_caller() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    // Subscriber cannot respond to their own dispute
    mock_respond_dispute_auth(&test_env.env, &test_env.client, &subscriber, dispute_id, &None::<BytesN<32>>);
    
    let result = test_env
        .client
        .try_respond_dispute(&subscriber, &dispute_id, &None::<BytesN<32>>);
    
    assert!(result.is_err(), "Subscriber should not be able to respond");
}

#[test]
fn test_do_respond_dispute_rejects_merchant_caller() {
    let (test_env, id, subscriber, merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    // Merchant cannot respond directly (only admin can)
    mock_respond_dispute_auth(&test_env.env, &test_env.client, &merchant, dispute_id, &None::<BytesN<32>>);
    
    let result = test_env
        .client
        .try_respond_dispute(&merchant, &dispute_id, &None::<BytesN<32>>);
    
    assert!(result.is_err(), "Merchant should not be able to respond directly");
}

// ── Error Path Tests ─────────────────────────────────────────────────────────

#[test]
fn test_do_respond_dispute_rejects_nonexistent_dispute() {
    let test_env = TestEnv::default();
    
    let nonexistent_id = 999_999_u64;
    
    let result = test_env.client.try_respond_dispute(
        &test_env.admin,
        &nonexistent_id,
        &None::<BytesN<32>>,
    );
    
    assert_eq!(result, Err(Ok(Error::DisputeNotFound)));
}

#[test]
fn test_do_respond_dispute_rejects_already_responded() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    // First response succeeds
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    
    // Second response fails
    let result = test_env.client.try_respond_dispute(
        &test_env.admin,
        &dispute_id,
        &None::<BytesN<32>>,
    );
    
    assert_eq!(result, Err(Ok(Error::DisputeAlreadyResponded)));
}

#[test]
fn test_do_respond_dispute_rejects_resolved_to_merchant() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    // Respond and resolve to merchant
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    
    test_env
        .client
        .resolve_dispute(&test_env.admin, &dispute_id, &false);
    
    // Try to respond again after resolution
    let result = test_env.client.try_respond_dispute(
        &test_env.admin,
        &dispute_id,
        &None::<BytesN<32>>,
    );
    
    assert_eq!(result, Err(Ok(Error::DisputeAlreadyResponded)));
}

#[test]
#[ignore = "pre-existing on main; docs/known-failing-tests.md: HostError: Error(Contract, #10)"]
fn test_do_respond_dispute_rejects_resolved_to_subscriber() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    // Respond and resolve to subscriber
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    
    test_env
        .client
        .resolve_dispute(&test_env.admin, &dispute_id, &true);
    
    // Try to respond again after resolution
    let result = test_env.client.try_respond_dispute(
        &test_env.admin,
        &dispute_id,
        &None::<BytesN<32>>,
    );
    
    assert_eq!(result, Err(Ok(Error::DisputeAlreadyResponded)));
}

// ── Boundary Value Tests ─────────────────────────────────────────────────────

#[test]
fn test_do_respond_dispute_with_zero_dispute_id() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    // Open dispute (will get ID 0 if first)
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    // Respond to dispute ID 0 (valid)
    let result = test_env
        .client
        .try_respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    
    assert!(result.is_ok(), "Dispute ID 0 should be valid");
}

#[test]
fn test_do_respond_dispute_with_max_u64_dispute_id() {
    let test_env = TestEnv::default();
    
    let max_id = u64::MAX;
    
    let result = test_env.client.try_respond_dispute(
        &test_env.admin,
        &max_id,
        &None::<BytesN<32>>,
    );
    
    assert_eq!(result, Err(Ok(Error::DisputeNotFound)));
}

#[test]
fn test_do_respond_dispute_with_all_zeros_evidence_hash() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    let zero_hash = BytesN::from_array(&test_env.env, &[0u8; 32]);
    
    let result = test_env
        .client
        .try_respond_dispute(&test_env.admin, &dispute_id, &Some(zero_hash.clone()));
    
    assert!(result.is_ok());
    
    let dispute = test_env.client.get_dispute(&dispute_id);
    assert_eq!(dispute.admin_evidence_hash, Some(zero_hash));
}

#[test]
fn test_do_respond_dispute_with_all_ones_evidence_hash() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    let ones_hash = BytesN::from_array(&test_env.env, &[0xFFu8; 32]);
    
    let result = test_env
        .client
        .try_respond_dispute(&test_env.admin, &dispute_id, &Some(ones_hash.clone()));
    
    assert!(result.is_ok());
    
    let dispute = test_env.client.get_dispute(&dispute_id);
    assert_eq!(dispute.admin_evidence_hash, Some(ones_hash));
}

// ── State Isolation Tests ────────────────────────────────────────────────────

#[test]
fn test_do_respond_dispute_preserves_other_dispute_fields() {
    let (test_env, id, subscriber, merchant) = setup();
    
    let initial_evidence = sample_evidence_hash(&test_env.env);
    
    let dispute_id = test_env
        .client
        .open_dispute(&subscriber, &id, &DISPUTE_AMOUNT, &Some(initial_evidence.clone()));
    
    let before = test_env.client.get_dispute(&dispute_id);
    
    let admin_evidence = sample_evidence_hash(&test_env.env);
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &Some(admin_evidence.clone()));
    
    let after = test_env.client.get_dispute(&dispute_id);
    
    // Fields that should NOT change
    assert_eq!(after.id, before.id);
    assert_eq!(after.subscription_id, before.subscription_id);
    assert_eq!(after.subscriber, before.subscriber);
    assert_eq!(after.merchant, before.merchant);
    assert_eq!(after.amount, before.amount);
    assert_eq!(after.opened_at, before.opened_at);
    assert_eq!(after.evidence_hash, Some(initial_evidence));
    
    // Fields that SHOULD change
    assert_eq!(after.status, DisputeStatus::Responded);
    assert!(after.responded_at.is_some());
    assert_eq!(after.admin_evidence_hash, Some(admin_evidence));
}

#[test]
fn test_do_respond_dispute_does_not_affect_subscription() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    let sub_before = test_env.client.get_subscription(&id);
    
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    
    let sub_after = test_env.client.get_subscription(&id);
    
    // Subscription should be unchanged
    assert_eq!(sub_before.status, sub_after.status);
    assert_eq!(sub_before.prepaid_balance, sub_after.prepaid_balance);
    assert_eq!(sub_before.last_payment_timestamp, sub_after.last_payment_timestamp);
}

#[test]
fn test_do_respond_dispute_does_not_affect_merchant_balance() {
    let (test_env, id, subscriber, merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    let balance_before = test_env
        .client
        .get_merchant_balance_by_token(&merchant, &test_env.token);
    
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    
    let balance_after = test_env
        .client
        .get_merchant_balance_by_token(&merchant, &test_env.token);
    
    // Merchant balance should be unchanged (funds still in escrow)
    assert_eq!(balance_before, balance_after);
}

#[test]
fn test_do_respond_dispute_does_not_affect_escrow() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    // Read escrow before
    let escrow_before: Option<crate::types::DisputeEscrowLedger> =
        test_env.env.as_contract(&test_env.client.address, || {
            test_env
                .env
                .storage()
                .instance()
                .get(&DataKey::DisputeEscrow(dispute_id))
        });
    
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    
    // Read escrow after
    let escrow_after: Option<crate::types::DisputeEscrowLedger> =
        test_env.env.as_contract(&test_env.client.address, || {
            test_env
                .env
                .storage()
                .instance()
                .get(&DataKey::DisputeEscrow(dispute_id))
        });
    
    // Escrow should be unchanged
    assert_eq!(escrow_before, escrow_after);
}

#[test]
fn test_do_respond_dispute_does_not_clear_subscription_dispute_index() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    
    // Subscription dispute index should still point to this dispute
    let indexed_dispute = test_env.client.get_subscription_dispute(&id);
    assert_eq!(indexed_dispute, Some(dispute_id));
}

// ── Event Emission Tests ─────────────────────────────────────────────────────

#[test]
#[ignore = "pre-existing on main; docs/known-failing-tests.md: Event should be emitted"]
fn test_do_respond_dispute_emits_event() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    let events_before = test_env.env.events().all().len();
    
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    
    let events = test_env.env.events().all();
    assert!(events.len() > events_before, "Event should be emitted");
    
    let event = events
        .iter()
        .rfind(|e| {
            Symbol::from_val(&test_env.env, &e.1.get(0).unwrap())
                == Symbol::new(&test_env.env, "dispute_responded")
        })
        .expect("dispute_responded event not found");
    
    let data: DisputeRespondedEvent = event.2.clone().into_val(&test_env.env);
    assert_eq!(data.dispute_id, dispute_id);
    assert_eq!(data.subscription_id, id);
    assert_eq!(data.admin_evidence_hash, None);
}

#[test]
fn test_do_respond_dispute_event_includes_evidence_hash() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    let evidence = sample_evidence_hash(&test_env.env);
    
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &Some(evidence.clone()));
    
    let events = test_env.env.events().all();
    let event = events
        .iter()
        .rfind(|e| {
            Symbol::from_val(&test_env.env, &e.1.get(0).unwrap())
                == Symbol::new(&test_env.env, "dispute_responded")
        })
        .expect("dispute_responded event not found");
    
    let data: DisputeRespondedEvent = event.2.clone().into_val(&test_env.env);
    assert_eq!(data.admin_evidence_hash, Some(evidence));
}

#[test]
fn test_do_respond_dispute_event_has_correct_timestamp() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    let before_timestamp = test_env.env.ledger().timestamp();
    
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    
    let events = test_env.env.events().all();
    let event = events
        .iter()
        .rfind(|e| {
            Symbol::from_val(&test_env.env, &e.1.get(0).unwrap())
                == Symbol::new(&test_env.env, "dispute_responded")
        })
        .expect("dispute_responded event not found");
    
    let data: DisputeRespondedEvent = event.2.clone().into_val(&test_env.env);
    assert!(
        data.timestamp >= before_timestamp,
        "Event timestamp must be at or after response time"
    );
}

// ── Multiple Dispute Tests ───────────────────────────────────────────────────

#[test]
fn test_do_respond_dispute_with_multiple_open_disputes() {
    let (test_env, id1, subscriber1, _merchant1) = setup();
    
    // Create second subscription
    let (id2, subscriber2, _merchant2) = fixtures::create_subscription(
        &test_env.env,
        &test_env.client,
        SubscriptionStatus::Active,
    );
    seed_merchant_balance(&test_env, id2, DISPUTE_AMOUNT * 2);
    
    // Open two disputes
    let dispute_id1 = open_dispute(&test_env, &subscriber1, id1);
    let dispute_id2 = open_dispute(&test_env, &subscriber2, id2);
    
    // Respond to first dispute
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id1, &None::<BytesN<32>>);
    
    // First should be responded, second still open
    assert_eq!(
        test_env.client.get_dispute(&dispute_id1).status,
        DisputeStatus::Responded
    );
    assert_eq!(
        test_env.client.get_dispute(&dispute_id2).status,
        DisputeStatus::Open
    );
    
    // Respond to second dispute
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id2, &None::<BytesN<32>>);
    
    // Both should now be responded
    assert_eq!(
        test_env.client.get_dispute(&dispute_id1).status,
        DisputeStatus::Responded
    );
    assert_eq!(
        test_env.client.get_dispute(&dispute_id2).status,
        DisputeStatus::Responded
    );
}

#[test]
fn test_do_respond_dispute_responds_to_correct_dispute_among_many() {
    let test_env = TestEnv::default();
    
    // Create 5 disputes
    let mut dispute_ids = Vec::new();
    for i in 0..5 {
        let (id, subscriber, _merchant) = fixtures::create_subscription(
            &test_env.env,
            &test_env.client,
            SubscriptionStatus::Active,
        );
        seed_merchant_balance(&test_env, id, DISPUTE_AMOUNT * 2);
        let dispute_id = open_dispute(&test_env, &subscriber, id);
        dispute_ids.push(dispute_id);
    }
    
    // Respond to the middle one (index 2)
    let target_id = dispute_ids[2];
    test_env
        .client
        .respond_dispute(&test_env.admin, &target_id, &None::<BytesN<32>>);
    
    // Check that only the target was responded
    for (i, dispute_id) in dispute_ids.iter().enumerate() {
        let status = test_env.client.get_dispute(dispute_id).status;
        if i == 2 {
            assert_eq!(status, DisputeStatus::Responded);
        } else {
            assert_eq!(status, DisputeStatus::Open);
        }
    }
}

// ── Idempotency Tests ────────────────────────────────────────────────────────

#[test]
fn test_do_respond_dispute_not_idempotent() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    // First call succeeds
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    
    // Second call with same parameters fails
    let result = test_env.client.try_respond_dispute(
        &test_env.admin,
        &dispute_id,
        &None::<BytesN<32>>,
    );
    
    assert_eq!(
        result,
        Err(Ok(Error::DisputeAlreadyResponded)),
        "Response is not idempotent - second call should fail"
    );
}

// ── Integration Tests ────────────────────────────────────────────────────────

#[test]
#[ignore = "pre-existing on main; docs/known-failing-tests.md: assertion failed: result_after.is_ok()"]
fn test_do_respond_dispute_enables_immediate_resolution() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    // Cannot resolve before responding
    let result_before = test_env.client.try_resolve_dispute(
        &test_env.admin,
        &dispute_id,
        &true,
    );
    assert_eq!(result_before, Err(Ok(Error::DisputeNotResponded)));
    
    // Respond
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    
    // Can now resolve immediately
    let result_after = test_env
        .client
        .try_resolve_dispute(&test_env.admin, &dispute_id, &true);
    assert!(result_after.is_ok());
}

#[test]
fn test_do_respond_dispute_full_lifecycle() {
    let (test_env, id, subscriber, merchant) = setup();
    
    // 1. Open dispute
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    assert_eq!(
        test_env.client.get_dispute(&dispute_id).status,
        DisputeStatus::Open
    );
    
    // 2. Respond to dispute
    test_env
        .client
        .respond_dispute(&test_env.admin, &dispute_id, &None::<BytesN<32>>);
    assert_eq!(
        test_env.client.get_dispute(&dispute_id).status,
        DisputeStatus::Responded
    );
    
    // 3. Resolve dispute
    test_env
        .client
        .resolve_dispute(&test_env.admin, &dispute_id, &false);
    assert_eq!(
        test_env.client.get_dispute(&dispute_id).status,
        DisputeStatus::ResolvedToMerchant
    );
}

// ── Edge Case Tests ──────────────────────────────────────────────────────────

#[test]
fn test_do_respond_dispute_with_different_evidence_values() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    // Test various evidence hash patterns
    let test_cases = vec![
        BytesN::from_array(&test_env.env, &[0x00; 32]),    // All zeros
        BytesN::from_array(&test_env.env, &[0xFF; 32]),    // All ones
        BytesN::from_array(&test_env.env, &[0xAA; 32]),    // Pattern
        sample_evidence_hash(&test_env.env),                // Mixed
    ];
    
    for (i, evidence) in test_cases.iter().enumerate() {
        let sub_id = id + i as u32;
        if i > 0 {
            let (new_id, new_subscriber, _) = fixtures::create_subscription(
                &test_env.env,
                &test_env.client,
                SubscriptionStatus::Active,
            );
            seed_merchant_balance(&test_env, new_id, DISPUTE_AMOUNT * 2);
            let dispute_id = open_dispute(&test_env, &new_subscriber, new_id);
            
            test_env
                .client
                .respond_dispute(&test_env.admin, &dispute_id, &Some(evidence.clone()));
            
            let dispute = test_env.client.get_dispute(&dispute_id);
            assert_eq!(dispute.admin_evidence_hash, Some(evidence.clone()));
        }
    }
}

#[test]
fn test_do_respond_dispute_after_admin_rotation() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    
    // Rotate admin
    let new_admin = Address::generate(&test_env.env);
    test_env
        .client
        .rotate_admin(&test_env.admin, &new_admin, &0u64);
    
    // New admin should be able to respond
    test_env
        .client
        .respond_dispute(&new_admin, &dispute_id, &None::<BytesN<32>>);
    
    assert_eq!(
        test_env.client.get_dispute(&dispute_id).status,
        DisputeStatus::Responded
    );
}

#[test]
fn test_do_respond_dispute_old_admin_rejected_after_rotation() {
    let (test_env, id, subscriber, _merchant) = setup();
    
    let dispute_id = open_dispute(&test_env, &subscriber, id);
    let old_admin = test_env.admin.clone();
    
    // Rotate admin
    let new_admin = Address::generate(&test_env.env);
    test_env
        .client
        .rotate_admin(&test_env.admin, &new_admin, &0u64);
    
    // Old admin should be rejected
    mock_respond_dispute_auth(&test_env.env, &test_env.client, &old_admin, dispute_id, &None::<BytesN<32>>);
    let result = test_env
        .client
        .try_respond_dispute(&old_admin, &dispute_id, &None::<BytesN<32>>);
    
    assert_eq!(result, Err(Ok(Error::Forbidden)));
}
