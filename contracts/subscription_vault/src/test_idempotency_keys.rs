use crate::{
    idempotency::{check_key, hash_idem_key, push_key, IDEM_HISTORY},
    ChargeExecutionResult, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, BytesN, Env,
};

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 86_400;
const DEPOSIT: i128 = 50_000_000;
const MIN_TOPUP: i128 = 1_000_000;

fn setup_test_env() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000_000);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &MIN_TOPUP, &(7 * 24 * 60 * 60));

    let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&contract_id, &1_000_000_000i128);

    (env, client, token)
}

fn create_and_fund_sub(
    env: &Env,
    client: &SubscriptionVaultClient,
    subscriber: &Address,
    merchant: &Address,
    token: &Address,
) -> u32 {
    let id = client.create_subscription(
        subscriber,
        merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
            &None::<soroban_sdk::Symbol>,
);

    let token_client = token::Client::new(env, token);
    if token_client.balance(subscriber) < DEPOSIT {
        token::StellarAssetClient::new(env, token).mint(subscriber, &(DEPOSIT * 2));
    }

    let none_key: Option<BytesN<32>> = None;
    client.deposit_funds(&id, subscriber, &DEPOSIT, &none_key);
    env.ledger().set_timestamp(env.ledger().timestamp() + 1);

    id
}

fn make_key(env: &Env, val: u8) -> BytesN<32> {
    let mut arr = [0u8; 32];
    arr[31] = val;
    BytesN::from_array(env, &arr)
}

#[test]
fn test_charge_subscription_idempotent_replay() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);

    env.ledger().set_timestamp(env.ledger().timestamp() + INTERVAL);

    let key = make_key(&env, 1);
    let r1 = client.charge_subscription(&id, &Some(key.clone()));
    assert_eq!(r1, ChargeExecutionResult::Charged);

    let r2 = client.charge_subscription(&id, &Some(key.clone()));
    assert_eq!(r2, ChargeExecutionResult::Charged);
}

#[test]
fn test_charge_subscription_different_keys_allowed() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);

    env.ledger().set_timestamp(env.ledger().timestamp() + INTERVAL);

    let key1 = make_key(&env, 1);
    let r1 = client.charge_subscription(&id, &Some(key1));
    assert_eq!(r1, ChargeExecutionResult::Charged);

    env.ledger().set_timestamp(env.ledger().timestamp() + INTERVAL);

    let key2 = make_key(&env, 2);
    let r2 = client.charge_subscription(&id, &Some(key2));
    assert_eq!(r2, ChargeExecutionResult::Charged);
}

#[test]
fn test_charge_subscription_none_key_ok() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);

    env.ledger().set_timestamp(env.ledger().timestamp() + INTERVAL);

    let none_key: Option<BytesN<32>> = None;
    let r = client.charge_subscription(&id, &none_key);
    assert_eq!(r, ChargeExecutionResult::Charged);
}

#[test]
fn test_deposit_funds_idempotent_replay() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);

    let key = make_key(&env, 10);
    let extra = 5_000_000i128;
    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &extra);

    client.deposit_funds(&id, &subscriber, &extra, &Some(key.clone()));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, DEPOSIT + extra);

    client.deposit_funds(&id, &subscriber, &extra, &Some(key.clone()));

    let sub2 = client.get_subscription(&id);
    assert_eq!(sub2.prepaid_balance, DEPOSIT + extra);
}

#[test]
fn test_deposit_funds_different_keys_allowed() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);

    let key1 = make_key(&env, 20);
    let key2 = make_key(&env, 21);
    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &20_000_000i128);

    client.deposit_funds(&id, &subscriber, &10_000_000i128, &Some(key1));
    client.deposit_funds(&id, &subscriber, &10_000_000i128, &Some(key2));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, DEPOSIT + 20_000_000i128);
}

#[test]
fn test_charge_one_off_idempotent_replay() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);

    let key = make_key(&env, 30);
    let amount: i128 = 5_000_000;

    client.charge_one_off(&id, &merchant, &amount, &Some(key.clone()));
    client.charge_one_off(&id, &merchant, &amount, &Some(key.clone()));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, DEPOSIT - amount);
}

#[test]
fn test_charge_one_off_different_keys_allowed() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);

    let key1 = make_key(&env, 31);
    let key2 = make_key(&env, 32);

    client.charge_one_off(&id, &merchant, &1_000_000i128, &Some(key1));
    client.charge_one_off(&id, &merchant, &2_000_000i128, &Some(key2));

    let sub = client.get_subscription(&id);
    assert_eq!(sub.prepaid_balance, DEPOSIT - 3_000_000i128);
}

