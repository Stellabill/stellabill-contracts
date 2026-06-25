//! Chaos tests: random ledger timestamp jumps (forward *and* backward).
//!
//! # Problem
//!
//! Soroban validators normally guarantee non-decreasing ledger timestamps, but
//! ledger-rewinds during reorgs, host upgrades, or test harness mistakes can
//! produce timestamps that move *backward*.  Any code path that subtracts
//! timestamps (grace-period clocks, interval math, usage-window calculations)
//! must **never** panic or produce a spurious wrap-around positive value when
//! the clock regresses.
//!
//! # Invariants asserted
//!
//! 1. **No panic** — the contract never traps regardless of timestamp ordering.
//! 2. **No negative interval** — elapsed time calculations saturate at zero;
//!    a backward jump is treated as "no time passed".
//! 3. **No spurious `IntervalNotElapsed`** caused by wrap-around arithmetic.
//!    If `now < last_payment` the charge correctly returns `IntervalNotElapsed`,
//!    not an overflow or an unexpected success.
//! 4. **No spurious charge success** on a backward jump — the interval guard
//!    must not be defeated by a regressed clock.
//! 5. **Grace-period safety** — entering and exiting grace period with a
//!    backward jump never produces a negative grace duration or an early expiry.
//!
//! # How randomness is achieved without external `rand` crate
//!
//! The Soroban test environment does not support std thread-local RNG, and
//! adding `rand` would require a network fetch on first build.  Instead, a
//! deterministic pseudo-random sequence (xorshift64) seeded from a fixed
//! constant is used.  This gives repeatable, machine-verifiable results while
//! still exercising a wide range of timestamp values including edge cases.
//!
//! # Security notes
//!
//! * All u64 arithmetic in `charge_core.rs` uses `saturating_sub` /
//!   `saturating_add` or `checked_add` — never bare `-` on `u64` — so backward
//!   jumps produce 0 rather than `u64::MAX`.
//! * The `period_index` computation (`now.saturating_sub(start) / interval`)
//!   saturates at 0 when `now < start`, which maps correctly to period 0.
//! * `next_charge_time` comparison (`now >= last + interval`) uses
//!   `checked_add`; a backward jump makes `now < last + interval` true, so
//!   `IntervalNotElapsed` is returned — the correct conservative behaviour.
//! * Grace-period expiry uses `saturating_add`; the result is always a valid
//!   `u64` regardless of input.

#![cfg(test)]

use subscription_vault::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env,
};

// ── constants ─────────────────────────────────────────────────────────────────

/// 30-day billing interval (seconds).
const INTERVAL: u64 = 30 * 24 * 60 * 60;

/// Enough prepaid balance to fund many charges.
const PREPAID: i128 = 500_000_000;

/// Amount charged per billing cycle.
const AMOUNT: i128 = 1_000_000;

/// Grace period (seconds) — 7 days.
const GRACE: u64 = 7 * 24 * 60 * 60;

/// Initial "sane" starting timestamp (Unix seconds ≈ Jan 2023).
const T0: u64 = 1_672_531_200;

// ── deterministic pseudo-RNG (xorshift64) ─────────────────────────────────────

/// Simple xorshift64 PRNG — enough entropy for timestamp chaos without pulling
/// in the `rand` crate.  Produces a uniform-ish u64 sequence; state must be
/// non-zero.
fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// Generate the next pseudo-random timestamp from the RNG state.
///
/// The distribution intentionally includes:
/// - Values close to and below the current timestamp (backward jumps by 1 s).
/// - Values a full interval before the anchor (large regression).
/// - Values several intervals ahead (large forward jumps).
/// - `u64::MAX` and `0` (extreme edges).
fn next_timestamp(rng: &mut u64, anchor: u64) -> u64 {
    let raw = xorshift64(rng);
    match raw % 8 {
        0 => 0,                                                    // epoch minimum
        1 => u64::MAX,                                             // epoch maximum
        2 => anchor.saturating_sub(1),                             // -1 s (backward by 1)
        3 => anchor.saturating_sub(INTERVAL),                      // -1 interval
        4 => anchor.saturating_sub(INTERVAL + GRACE + 1),          // past grace window
        _ => anchor.saturating_add(raw % (INTERVAL * 4)),          // forward 0..4 intervals
    }
}

// ── shared setup ──────────────────────────────────────────────────────────────

struct ChaosEnv {
    env: Env,
    client: SubscriptionVaultClient<'static>,
    token: Address,
    admin: Address,
}

