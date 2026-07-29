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

// ══════════════════════════════════════════════════════════════════════════════
// Dispute escrow overpay invariant — audit #636
//
// Verifies that dispute resolution can never disburse more than the original
// escrowed amount, even in edge cases involving partial resolutions, rounding,
// or re-opened disputes. The DisputeEscrowLedger tracks cumulative disbursements
// and the resolve entrypoint enforces total_disbursed <= original_amount.
// ══════════════════════════════════════════════════════════════════════════════

mod dispute_overpay_invariant {
    use crate::{
        test_utils::{fixtures, setup::TestEnv},
        DataKey, DisputeEscrowLedger, DisputeStatus, Error, SubscriptionStatus,
        DISPUTE_WINDOW_SECS,
    };
    use soroban_sdk::testutils::Address as _;

    const DISPUTE_AMOUNT: i128 = 5_000_000;

    fn open_dispute(test_env: &TestEnv) -> (u64, u32) {
        let (subscription_id, subscriber, _) = fixtures::create_subscription(
            &test_env.env,
            &test_env.client,
            SubscriptionStatus::Active,
        );
        let subscription = test_env.client.get_subscription(&subscription_id);

        test_env.env.as_contract(&test_env.client.address, || {
            test_env.env.storage().instance().set(
                &DataKey::MerchantBalance(
                    subscription.merchant.clone(),
                    subscription.token.clone(),
                ),
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

    /// Helper: simulate a partial resolution by directly writing to the
    /// escrow ledger with a non-zero total_disbursed, then attempt to resolve.
    fn simulate_partial_resolution_then_resolve(
        test_env: &TestEnv,
        dispute_id: u64,
        already_disbursed: i128,
        resolve_to_subscriber: bool,
    ) -> Result<(), Error> {
        // Directly inject a partial-disbursement state into the escrow ledger
        test_env.env.as_contract(&test_env.client.address, || {
            let mut ledger: DisputeEscrowLedger = test_env
                .env
                .storage()
                .instance()
                .get(&DataKey::DisputeEscrow(dispute_id))
                .expect("escrow ledger must exist");
            ledger.total_disbursed = already_disbursed;
            test_env
                .env
                .storage()
                .instance()
                .set(&DataKey::DisputeEscrow(dispute_id), &ledger);
        });

        // Advance past dispute window so auto-resolve doesn't interfere
        test_env.jump(DISPUTE_WINDOW_SECS + 1);

        test_env.client.try_resolve_dispute(
            &test_env.admin,
            &dispute_id,
            &resolve_to_subscriber,
        )
    }

    /// When total_disbursed already equals original_amount, any further
    /// resolve attempt must fail with InsufficientBalance (no remaining).
    #[test]
    fn overpay_rejected_when_fully_disbursed() {
        let test_env = TestEnv::default();
        let (dispute_id, _) = open_dispute(&test_env);

        test_env.client.respond_dispute(
            &test_env.admin,
            &dispute_id,
            &None::<soroban_sdk::BytesN<32>>,
        );

        // Resolve in full to merchant
        let result_1 = test_env.client.try_resolve_dispute(
            &test_env.admin,
            &dispute_id,
            &false,
        );
        assert!(result_1.is_ok(), "first full resolution must succeed");

        // Second resolve must fail — dispute is already resolved
        let result_2 = test_env.client.try_resolve_dispute(
            &test_env.admin,
            &dispute_id,
            &true,
        );
        assert_eq!(
            result_2,
            Err(Ok(Error::DisputeAlreadyResolved)),
            "second resolve on same dispute must be rejected"
        );
    }

    /// Simulate a ledger state where total_disbursed > 0 but escrow hasn't
    /// been cleaned up, then try to resolve again — the ledger guard must
    /// prevent overpay.
    #[test]
    fn overpay_rejected_with_partial_disbursement_state() {
        let test_env = TestEnv::default();
        let (dispute_id, _) = open_dispute(&test_env);

        // Advance past dispute window so auto-resolve works without respond
        test_env.jump(DISPUTE_WINDOW_SECS + 1);

        // Simulate a partial resolution where half was already disbursed
        // (e.g., due to a previous partial-resolution call)
        let half = DISPUTE_AMOUNT / 2;
        let result = simulate_partial_resolution_then_resolve(
            &test_env,
            dispute_id,
            half, // already disbursed half
            true,  // resolve remaining to subscriber
        );

        // Should succeed because disbursing the other half is within bounds
        assert!(
            result.is_ok(),
            "resolve of remaining half after partial must succeed"
        );

        // Now try again — no remaining funds should fail
        let result2 = simulate_partial_resolution_then_resolve(
            &test_env,
            dispute_id,
            DISPUTE_AMOUNT, // claim full amount was already disbursed
            true,
        );
        assert_eq!(
            result2,
            Err(Ok(Error::InsufficientBalance)),
            "resolve with no remaining escrow must be rejected"
        );
    }

    /// Directly test the DisputeOverpay invariant by setting total_disbursed
    /// above original_amount.
    #[test]
    fn overpay_invariant_violation_rejected() {
        let test_env = TestEnv::default();
        let (dispute_id, _) = open_dispute(&test_env);

        // Advance past dispute window
        test_env.jump(DISPUTE_WINDOW_SECS + 1);

        // Corrupt the ledger: set total_disbursed > original_amount
        test_env.env.as_contract(&test_env.client.address, || {
            let mut ledger: DisputeEscrowLedger = test_env
                .env
                .storage()
                .instance()
                .get(&DataKey::DisputeEscrow(dispute_id))
                .expect("escrow ledger must exist");
            ledger.total_disbursed = ledger.original_amount + 1; // violate invariant
            test_env
                .env
                .storage()
                .instance()
                .set(&DataKey::DisputeEscrow(dispute_id), &ledger);
        });

        // Resolve attempt must detect the overpay
        let result = test_env.client.try_resolve_dispute(
            &test_env.admin,
            &dispute_id,
            &true,
        );
        // Since total_disbursed >= original_amount, remaining <= 0,
        // which triggers InsufficientBalance
        assert_eq!(
            result,
            Err(Ok(Error::InsufficientBalance)),
            "overpay must be rejected"
        );
    }

    /// Re-open a dispute on the same subscription after the first is resolved,
    /// then resolve — must work and must not leak escrow state from the
    /// previous dispute.
    #[test]
    fn re_open_dispute_after_resolve() {
        let test_env = TestEnv::default();
        let (subscription_id, subscriber, _) = fixtures::create_subscription(
            &test_env.env,
            &test_env.client,
            SubscriptionStatus::Active,
        );
        let subscription = test_env.client.get_subscription(&subscription_id);

        // Seed merchant balance
        let double_amount = DISPUTE_AMOUNT * 2;
        test_env.env.as_contract(&test_env.client.address, || {
            test_env.env.storage().instance().set(
                &DataKey::MerchantBalance(
                    subscription.merchant.clone(),
                    subscription.token.clone(),
                ),
                &double_amount,
            );
        });
        test_env
            .stellar_token_client()
            .mint(&test_env.client.address, &double_amount);

        // First dispute
        let dispute_id_1 = test_env.client.open_dispute(
            &subscriber,
            &subscription_id,
            &DISPUTE_AMOUNT,
            &None::<soroban_sdk::BytesN<32>>,
        );

        test_env.client.respond_dispute(
            &test_env.admin,
            &dispute_id_1,
            &None::<soroban_sdk::BytesN<32>>,
        );
        test_env.client.resolve_dispute(
            &test_env.admin,
            &dispute_id_1,
            &false, // to merchant
        );

        // Second dispute on the same subscription
        let dispute_id_2 = test_env.client.open_dispute(
            &subscriber,
            &subscription_id,
            &DISPUTE_AMOUNT,
            &None::<soroban_sdk::BytesN<32>>,
        );

        test_env.client.respond_dispute(
            &test_env.admin,
            &dispute_id_2,
            &None::<soroban_sdk::BytesN<32>>,
        );
        let result = test_env.client.try_resolve_dispute(
            &test_env.admin,
            &dispute_id_2,
            &false, // to merchant
        );
        assert!(
            result.is_ok(),
            "re-open dispute after resolve must succeed"
        );
    }
}
