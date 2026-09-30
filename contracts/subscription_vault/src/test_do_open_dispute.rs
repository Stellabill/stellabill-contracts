//! Adversarial coverage for `do_open_dispute` in `dispute.rs` (issue #1042).
//!
//! `do_open_dispute` is the subscriber-facing entry point that starts the
//! dispute / chargeback lifecycle. It is security- and accounting-sensitive:
//!
//! * requires subscriber authorisation AND that the caller *is* the
//!   subscription's registered subscriber;
//! * rejects non-positive amounts (`InvalidAmount`);
//! * rejects a second concurrent dispute for the same subscription
//!   (`DisputeAlreadyOpen`);
//! * refuses to escrow more than the merchant actually holds
//!   (`InsufficientBalance`);
//! * debits the merchant balance and credits a `DisputeEscrow` ledger;
//! * records a `Dispute` in `Open` status and an index entry;
//! * emits `DisputeOpenedEvent` only after all state is durable (CEI).
//!
//! The existing suite only exercised the happy path indirectly through
//! `open_dispute`. The cases below pin the check ordering, the exact error
//! codes, the arithmetic boundaries, and — critically — that *every rejected
//! call leaves merchant balance, escrow, dispute records, the subscription
//! index and the dispute-id counter untouched.

use crate::{
    test_utils::{fixtures, setup::TestEnv},
    types::{
        DataKey, Dispute, DisputeEscrowLedger, DisputeOpenedEvent, DisputeStatus, Error,
        EVENT_SCHEMA_VERSION,
    },
    SubscriptionStatus, DISPUTE_WINDOW_SECS,
};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    token::Client as TokenClient,
    Address, BytesN, FromVal, IntoVal, Symbol,
};

// ── Constants ────────────────────────────────────────────────────────────────

const AMOUNT: i128 = 5_000_000; // 5 USDC (6 decimals)

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Create an active subscription and seed `balance` of unbacked merchant
/// balance directly into instance storage. No real tokens are moved at
/// `open_dispute` time, so this is sufficient for the open paths.
fn setup_with_balance(balance: i128) -> (TestEnv, u32, Address, Address) {
    let te = TestEnv::default();
    let (sub_id, subscriber, merchant) =
        fixtures::create_subscription(&te.env, &te.client, SubscriptionStatus::Active);
    seed_merchant_balance(&te, &merchant, balance);
    (te, sub_id, subscriber, merchant)
}

/// Seed the merchant's balance for the vault's settlement token.
fn seed_merchant_balance(te: &TestEnv, merchant: &Address, amount: i128) {
    te.env.as_contract(&te.client.address, || {
        te.env.storage().instance().set(
            &DataKey::MerchantBalance(merchant.clone(), te.token.clone()),
            &amount,
        );
    });
}

/// Read the merchant's current vault balance for the settlement token.
fn merchant_balance(te: &TestEnv, merchant: &Address) -> i128 {
    te.client.get_merchant_balance_by_token(merchant, &te.token)
}

/// Read the escrow ledger for `dispute_id`, if present.
fn escrow(te: &TestEnv, dispute_id: u64) -> Option<DisputeEscrowLedger> {
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .instance()
            .get(&DataKey::DisputeEscrow(dispute_id))
    })
}

/// Read the raw `Dispute` record for `dispute_id`, if present.
fn dispute_record(te: &TestEnv, dispute_id: u64) -> Option<Dispute> {
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .persistent()
            .get(&DataKey::Dispute(dispute_id))
    })
}

/// Monotonic dispute-id counter (0 when never written).
fn next_dispute_id(te: &TestEnv) -> u64 {
    te.env.as_contract(&te.client.address, || {
        te.env
            .storage()
            .instance()
            .get::<_, u64>(&DataKey::NextDisputeId)
            .unwrap_or(0)
    })
}