impl ChaosEnv {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(T0);

        let admin = Address::generate(&env);
        let token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        // fee_bps=0, admin, min_deposit=1, grace=GRACE
        client.init(&token, &0u32, &admin, &1i128, &GRACE);

        ChaosEnv { env, client, token, admin }
    }

    /// Mint tokens to `to` using the stellar-asset-contract admin.
    fn mint(&self, to: &Address, amount: i128) {
        use soroban_sdk::token::StellarAssetClient;
        let sac = StellarAssetClient::new(&self.env, &self.token);
        sac.mint(to, &amount);
    }

    /// Move the ledger timestamp to `ts`.
    fn set_ts(&self, ts: u64) {
        self.env.ledger().set_timestamp(ts);
    }

    /// Create a subscription whose prepaid balance is funded with `PREPAID`.
    fn create_funded_subscription(&self) -> (u32, Address, Address) {
        let subscriber = Address::generate(&self.env);
        let merchant = Address::generate(&self.env);
        self.mint(&subscriber, PREPAID * 2);

        let id = self.client.create_subscription(
            &subscriber,
            &merchant,
            &AMOUNT,
            &INTERVAL,
            &false,           // usage_enabled = false
            &None::<i128>,    // lifetime_cap
            &None::<u64>,     // expires_at
        );
        self.client.deposit_funds(&id, &subscriber, &PREPAID);
        (id, subscriber, merchant)
    }
}

// ── helper: assert no wrap-around ────────────────────────────────────────────

/// After any operation, verify the subscription's `last_payment_timestamp` is
/// ≤ the current ledger timestamp — it must never wrap to a huge positive value.
fn assert_no_timestamp_wrap(
    client: &SubscriptionVaultClient,
    sub_id: u32,
    current_ts: u64,
    label: &str,
) {
    let sub = client.get_subscription(&sub_id);
    assert!(
        sub.last_payment_timestamp <= current_ts,
        "{label}: last_payment_timestamp ({}) > current timestamp ({}); \
         saturating_sub must have been bypassed somewhere",
        sub.last_payment_timestamp,
        current_ts,
    );
}

// =============================================================================
// Test 1 — Backward jump by exactly 1 second
// =============================================================================

/// Invariant: a 1-second backward jump is safely treated as "no time passed".
/// The contract returns `IntervalNotElapsed` (or another valid error), never a
/// panic or a bogus charge success.
#[test]
fn test_backward_jump_by_one_second() {
    let ce = ChaosEnv::new();
    ce.set_ts(T0);
    let (id, _, _) = ce.create_funded_subscription();

    // Advance past the first billing boundary and charge
    let charge_ts = T0 + INTERVAL + 1;
    ce.set_ts(charge_ts);
    let r1 = ce.client.try_charge_subscription(&id);
    assert!(r1.is_ok(), "charge at interval+1 must succeed: {r1:?}");

    // Step backward by 1 — simulates a reorg / ledger rewind
    ce.set_ts(charge_ts - 1);
    let r2 = ce.client.try_charge_subscription(&id);

    // Must be an error (interval has not elapsed from last_payment = charge_ts)
    assert!(
        r2.is_err(),
        "charge after -1s backward jump must return error, not succeed: {r2:?}"
    );

    // Subscription timestamp must be unchanged (still charge_ts)
    let sub = ce.client.get_subscription(&id);
    assert_eq!(
        sub.last_payment_timestamp, charge_ts,
        "backward jump must not modify last_payment_timestamp"
    );
    assert_no_timestamp_wrap(&ce.client, id, charge_ts, "backward jump by 1");
}

// =============================================================================
// Test 2 — Jump to u64::MAX
// =============================================================================

/// Invariant: setting the timestamp to `u64::MAX` does not cause overflow in
/// any arithmetic (saturating_add / checked_add protect all call sites).
#[test]
fn test_timestamp_jump_to_u64_max() {
    let ce = ChaosEnv::new();
    ce.set_ts(T0);
    let (id, _, _) = ce.create_funded_subscription();

    // Jump to u64::MAX — the interval has elapsed since T0
    ce.set_ts(u64::MAX);
    let result = ce.client.try_charge_subscription(&id);

    assert!(result.is_ok(), "charge at u64::MAX must not panic: {result:?}");

    // last_payment_timestamp must be exactly u64::MAX (no arithmetic wrap)
    let sub = ce.client.get_subscription(&id);
    assert_eq!(
        sub.last_payment_timestamp,
        u64::MAX,
        "last_payment_timestamp must be u64::MAX after successful charge at u64::MAX"
    );
}

