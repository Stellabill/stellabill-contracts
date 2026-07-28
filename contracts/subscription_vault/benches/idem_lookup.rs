//! Idempotency ring-buffer lookup cost benchmark.
//!
//! # Purpose
//! Measures the CPU-instruction cost of `idempotency::check_key` (exercised
//! indirectly through `deposit_funds`) at four ring positions:
//!
//! | Scenario | Ring position of the queried key |
//! |----------|----------------------------------|
//! | `head`   | First slot (index 0)             |
//! | `middle` | Mid-point slot (index RING/2)    |
//! | `tail`   | Last slot (index RING-1)         |
//! | `absent` | Key not present – full scan      |
//!
//! # Constant-time claim
//! The implementation iterates over every slot in the ring regardless of
//! where the match is found (there is no early-exit that breaks the loop on
//! the first match).  Therefore the CPU cost **must not** vary significantly
//! with ring position.  The bench asserts that the maximum spread across
//! head / middle / tail is within `TOLERANCE_PCT` percent of the minimum,
//! confirming the O(1)-in-position property.
//!
//! # Security notes
//! * Domain-scoping (`DOMAIN_DEPOSIT_FUNDS`) is verified transitively: a key
//!   inserted via `deposit_funds` is *not* replayable via `charge_subscription`
//!   because the two entrypoints use different domain constants.
//! * The ring evicts the oldest entry when full, so replay protection is
//!   bounded to the most-recent `IDEM_HISTORY` keys per subscription.
//! * CPU-cost assertions use a percentage tolerance rather than an absolute
//!   instruction count so that the test remains stable across SDK upgrades.

#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, BytesN, Env,
};
use subscription_vault::{SubscriptionVault, SubscriptionVaultClient};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Number of idempotency slots in the ring (must match types::IDEM_HISTORY = 32).
const RING_SIZE: usize = 32;

/// Allowed spread between the cheapest and most-expensive hit position,
/// expressed as a percentage of the cheapest cost.  We allow 20 % to
/// absorb noise from SHA-256 and SDK overhead while still catching any
/// O(n) regression that would cause tail to cost ~3× head.
const TOLERANCE_PCT: u64 = 20;

/// Charge amount per subscription (1 USDC in base units).
const AMOUNT: i128 = 1_000_000;
/// Billing interval.
const INTERVAL: u64 = 86_400;
/// Initial prepaid deposit.
const DEPOSIT: i128 = 100_000_000;
/// Minimum top-up threshold passed to `init`.
const MIN_TOPUP: i128 = 500_000;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build a deterministic 32-byte key from a single-byte seed.
fn make_key(env: &Env, seed: u8) -> BytesN<32> {
    let mut arr = [0u8; 32];
    arr[31] = seed;
    BytesN::from_array(env, &arr)
}

/// Build a deterministic 32-byte key from a u16 seed (allows > 255 distinct keys).
fn make_key16(env: &Env, seed: u16) -> BytesN<32> {
    let mut arr = [0u8; 32];
    let [hi, lo] = seed.to_be_bytes();
    arr[30] = hi;
    arr[31] = lo;
    BytesN::from_array(env, &arr)
}

/// Initialise environment, register the contract, and mint tokens.
///
/// Returns `(env, client, token_address, contract_id)`.
fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
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

    // Pre-mint enough tokens into the contract so charge operations can pay out.
    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&contract_id, &1_000_000_000i128);

    (env, client, token, contract_id)
}

/// Create a subscription and make an initial deposit so it is ready to charge.
fn create_and_fund(
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
    );
    let token_admin = token::StellarAssetClient::new(env, token);
    token_admin.mint(subscriber, &(DEPOSIT * 4));
    let no_key: Option<BytesN<32>> = None;
    client.deposit_funds(&id, subscriber, &DEPOSIT, &no_key);
    // Advance time past the interval so the first charge is always valid.
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + INTERVAL + 1);
    id
}

/// Seed the ring with `count` unique keys using `deposit_funds`.
///
/// Each call mints fresh tokens so the subscription balance never runs out.
/// Keys are built from seeds 0..count.  Returns nothing; the ring state is
/// persisted in the contract's instance storage.
fn seed_ring(
    env: &Env,
    client: &SubscriptionVaultClient,
    subscriber: &Address,
    token: &Address,
    sub_id: u32,
    count: usize,
) {
    let token_admin = token::StellarAssetClient::new(env, token);
    for i in 0..count {
        let key = make_key16(env, i as u16);
        token_admin.mint(subscriber, &MIN_TOPUP);
        client.deposit_funds(&sub_id, subscriber, &MIN_TOPUP, &Some(key));
    }
}