/// Assert that no dispute side effects exist for a freshly-opened id.
fn assert_no_dispute_side_effects(te: &TestEnv, sub_id: u32) {
    assert_eq!(escrow(te, 0), None, "no escrow may be created");
    assert!(
        dispute_record(te, 0).is_none(),
        "no dispute record may exist"
    );
    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        None,
        "subscription index must stay empty"
    );
}

// ── Happy paths ──────────────────────────────────────────────────────────────

/// The first dispute ever opened gets id 0 and is returned verbatim.
#[test]
fn first_dispute_gets_id_zero() {
    let (te, sub_id, subscriber, _merchant) = setup_with_balance(AMOUNT * 2);

    let dispute_id = te
        .client
        .open_dispute(&subscriber, &sub_id, &AMOUNT, &None::<BytesN<32>>);

    assert_eq!(dispute_id, 0, "first dispute id must be 0");
    assert_eq!(te.client.get_subscription_dispute(&sub_id), Some(0));
}

/// Opening a dispute debits the merchant balance by exactly `amount`.
#[test]
fn open_dispute_debits_merchant_balance_exactly() {
    let (te, sub_id, subscriber, merchant) = setup_with_balance(AMOUNT * 3);

    te.client
        .open_dispute(&subscriber, &sub_id, &AMOUNT, &None::<BytesN<32>>);

    assert_eq!(
        merchant_balance(&te, &merchant),
        AMOUNT * 2,
        "merchant balance must drop by exactly the disputed amount"
    );
}

/// The escrow ledger records the full amount and zero prior disbursements.
#[test]
fn open_dispute_creates_escrow_ledger() {
    let (te, sub_id, subscriber, _merchant) = setup_with_balance(AMOUNT * 2);

    let dispute_id = te
        .client
        .open_dispute(&subscriber, &sub_id, &AMOUNT, &None::<BytesN<32>>);

    assert_eq!(
        escrow(&te, dispute_id),
        Some(DisputeEscrowLedger {
            original_amount: AMOUNT,
            total_disbursed: 0,
        }),
        "escrow must be created with the full amount and zero disbursed"
    );
}

/// Every field of the stored dispute is populated correctly.
#[test]
fn open_dispute_records_all_dispute_fields() {
    let (te, sub_id, subscriber, merchant) = setup_with_balance(AMOUNT * 2);
    te.env.ledger().set_timestamp(12_345);

    let dispute_id = te
        .client
        .open_dispute(&subscriber, &sub_id, &AMOUNT, &None::<BytesN<32>>);

    let d = te.client.get_dispute(&dispute_id);
    assert_eq!(d.id, dispute_id);
    assert_eq!(d.subscription_id, sub_id);
    assert_eq!(d.subscriber, subscriber);
    assert_eq!(d.merchant, merchant);
    assert_eq!(d.amount, AMOUNT);
    assert_eq!(d.opened_at, 12_345);
    assert_eq!(d.status, DisputeStatus::Open);
    assert_eq!(d.evidence_hash, None);
    assert_eq!(d.responded_at, None, "a fresh dispute is not responded");
    assert_eq!(d.admin_evidence_hash, None);
}

/// An evidence hash supplied at open time is persisted verbatim.
#[test]
fn open_dispute_persists_evidence_hash() {
    let (te, sub_id, subscriber, _merchant) = setup_with_balance(AMOUNT * 2);
    let evidence = BytesN::from_array(&te.env, &[0xAB; 32]);

    let dispute_id = te
        .client
        .open_dispute(&subscriber, &sub_id, &AMOUNT, &Some(evidence.clone()));

    assert_eq!(
        te.client.get_dispute(&dispute_id).evidence_hash,
        Some(evidence)
    );
}

/// The subscription -> dispute index is set to the new dispute id.
#[test]
fn open_dispute_sets_subscription_index() {
    let (te, sub_id, subscriber, _merchant) = setup_with_balance(AMOUNT * 2);

    let dispute_id = te
        .client
        .open_dispute(&subscriber, &sub_id, &AMOUNT, &None::<BytesN<32>>);

    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        Some(dispute_id)
    );
}