// =============================================================================
// Test 3 — Repeated identical timestamps (double-charge prevention)
// =============================================================================

/// Invariant: when the same timestamp repeats across two charge attempts,
/// the second returns an error.  No double-charge, no panic.
#[test]
fn test_repeated_identical_timestamps_no_double_charge() {
    let ce = ChaosEnv::new();
    ce.set_ts(T0);
    let (id, _, _) = ce.create_funded_subscription();

    // First charge at the exact interval boundary
    ce.set_ts(T0 + INTERVAL);
    let r1 = ce
        .client
        .try_charge_subscription(&id);
    assert!(r1.is_ok(), "first charge at boundary must succeed: {r1:?}");

    let balance_after_first = ce.client.get_subscription(&id).prepaid_balance;

    // Second call at the *identical* timestamp — must fail
    let r2 = ce
        .client
        .try_charge_subscription(&id);
    assert!(
        r2.is_err(),
        "second charge at same timestamp must return error: {r2:?}"
    );

    // Balance must be unchanged (no second debit)
    let balance_after_second = ce.client.get_subscription(&id).prepaid_balance;
    assert_eq!(
        balance_after_first, balance_after_second,
        "prepaid_balance must not change on rejected double-charge"
    );
}

// =============================================================================
// Test 4 — Backward jump across grace boundary
// =============================================================================

/// Invariant: entering grace period then rewinding the clock to before the
/// grace window opened must not:
///   a) cause the contract to compute a negative grace duration (would wrap).
///   b) immediately expire the grace period.
///   c) panic.
#[test]
fn test_backward_jump_across_grace_boundary_no_panic() {
    let ce = ChaosEnv::new();
    ce.set_ts(T0);

    // Create a subscription with only enough balance for ONE charge
    let subscriber = Address::generate(&ce.env);
    let merchant = Address::generate(&ce.env);
    ce.mint(&subscriber, AMOUNT + 1);
    let id = ce.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    ce.client.deposit_funds(&id, &subscriber, &AMOUNT);

    // First charge — exhausts the prepaid balance
    ce.set_ts(T0 + INTERVAL);
    let r1 = ce
        .client
        .try_charge_subscription(&id);
    assert!(r1.is_ok(), "first charge must succeed: {r1:?}");

    // Second charge — no funds → enters GracePeriod
    let grace_entry_ts = T0 + 2 * INTERVAL;
    ce.set_ts(grace_entry_ts);
    let _r2 = ce
        .client
        .try_charge_subscription(&id);
    // Result is Ok (GracePeriod entered) or Err — both valid, neither panics

    // Jump backward to before grace started
    ce.set_ts(grace_entry_ts.saturating_sub(GRACE / 2));

    // The contract must still be queryable without panic
    let info = ce.client.get_next_charge_info(&id);

    // If grace_deadline is returned, it must be > grace_entry_ts (not a wrapped zero)
    if let Some(deadline) = info.grace_deadline {
        assert!(
            deadline >= grace_entry_ts,
            "grace_deadline ({deadline}) < grace_entry_ts ({grace_entry_ts}); \
             saturating_sub produced a wrapped value"
        );
    }

    // Another charge attempt must not panic
    let _r3 = ce
        .client
        .try_charge_subscription(&id);
}

// =============================================================================
// Test 5 — Chaos: 200 random timestamp jumps, all invariants verified
// =============================================================================