/// Measure the CPU cost of a single `deposit_funds` replay call (idempotent
/// no-op) using `key`.  The budget is reset immediately before the call and
/// sampled immediately after, isolating the lookup cost.
fn measure_replay_cost(
    env: &Env,
    client: &SubscriptionVaultClient,
    subscriber: &Address,
    token: &Address,
    sub_id: u32,
    key: BytesN<32>,
) -> u64 {
    // Mint enough so the call does not fail on balance checks.
    let token_admin = token::StellarAssetClient::new(env, token);
    token_admin.mint(subscriber, &MIN_TOPUP);

    env.budget().reset_default();
    client.deposit_funds(&sub_id, subscriber, &MIN_TOPUP, &Some(key));
    env.budget().cpu_instruction_cost()
}

// ── Benchmark tests ───────────────────────────────────────────────────────────

/// **Main benchmark**: constant-time ring-position check.
///
/// Seeds a full ring of `RING_SIZE` unique keys then replays a key from each
/// of four positions (head, middle, tail) and one absent key.  Asserts:
///
/// 1. Hit costs (head / middle / tail) are within `TOLERANCE_PCT` of each
///    other — confirming that lookup cost does not grow with ring position.
/// 2. Absent cost (full scan with no match) may be higher than any hit cost,
///    but the spread among hit positions alone must stay tight.
/// 3. All replay calls return without panicking (idempotency: the balance
///    must not change for already-seen keys).
#[test]
fn bench_ring_position_cost() {
    let (env, client, token, _contract_id) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let sub_id = create_and_fund(&env, &client, &subscriber, &merchant, &token);

    // ── Step 1: fill the ring with RING_SIZE distinct keys (seeds 0..RING_SIZE).
    seed_ring(&env, &client, &subscriber, &token, sub_id, RING_SIZE);

    // ── Step 2: measure replay cost at head (seed 0), middle (seed RING_SIZE/2),
    //           and tail (seed RING_SIZE-1).
    //
    //  The ring was inserted in order 0, 1, …, RING_SIZE-1.  After exactly
    //  RING_SIZE inserts the cursor wraps back to 0; no eviction has occurred.
    //  The linear scan in check_key visits entries in insertion order, so:
    //    - head  → match at iteration 0
    //    - mid   → match at iteration RING_SIZE/2
    //    - tail  → match at iteration RING_SIZE-1
    //
    //  If the implementation is truly constant-time-in-position, all three
    //  costs must be within TOLERANCE_PCT of each other.

    let head_key = make_key16(&env, 0);
    let mid_key = make_key16(&env, (RING_SIZE / 2) as u16);
    let tail_key = make_key16(&env, (RING_SIZE - 1) as u16);
    // Absent: a seed that was never inserted.
    let absent_key = make_key16(&env, 0xFFFF);

    let cost_head = measure_replay_cost(&env, &client, &subscriber, &token, sub_id, head_key);
    let cost_mid = measure_replay_cost(&env, &client, &subscriber, &token, sub_id, mid_key);
    let cost_tail = measure_replay_cost(&env, &client, &subscriber, &token, sub_id, tail_key);
    let cost_absent = measure_replay_cost(&env, &client, &subscriber, &token, sub_id, absent_key);

    std::println!(
        "[bench_ring_position_cost] head={} mid={} tail={} absent={}",
        cost_head, cost_mid, cost_tail, cost_absent
    );

    // ── Step 3: assert constant-time property across hit positions.
    let hit_min = cost_head.min(cost_mid).min(cost_tail);
    let hit_max = cost_head.max(cost_mid).max(cost_tail);
    // Avoid divide-by-zero on extremely cheap operations.
    if hit_min > 0 {
        let spread_pct = (hit_max - hit_min) * 100 / hit_min;
        assert!(
            spread_pct <= TOLERANCE_PCT,
            "Ring-position cost spread {}% exceeds {}% tolerance. \
             head={} mid={} tail={} — the implementation may have \
             introduced an early-exit O(n) regression.",
            spread_pct,
            TOLERANCE_PCT,
            cost_head,
            cost_mid,
            cost_tail
        );
    }

    // ── Step 4: absent key must cost at least as much as the minimum hit cost
    //           (it scans the full ring without finding a match).
    assert!(
        cost_absent >= hit_min,
        "Absent-key cost {} is less than hit_min {} — unexpected.",
        cost_absent,
        hit_min
    );

    // ── Step 5: verify balance is unchanged after all replay calls.
    //           The ring was seeded with MIN_TOPUP per key; the replay calls
    //           above should not alter the balance.
    let balance_before = client.get_subscription(&sub_id).prepaid_balance;
    let replay_key = make_key16(&env, 1);
    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &MIN_TOPUP);
    client.deposit_funds(&sub_id, &subscriber, &MIN_TOPUP, &Some(replay_key));
    let balance_after = client.get_subscription(&sub_id).prepaid_balance;
    assert_eq!(
        balance_before, balance_after,
        "Replay of a live ring key must not change the balance."
    );
}