/// `DisputeOpenedEvent` carries every field required by indexers, and the
/// timestamp matches the ledger time at which state was written.
#[test]
fn open_dispute_emits_dispute_opened_event() {
    let (te, sub_id, subscriber, merchant) = setup_with_balance(AMOUNT * 2);
    te.env.ledger().set_timestamp(77_777);
    let evidence = BytesN::from_array(&te.env, &[0x11; 32]);

    let dispute_id = te
        .client
        .open_dispute(&subscriber, &sub_id, &AMOUNT, &Some(evidence.clone()));

    let events = te.env.events().all();
    let event = events
        .iter()
        .rfind(|e| {
            Symbol::from_val(&te.env, &e.1.get(0).unwrap())
                == Symbol::new(&te.env, "dispute_opened")
        })
        .expect("dispute_opened event not found");

    let data: DisputeOpenedEvent = event.2.clone().into_val(&te.env);
    assert_eq!(data.dispute_id, dispute_id);
    assert_eq!(data.subscription_id, sub_id);
    assert_eq!(data.subscriber, subscriber);
    assert_eq!(data.merchant, merchant);
    assert_eq!(data.amount, AMOUNT);
    assert_eq!(data.evidence_hash, Some(evidence));
    assert_eq!(data.timestamp, 77_777);
    assert_eq!(data.schema_version, EVENT_SCHEMA_VERSION);
}

/// Consecutive disputes on different subscriptions receive distinct,
/// monotonically increasing ids.
#[test]
fn open_dispute_allocates_monotonic_ids() {
    let te = TestEnv::default();

    let (sub_a, sub_a_owner, _) =
        fixtures::create_subscription(&te.env, &te.client, SubscriptionStatus::Active);
    let (sub_b, sub_b_owner, _) =
        fixtures::create_subscription(&te.env, &te.client, SubscriptionStatus::Active);
    seed_merchant_balance_for(&te, &sub_a);
    seed_merchant_balance_for(&te, &sub_b);

    let id_a = te
        .client
        .open_dispute(&sub_a_owner, &sub_a, &AMOUNT, &None::<BytesN<32>>);
    let id_b = te
        .client
        .open_dispute(&sub_b_owner, &sub_b, &AMOUNT, &None::<BytesN<32>>);

    assert_eq!(id_a, 0);
    assert_eq!(id_b, 1, "second dispute must get the next id");
    assert_eq!(next_dispute_id(&te), 2, "counter advances once per open");
}

// Seed the merchant balance of an existing subscription by id.
fn seed_merchant_balance_for(te: &TestEnv, sub_id: &u32) {
    let sub = te.client.get_subscription(sub_id);
    seed_merchant_balance(te, &sub.merchant, AMOUNT * 2);
}

// ── Arithmetic / boundary values ─────────────────────────────────────────────

/// Zero amount is rejected before any state is touched.
#[test]
fn open_dispute_rejects_zero_amount() {
    let (te, sub_id, subscriber, merchant) = setup_with_balance(AMOUNT * 2);

    let result = te
        .client
        .try_open_dispute(&subscriber, &sub_id, &0i128, &None::<BytesN<32>>);

    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
    assert_eq!(merchant_balance(&te, &merchant), AMOUNT * 2);
    assert_eq!(next_dispute_id(&te), 0);
    assert_no_dispute_side_effects(&te, sub_id);
}

/// A negative amount is rejected.
#[test]
fn open_dispute_rejects_negative_amount() {
    let (te, sub_id, subscriber, merchant) = setup_with_balance(AMOUNT * 2);

    let result = te
        .client
        .try_open_dispute(&subscriber, &sub_id, &-1i128, &None::<BytesN<32>>);

    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
    assert_eq!(merchant_balance(&te, &merchant), AMOUNT * 2);
    assert_no_dispute_side_effects(&te, sub_id);
}

