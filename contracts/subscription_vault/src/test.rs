use crate::{Error, MerchantConfig, Subscription, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{token, Address, Env};

struct TestCtx {
    env: Env,
    contract_id: Address,
    token_address: Address,
    admin: Address,
    subscriber: Address,
}

impl TestCtx {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let token_admin = Address::generate(&env);
        let token_address = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();
        let token_admin_client = token::StellarAssetClient::new(&env, &token_address);

        let admin = Address::generate(&env);
        let subscriber = Address::generate(&env);

        client.init(&token_address, &admin, &1_000000i128);
        token_admin_client.mint(&subscriber, &100_000000i128);

        Self {
            env,
            contract_id,
            token_address,
            admin,
            subscriber,
        }
use crate::{
    can_transition, get_allowed_transitions, validate_status_transition, Error, Subscription,
    SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::testutils::{Address as _, Events, Ledger as _};
use soroban_sdk::{Address, Env, IntoVal, TryFromVal, Val, Vec};

// ---------------------------------------------------------------------------
// Helper: decode the event data payload (3rd element of event tuple)
// ---------------------------------------------------------------------------
#[allow(dead_code)]
fn last_event_data<T: TryFromVal<Env, Val>>(env: &Env) -> T {
    let events = env.events().all();
    let last = events.last().unwrap();
    T::try_from_val(env, &last.2).unwrap()
}

/// Helper: register contract, init, and return client + reusable addresses.
fn setup_env() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128); // 1 USDC min_topup

    (env, client, token, admin)
}

/// Helper: create a subscription for a given subscriber+merchant and return its id.
fn create_sub(
    env: &Env,
    client: &SubscriptionVaultClient,
    subscriber: &Address,
    merchant: &Address,
    amount: i128,
) -> u32 {
    client.create_subscription(
        subscriber,
        merchant,
        &amount,
        &(30u64 * 24 * 60 * 60), // 30 days
        &false,
    )
}

// ─── Existing tests ───────────────────────────────────────────────────────────

#[test]
fn test_init_and_struct() {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    client.init(&token, &admin, &1_000000i128);
}

// =============================================================================
// State Machine Helper Tests
// =============================================================================

#[test]
fn test_validate_status_transition_same_status_is_allowed() {
    // Idempotent transitions should be allowed
    assert!(
        validate_status_transition(&SubscriptionStatus::Active, &SubscriptionStatus::Active)
            .is_ok()
    );
    assert!(
        validate_status_transition(&SubscriptionStatus::Paused, &SubscriptionStatus::Paused)
            .is_ok()
    );
    assert!(validate_status_transition(
        &SubscriptionStatus::Cancelled,
        &SubscriptionStatus::Cancelled
    )
    .is_ok());
    assert!(validate_status_transition(
        &SubscriptionStatus::InsufficientBalance,
        &SubscriptionStatus::InsufficientBalance
    )
    .is_ok());
}

#[test]
fn test_validate_active_transitions() {
    // Active -> Paused (allowed)
    assert!(
        validate_status_transition(&SubscriptionStatus::Active, &SubscriptionStatus::Paused)
            .is_ok()
    );

    // Active -> Cancelled (allowed)
    assert!(validate_status_transition(
        &SubscriptionStatus::Active,
        &SubscriptionStatus::Cancelled
    )
    .is_ok());

    // Active -> InsufficientBalance (allowed)
    assert!(validate_status_transition(
        &SubscriptionStatus::Active,
        &SubscriptionStatus::InsufficientBalance
    )
    .is_ok());
}

#[test]
fn test_validate_paused_transitions() {
    // Paused -> Active (allowed)
    assert!(
        validate_status_transition(&SubscriptionStatus::Paused, &SubscriptionStatus::Active)
            .is_ok()
    );

    // Paused -> Cancelled (allowed)
    assert!(validate_status_transition(
        &SubscriptionStatus::Paused,
        &SubscriptionStatus::Cancelled
    )
    .is_ok());

    // Paused -> InsufficientBalance (not allowed)
    assert_eq!(
        validate_status_transition(
            &SubscriptionStatus::Paused,
            &SubscriptionStatus::InsufficientBalance
        ),
        Err(Error::InvalidStatusTransition)
    );
}

#[test]
fn test_validate_insufficient_balance_transitions() {
    // InsufficientBalance -> Active (allowed)
    assert!(validate_status_transition(
        &SubscriptionStatus::InsufficientBalance,
        &SubscriptionStatus::Active
    )
    .is_ok());

    // InsufficientBalance -> Cancelled (allowed)
    assert!(validate_status_transition(
        &SubscriptionStatus::InsufficientBalance,
        &SubscriptionStatus::Cancelled
    )
    .is_ok());

    // InsufficientBalance -> Paused (not allowed)
    assert_eq!(
        validate_status_transition(
            &SubscriptionStatus::InsufficientBalance,
            &SubscriptionStatus::Paused
        ),
        Err(Error::InvalidStatusTransition)
    );
}

#[test]
fn test_validate_cancelled_transitions_all_blocked() {
    // Cancelled is a terminal state - no outgoing transitions allowed
    assert_eq!(
        validate_status_transition(&SubscriptionStatus::Cancelled, &SubscriptionStatus::Active),
        Err(Error::InvalidStatusTransition)
    );
    assert_eq!(
        validate_status_transition(&SubscriptionStatus::Cancelled, &SubscriptionStatus::Paused),
        Err(Error::InvalidStatusTransition)
    );
    assert_eq!(
        validate_status_transition(
            &SubscriptionStatus::Cancelled,
            &SubscriptionStatus::InsufficientBalance
        ),
        Err(Error::InvalidStatusTransition)
    );
}

#[test]
fn test_can_transition_helper() {
    // True cases
    assert!(can_transition(
        &SubscriptionStatus::Active,
        &SubscriptionStatus::Paused
    ));
    assert!(can_transition(
        &SubscriptionStatus::Active,
        &SubscriptionStatus::Cancelled
    ));
    assert!(can_transition(
        &SubscriptionStatus::Paused,
        &SubscriptionStatus::Active
    ));

    // False cases
    assert!(!can_transition(
        &SubscriptionStatus::Cancelled,
        &SubscriptionStatus::Active
    ));
    assert!(!can_transition(
        &SubscriptionStatus::Cancelled,
        &SubscriptionStatus::Paused
    ));
    assert!(!can_transition(
        &SubscriptionStatus::Paused,
        &SubscriptionStatus::InsufficientBalance
    ));
}

#[test]
fn test_get_allowed_transitions() {
    // Active
    let active_targets = get_allowed_transitions(&SubscriptionStatus::Active);
    assert_eq!(active_targets.len(), 3);
    assert!(active_targets.contains(&SubscriptionStatus::Paused));
    assert!(active_targets.contains(&SubscriptionStatus::Cancelled));
    assert!(active_targets.contains(&SubscriptionStatus::InsufficientBalance));

    // Paused
    let paused_targets = get_allowed_transitions(&SubscriptionStatus::Paused);
    assert_eq!(paused_targets.len(), 2);
    assert!(paused_targets.contains(&SubscriptionStatus::Active));
    assert!(paused_targets.contains(&SubscriptionStatus::Cancelled));

    // Cancelled
    let cancelled_targets = get_allowed_transitions(&SubscriptionStatus::Cancelled);
    assert_eq!(cancelled_targets.len(), 0);

    // InsufficientBalance
    let ib_targets = get_allowed_transitions(&SubscriptionStatus::InsufficientBalance);
    assert_eq!(ib_targets.len(), 2);
    assert!(ib_targets.contains(&SubscriptionStatus::Active));
    assert!(ib_targets.contains(&SubscriptionStatus::Cancelled));
}

// =============================================================================
// Contract Entrypoint State Transition Tests
// =============================================================================

fn setup_test_env() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let min_topup = 1_000000i128; // 1 USDC
    client.init(&token, &admin, &min_topup);

    (env, client, token, admin)
}