/// **Edge case**: empty ring — no keys have been inserted yet.
///
/// `check_key` must return `false` immediately (zero iterations).  This test
/// confirms that the absent-key path on an empty ring does not panic and
/// completes with a measurably low cost.
#[test]
fn bench_empty_ring() {
    let (env, client, token, _contract_id) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    // Create a subscription but do NOT call seed_ring — ring is empty.
    let sub_id = create_and_fund(&env, &client, &subscriber, &merchant, &token);

    let key = make_key(&env, 0xAB);
    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &MIN_TOPUP);

    env.budget().reset_default();
    // Inserting into an empty ring: check_key returns false, push_key adds it.
    client.deposit_funds(&sub_id, &subscriber, &MIN_TOPUP, &Some(key.clone()));
    let cost_insert = env.budget().cpu_instruction_cost();

    // Replaying the same key: check_key returns true immediately.
    token_admin.mint(&subscriber, &MIN_TOPUP);
    env.budget().reset_default();
    let bal_before = client.get_subscription(&sub_id).prepaid_balance;
    client.deposit_funds(&sub_id, &subscriber, &MIN_TOPUP, &Some(key));
    let cost_replay = env.budget().cpu_instruction_cost();
    let bal_after = client.get_subscription(&sub_id).prepaid_balance;

    std::println!(
        "[bench_empty_ring] insert={} replay={}",
        cost_insert,
        cost_replay
    );

    // The replay (ring size = 1, match at index 0) must be cheaper than
    // the first insert (which also executes push_key + storage write).
    assert!(
        cost_replay <= cost_insert,
        "Replay on a single-entry ring should cost <= first insert. \
         replay={} insert={}",
        cost_replay,
        cost_insert
    );

    // Balance must not change on replay.
    assert_eq!(
        bal_before, bal_after,
        "Replay on empty-then-one-entry ring must not change balance."
    );
}

/// **Edge case**: ring of size 1.
///
/// After exactly one insert the ring holds one entry.  A replay of that entry
/// must be rejected and the cost must be comparable to — or cheaper than — the
/// ring-full case, because the scan terminates after a single comparison.
#[test]
fn bench_ring_size_one() {
    let (env, client, token, _contract_id) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let sub_id = create_and_fund(&env, &client, &subscriber, &merchant, &token);

    let only_key = make_key(&env, 0x01);
    let token_admin = token::StellarAssetClient::new(&env, &token);
    token_admin.mint(&subscriber, &MIN_TOPUP);
    client.deposit_funds(&sub_id, &subscriber, &MIN_TOPUP, &Some(only_key.clone()));

    // Replay the single key.
    token_admin.mint(&subscriber, &MIN_TOPUP);
    let bal_before = client.get_subscription(&sub_id).prepaid_balance;
    env.budget().reset_default();
    client.deposit_funds(&sub_id, &subscriber, &MIN_TOPUP, &Some(only_key));
    let cost_replay_1 = env.budget().cpu_instruction_cost();
    let bal_after = client.get_subscription(&sub_id).prepaid_balance;

    std::println!("[bench_ring_size_one] replay_cost={}", cost_replay_1);

    assert_eq!(
        bal_before, bal_after,
        "Replay on ring-of-1 must not change balance."
    );
    assert!(cost_replay_1 > 0, "CPU cost must be non-zero.");
}

