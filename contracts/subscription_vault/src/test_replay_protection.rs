use crate::{
    nonce::{DOMAIN_ADMIN_ROTATION, DOMAIN_BATCH_CHARGE},
    Error, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    Address, Env, Vec as SorobanVec,
};

// ── Constants ─────────────────────────────────────────────────────────────────

const T0: u64 = 1_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const AMOUNT: i128 = 10_000_000; // 10 USDC
const PREPAID: i128 = 50_000_000; // 50 USDC

// ── Helpers ───────────────────────────────────────────────────────────────────

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, client, token, admin)
}

// ── get_admin_nonce query ─────────────────────────────────────────────────────

#[test]
fn test_get_admin_nonce_initial_value_is_zero() {
    let (env, client, _, admin) = setup();
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE), 0u64);
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_ADMIN_ROTATION), 0u64);
}

#[test]
fn test_get_admin_nonce_advances_after_batch_charge() {
    let (env, client, token, admin) = setup();
    env.ledger().with_mut(|li| li.timestamp = T0);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_client.mint(&subscriber, &PREPAID);
    client.deposit_funds(&id, &subscriber, &PREPAID);

    env.ledger().with_mut(|li| li.timestamp = T0 + INTERVAL + 1);

    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE), 0u64);
    let ids = SorobanVec::from_array(&env, [id]);
    client.batch_charge(&ids, &0u64);
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE), 1u64);
}

#[test]
fn test_get_admin_nonce_advances_after_rotate_admin() {
    let (env, client, _, admin) = setup();
    let new_admin = Address::generate(&env);

    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_ADMIN_ROTATION), 0u64);
    client.rotate_admin(&admin, &new_admin, &0u64);
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_ADMIN_ROTATION), 1u64);
}

// ── Nonce enforcement for rotate_admin ───────────────────────────────────────

#[test]
fn test_rotate_admin_nonce_zero_succeeds() {
    let (env, client, _, admin) = setup();
    let new_admin = Address::generate(&env);
    // Fresh env: nonce starts at 0, providing 0 must succeed.
    client.rotate_admin(&admin, &new_admin, &0u64);
    assert_eq!(client.get_admin(), new_admin);
}

#[test]
fn test_rotate_admin_wrong_nonce_rejected() {
    let (env, client, _, admin) = setup();
    let new_admin = Address::generate(&env);
    // Nonce is 0 but caller provides 1 → NonceAlreadyUsed.
    let result = client.try_rotate_admin(&admin, &new_admin, &1u64);
    assert_eq!(result, Err(Ok(Error::NonceAlreadyUsed)));
    // State must not have changed.
    assert_eq!(client.get_admin(), admin);
}

#[test]
fn test_rotate_admin_replay_rejected() {
    let (env, client, _, admin) = setup();
    let admin2 = Address::generate(&env);
    let admin3 = Address::generate(&env);

    // First rotation succeeds, nonce advances to 1 for `admin`.
    client.rotate_admin(&admin, &admin2, &0u64);
    assert_eq!(client.get_admin(), admin2);

    // Attempt to replay the same rotation with old admin and stale nonce 0.
    // Auth fails first (admin is no longer stored admin), but nonce remains 1.
    let replay = client.try_rotate_admin(&admin, &admin3, &0u64);
    assert_eq!(replay, Err(Ok(Error::Unauthorized)));

    // Even providing the correct nonce (1) for the stale admin still fails on auth.
    let replay2 = client.try_rotate_admin(&admin, &admin3, &1u64);
    assert_eq!(replay2, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_rotate_admin_sequential_nonces() {
    // Each admin address has its own independent nonce counter.
    let (env, client, _, admin) = setup();
    let admin2 = Address::generate(&env);
    let admin3 = Address::generate(&env);
    let admin4 = Address::generate(&env);

    // admin uses nonce 0 to rotate to admin2.
    client.rotate_admin(&admin, &admin2, &0u64);
    // admin2 uses nonce 0 (fresh for admin2) to rotate to admin3.
    client.rotate_admin(&admin2, &admin3, &0u64);
    // admin3 uses nonce 0 to rotate to admin4.
    client.rotate_admin(&admin3, &admin4, &0u64);
    assert_eq!(client.get_admin(), admin4);

    // Verify each admin's domain nonce incremented independently.
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_ADMIN_ROTATION), 1u64);
    assert_eq!(client.get_admin_nonce(&admin2, &DOMAIN_ADMIN_ROTATION), 1u64);
    assert_eq!(client.get_admin_nonce(&admin3, &DOMAIN_ADMIN_ROTATION), 1u64);
    // admin4 never rotated; its nonce is still 0.
    assert_eq!(client.get_admin_nonce(&admin4, &DOMAIN_ADMIN_ROTATION), 0u64);
}

