//! Invariant tests for per-subscriber credit limits and aggregate exposure.
//!
//! These lock the accounting guarantees behind `get_subscriber_exposure`
//! ([`subscription::compute_subscriber_exposure`]) and
//! `set_subscriber_credit_limit` *before* mainnet turns on credit checks:
//!
//! 1. **No-overflow summation** — exposure is the sum of prepaid balances plus
//!    the next-interval `amount` of every active subscription for a
//!    `(subscriber, token)` pair. The sum is computed with `safe_math::safe_add`
//!    (checked add), so a malicious merchant cannot wrap the `i128` exposure
//!    counter: instead of silently overflowing, the read returns
//!    `Error::Overflow`. See [`overflow_at_i128_boundary_yields_error`].
//!
//! 2. **No over-extension** — an operation that increases exposure (a new
//!    subscription) is rejected with `Error::CreditLimitExceeded` whenever it
//!    would push exposure above a configured non-zero limit, and is otherwise
//!    accepted. After any *accepted* increase, `exposure <= limit`.
//!
//! 3. **No claw-back** — shrinking a limit below current exposure succeeds and
//!    never mutates existing exposure; it only blocks *future* increases.
//!
//! 4. **Per-token isolation** — exposure and limits for one settlement token
//!    are independent of subscriptions denominated in another token.
//!
//! The headline test, [`credit_limit_invariant_fuzz`], drives a randomized
//! 500-step sequence of create / cancel / set-limit operations and re-checks
//! invariants (1)–(3) after every step against an independently-maintained
//! model. Sequences are deterministic in a `u64` seed; failing seeds are pinned
//! under `tests/fixtures/credit_limit/` (see that directory's `README.md`).

#![cfg(test)]

extern crate alloc;

use alloc::vec::Vec;

use soroban_sdk::{testutils::Address as _, Address, Env};
use subscription_vault::{Error, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient};

// ── constants ────────────────────────────────────────────────────────────────

const DECIMALS: u32 = 7;
const MIN_TOPUP: i128 = 1_000_000;
const GRACE_PERIOD: u64 = 3 * 24 * 60 * 60;
/// A fixed, always-valid interval (60s ..= 365d); credit limits are interval
/// independent, so a constant keeps the model focused on exposure.
const INTERVAL: u64 = 30 * 24 * 60 * 60;

/// Number of operations per randomized sequence (per the issue spec).
const STEPS_PER_SEED: u32 = 500;

/// Upper bound on the number of subscriptions *created* in a single sequence.
///
/// `get_subscriber_exposure` scans every id ever allocated (including cancelled
/// ones), so an unbounded create count would make the per-step exposure check
/// quadratic in the step count. Capping creations keeps each sequence fast
/// while still exercising hundreds of cancel / set-limit / create-attempt steps
/// and repeatedly driving exposure up against the limit and back down.
const MAX_CREATES_PER_SEED: u32 = 32;

/// Pinned seeds, embedded at compile time, that drive the fuzz test. The
/// baseline corpus is a small representative set; newly-discovered failing
/// seeds are appended here so they are replayed forever. See the fixtures
/// README. Each entry is a full independent 500-step sequence, so a handful of
/// seeds gives broad coverage while keeping the suite fast.
const REGRESSION_SEEDS: &str = include_str!("fixtures/credit_limit/regression_seeds.txt");

// ── deterministic PRNG (splitmix64) ──────────────────────────────────────────
//
// A self-contained, dependency-free generator so every sequence is a pure
// function of its seed and any failure is replayable by re-running the seed.

struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Rng { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform-ish value in `0..bound` (`bound` must be > 0).
    fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }
}

// ── shared setup ─────────────────────────────────────────────────────────────

struct Harness {
    env: Env,
    client: SubscriptionVaultClient<'static>,
    admin: Address,
    subscriber: Address,
    merchant: Address,
    token: Address,
}