/// `i128::MIN` is non-positive and must not overflow `amount <= 0`.
#[test]
fn open_dispute_rejects_i128_min_amount() {
    let (te, sub_id, subscriber, _merchant) = setup_with_balance(i128::MAX);

    let result = te
        .client
        .try_open_dispute(&subscriber, &sub_id, &i128::MIN, &None::<BytesN<32>>);

    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
    assert_no_dispute_side_effects(&te, sub_id);
}

/// A dispute for exactly the merchant's whole balance succeeds and drains it.
#[test]
fn open_dispute_succeeds_when_balance_equals_amount() {
    let (te, sub_id, subscriber, merchant) = setup_with_balance(AMOUNT);

    let dispute_id = te
        .client
        .open_dispute(&subscriber, &sub_id, &AMOUNT, &None::<BytesN<32>>);

    assert_eq!(merchant_balance(&te, &merchant), 0);
    assert_eq!(escrow(&te, dispute_id).unwrap().original_amount, AMOUNT);
}

/// One unit more than the merchant holds is rejected.
#[test]
fn open_dispute_rejects_when_balance_is_one_less_than_amount() {
    let (te, sub_id, subscriber, merchant) = setup_with_balance(AMOUNT - 1);

    let result = te
        .client
        .try_open_dispute(&subscriber, &sub_id, &AMOUNT, &None::<BytesN<32>>);

    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));
    assert_eq!(merchant_balance(&te, &merchant), AMOUNT - 1);
    assert_no_dispute_side_effects(&te, sub_id);
}

/// A merchant with no balance cannot escrow anything.
#[test]
fn open_dispute_rejects_when_merchant_balance_is_zero() {
    let (te, sub_id, subscriber, _merchant) = setup_with_balance(0);

    let result = te
        .client
        .try_open_dispute(&subscriber, &sub_id, &1i128, &None::<BytesN<32>>);

    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));
    assert_no_dispute_side_effects(&te, sub_id);
}

/// `i128::MAX` against a small balance is an insufficient-balance error, not
/// an arithmetic overflow.
#[test]
fn open_dispute_rejects_i128_max_when_balance_insufficient() {
    let (te, sub_id, subscriber, _merchant) = setup_with_balance(AMOUNT);

    let result = te
        .client
        .try_open_dispute(&subscriber, &sub_id, &i128::MAX, &None::<BytesN<32>>);

    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));
    assert_no_dispute_side_effects(&te, sub_id);
}

/// A partial dispute leaves the exact remainder in the merchant balance.
#[test]
fn open_dispute_partially_drains_merchant_balance() {
    let (te, sub_id, subscriber, merchant) = setup_with_balance(10 * AMOUNT);

    te.client
        .open_dispute(&subscriber, &sub_id, &(3 * AMOUNT), &None::<BytesN<32>>);

    assert_eq!(merchant_balance(&te, &merchant), 7 * AMOUNT);
}

// ── Authorisation / identity ─────────────────────────────────────────────────

/// A non-existent subscription is rejected with `NotFound` (the amount is
/// valid, so the amount check does not mask this).
#[test]
fn open_dispute_rejects_nonexistent_subscription() {
    let te = TestEnv::default();
    let stranger = Address::generate(&te.env);

    let result = te
        .client
        .try_open_dispute(&stranger, &9_999u32, &AMOUNT, &None::<BytesN<32>>);

    assert_eq!(result, Err(Ok(Error::NotFound)));
}

/// A valid subscription but a different caller address is rejected as
/// unauthorised, with no state change.
#[test]
fn open_dispute_rejects_subscriber_mismatch() {
    let (te, sub_id, _subscriber, merchant) = setup_with_balance(AMOUNT * 2);
    let attacker = Address::generate(&te.env);

    let result = te
        .client
        .try_open_dispute(&attacker, &sub_id, &AMOUNT, &None::<BytesN<32>>);

    assert_eq!(result, Err(Ok(Error::Unauthorized)));
    assert_eq!(merchant_balance(&te, &merchant), AMOUNT * 2);
    assert_no_dispute_side_effects(&te, sub_id);
}