// ── Nonce enforcement for batch_charge ────────────────────────────────────────

#[test]
fn test_batch_charge_wrong_nonce_rejected() {
    let (env, client, token, admin) = setup();
    env.ledger().with_mut(|li| li.timestamp = T0);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_client.mint(&subscriber, &PREPAID);
    client.deposit_funds(&id, &subscriber, &PREPAID);
    env.ledger().with_mut(|li| li.timestamp = T0 + INTERVAL + 1);

    let ids = SorobanVec::from_array(&env, [id]);
    // Nonce is 0, providing 1 must be rejected.
    let result = client.try_batch_charge(&ids, &1u64);
    assert_eq!(result, Err(Ok(Error::NonceAlreadyUsed)));
    // Nonce must not have advanced.
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE), 0u64);
}

#[test]
fn test_batch_charge_replay_nonce_rejected() {
    let (env, client, token, admin) = setup();
    env.ledger().with_mut(|li| li.timestamp = T0);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_client.mint(&subscriber, &PREPAID);
    client.deposit_funds(&id, &subscriber, &PREPAID);
    env.ledger().with_mut(|li| li.timestamp = T0 + INTERVAL + 1);

    let ids = SorobanVec::from_array(&env, [id]);
    // First charge succeeds.
    client.batch_charge(&ids, &0u64);
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE), 1u64);

    // Replay with the same nonce (0) must be rejected.
    let replay = client.try_batch_charge(&ids, &0u64);
    assert_eq!(replay, Err(Ok(Error::NonceAlreadyUsed)));
    // Nonce must not have advanced further.
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE), 1u64);
}

#[test]
fn test_batch_charge_nonce_increments_each_call() {
    let (env, client, token, admin) = setup();
    env.ledger().with_mut(|li| li.timestamp = T0);

    // Create two subscriptions so we can charge in two separate rounds.
    let subscriber1 = Address::generate(&env);
    let subscriber2 = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);

    let id1 = client.create_subscription(
        &subscriber1, &merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>, &None::<u64>,
    );
    let id2 = client.create_subscription(
        &subscriber2, &merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>, &None::<u64>,
    );
    token_client.mint(&subscriber1, &PREPAID);
    token_client.mint(&subscriber2, &PREPAID);
    client.deposit_funds(&id1, &subscriber1, &PREPAID);
    client.deposit_funds(&id2, &subscriber2, &PREPAID);

    env.ledger().with_mut(|li| li.timestamp = T0 + INTERVAL + 1);

    // First charge batch.
    let ids1 = SorobanVec::from_array(&env, [id1]);
    client.batch_charge(&ids1, &0u64);
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE), 1u64);

    // Second charge batch (next billing cycle).
    env.ledger().with_mut(|li| li.timestamp = T0 + 2 * INTERVAL + 1);
    let ids2 = SorobanVec::from_array(&env, [id2]);
    client.batch_charge(&ids2, &1u64);
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE), 2u64);

    // Skipping nonce (providing 3 instead of 2) is rejected.
    let result = client.try_batch_charge(&ids1, &3u64);
    assert_eq!(result, Err(Ok(Error::NonceAlreadyUsed)));
}

// ── Domain separation ─────────────────────────────────────────────────────────