#[test]
fn test_same_raw_key_different_entrypoints_no_collision() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);
    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &10_000_000i128);

    let key = make_key(&env, 99);

    client.charge_one_off(&id, &merchant, &1_000_000i128, &Some(key.clone()));
    client.deposit_funds(&id, &subscriber, &5_000_000i128, &Some(key.clone()));

    env.ledger().set_timestamp(env.ledger().timestamp() + INTERVAL);
    client.charge_subscription(&id, &Some(key.clone()));
}

#[test]
fn test_ring_buffer_evicts_oldest_key() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);
    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &1_000_000_000i128);

    // Insert 33 unique keys to fill buffer (32) + evict oldest (key 0)
    for i in 0..33u8 {
        let key = make_key(&env, i);
        token_admin.mint(&subscriber, &MIN_TOPUP);
        client.deposit_funds(&id, &subscriber, &MIN_TOPUP, &Some(key));
    }

    // Buffer now holds [32, 1, 2, 3, ..., 31], cursor = 1.
    // Key 0 was evicted (overwritten by key 32 at index 0).

    let balance_before = client.get_subscription(&id).prepaid_balance;

    // Key 1 is still present → idempotent no-op (balance unchanged)
    let key1 = make_key(&env, 1);
    client.deposit_funds(&id, &subscriber, &MIN_TOPUP, &Some(key1));
    assert_eq!(
        client.get_subscription(&id).prepaid_balance,
        balance_before,
        "key 1 should be idempotent (no balance change)"
    );

    // Key 0 was evicted → fresh deposit (balance increases)
    let key0 = make_key(&env, 0);
    token_admin.mint(&subscriber, &MIN_TOPUP);
    client.deposit_funds(&id, &subscriber, &MIN_TOPUP, &Some(key0));
    assert_eq!(
        client.get_subscription(&id).prepaid_balance,
        balance_before + MIN_TOPUP,
        "key 0 should be a fresh deposit"
    );
}

/// Idempotency ring-buffer wraparound test.
///
/// Feeds `IDEM_HISTORY + 3` unique hashes through `deposit_funds` and
/// validates:
///
/// * The freshest hash is still rejected (idempotent no-op).
/// * The oldest hash that was evicted can be replayed as a fresh deposit.
/// * Buffer eviction order matches the circular overwrite documentation:
///   after filling the buffer, each new entry overwrites the oldest slot
///   and advances the cursor.
/// * Inserting a duplicate hash within the live window is a no-op.
#[test]
fn test_idem_ring_wraparound_preserves_rejection_semantics() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);
    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &1_000_000_000i128);

    let domain: u32 = 0; // deposit_funds domain
    let subscription_id = id;

    let extra_per = 500_000i128;
    let total_inserts = IDEM_HISTORY + 3;
    let mut seen_hashes: Vec<BytesN<32>> = soroban_sdk::Vec::new(&env);

    // ── Phase 1: fill the buffer past capacity ──────────────────────
    for i in 0..total_inserts {
        let raw = make_key(&env, i as u8);
        let hashed = hash_idem_key(&env, domain, subscription_id, &raw);
        seen_hashes.push_back(hashed.clone());

        token_admin.mint(&subscriber, &extra_per);
        let bal_before = client.get_subscription(&id).prepaid_balance;
        let r = client.deposit_funds(&id, &subscriber, &extra_per, &Some(raw));
        assert_eq!(r, ChargeExecutionResult::Charged);

        // Every insert while the hash is new must increase the balance.
        let bal_after = client.get_subscription(&id).prepaid_balance;
        assert_eq!(
            bal_after,
            bal_before + extra_per,
            "insert {i}: balance must increase"
        );
    }

    // ── Phase 2: freshest hash must still be rejected ───────────────
    let freshest_hash = seen_hashes.get(total_inserts - 1).unwrap();
    let freshest_raw = make_key(&env, (total_inserts - 1) as u8);
    let bal_before = client.get_subscription(&id).prepaid_balance;
    client.deposit_funds(&id, &subscriber, &extra_per, &Some(freshest_raw));
    let bal_after = client.get_subscription(&id).prepaid_balance;
    assert_eq!(
        bal_before, bal_after,
        "freshest hash must be rejected (idempotent)"
    );

    // Also verify via the low-level check_key helper.
    assert!(
        check_key(&env, subscription_id, &freshest_hash),
        "check_key must recognise the freshest hash"
    );

    // ── Phase 3: oldest evicted hash can be replayed ────────────────
    // The buffer holds only IDEM_HISTORY entries.  The first 3 hashes
    // (indices 0, 1, 2) were overwritten by the last 3 insertions
    // (indices IDEM_HISTORY, IDEM_HISTORY+1, IDEM_HISTORY+2).
    let evicted_indices = [0u8, 1, 2];
    for &idx in &evicted_indices {
        let raw = make_key(&env, idx);
        let hashed = hash_idem_key(&env, domain, subscription_id, &raw);
        assert!(
            !check_key(&env, subscription_id, &hashed),
            "evicted hash {idx} must NOT be in the ring"
        );

        let bal_before = client.get_subscription(&id).prepaid_balance;
        token_admin.mint(&subscriber, &extra_per);
        client.deposit_funds(&id, &subscriber, &extra_per, &Some(raw));
        let bal_after = client.get_subscription(&id).prepaid_balance;
        assert_eq!(
            bal_after,
            bal_before + extra_per,
            "replay of evicted hash {idx} must be treated as fresh"
        );
    }

    // ── Phase 4: still-live hashes inside the ring remain rejected ──
    // Indices 3..=IDEM_HISTORY-1 were never evicted.
    for idx in 3..IDEM_HISTORY {
        let raw = make_key(&env, idx as u8);
        let hashed = hash_idem_key(&env, domain, subscription_id, &raw);
        assert!(
            check_key(&env, subscription_id, &hashed),
            "hash {idx} should still live in the ring"
        );

        let bal_before = client.get_subscription(&id).prepaid_balance;
        client.deposit_funds(&id, &subscriber, &extra_per, &Some(raw));
        let bal_after = client.get_subscription(&id).prepaid_balance;
        assert_eq!(
            bal_before, bal_after,
            "live hash {idx} must be rejected (idempotent)"
        );
    }

    // ── Phase 5: duplicate insertion within live window is no-op ─────
    let dup_idx = IDEM_HISTORY + 1;
    let dup_raw = make_key(&env, dup_idx as u8);
    let bal_before = client.get_subscription(&id).prepaid_balance;
    client.deposit_funds(&id, &subscriber, &extra_per, &Some(dup_raw));
    let bal_after = client.get_subscription(&id).prepaid_balance;
    assert_eq!(
        bal_before, bal_after,
        "duplicate of live hash {dup_idx} must be rejected"
    );
}