/// The merchant of the subscription is not allowed to open the dispute.
#[test]
fn open_dispute_rejects_merchant_caller() {
    let (te, sub_id, _subscriber, merchant) = setup_with_balance(AMOUNT * 2);

    let result = te
        .client
        .try_open_dispute(&merchant, &sub_id, &AMOUNT, &None::<BytesN<32>>);

    assert_eq!(result, Err(Ok(Error::Unauthorized)));
    assert_no_dispute_side_effects(&te, sub_id);
}

/// An unrelated account is rejected.
#[test]
fn open_dispute_rejects_unrelated_stranger() {
    let (te, sub_id, _subscriber, _merchant) = setup_with_balance(AMOUNT * 2);
    let stranger = Address::generate(&te.env);

    let result = te
        .client
        .try_open_dispute(&stranger, &sub_id, &AMOUNT, &None::<BytesN<32>>);

    assert_eq!(result, Err(Ok(Error::Unauthorized)));
    assert_no_dispute_side_effects(&te, sub_id);
}

/// Without any authorisation the subscriber's own call must fail at the
/// `require_auth` gate before any storage is written.
#[test]
fn open_dispute_requires_subscriber_auth() {
    let (te, sub_id, subscriber, merchant) = setup_with_balance(AMOUNT * 2);

    te.env.set_auths(&[]);
    let result = te
        .client
        .try_open_dispute(&subscriber, &sub_id, &AMOUNT, &None::<BytesN<32>>);

    assert!(result.is_err(), "unauthenticated call must be rejected");
    assert_eq!(merchant_balance(&te, &merchant), AMOUNT * 2);
    assert_no_dispute_side_effects(&te, sub_id);
}

// ── Double-open protection ───────────────────────────────────────────────────

/// A second dispute for the same subscription is rejected while the first is
/// still open, and none of the first dispute's state is disturbed.
#[test]
fn open_dispute_rejects_second_dispute_while_open() {
    let (te, sub_id, subscriber, merchant) = setup_with_balance(AMOUNT * 4);

    let first = te
        .client
        .open_dispute(&subscriber, &sub_id, &AMOUNT, &None::<BytesN<32>>);
    let balance_after_first = merchant_balance(&te, &merchant);
    let id_counter_after_first = next_dispute_id(&te);

    let result = te
        .client
        .try_open_dispute(&subscriber, &sub_id, &AMOUNT, &None::<BytesN<32>>);

    assert_eq!(result, Err(Ok(Error::DisputeAlreadyOpen)));
    assert_eq!(
        merchant_balance(&te, &merchant),
        balance_after_first,
        "rejected double-open must not debit again"
    );
    assert_eq!(
        next_dispute_id(&te),
        id_counter_after_first,
        "rejected double-open must not consume a dispute id"
    );
    assert_eq!(
        te.client.get_subscription_dispute(&sub_id),
        Some(first),
        "index must still point at the first dispute"
    );
    assert_eq!(
        escrow(&te, first).unwrap().original_amount,
        AMOUNT,
        "first escrow must be untouched"
    );
    assert_eq!(te.client.get_dispute(&first).status, DisputeStatus::Open);
}

/// After a dispute resolves and the merchant balance is restored, a new
/// dispute on the same subscription is allowed and gets a fresh id.
#[test]
fn open_dispute_allowed_again_after_resolve_to_merchant() {
    let (te, sub_id, subscriber, merchant) = setup_with_balance(AMOUNT * 2);

    let first = te
        .client
        .open_dispute(&subscriber, &sub_id, &AMOUNT, &None::<BytesN<32>>);
    te.client
        .respond_dispute(&te.admin, &first, &None::<BytesN<32>>);
    te.client.resolve_dispute(&te.admin, &first, &false);

    // Funds were returned to the merchant, so the balance is spendable again.
    assert_eq!(merchant_balance(&te, &merchant), AMOUNT * 2);

    let second = te
        .client
        .open_dispute(&subscriber, &sub_id, &AMOUNT, &None::<BytesN<32>>);

    assert_ne!(first, second, "re-open must allocate a new dispute id");
    assert_eq!(te.client.get_subscription_dispute(&sub_id), Some(second));
    assert_eq!(
        te.client.get_dispute(&first).status,
        DisputeStatus::ResolvedToMerchant,
        "the resolved dispute must not be rewritten"
    );
}