fn setup() -> Harness {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    client.init(&token, &DECIMALS, &admin, &MIN_TOPUP, &GRACE_PERIOD);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    Harness {
        env,
        client,
        admin,
        subscriber,
        merchant,
        token,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Randomized 500-step invariant sequence
// ─────────────────────────────────────────────────────────────────────────────

/// Mirror of the on-chain exposure for one `(subscriber, token)` pair.
///
/// Only `create` / `cancel` / `set_limit` operations are exercised and no
/// deposits are made, so every subscription's `prepaid_balance` stays `0` and
/// exposure is exactly the sum of active-subscription `amount`s — small enough
/// to never approach the `i128` boundary (the overflow edge is covered
/// separately by [`overflow_at_i128_boundary_yields_error`]).
struct Model {
    /// `(subscription_id, amount)` for currently-active subscriptions.
    active: Vec<(u32, i128)>,
    /// Configured limit; `0` means "no limit".
    limit: i128,
}

impl Model {
    fn new() -> Self {
        Model {
            active: Vec::new(),
            limit: 0,
        }
    }

    fn exposure(&self) -> i128 {
        self.active.iter().map(|(_, a)| *a).sum()
    }
}

/// Run one deterministic sequence and assert the credit-limit invariants after
/// every step. Panics (failing the test) on the first violation, naming the
/// seed and step so it can be pinned as a regression fixture.
fn run_sequence(seed: u64) {
    let h = setup();
    let mut rng = Rng::new(seed);
    let mut model = Model::new();
    let mut creates = 0u32;

    for step in 0..STEPS_PER_SEED {
        // Op mix (out of 8): weighted toward `create` so exposure accumulates
        // and the limit is exercised in both the accept and reject directions.
        //   0..=3 create, 4..=5 cancel, 6..=7 set-limit
        match rng.below(8) {
            // Bound total creations (see `MAX_CREATES_PER_SEED`); once reached,
            // a would-be create falls through to a cancel so the sequence keeps
            // mutating exposure without growing the id-scan cost.
            0..=3 if creates < MAX_CREATES_PER_SEED => {
                creates += 1;
                // Amount in 1..=64 base units; tiny so the model never overflows.
                let amount = (rng.below(64) + 1) as i128;
                let predict_reject = model.limit != 0 && model.exposure() + amount > model.limit;

                if predict_reject {
                    let res = h.client.try_create_subscription(
                        &h.subscriber,
                        &h.merchant,
                        &amount,
                        &INTERVAL,
                        &false,
                        &None::<i128>,
                        &None::<u64>,
                    );
                    assert_eq!(
                        res,
                        Err(Ok(Error::CreditLimitExceeded)),
                        "seed {seed} step {step}: create of amount {amount} must be rejected \
                         (exposure {} + {amount} > limit {})",
                        model.exposure(),
                        model.limit,
                    );
                    // Rejection must not mutate exposure.
                    assert_eq!(
                        h.client.get_subscriber_exposure(&h.subscriber, &h.token),
                        model.exposure(),
                        "seed {seed} step {step}: exposure changed despite rejected create",
                    );
                } else {
                    let id = h.client.create_subscription(
                        &h.subscriber,
                        &h.merchant,
                        &amount,
                        &INTERVAL,
                        &false,
                        &None::<i128>,
                        &None::<u64>,
                    );
                    model.active.push((id, amount));

                    // Invariant (2): an accepted increase never over-extends a
                    // configured limit.
                    if model.limit != 0 {
                        assert!(
                            model.exposure() <= model.limit,
                            "seed {seed} step {step}: accepted create left exposure {} > limit {}",
                            model.exposure(),
                            model.limit,
                        );
                    }
                }
            }
            6..=7 => {
                // Set a new limit, frequently *below* current exposure to probe
                // the no-claw-back guarantee. `0` (no limit) ~1/7 of the time.
                let raw = rng.next_u64();
                let new_limit: i128 = if raw % 7 == 0 {
                    0
                } else {
                    (raw % 4096 + 1) as i128
                };

                let before = h.client.get_subscriber_exposure(&h.subscriber, &h.token);
                h.client
                    .set_subscriber_credit_limit(&h.admin, &h.subscriber, &h.token, &new_limit);
                model.limit = new_limit;

                // Invariant (3): changing the limit never claws back exposure,
                // even when shrunk below the current value.
                let after = h.client.get_subscriber_exposure(&h.subscriber, &h.token);
                assert_eq!(
                    before, after,
                    "seed {seed} step {step}: set_subscriber_credit_limit mutated exposure \
                     ({before} -> {after})",
                );
            }
            // Cancel a random active subscription (exposure decreases). Also the
            // landing arm for create rolls suppressed by `MAX_CREATES_PER_SEED`.
            _ => {
                if model.active.is_empty() {
                    continue;
                }
                let idx = rng.below(model.active.len() as u64) as usize;
                let (id, _amount) = model.active.remove(idx);
                h.client.cancel_subscription(&id, &h.subscriber);

                let sub = h.client.get_subscription(&id);
                assert_eq!(
                    sub.status,
                    SubscriptionStatus::Cancelled,
                    "seed {seed} step {step}: subscription {id} not Cancelled after cancel",
                );
            }
        }

        // Invariant (1): the contract's exposure equals our model at all times —
        // the checked summation neither drops nor double-counts a subscription
        // and never silently wraps.
        let on_chain = h.client.get_subscriber_exposure(&h.subscriber, &h.token);
        assert_eq!(
            on_chain,
            model.exposure(),
            "seed {seed} step {step}: on-chain exposure {on_chain} != model {}",
            model.exposure(),
        );
    }
}

/// Parse the pinned regression corpus (one `u64` per line, `#` comments).
fn regression_seeds() -> Vec<u64> {
    let mut seeds = Vec::new();
    for line in REGRESSION_SEEDS.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let seed: u64 = line
            .parse()
            .unwrap_or_else(|_| panic!("invalid seed in regression_seeds.txt: {line:?}"));
        seeds.push(seed);
    }
    seeds
}

#[test]
fn credit_limit_invariant_fuzz() {
    let seeds = regression_seeds();
    assert!(!seeds.is_empty(), "regression_seeds.txt must list at least one seed");
    for seed in seeds {
        run_sequence(seed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Targeted edge cases
// ─────────────────────────────────────────────────────────────────────────────

/// `i128` boundary: a single active subscription of `i128::MAX` is reported
/// exactly, but a second active subscription tips the checked summation over
/// the boundary and the read returns `Error::Overflow` instead of wrapping.
///
/// No credit limit is configured, so `create` skips the enforcement scan and
/// the unbounded amounts are admitted — exactly the state a malicious merchant
/// would try to weaponize against a naive `i128` accumulator.
#[test]
fn overflow_at_i128_boundary_yields_error() {
    let h = setup();

    // First subscription pins exposure at exactly i128::MAX.
    h.client.create_subscription(
        &h.subscriber,
        &h.merchant,
        &i128::MAX,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    assert_eq!(
        h.client.get_subscriber_exposure(&h.subscriber, &h.token),
        i128::MAX,
        "single max-amount subscription must report exposure == i128::MAX",
    );

    // A second active subscription pushes the sum past i128::MAX.
    h.client.create_subscription(
        &h.subscriber,
        &h.merchant,
        &1i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    assert_eq!(
        h.client
            .try_get_subscriber_exposure(&h.subscriber, &h.token),
        Err(Ok(Error::Overflow)),
        "summing past i128::MAX must surface Error::Overflow, never a wrapped value",
    );
}

/// Shrinking a limit below current exposure succeeds, performs no claw-back,
/// leaves existing subscriptions active, and blocks only *future* increases.
#[test]
fn limit_shrink_below_exposure_has_no_clawback() {
    let h = setup();

    let id = h.client.create_subscription(
        &h.subscriber,
        &h.merchant,
        &10_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    let exposure = h.client.get_subscriber_exposure(&h.subscriber, &h.token);
    assert_eq!(exposure, 10_000);

    // Shrink the limit far below current exposure.
    h.client
        .set_subscriber_credit_limit(&h.admin, &h.subscriber, &h.token, &1i128);

    // No claw-back: exposure and the existing subscription are untouched.
    assert_eq!(
        h.client.get_subscriber_exposure(&h.subscriber, &h.token),
        exposure,
        "shrinking the limit must not change exposure",
    );
    assert_eq!(
        h.client.get_subscription(&id).status,
        SubscriptionStatus::Active,
        "existing subscription must remain active after a limit shrink",
    );

    // But a new exposure-increasing op is now rejected.
    let res = h.client.try_create_subscription(
        &h.subscriber,
        &h.merchant,
        &1i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    assert_eq!(
        res,
        Err(Ok(Error::CreditLimitExceeded)),
        "new exposure must be blocked once the limit sits below current exposure",
    );

    // Cancelling frees capacity again.
    h.client.cancel_subscription(&id, &h.subscriber);
    assert_eq!(
        h.client.get_subscriber_exposure(&h.subscriber, &h.token),
        0,
        "cancelling the only subscription must drop exposure to zero",
    );
}

/// Exposure and limits are isolated per settlement token: a subscription in
/// token B neither inflates token A's exposure nor consumes token A's limit.
#[test]
fn exposure_is_isolated_per_token() {
    let h = setup();

    // Register a second accepted settlement token.
    let token_b = h
        .env
        .register_stellar_asset_contract_v2(h.admin.clone())
        .address();
    h.client
        .add_accepted_token(&h.admin, &token_b, &DECIMALS);

    // One subscription per token for the same subscriber.
    h.client.create_subscription(
        &h.subscriber,
        &h.merchant,
        &10_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    h.client.create_subscription_with_token(
        &h.subscriber,
        &h.merchant,
        &token_b,
        &777i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    // Each token reports only its own exposure.
    assert_eq!(
        h.client.get_subscriber_exposure(&h.subscriber, &h.token),
        10_000,
        "token A exposure must exclude token B subscriptions",
    );
    assert_eq!(
        h.client.get_subscriber_exposure(&h.subscriber, &token_b),
        777,
        "token B exposure must exclude token A subscriptions",
    );

    // A limit on token A does not constrain token B.
    h.client
        .set_subscriber_credit_limit(&h.admin, &h.subscriber, &h.token, &10_000i128);

    // Token A is at its limit: a further token-A subscription is rejected...
    let blocked = h.client.try_create_subscription(
        &h.subscriber,
        &h.merchant,
        &1i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    assert_eq!(
        blocked,
        Err(Ok(Error::CreditLimitExceeded)),
        "token A subscription must be blocked once token A is at its limit",
    );

    // ...while token B (no limit configured) still accepts new subscriptions.
    let ok = h.client.try_create_subscription_with_token(
        &h.subscriber,
        &h.merchant,
        &token_b,
        &1_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    assert!(
        ok.is_ok(),
        "token B subscription must succeed: token A's limit must not bind token B",
    );
    assert_eq!(
        h.client.get_subscriber_exposure(&h.subscriber, &token_b),
        1_777,
        "token B exposure must aggregate only its own subscriptions",
    );
}