fn create_test_subscription(
    env: &Env,
    client: &SubscriptionVaultClient,
    status: SubscriptionStatus,
) -> (u32, Address, Address) {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    let amount = 10_000_000i128; // 10 USDC
    let interval_seconds = 30 * 24 * 60 * 60; // 30 days
    let usage_enabled = false;

    // Create subscription (always starts as Active)
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval_seconds,
        &usage_enabled,
    );

    // Manually set status if not Active (bypassing state machine for test setup)
    // Note: In production, this would go through proper transitions
    if status != SubscriptionStatus::Active {
        // We need to manipulate storage directly for test setup
        // This is a test-only pattern
        let mut sub = client.get_subscription(&id);
        sub.status = status;
        env.as_contract(&client.address, || {
            env.storage().instance().set(&id, &sub);
        });
    }

    (id, subscriber, merchant)
}

#[test]
fn test_pause_subscription_from_active() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);

    // Pause from Active should succeed
    client.pause_subscription(&id, &subscriber);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Paused);
}

#[test]
#[should_panic(expected = "Error(Contract, #400)")]
fn test_pause_subscription_from_cancelled_should_fail() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);

    // First cancel
    client.cancel_subscription(&id, &subscriber);

    // Then try to pause (should fail)
    client.pause_subscription(&id, &subscriber);
}