#[test]
fn test_domain_nonces_are_independent() {
    // Consuming a batch_charge nonce must not affect the admin_rotation nonce,
    // and vice versa.
    let (env, client, token, admin) = setup();
    env.ledger().with_mut(|li| li.timestamp = T0);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = client.create_subscription(
        &subscriber, &merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>, &None::<u64>,
    );
    let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_client.mint(&subscriber, &PREPAID);
    client.deposit_funds(&id, &subscriber, &PREPAID);
    env.ledger().with_mut(|li| li.timestamp = T0 + INTERVAL + 1);

    // Consume batch_charge nonce 0.
    let ids = SorobanVec::from_array(&env, [id]);
    client.batch_charge(&ids, &0u64);
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE), 1u64);

    // admin_rotation nonce for same admin must remain 0.
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_ADMIN_ROTATION), 0u64);

    // Now consume admin_rotation nonce 0.
    let new_admin = Address::generate(&env);
    client.rotate_admin(&admin, &new_admin, &0u64);
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_ADMIN_ROTATION), 1u64);

    // batch_charge nonce must remain 1 (not affected by rotation).
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_BATCH_CHARGE), 1u64);
}

#[test]
fn test_nonces_are_per_signer_not_global() {
    // Different signers have independent nonce counters for the same domain.
    let (env, client, _, admin) = setup();
    let admin2 = Address::generate(&env);
    let admin3 = Address::generate(&env);

    // admin rotates to admin2 using nonce 0.
    client.rotate_admin(&admin, &admin2, &0u64);
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_ADMIN_ROTATION), 1u64);

    // admin2's nonce for the same domain starts at 0, not 1.
    assert_eq!(client.get_admin_nonce(&admin2, &DOMAIN_ADMIN_ROTATION), 0u64);

    // admin2 rotates using nonce 0 successfully.
    client.rotate_admin(&admin2, &admin3, &0u64);
    assert_eq!(client.get_admin_nonce(&admin2, &DOMAIN_ADMIN_ROTATION), 1u64);
}

// ── NonceConsumedEvent emission ───────────────────────────────────────────────

#[test]
fn test_nonce_consumed_event_emitted_on_rotate_admin() {
    let (env, client, _, admin) = setup();
    let new_admin = Address::generate(&env);

    let events_before = env.events().all().len();
    client.rotate_admin(&admin, &new_admin, &0u64);
    let events_after = env.events().all();

    // At least one new event must have been emitted (the nonce_consumed event
    // plus the admin_rotated event).
    assert!(events_after.len() > events_before);
}

#[test]
fn test_nonce_consumed_event_emitted_on_batch_charge() {
    let (env, client, token, _admin) = setup();
    env.ledger().with_mut(|li| li.timestamp = T0);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = client.create_subscription(
        &subscriber, &merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>, &None::<u64>,
    );
    let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_client.mint(&subscriber, &PREPAID);
    client.deposit_funds(&id, &subscriber, &PREPAID);
    env.ledger().with_mut(|li| li.timestamp = T0 + INTERVAL + 1);

    let events_before = env.events().all().len();
    let ids = SorobanVec::from_array(&env, [id]);
    client.batch_charge(&ids, &0u64);
    let events_after = env.events().all();

    assert!(events_after.len() > events_before);
}

// ── Unauthorized never advances nonce ────────────────────────────────────────

#[test]
fn test_unauthorized_rotate_admin_does_not_advance_nonce() {
    let (env, client, _, admin) = setup();
    let stranger = Address::generate(&env);
    let target = Address::generate(&env);

    let nonce_before = client.get_admin_nonce(&stranger, &DOMAIN_ADMIN_ROTATION);
    assert_eq!(nonce_before, 0u64);

    // stranger is not the stored admin → Unauthorized before nonce is checked.
    let result = client.try_rotate_admin(&stranger, &target, &0u64);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));

    // Nonce for stranger must not have changed.
    assert_eq!(
        client.get_admin_nonce(&stranger, &DOMAIN_ADMIN_ROTATION),
        0u64
    );
    // Admin nonce must not have changed either.
    assert_eq!(client.get_admin_nonce(&admin, &DOMAIN_ADMIN_ROTATION), 0u64);
}