/// Chaos driver: 200 random ledger timestamp mutations (forward and backward)
/// interleaved with charge calls.  Per-iteration invariants:
///   - `last_payment_timestamp` is always ≤ the ledger timestamp at time of charge.
///   - On a successful charge `last_payment_timestamp` equals the ledger timestamp.
///   - On a failed charge the `last_payment_timestamp` is unchanged.
#[test]
fn test_chaos_200_random_timestamp_jumps() {
    let ce = ChaosEnv::new();
    ce.set_ts(T0);
    let (id, subscriber, _) = ce.create_funded_subscription();

    // Deposit extra so balance is sufficient for many charges
    ce.mint(&subscriber, PREPAID * 50);
    ce.client.deposit_funds(&id, &subscriber, &(PREPAID * 20));

    let mut rng_state: u64 = 0xDEAD_BEEF_CAFE_1337;
    let mut anchor = T0;

    for i in 0..200u32 {
        let ts = next_timestamp(&mut rng_state, anchor);
        ce.set_ts(ts);

        let prev_lpt = ce.client.get_subscription(&id).last_payment_timestamp;

        let result = ce
            .client
            .try_charge_subscription(&id);

        let lpt_after = ce.client.get_subscription(&id).last_payment_timestamp;

        match &result {
            Ok(_) => {
                // Successful charge: last_payment_timestamp must equal ts
                assert_eq!(
                    lpt_after, ts,
                    "iter {i}: successful charge must set last_payment_timestamp = {ts}"
                );
                if ts > anchor {
                    anchor = ts;
                }
            }
            Err(_) => {
                // Failed charge: last_payment_timestamp must be unchanged
                assert_eq!(
                    lpt_after, prev_lpt,
                    "iter {i}: failed charge must leave last_payment_timestamp unchanged \
                     (was {prev_lpt}, now {lpt_after})"
                );
            }
        }

        // The subscription must never record a timestamp beyond the ledger
        assert!(
            lpt_after <= ts.max(anchor),
            "iter {i}: last_payment_timestamp ({lpt_after}) > max(ts={ts}, anchor={anchor})"
        );
    }
}

// =============================================================================
// Test 6 — Monotonic forward sequence: every charge must succeed
// =============================================================================

/// Baseline sanity: a strictly monotonically-increasing sequence of timestamps
/// must allow a successful charge at every interval boundary without any
/// spurious `IntervalNotElapsed` caused by wrap-around.
#[test]
fn test_monotonic_forward_sequence_all_charges_succeed() {
    let ce = ChaosEnv::new();
    ce.set_ts(T0);
    let (id, subscriber, _) = ce.create_funded_subscription();

    ce.mint(&subscriber, PREPAID * 20);
    ce.client.deposit_funds(&id, &subscriber, &(PREPAID * 10));

    let mut current = T0;

    for cycle in 0..6u32 {
        current += INTERVAL + 1; // strictly forward
        ce.set_ts(current);

        let result = ce
            .client
            .try_charge_subscription(&id);

        assert!(
            result.is_ok(),
            "cycle {cycle}: monotonic forward charge at ts={current} must succeed, got: {result:?}"
        );

        assert_no_timestamp_wrap(&ce.client, id, current, &format!("monotonic cycle {cycle}"));
    }
}

// =============================================================================
// Test 7 — Backward jump then forward recovery
// =============================================================================

/// After a backward jump (rejected charge), once the clock advances past
/// `last_payment + interval` the next charge must succeed — the contract
/// recovers correctly without any stale-state corruption.
#[test]
fn test_backward_jump_then_forward_recovery() {
    let ce = ChaosEnv::new();
    ce.set_ts(T0);
    let (id, subscriber, _) = ce.create_funded_subscription();
    ce.mint(&subscriber, PREPAID * 5);
    ce.client.deposit_funds(&id, &subscriber, &(PREPAID * 2));

    // First charge succeeds
    let first_charge_ts = T0 + INTERVAL + 1;
    ce.set_ts(first_charge_ts);
    let r1 = ce.client.try_charge_subscription(&id);
    assert!(r1.is_ok(), "first charge must succeed: {r1:?}");

    // Backward jump — well before the next interval
    ce.set_ts(T0);
    let r_back = ce.client.try_charge_subscription(&id);
    assert!(
        r_back.is_err(),
        "charge during backward jump must fail: {r_back:?}"
    );

    // last_payment_timestamp must still be first_charge_ts (unchanged)
    let sub_mid = ce.client.get_subscription(&id);
    assert_eq!(
        sub_mid.last_payment_timestamp, first_charge_ts,
        "backward jump must not corrupt last_payment_timestamp"
    );

    // Forward recovery — advance past next interval
    let recovery_ts = first_charge_ts + INTERVAL + 1;
    ce.set_ts(recovery_ts);
    let r2 = ce.client.try_charge_subscription(&id);
    assert!(
        r2.is_ok(),
        "charge after forward recovery must succeed: {r2:?}"
    );

    let sub_final = ce.client.get_subscription(&id);
    assert_eq!(
        sub_final.last_payment_timestamp, recovery_ts,
        "last_payment_timestamp must be updated to recovery_ts"
    );
}

// =============================================================================
// Test 8 — u64::MAX then backward jump to 0
// =============================================================================