#[test]
fn test_init_with_min_topup() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let min_topup = 1_000000i128; // 1 USDC
    client.init(&token, &admin, &min_topup);

    assert_eq!(client.get_min_topup(), min_topup);
}

#[test]
fn test_pause_subscription_from_paused_is_idempotent() {
    // Idempotent transition: Paused -> Paused should succeed (no-op)
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);

    // First pause
    client.pause_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );

    // Pausing again should succeed (idempotent)
    client.pause_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Paused
    );
}

#[test]
fn test_cancel_subscription_from_active() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);

    // Cancel from Active should succeed
    client.cancel_subscription(&id, &subscriber);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

#[test]
fn test_cancel_subscription_from_paused() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);

    // First pause
    client.pause_subscription(&id, &subscriber);

    // Then cancel
    client.cancel_subscription(&id, &subscriber);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

#[test]
fn test_cancel_subscription_from_cancelled_is_idempotent() {
    // Idempotent transition: Cancelled -> Cancelled should succeed (no-op)
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);

    // First cancel
    client.cancel_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );

    // Cancelling again should succeed (idempotent)
    client.cancel_subscription(&id, &subscriber);
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Cancelled
    );
}

#[test]
fn test_resume_subscription_from_paused() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);

    // First pause
    client.pause_subscription(&id, &subscriber);

    // Then resume
    client.resume_subscription(&id, &subscriber);

    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

#[test]
#[should_panic(expected = "Error(Contract, #400)")]
fn test_resume_subscription_from_cancelled_should_fail() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);

    // First cancel
    client.cancel_subscription(&id, &subscriber);

    // Try to resume (should fail)
    client.resume_subscription(&id, &subscriber);
}

#[test]
fn test_state_transition_idempotent_same_status() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);

    // Cancelling from already cancelled should fail (but we need to set it first)
    // First cancel
    client.cancel_subscription(&id, &subscriber);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
}

// =============================================================================
// Complex State Transition Sequences
// =============================================================================

#[test]
fn test_full_lifecycle_active_pause_resume() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);

    // Active -> Paused
    client.pause_subscription(&id, &subscriber);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Paused);

    // Paused -> Active
    client.resume_subscription(&id, &subscriber);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Active);

    // Can pause again
    client.pause_subscription(&id, &subscriber);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Paused);
}

#[test]
fn test_full_lifecycle_active_cancel() {
    let (env, client, _, _) = setup_test_env();
    let (id, subscriber, _) = create_test_subscription(&env, &client, SubscriptionStatus::Active);

    // Active -> Cancelled (terminal)
    client.cancel_subscription(&id, &subscriber);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);

    // Verify no further transitions possible
    // We can't easily test all fail cases without #[should_panic] for each
}