/// Wraparound at exactly IDEM_HISTORY capacity.
///
/// Insert exactly `IDEM_HISTORY` keys, then one more to trigger the
/// first overwrite, and verify the overwritten slot is now free.
#[test]
fn test_idem_ring_exact_capacity_then_overwrite() {
    let (env, client, token) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let id = create_and_fund_sub(&env, &client, &subscriber, &merchant, &token);
    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &1_000_000_000i128);

    let domain: u32 = 0;
    let extra = 500_000i128;

    // Fill to exactly IDEM_HISTORY.
    for i in 0..IDEM_HISTORY {
        let raw = make_key(&env, i as u8);
        token_admin.mint(&subscriber, &extra);
        client.deposit_funds(&id, &subscriber, &extra, &Some(raw));
    }

    // All IDEM_HISTORY slots are occupied – re-inserting any of them
    // must be idempotent.
    let mid = IDEM_HISTORY / 2;
    let mid_raw = make_key(&env, mid as u8);
    let bal_before = client.get_subscription(&id).prepaid_balance;
    client.deposit_funds(&id, &subscriber, &extra, &Some(mid_raw));
    assert_eq!(
        client.get_subscription(&id).prepaid_balance,
        bal_before,
        "mid-range hash must be idempotent at capacity"
    );

    // One more insert overwrites slot 0.
    let overwrite_raw = make_key(&env, 0xFF);
    token_admin.mint(&subscriber, &extra);
    let bal_before = client.get_subscription(&id).prepaid_balance;
    client.deposit_funds(&id, &subscriber, &extra, &Some(overwrite_raw));
    let bal_after = client.get_subscription(&id).prepaid_balance;
    assert_eq!(
        bal_after,
        bal_before + extra,
        "new hash must be accepted"
    );

    // Key 0 was overwritten – re-inserting it must be fresh.
    let key0_raw = make_key(&env, 0);
    let key0_hashed = hash_idem_key(&env, domain, id, &key0_raw);
    assert!(
        !check_key(&env, id, &key0_hashed),
        "key 0 must have been evicted"
    );
    let bal_before = client.get_subscription(&id).prepaid_balance;
    token_admin.mint(&subscriber, &extra);
    client.deposit_funds(&id, &subscriber, &extra, &Some(key0_raw));
    assert_eq!(
        client.get_subscription(&id).prepaid_balance,
        bal_before + extra,
        "re-inserting evicted key 0 must succeed"
    );
}