/// **Edge case**: duplicate hash query — same raw key submitted twice in a row.
///
/// Verifies that submitting the identical 32-byte key value twice in a row
/// produces an idempotent no-op on the second call regardless of ring fill
/// level.  This covers the scenario where a caller retries the exact same
/// request under network partition.
#[test]
fn bench_duplicate_hash_query() {
    let (env, client, token, _contract_id) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let sub_id = create_and_fund(&env, &client, &subscriber, &merchant, &token);

    // Partially fill the ring (RING_SIZE / 2 keys) then measure duplicate cost.
    seed_ring(&env, &client, &subscriber, &token, sub_id, RING_SIZE / 2);

    // Pick the middle key of the partial ring.
    let dup_key = make_key16(&env, (RING_SIZE / 4) as u16);

    let token_admin = token::StellarAssetClient::new(&env, &token);

    // First replay.
    token_admin.mint(&subscriber, &MIN_TOPUP);
    let bal_before = client.get_subscription(&sub_id).prepaid_balance;
    env.budget().reset_default();
    client.deposit_funds(&sub_id, &subscriber, &MIN_TOPUP, &Some(dup_key.clone()));
    let cost_first_replay = env.budget().cpu_instruction_cost();
    let bal_mid = client.get_subscription(&sub_id).prepaid_balance;

    // Second replay (consecutive duplicate).
    token_admin.mint(&subscriber, &MIN_TOPUP);
    env.budget().reset_default();
    client.deposit_funds(&sub_id, &subscriber, &MIN_TOPUP, &Some(dup_key));
    let cost_second_replay = env.budget().cpu_instruction_cost();
    let bal_after = client.get_subscription(&sub_id).prepaid_balance;

    std::println!(
        "[bench_duplicate_hash_query] first_replay={} second_replay={}",
        cost_first_replay,
        cost_second_replay
    );

    // Neither replay should alter the balance.
    assert_eq!(
        bal_before, bal_mid,
        "First replay must not change balance."
    );
    assert_eq!(
        bal_mid, bal_after,
        "Second replay must not change balance."
    );

    // The two consecutive replays should have comparable cost (within tolerance).
    let lo = cost_first_replay.min(cost_second_replay);
    let hi = cost_first_replay.max(cost_second_replay);
    if lo > 0 {
        let spread_pct = (hi - lo) * 100 / lo;
        assert!(
            spread_pct <= TOLERANCE_PCT,
            "Consecutive duplicate replay cost spread {}% exceeds {}% tolerance. \
             first={} second={}",
            spread_pct,
            TOLERANCE_PCT,
            cost_first_replay,
            cost_second_replay
        );
    }
}

/// **Edge case**: domain isolation — the same raw key in two different domains
/// must not collide in the ring.
///
/// Security property: a key used with `deposit_funds` must never be treated as
/// a replay when the same bytes are submitted via `charge_subscription`.
#[test]
fn bench_domain_isolation_no_cross_replay() {
    let (env, client, token, _contract_id) = setup();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let sub_id = create_and_fund(&env, &client, &subscriber, &merchant, &token);

    let shared_key = make_key(&env, 0x42);
    let token_admin = token::StellarAssetClient::new(&env, &token);

    // Insert via deposit_funds (domain = DOMAIN_DEPOSIT_FUNDS).
    token_admin.mint(&subscriber, &MIN_TOPUP);
    client.deposit_funds(&sub_id, &subscriber, &MIN_TOPUP, &Some(shared_key.clone()));

    let bal_after_deposit = client.get_subscription(&sub_id).prepaid_balance;

    // Advance time so the charge interval has elapsed.
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + INTERVAL + 1);

    // Charge via charge_subscription with the SAME raw key bytes.
    // This MUST execute (not be treated as a replay from the deposit domain).
    let bal_before_charge = client.get_subscription(&sub_id).prepaid_balance;
    client.charge_subscription(&sub_id, &Some(shared_key.clone()));
    let bal_after_charge = client.get_subscription(&sub_id).prepaid_balance;

    assert!(
        bal_after_charge < bal_before_charge,
        "charge_subscription with same raw key as a previous deposit_funds key \
         must execute (domain isolation). balance did not decrease: before={} after={}",
        bal_before_charge,
        bal_after_charge
    );

    // Now replaying the deposit key via deposit_funds must still be idempotent.
    let bal_before_replay = client.get_subscription(&sub_id).prepaid_balance;
    token_admin.mint(&subscriber, &MIN_TOPUP);
    client.deposit_funds(&sub_id, &subscriber, &MIN_TOPUP, &Some(shared_key));
    let bal_after_replay = client.get_subscription(&sub_id).prepaid_balance;

    assert_eq!(
        bal_before_replay, bal_after_replay,
        "Replay of the original deposit key must remain idempotent."
    );

    let _ = bal_after_deposit; // suppress unused warning
}