/// Auto-resolution (window elapsed while still open) also frees the
/// subscription for a new dispute.
#[test]
fn open_dispute_allowed_again_after_auto_resolve() {
    let (te, sub_id, subscriber, _merchant) = setup_with_balance(AMOUNT * 2);
    // Auto-resolution pays the subscriber from custody; fund the vault so the
    // transfer cannot fail.
    te.stellar_token_client().mint(&te.client.address, &AMOUNT);

    let first = te
        .client
        .open_dispute(&subscriber, &sub_id, &AMOUNT, &None::<BytesN<32>>);
    te.jump(DISPUTE_WINDOW_SECS + 1);
    te.client.resolve_dispute(&te.admin, &first, &false);

    assert_eq!(
        te.client.get_dispute(&first).status,
        DisputeStatus::ResolvedToSubscriber,
        "unresponded dispute auto-resolves to the subscriber"
    );

    // Top up the merchant balance that the first escrow consumed.
    let sub = te.client.get_subscription(&sub_id);
    seed_merchant_balance(&te, &sub.merchant, AMOUNT);

    let second = te
        .client
        .open_dispute(&subscriber, &sub_id, &AMOUNT, &None::<BytesN<32>>);

    assert_ne!(first, second);
    assert_eq!(te.client.get_subscription_dispute(&sub_id), Some(second));
}

// ── State isolation ──────────────────────────────────────────────────────────

/// A successful open on one subscription must not leak into another.
#[test]
fn open_dispute_is_isolated_between_subscriptions() {
    let te = TestEnv::default();

    let (sub_a, owner_a, _merchant_a) =
        fixtures::create_subscription(&te.env, &te.client, SubscriptionStatus::Active);
    let (sub_b, owner_b, _merchant_b) =
        fixtures::create_subscription(&te.env, &te.client, SubscriptionStatus::Active);
    seed_merchant_balance_for(&te, &sub_a);
    seed_merchant_balance_for(&te, &sub_b);

    let dispute_a = te
        .client
        .open_dispute(&owner_a, &sub_a, &AMOUNT, &None::<BytesN<32>>);

    assert_eq!(te.client.get_subscription_dispute(&sub_a), Some(dispute_a));
    assert_eq!(
        te.client.get_subscription_dispute(&sub_b),
        None,
        "dispute on A must not touch B"
    );
    assert_eq!(
        dispute_record(&te, dispute_a).unwrap().subscription_id,
        sub_a
    );
    assert!(dispute_record(&te, dispute_a + 1).is_none());

    // B can still open independently and gets its own id.
    let dispute_b = te
        .client
        .open_dispute(&owner_b, &sub_b, &AMOUNT, &None::<BytesN<32>>);
    assert_ne!(dispute_a, dispute_b);
    assert_eq!(
        dispute_record(&te, dispute_b).unwrap().subscription_id,
        sub_b
    );
}

/// Opening a dispute does not move any tokens: custody is only touched at
/// resolution time. This pins the "escrow is accounting-only at open" design.
#[test]
fn open_dispute_does_not_transfer_tokens() {
    let (te, sub_id, subscriber, _merchant) = setup_with_balance(AMOUNT);
    let token = TokenClient::new(&te.env, &te.token);
    let vault_before = token.balance(&te.client.address);

    te.client
        .open_dispute(&subscriber, &sub_id, &AMOUNT, &None::<BytesN<32>>);

    assert_eq!(
        token.balance(&te.client.address),
        vault_before,
        "open_dispute must not transfer tokens into or out of the vault"
    );
}