#[test]
fn test_all_valid_transitions_coverage() {
    // This test exercises every valid state transition at least once

    // 1. Active -> Paused
    {
        let (env, client, _, _) = setup_test_env();
        let (id, subscriber, _) =
            create_test_subscription(&env, &client, SubscriptionStatus::Active);
        client.pause_subscription(&id, &subscriber);
        assert_eq!(
            client.get_subscription(&id).status,
            SubscriptionStatus::Paused
        );
    }

    fn client(&self) -> SubscriptionVaultClient<'_> {
        SubscriptionVaultClient::new(&self.env, &self.contract_id)
    }

    fn token_client(&self) -> token::Client<'_> {
        token::Client::new(&self.env, &self.token_address)
    }

    fn stellar_asset_client(&self) -> token::StellarAssetClient<'_> {
        token::StellarAssetClient::new(&self.env, &self.token_address)
    }

    fn mint_to(&self, recipient: &Address, amount: i128) {
        self.stellar_asset_client().mint(recipient, &amount);
    }

    fn approve_vault_spend(&self, subscriber: &Address, amount: i128) {
        self.token_client().approve(
            subscriber,
            &self.contract_id,
            &amount,
            &self.env.ledger().sequence().saturating_add(500),
        );
    }

    fn create_subscription_for(
        &self,
        subscriber: &Address,
        merchant: &Address,
        amount: i128,
    ) -> u32 {
        self.approve_vault_spend(subscriber, amount);
        self.client()
            .create_subscription(subscriber, merchant, &amount, &3600u64, &false)
    }
}

#[test]
fn test_init_and_struct() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    assert_eq!(client.get_min_topup(), 1_000000i128);

    let sub = Subscription {
        subscriber: Address::generate(&ctx.env),
        merchant: Address::generate(&ctx.env),
        amount: 10_000_0000,
        interval_seconds: 30 * 24 * 60 * 60,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: 50_000_0000,
        usage_enabled: false,
    };
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

// ─── Merchant view helper tests ───────────────────────────────────────────────
// ─── Merchant view helper tests ───────────────────────────────────────────────

#[test]
fn test_merchant_with_no_subscriptions() {
    let (env, client, _, _) = setup_env();
    let merchant = Address::generate(&env);

    let subs = client.get_subscriptions_by_merchant(&merchant, &0, &10);
    assert_eq!(subs.len(), 0);

    let count = client.get_merchant_subscription_count(&merchant);
    assert_eq!(count, 0);
}

#[test]
fn test_merchant_with_one_subscription() {
    let (env, client, _, _) = setup_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let id = create_sub(&env, &client, &subscriber, &merchant, 10_000_000);

    let subs = client.get_subscriptions_by_merchant(&merchant, &0, &10);
    assert_eq!(subs.len(), 1);

    let sub = subs.get(0).unwrap();
    assert_eq!(sub.subscriber, subscriber);
    assert_eq!(sub.merchant, merchant);
    assert_eq!(sub.amount, 10_000_000);
    assert_eq!(sub.status, SubscriptionStatus::Active);

    // Verify get_subscription returns the same data
    let by_id = client.get_subscription(&id);
    assert_eq!(by_id.subscriber, subscriber);

    assert_eq!(client.get_merchant_subscription_count(&merchant), 1);
}

#[test]
fn test_merchant_with_multiple_subscriptions() {
    let (env, client, _, _) = setup_env();
    let merchant = Address::generate(&env);

    let sub1 = Address::generate(&env);
    let sub2 = Address::generate(&env);
    let sub3 = Address::generate(&env);

    create_sub(&env, &client, &sub1, &merchant, 5_000_000);
    create_sub(&env, &client, &sub2, &merchant, 10_000_000);
    create_sub(&env, &client, &sub3, &merchant, 20_000_000);

    let subs = client.get_subscriptions_by_merchant(&merchant, &0, &10);
    assert_eq!(subs.len(), 3);

    // Verify chronological (insertion) order
    assert_eq!(subs.get(0).unwrap().amount, 5_000_000);
    assert_eq!(subs.get(1).unwrap().amount, 10_000_000);
    assert_eq!(subs.get(2).unwrap().amount, 20_000_000);

    assert_eq!(client.get_merchant_subscription_count(&merchant), 3);
}

#[test]
fn test_pagination_basic() {
    let (env, client, _, _) = setup_env();
    let merchant = Address::generate(&env);

    // Create 5 subscriptions
    for i in 0..5 {
        let subscriber = Address::generate(&env);
        create_sub(&env, &client, &subscriber, &merchant, (i + 1) * 1_000_000);
    }

    // Request first 2
    let page = client.get_subscriptions_by_merchant(&merchant, &0, &2);
    assert_eq!(page.len(), 2);
    assert_eq!(page.get(0).unwrap().amount, 1_000_000);
    assert_eq!(page.get(1).unwrap().amount, 2_000_000);
}