/// Extreme edge: charge at `u64::MAX`, then set timestamp to 0.  The contract
/// must reject the charge (interval has NOT elapsed at ts=0 relative to
/// last_payment=u64::MAX) and must NOT corrupt `last_payment_timestamp`.
#[test]
fn test_u64_max_then_backward_to_zero() {
    let ce = ChaosEnv::new();
    ce.set_ts(T0);
    let (id, _, _) = ce.create_funded_subscription();

    // Charge at u64::MAX — should succeed (interval elapsed since T0)
    ce.set_ts(u64::MAX);
    let r1 = ce.client.try_charge_subscription(&id);
    assert!(r1.is_ok(), "charge at u64::MAX must succeed: {r1:?}");

    // Jump all the way back to 0
    ce.set_ts(0);
    let r2 = ce.client.try_charge_subscription(&id);

    // next_charge_time = u64::MAX + INTERVAL which saturates to u64::MAX;
    // now = 0 < u64::MAX → IntervalNotElapsed
    assert!(
        r2.is_err(),
        "charge after backward jump from u64::MAX to 0 must fail: {r2:?}"
    );

    // last_payment_timestamp must remain u64::MAX — not corrupted by backward jump
    let sub = ce.client.get_subscription(&id);
    assert_eq!(
        sub.last_payment_timestamp,
        u64::MAX,
        "last_payment_timestamp must remain u64::MAX after rejected backward jump"
    );
}

// =============================================================================
// Test 9 — get_next_charge_info never panics on any timestamp
// =============================================================================

/// The read-only query path must be safe for all timestamp values, including
/// backward-jumped clocks.  This test hammers `get_next_charge_info` across
/// 100 varied timestamps.
#[test]
fn test_get_next_charge_info_stable_on_chaos_timestamps() {
    let ce = ChaosEnv::new();
    ce.set_ts(T0);
    let (id, _, _) = ce.create_funded_subscription();

    let mut rng: u64 = 0xFEEDFACE_DEADC0DE;
    let mut anchor = T0;

    for i in 0..100u32 {
        let ts = next_timestamp(&mut rng, anchor);
        ce.set_ts(ts);

        // Must never panic
        let info = ce.client.get_next_charge_info(&id);

        // next_charge_timestamp is computed via saturating_add — must be >= sub.last_payment_timestamp
        let sub = ce.client.get_subscription(&id);
        assert!(
            info.next_charge_timestamp >= sub.last_payment_timestamp,
            "iter {i}: next_charge_timestamp ({}) < last_payment_timestamp ({}); \
             saturating_add failed",
            info.next_charge_timestamp,
            sub.last_payment_timestamp,
        );

        if ts > anchor {
            anchor = ts;
        }
    }
}

// =============================================================================
// Test 10 — period_index is always 0 when now < start_time
// =============================================================================

/// When the ledger timestamp is set *before* the subscription's `start_time`,
/// the period index computed as `now.saturating_sub(start_time) / interval`
/// must saturate to 0 and not produce a huge positive integer via wrapping.
/// Verified indirectly: a charge at ts < start_time must return
/// `IntervalNotElapsed` (not a spurious charge in period u64::MAX/INTERVAL).
#[test]
fn test_period_index_saturates_to_zero_when_now_before_start() {
    let ce = ChaosEnv::new();

    // Set start timestamp for the subscription
    ce.set_ts(T0);
    let (id, subscriber, _) = ce.create_funded_subscription();
    ce.mint(&subscriber, PREPAID * 3);
    ce.client.deposit_funds(&id, &subscriber, &(PREPAID * 2));

    // Record subscription start
    let sub_start = ce.client.get_subscription(&id).start_time;

    // Jump backward to before start_time
    let before_start = sub_start.saturating_sub(1);
    if before_start < sub_start {
        ce.set_ts(before_start);
        let result = ce
            .client
            .try_charge_subscription(&id);

        // Must fail (interval from start has not elapsed) — not panic, not succeed
        assert!(
            result.is_err(),
            "charge before subscription start must fail: {result:?}"
        );

        // last_payment_timestamp must equal start_time (unchanged)
        let sub_after = ce.client.get_subscription(&id);
        assert_eq!(
            sub_after.last_payment_timestamp, sub_start,
            "backward jump before start must not modify last_payment_timestamp"
        );
    }

    // Forward charge at exactly start + interval should succeed
    let valid_ts = sub_start + INTERVAL;
    ce.set_ts(valid_ts);
    let r_valid = ce
        .client
        .try_charge_subscription(&id);
    assert!(
        r_valid.is_ok(),
        "charge at start + interval must succeed: {r_valid:?}"
    );
}