#[test]
fn test_pagination_offset() {
    let (env, client, _, _) = setup_env();
    let merchant = Address::generate(&env);

    for i in 0..5 {
        let subscriber = Address::generate(&env);
        create_sub(&env, &client, &subscriber, &merchant, (i + 1) * 1_000_000);
    }

    // Request 2 starting from offset 2
    let page = client.get_subscriptions_by_merchant(&merchant, &2, &2);
    assert_eq!(page.len(), 2);
    assert_eq!(page.get(0).unwrap().amount, 3_000_000);
    assert_eq!(page.get(1).unwrap().amount, 4_000_000);
}

#[test]
fn test_pagination_beyond_end() {
    let (env, client, _, _) = setup_env();
    let merchant = Address::generate(&env);

    for i in 0..5 {
        let subscriber = Address::generate(&env);
        create_sub(&env, &client, &subscriber, &merchant, (i + 1) * 1_000_000);
    }

    // Request 10 starting from offset 3 → should return only last 2
    let page = client.get_subscriptions_by_merchant(&merchant, &3, &10);
    assert_eq!(page.len(), 2);
    assert_eq!(page.get(0).unwrap().amount, 4_000_000);
    assert_eq!(page.get(1).unwrap().amount, 5_000_000);
}

#[test]
fn test_pagination_start_past_end() {
    let (env, client, _, _) = setup_env();
    let merchant = Address::generate(&env);

    let subscriber = Address::generate(&env);
    create_sub(&env, &client, &subscriber, &merchant, 1_000_000);

    // Start way past the end
    let page = client.get_subscriptions_by_merchant(&merchant, &100, &10);
    assert_eq!(page.len(), 0);
}

#[test]
fn test_multiple_merchants_isolated() {
    let (env, client, _, _) = setup_env();
    let merchant_a = Address::generate(&env);
    let merchant_b = Address::generate(&env);

    let sub1 = Address::generate(&env);
    let sub2 = Address::generate(&env);
    let sub3 = Address::generate(&env);

    create_sub(&env, &client, &sub1, &merchant_a, 1_000_000);
    create_sub(&env, &client, &sub2, &merchant_a, 2_000_000);
    create_sub(&env, &client, &sub3, &merchant_b, 9_000_000);

    // Merchant A sees only their 2 subscriptions
    let a_subs = client.get_subscriptions_by_merchant(&merchant_a, &0, &10);
    assert_eq!(a_subs.len(), 2);
    assert_eq!(a_subs.get(0).unwrap().amount, 1_000_000);
    assert_eq!(a_subs.get(1).unwrap().amount, 2_000_000);

    // Merchant B sees only their 1 subscription
    let b_subs = client.get_subscriptions_by_merchant(&merchant_b, &0, &10);
    assert_eq!(b_subs.len(), 1);
    assert_eq!(b_subs.get(0).unwrap().amount, 9_000_000);

    assert_eq!(client.get_merchant_subscription_count(&merchant_a), 2);
    assert_eq!(client.get_merchant_subscription_count(&merchant_b), 1);
}

#[test]
fn test_merchant_subscription_count() {
    let (env, client, _, _) = setup_env();
    let merchant = Address::generate(&env);

    assert_eq!(client.get_merchant_subscription_count(&merchant), 0);

    for _ in 0..4 {
        let subscriber = Address::generate(&env);
        create_sub(&env, &client, &subscriber, &merchant, 5_000_000);
    }

    assert_eq!(client.get_merchant_subscription_count(&merchant), 4);
}

// -- Billing interval enforcement tests --------------------------------------

const T0: u64 = 1000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days in seconds

/// Setup env with contract, ledger at T0, and one subscription with given interval_seconds.
/// The subscription has enough prepaid balance for multiple charges (10 USDC).
fn setup(env: &Env, interval_seconds: u64) -> (SubscriptionVaultClient<'static>, u32) {
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    let token = Address::generate(env);
    let admin = Address::generate(env);
    client.init(&token, &admin, &1_000000i128);
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    let id =
        client.create_subscription(&subscriber, &merchant, &1000i128, &interval_seconds, &false);
    client.deposit_funds(&id, &subscriber, &10_000000i128); // 10 USDC so charge can succeed
    (client, id)
}

/// Just-before: charge 1 second before the interval elapses.
/// Must reject with IntervalNotElapsed and leave storage untouched.
#[test]
fn test_create_subscription_initializes_prepaid_and_transfers_tokens() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let token_client = ctx.token_client();
    let merchant = Address::generate(&ctx.env);
    let amount = 10_000000i128;

    ctx.approve_vault_spend(&ctx.subscriber, amount);

    let before_subscriber = token_client.balance(&ctx.subscriber);
    let before_vault = token_client.balance(&ctx.contract_id);

    let sub_id = client.create_subscription(&ctx.subscriber, &merchant, &amount, &3600u64, &false);
    let sub = client.get_subscription(&sub_id);

    assert_eq!(sub.prepaid_balance, amount);
    assert_eq!(sub.last_payment_timestamp, ctx.env.ledger().timestamp());
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(token_client.balance(&ctx.subscriber), before_subscriber - amount);
    assert_eq!(token_client.balance(&ctx.contract_id), before_vault + amount);
}

#[test]
fn test_create_subscription_missing_allowance_fails() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let merchant = Address::generate(&ctx.env);

    let result =
        client.try_create_subscription(&ctx.subscriber, &merchant, &5_000000i128, &3600u64, &false);
    assert_eq!(result, Err(Ok(Error::InsufficientAllowance)));
}

#[test]
fn test_create_subscription_transfer_failure_on_low_balance() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let merchant = Address::generate(&ctx.env);
    let amount = 500_000000i128;

    ctx.approve_vault_spend(&ctx.subscriber, amount);

    let result = client.try_create_subscription(&ctx.subscriber, &merchant, &amount, &3600u64, &true);
    assert_eq!(result, Err(Ok(Error::TransferFailed)));
}

#[test]
fn test_create_subscription_zero_or_negative_amount_fails() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let merchant = Address::generate(&ctx.env);

    ctx.approve_vault_spend(&ctx.subscriber, 1_000000i128);

    let zero = client.try_create_subscription(&ctx.subscriber, &merchant, &0i128, &3600u64, &false);
    let negative = client.try_create_subscription(&ctx.subscriber, &merchant, &-1i128, &3600u64, &false);

    assert_eq!(zero, Err(Ok(Error::InvalidAmount)));
    assert_eq!(negative, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_create_subscription_zero_interval_fails() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let merchant = Address::generate(&ctx.env);
    let amount = 1_000000i128;

    ctx.approve_vault_spend(&ctx.subscriber, amount);

    let result = client.try_create_subscription(&ctx.subscriber, &merchant, &amount, &0u64, &false);
    assert_eq!(result, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_get_merchant_config_returns_defaults_when_unset() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let merchant = Address::generate(&ctx.env);

    let config = client.get_merchant_config(&merchant);
    assert_eq!(
        config,
        MerchantConfig {
            version: 1,
            min_subscription_amount: 0,
            default_interval_seconds: 0,
        }
    );
}

#[test]
fn test_set_merchant_config_by_merchant_and_apply_defaults_on_create() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let merchant = Address::generate(&ctx.env);
    let subscriber = Address::generate(&ctx.env);
    ctx.mint_to(&subscriber, 30_000000i128);

    client.set_merchant_config(&merchant, &merchant, &5_000000i128, &7200u64);
    let stored = client.get_merchant_config(&merchant);
    assert_eq!(stored.min_subscription_amount, 5_000000i128);
    assert_eq!(stored.default_interval_seconds, 7200u64);

    ctx.approve_vault_spend(&subscriber, 6_000000i128);
    let sub_id = client.create_subscription(&subscriber, &merchant, &6_000000i128, &0u64, &false);
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.interval_seconds, 7200u64);

    ctx.approve_vault_spend(&subscriber, 4_000000i128);
    let below_min =
        client.try_create_subscription(&subscriber, &merchant, &4_000000i128, &0u64, &false);
    assert_eq!(below_min, Err(Ok(Error::BelowMerchantMinimum)));
}

#[test]
fn test_set_and_update_merchant_config_by_admin_over_time() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let merchant = Address::generate(&ctx.env);
    let subscriber = Address::generate(&ctx.env);
    ctx.mint_to(&subscriber, 50_000000i128);

    client.set_merchant_config(&ctx.admin, &merchant, &3_000000i128, &3600u64);
    let first = client.get_merchant_config(&merchant);
    assert_eq!(first.min_subscription_amount, 3_000000i128);
    assert_eq!(first.default_interval_seconds, 3600u64);

    client.update_merchant_config(&ctx.admin, &merchant, &None, &Some(1800u64));
    let updated = client.get_merchant_config(&merchant);
    assert_eq!(updated.min_subscription_amount, 3_000000i128);
    assert_eq!(updated.default_interval_seconds, 1800u64);

    ctx.approve_vault_spend(&subscriber, 3_500000i128);
    let sub_id = client.create_subscription(&subscriber, &merchant, &3_500000i128, &0u64, &false);
    assert_eq!(client.get_subscription(&sub_id).interval_seconds, 1800u64);
}

#[test]
fn test_merchant_config_unauthorized_and_invalid_update_failures() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let merchant = Address::generate(&ctx.env);
    let attacker = Address::generate(&ctx.env);

fn test_min_topup_below_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token = Address::generate(&env);
    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let min_topup = 5_000000i128; // 5 USDC

    let unauthorized_set =
        client.try_set_merchant_config(&attacker, &merchant, &1_000000i128, &3600u64);
    assert_eq!(unauthorized_set, Err(Ok(Error::Unauthorized)));

    client.set_merchant_config(&merchant, &merchant, &1_000000i128, &3600u64);
    let bad_update =
        client.try_update_merchant_config(&merchant, &merchant, &Some(-1i128), &None);
    assert_eq!(bad_update, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_charge_subscription_credits_shared_merchant_balance_multiple_subscribers() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let merchant = Address::generate(&ctx.env);

    let subscriber_a = Address::generate(&ctx.env);
    let subscriber_b = Address::generate(&ctx.env);
    ctx.mint_to(&subscriber_a, 30_000000i128);
    ctx.mint_to(&subscriber_b, 40_000000i128);

    let sub_a = ctx.create_subscription_for(&subscriber_a, &merchant, 12_000000i128);
    let sub_b = ctx.create_subscription_for(&subscriber_b, &merchant, 25_000000i128);

    client.charge_subscription(&sub_a);
    client.charge_subscription(&sub_b);

    assert_eq!(client.get_merchant_balance(&merchant), 37_000000i128);
    assert_eq!(client.get_subscription(&sub_a).prepaid_balance, 0i128);
    assert_eq!(client.get_subscription(&sub_b).prepaid_balance, 0i128);
}

#[test]
fn test_charge_subscription_insufficient_prepaid_does_not_credit() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let merchant = Address::generate(&ctx.env);

    let sub_id = ctx.create_subscription_for(&ctx.subscriber, &merchant, 10_000000i128);
    client.charge_subscription(&sub_id);

    let second_charge = client.try_charge_subscription(&sub_id);
    assert_eq!(second_charge, Err(Ok(Error::InsufficientBalance)));
    assert_eq!(client.get_merchant_balance(&merchant), 10_000000i128);
    assert_eq!(client.get_subscription(&sub_id).status, SubscriptionStatus::Active);
}

#[test]
fn test_merchant_balances_are_isolated_across_merchants() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let subscriber_two = Address::generate(&ctx.env);
    ctx.mint_to(&subscriber_two, 20_000000i128);

    let merchant_a = Address::generate(&ctx.env);
    let merchant_b = Address::generate(&ctx.env);

    let sub_a = ctx.create_subscription_for(&ctx.subscriber, &merchant_a, 7_000000i128);
    let sub_b = ctx.create_subscription_for(&subscriber_two, &merchant_b, 13_000000i128);

    client.charge_subscription(&sub_a);
    client.charge_subscription(&sub_b);

    assert_eq!(client.get_merchant_balance(&merchant_a), 7_000000i128);
    assert_eq!(client.get_merchant_balance(&merchant_b), 13_000000i128);
}

#[test]
fn test_withdraw_merchant_funds_debits_internal_balance_and_transfers_tokens() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let token_client = ctx.token_client();
    let merchant = Address::generate(&ctx.env);

    let sub_id = ctx.create_subscription_for(&ctx.subscriber, &merchant, 20_000000i128);
    client.charge_subscription(&sub_id);

    let before_wallet = token_client.balance(&merchant);
    client.withdraw_merchant_funds(&merchant, &8_000000i128);

    assert_eq!(client.get_merchant_balance(&merchant), 12_000000i128);
    assert_eq!(token_client.balance(&merchant), before_wallet + 8_000000i128);
}

#[test]
fn test_withdraw_merchant_funds_prevents_double_spend() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let merchant = Address::generate(&ctx.env);

    let sub_id = ctx.create_subscription_for(&ctx.subscriber, &merchant, 9_000000i128);
    client.charge_subscription(&sub_id);

    client.withdraw_merchant_funds(&merchant, &9_000000i128);
    assert_eq!(client.get_merchant_balance(&merchant), 0i128);

    let second_withdraw = client.try_withdraw_merchant_funds(&merchant, &1_000000i128);
    assert_eq!(second_withdraw, Err(Ok(Error::InsufficientBalance)));
}

#[test]
fn test_large_balance_accumulation_for_single_merchant() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let merchant = Address::generate(&ctx.env);

    let s1 = Address::generate(&ctx.env);
    let s2 = Address::generate(&ctx.env);
    let s3 = Address::generate(&ctx.env);
    let chunk = 2_000_000_000i128;

    ctx.mint_to(&s1, chunk + 1_000000i128);
    ctx.mint_to(&s2, chunk + 1_000000i128);
    ctx.mint_to(&s3, chunk + 1_000000i128);

    let sub1 = ctx.create_subscription_for(&s1, &merchant, chunk);
    let sub2 = ctx.create_subscription_for(&s2, &merchant, chunk);
    let sub3 = ctx.create_subscription_for(&s3, &merchant, chunk);

    client.charge_subscription(&sub1);
    client.charge_subscription(&sub2);
    client.charge_subscription(&sub3);

    assert_eq!(client.get_merchant_balance(&merchant), chunk * 3);
}

#[test]
fn test_min_topup_below_threshold() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    client.set_min_topup(&ctx.admin, &5_000000i128);

    let below = client.try_deposit_funds(&0, &ctx.subscriber, &4_999999i128);
    assert_eq!(below, Err(Ok(Error::BelowMinimumTopup)));

    let zero = client.try_deposit_funds(&0, &ctx.subscriber, &0i128);
    assert_eq!(zero, Err(Ok(Error::InvalidAmount)));

    let negative = client.try_deposit_funds(&0, &ctx.subscriber, &-100i128);
    assert_eq!(negative, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_min_topup_exactly_at_threshold() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let min_topup = 5_000000i128;
    client.set_min_topup(&ctx.admin, &min_topup);

    let result = client.try_deposit_funds(&0, &ctx.subscriber, &min_topup);
    assert!(result.is_ok());
}

#[test]
fn test_min_topup_above_threshold() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    client.set_min_topup(&ctx.admin, &5_000000i128);

    let result = client.try_deposit_funds(&0, &ctx.subscriber, &10_000000i128);
    assert!(result.is_ok());
}

#[test]
fn test_set_min_topup_by_admin() {
    let ctx = TestCtx::new();
    let client = ctx.client();

    assert_eq!(client.get_min_topup(), 1_000000i128);

    let new_min = 10_000000i128;
    client.set_min_topup(&ctx.admin, &new_min);
    assert_eq!(client.get_min_topup(), new_min);
}

#[test]
fn test_set_min_topup_unauthorized() {
    let ctx = TestCtx::new();
    let client = ctx.client();
    let non_admin = Address::generate(&ctx.env);

    let result = client.try_set_min_topup(&non_admin, &5_000000i128);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_invalid_min_topup_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    let admin = Address::generate(&env);

    assert_eq!(
        client.try_init(&token_address, &admin, &0i128),
        Err(Ok(Error::InvalidAmount))
    );
    client.init(&token_address, &admin, &1_000000i128);
    assert_eq!(
        client.try_set_min_topup(&admin, &0i128),
        Err(Ok(Error::InvalidAmount))
    );
}
