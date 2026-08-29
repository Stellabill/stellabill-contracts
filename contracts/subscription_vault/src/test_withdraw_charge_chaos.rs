//! Chaos test: order-independence of `withdraw_merchant_funds` and
//! `charge_subscription` over the shared merchant accounting path.
//!
//! # What this proves
//!
//! `charge_subscription` credits a merchant (`credit_merchant_balance_for_token`)
//! and `withdraw_merchant_funds` debits one (`withdraw_merchant_funds_for_token`).
//! Both mutate the same three pieces of state:
//!
//! * `DataKey::MerchantBalance(merchant, token)` — the authoritative balance
//! * `DataKey::MerchantEarnings(merchant, token)` — the derived `TokenEarnings` ledger
//! * `DataKey::TotalAccounted(token)` — the global accounting anchor
//!
//! Soroban executes one invocation at a time, so there is no true data race
//! here. What *can* desync is the **derived** ledger: if a credit or debit
//! updates `MerchantBalance` and `TokenEarnings` inconsistently, the two views
//! drift apart, and the drift depends on the interleaving. That is the
//! race-adjacent bug class this test targets.
//!
//! The core property is a commutativity claim. For a fixed multiset of
//! operations — `c` successful charges of `amount` each, and a set of
//! withdrawals summing to `w` — the final state must not depend on the order
//! in which they were applied:
//!
//! ```text
//! MerchantBalance          == c * amount - w
//! accruals.interval        == c * amount        (monotonic; withdrawals never touch it)
//! withdrawals              == w                 (monotonic; charges never touch it)
//! refunds                  == 0                 (no refund was ever issued)
//! computed_balance         == MerchantBalance   (derived view agrees with stored)
//! ```
//!
//! where `computed_balance = accruals - withdrawals - refunds`, matching
//! `merchant::get_reconciliation_snapshot`.
//!
//! Note the *expected commutativity* qualifier in the property: charges and
//! withdrawals commute only up to feasibility. A withdrawal of `w` is rejected
//! when the current balance is below `w`, so a permutation that front-loads
//! withdrawals legitimately performs fewer of them than one that front-loads
//! charges. The harness therefore records which operations actually succeeded
//! and asserts the invariant against that realised multiset, rather than
//! demanding that every permutation execute identically. Asserting the latter
//! would be wrong, not strict.
//!
//! # Trace generation and shrinking
//!
//! `proptest` generates a permutation of an operation multiset via a
//! Fisher-Yates shuffle driven by a generated index vector, so the shuffle
//! itself shrinks. On failure the harness prints the shrunk permutation as a
//! replayable `OpKind` sequence together with the per-step state transitions,
//! so a maintainer can paste the trace into a `#[test]` without re-running
//! proptest.
//!
//! # Edge cases covered by the deterministic tests below
//!
//! * `withdraw_immediately_after_charge` — no idle interval between credit and debit
//! * `batch_charge_then_drain` — many charges accumulate, then one full-balance drain
//! * `withdraw_at_zero_balance` — withdrawal against an untouched//drained ledger
//!
//! # ⚠ STATUS: UNVERIFIED — THIS TEST HAS NEVER BEEN EXECUTED
//!
//! `cargo test` cannot build this crate at the commit this file was written
//! against (`c2473ed`). `cargo check --lib` reports 169 pre-existing errors in
//! `lib.rs`, `subscription.rs`, `merchant.rs`, `types.rs`, `queries.rs`,
//! `admin.rs`, `charge_core.rs` and `dispute.rs`, caused by ~485 lines of
//! definitions going missing from `types.rs` between `ab3b2f7` and `1312f1e`
//! (missing `DataKey` variants, `Error` variants, `CancellationEscrow`,
//! `normalize_amount`, and the `Subscription::auto_renew` field). Two
//! duplicate-definition errors compound it: the `use crate::types::{...}` block
//! at `merchant.rs:24` names six items twice, and `mod test_subscription_transfer;`
//! is declared twice in `lib.rs` (3510 and 3513).
//!
//! None of that is caused by this file, and none of it can be fixed from here.
//! Consequently the assertions below are written from a reading of the source,
//! not from observed behaviour, and the signatures they call may need
//! adjustment once the crate builds. Treat every `assert_eq!` as a hypothesis.
//!
//! # Two known-broken invariants this test deliberately does NOT assert green
//!
//! **1. `refunds` is double-counted on withdrawal.**
//! `merchant.rs:901-913` increments *both* `earnings.refunds` *and*
//! `earnings.withdrawals` by `amount` on every withdrawal, while
//! `MerchantBalance` is decremented once. `merchant_refund` (`merchant.rs:988`)
//! touches only `refunds`, confirming that bucket is not meant to absorb
//! withdrawals. So `computed_balance = accruals - withdrawals - refunds`
//! understates the true balance by exactly the withdrawn amount, and
//! `get_reconciliation_snapshot`'s `checked_sub(..).unwrap_or(0)` clamps the
//! negative result to `0` so the desync never surfaces as an error.
//!
//! `prop_withdraw_charge_order_independent` asserts `refunds == 0` and
//! `computed_balance == stored_balance`, both of which should FAIL on the
//! current source. That is intentional: this is the bug the test exists to
//! catch. See `xfail_refunds_double_count_on_withdraw` below for a minimal
//! deterministic reproduction.
//!
//! **2. `TotalAccounted` cannot be asserted at all.**
//! `src/accounting.rs` is not compiled into the crate — there is no
//! `mod accounting;` declaration anywhere. `lib.rs:327` defines an inline
//! `pub mod accounting` whose three functions ignore their arguments and whose
//! `get_total_accounted` unconditionally returns `0`. Every
//! `crate::accounting::add_total_accounted` / `sub_total_accounted` call in the
//! charge and withdraw paths is therefore a no-op against the real
//! `DataKey::TotalAccounted` storage.
//!
//! Asserting `get_total_accounted() == expected` would compare `0` to `0` for
//! the empty case and pass while proving nothing. The `TotalAccounted` half of
//! the invariant is marked `#[ignore]` in
//! `ignored_total_accounted_tracks_net_flow` with instructions for re-enabling
//! it once `accounting.rs` is actually wired in.

#![cfg(test)]

use crate::types::{TokenEarnings, OP_CHARGE, OP_WITHDRAW};
use crate::{SubscriptionVault, SubscriptionVaultClient};
use proptest::prelude::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env, String as SorobanString,
};

const INTERVAL: u64 = 30 * 24 * 60 * 60;
const CHARGE_AMOUNT: i128 = 10_000_000;
const START_TS: u64 = 1_000_000;
const TOKEN_DECIMALS: u32 = 7;
const MIN_TOPUP: i128 = 1_000_000;
const GRACE_PERIOD: u64 = 7 * 24 * 60 * 60;

/// Deposit enough to fund `n` charges of `CHARGE_AMOUNT`.
fn deposit_for(n: u32) -> i128 {
    CHARGE_AMOUNT * (n as i128) + CHARGE_AMOUNT
}

/// One step in a chaos trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpKind {
    /// Advance the ledger one full interval and charge the subscription.
    Charge,
    /// Attempt to withdraw a fixed amount of `CHARGE_AMOUNT`.
    WithdrawFixed,
    /// Attempt to withdraw the merchant's entire current balance.
    WithdrawAll,
}

impl OpKind {
    fn as_src(&self) -> &'static str {
        match self {
            OpKind::Charge => "OpKind::Charge",
            OpKind::WithdrawFixed => "OpKind::WithdrawFixed",
            OpKind::WithdrawAll => "OpKind::WithdrawAll",
        }
    }
}

/// What a trace actually managed to execute. Withdrawals may be legitimately
/// refused when the balance is short, so the realised multiset is an output of
/// the run, not an input to it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Realised {
    /// Number of charges that returned `Charged`.
    charges_applied: u32,
    /// Total value credited to the merchant across all successful charges.
    credited: i128,
    /// Total value successfully withdrawn.
    withdrawn: i128,
    /// Withdrawals rejected for insufficient balance / empty ledger.
    withdrawals_refused: u32,
}

struct Harness {
    env: Env,
    client: SubscriptionVaultClient<'static>,
    token: Address,
    merchant: Address,
    sub_id: u32,
    now: u64,
}

/// Build a vault with one funded, charge-ready subscription and a merchant that
/// has an initialised config (required by the withdraw path) permitting both
/// charge and withdraw operations.
fn setup(charge_budget: u32) -> Harness {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(START_TS);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token, &TOKEN_DECIMALS, &admin, &MIN_TOPUP, &GRACE_PERIOD);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // The withdraw path requires a merchant config to exist (`Error::NotFound`
    // otherwise). fee_bips = 0 keeps the accounting arithmetic exact: with a
    // protocol fee the merchant credit would be `amount - fee` and any rounding
    // would muddy the commutativity assertions.
    client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0i32,
        &(OP_CHARGE | OP_WITHDRAW),
        &None::<Address>,
        &SorobanString::from_str(&env, ""),
    );

    let funding = deposit_for(charge_budget);
    token::StellarAssetClient::new(&env, &token).mint(&subscriber, &funding);

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &CHARGE_AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    client.deposit_funds(&sub_id, &subscriber, &funding, &None::<soroban_sdk::BytesN<32>>);

    Harness {
        env,
        client,
        token,
        merchant,
        sub_id,
        now: START_TS,
    }
}

impl Harness {
    fn balance(&self) -> i128 {
        self.client.get_merchant_balance(&self.merchant)
    }

    fn earnings(&self) -> TokenEarnings {
        self.client
            .get_merchant_token_earnings(&self.merchant, &self.token)
    }

    /// Total accrued across all three billing kinds.
    fn accrued(&self) -> i128 {
        let e = self.earnings();
        e.accruals.interval + e.accruals.usage + e.accruals.one_off
    }

    /// The derived balance, mirroring `get_reconciliation_snapshot`.
    fn computed_balance(&self) -> i128 {
        let e = self.earnings();
        self.accrued() - e.withdrawals - e.refunds
    }

    /// Apply one operation, recording what actually happened.
    ///
    /// Charges advance the ledger by a full interval first, since `charge_one`
    /// enforces `IntervalNotElapsed` and a per-period replay guard. Withdrawals
    /// do not move the clock — that is what makes "withdraw immediately after
    /// charge" reachable inside a shuffled trace.
    fn apply(&mut self, op: OpKind, realised: &mut Realised) {
        match op {
            OpKind::Charge => {
                self.now += INTERVAL + 1;
                self.env.ledger().set_timestamp(self.now);

                let before = self.balance();
                let res = self
                    .client
                    .try_charge_subscription(&self.sub_id, &None::<soroban_sdk::BytesN<32>>);

                if res.is_ok() {
                    let delta = self.balance() - before;
                    if delta > 0 {
                        realised.charges_applied += 1;
                        realised.credited += delta;
                    }
                }
            }
            OpKind::WithdrawFixed => {
                self.try_withdraw(CHARGE_AMOUNT, realised);
            }
            OpKind::WithdrawAll => {
                let all = self.balance();
                // A zero-balance withdraw is still worth issuing: it exercises
                // the `current == 0 -> Error::NotFound` branch.
                self.try_withdraw(if all > 0 { all } else { 1 }, realised);
            }
        }
    }

    fn try_withdraw(&mut self, amount: i128, realised: &mut Realised) {
        let before = self.balance();
        let res = self
            .client
            .try_withdraw_merchant_funds(&self.merchant, &amount);

        if res.is_ok() {
            let moved = before - self.balance();
            realised.withdrawn += moved;
        } else {
            realised.withdrawals_refused += 1;
        }
    }
}

/// Render a trace as pasteable Rust so a failure is reproducible without proptest.
fn render_trace(ops: &[OpKind]) -> std::string::String {
    let body = ops
        .iter()
        .map(|o| o.as_src())
        .collect::<std::vec::Vec<_>>()
        .join(", ");
    format!("let trace = [{}];", body)
}

/// Run a trace and assert the order-independent invariants against the multiset
/// of operations that actually succeeded.
///
/// Returns the realised multiset so callers can make additional claims.
fn run_and_assert(ops: &[OpKind]) -> Realised {
    let charge_budget = ops.iter().filter(|o| **o == OpKind::Charge).count() as u32;
    let mut h = setup(charge_budget.max(1));

    let mut realised = Realised::default();
    // Per-step transitions, printed only if an assertion below fails.
    let mut journal: std::vec::Vec<std::string::String> = std::vec::Vec::new();

    for (i, op) in ops.iter().enumerate() {
        h.apply(*op, &mut realised);
        let e = h.earnings();
        journal.push(format!(
            "  [{:>2}] {:<16} stored={:<14} accrued={:<14} withdrawals={:<14} refunds={:<14}",
            i,
            format!("{:?}", op),
            h.balance(),
            h.accrued(),
            e.withdrawals,
            e.refunds,
        ));
    }

    let ctx = || {
        format!(
            "\n\n=== SHRUNK FAILING PERMUTATION ===\n{}\n\n\
             realised: {:?}\n\n\
             per-step journal:\n{}\n",
            render_trace(ops),
            realised,
            journal.join("\n"),
        )
    };

    let stored = h.balance();
    let e = h.earnings();

    // (1) The authoritative balance is exactly credits minus debits. This is
    //     the order-independence claim proper.
    assert_eq!(
        stored,
        realised.credited - realised.withdrawn,
        "stored MerchantBalance must equal credited - withdrawn, \
         independent of interleaving{}",
        ctx()
    );

    // (2) Accruals are monotonic: a withdrawal must never reduce them.
    assert_eq!(
        h.accrued(),
        realised.credited,
        "accruals must equal total credited; withdrawals must not touch accruals{}",
        ctx()
    );

    // (3) Withdrawals are monotonic and charges must never touch them.
    assert_eq!(
        e.withdrawals, realised.withdrawn,
        "earnings.withdrawals must equal total withdrawn{}",
        ctx()
    );

    // (4) No refund was ever issued in this trace, so the bucket must be clean.
    //     EXPECTED TO FAIL on current source — see module docs, bug (1).
    assert_eq!(
        e.refunds,
        0,
        "earnings.refunds must stay 0 when no refund was issued; \
         withdraw_merchant_funds_for_token wrongly credits it (merchant.rs:901-913){}",
        ctx()
    );

    // (5) The derived view must agree with the authoritative one.
    //     EXPECTED TO FAIL on current source — same root cause as (4).
    assert_eq!(
        h.computed_balance(),
        stored,
        "computed_balance (accruals - withdrawals - refunds) must equal stored \
         MerchantBalance; a mismatch means the derived ledger has desynced{}",
        ctx()
    );

    // (6) Balances are never negative under any ordering.
    assert!(
        stored >= 0,
        "stored balance must never go negative{}",
        ctx()
    );

    realised
}

/// Shuffle `ops` in place using a Fisher-Yates pass driven by `swaps`, so that
/// proptest can shrink the permutation itself rather than an opaque seed.
fn permute(ops: &mut [OpKind], swaps: &[usize]) {
    if ops.is_empty() {
        return;
    }
    for i in (1..ops.len()).rev() {
        let j = swaps.get(i).copied().unwrap_or(0) % (i + 1);
        ops.swap(i, j);
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        // Each case boots a fresh Soroban env and registers a contract, so
        // cases are expensive; 48 is a deliberate CI-time compromise.
        cases: 48,
        .. ProptestConfig::default()
    })]

    /// Core property: over a random permutation of a mixed charge/withdraw
    /// multiset, final balances and the derived earnings ledger depend only on
    /// which operations succeeded — never on their order.
    #[test]
    fn prop_withdraw_charge_order_independent(
        n_charges in 1usize..6,
        n_fixed in 0usize..4,
        n_all in 0usize..2,
        swaps in prop::collection::vec(0usize..64, 0..16),
    ) {
        let mut ops = std::vec::Vec::new();
        ops.extend(std::iter::repeat(OpKind::Charge).take(n_charges));
        ops.extend(std::iter::repeat(OpKind::WithdrawFixed).take(n_fixed));
        ops.extend(std::iter::repeat(OpKind::WithdrawAll).take(n_all));

        permute(&mut ops, &swaps);
        run_and_assert(&ops);
    }

    /// Sharper claim on a charge-only prefix: with every withdrawal deferred to
    /// the end, all charges are feasible, so the realised multiset is fully
    /// determined and the final balance must be exact.
    #[test]
    fn prop_charges_then_single_drain_is_exact(
        n_charges in 1usize..7,
    ) {
        let mut ops: std::vec::Vec<OpKind> =
            std::iter::repeat(OpKind::Charge).take(n_charges).collect();
        ops.push(OpKind::WithdrawAll);

        let realised = run_and_assert(&ops);

        prop_assert_eq!(
            realised.charges_applied as usize, n_charges,
            "every charge must succeed when withdrawals are deferred"
        );
        prop_assert_eq!(
            realised.credited, CHARGE_AMOUNT * n_charges as i128,
            "credited must be exactly n * CHARGE_AMOUNT at zero fee"
        );
        prop_assert_eq!(
            realised.withdrawn, realised.credited,
            "a terminal WithdrawAll must drain the full accumulated balance"
        );
    }
}

// ── Deterministic edge cases ────────────────────────────────────────────────

/// Edge case: withdraw with no idle interval between the credit and the debit.
#[test]
fn withdraw_immediately_after_charge() {
    let realised = run_and_assert(&[OpKind::Charge, OpKind::WithdrawAll]);

    assert_eq!(realised.charges_applied, 1);
    assert_eq!(realised.credited, CHARGE_AMOUNT);
    assert_eq!(
        realised.withdrawn, CHARGE_AMOUNT,
        "the full charge must be withdrawable in the same breath"
    );
}

/// Edge case: batch of charges accumulates, then a single full drain.
#[test]
fn batch_charge_then_drain() {
    let ops = [
        OpKind::Charge,
        OpKind::Charge,
        OpKind::Charge,
        OpKind::Charge,
        OpKind::WithdrawAll,
    ];
    let realised = run_and_assert(&ops);

    assert_eq!(realised.charges_applied, 4);
    assert_eq!(realised.credited, CHARGE_AMOUNT * 4);
    assert_eq!(realised.withdrawn, CHARGE_AMOUNT * 4);
}

/// Edge case: withdrawal against a zero balance, both before any charge and
/// again after a full drain. Neither may corrupt the ledger.
#[test]
fn withdraw_at_zero_balance() {
    // Withdraw-first: nothing has ever been credited.
    let realised = run_and_assert(&[OpKind::WithdrawFixed, OpKind::Charge]);
    assert_eq!(
        realised.withdrawals_refused, 1,
        "withdrawing from an untouched ledger must be refused"
    );
    assert_eq!(realised.withdrawn, 0);

    // Drain, then withdraw again against the emptied balance.
    let realised = run_and_assert(&[
        OpKind::Charge,
        OpKind::WithdrawAll,
        OpKind::WithdrawFixed,
    ]);
    assert_eq!(
        realised.withdrawals_refused, 1,
        "withdrawing from a drained ledger must be refused"
    );
    assert_eq!(realised.withdrawn, CHARGE_AMOUNT);
}

/// Interleaving is what distinguishes this from the existing drain tests:
/// charge, partial withdraw, charge, drain.
#[test]
fn interleaved_charge_withdraw_charge_drain() {
    let ops = [
        OpKind::Charge,
        OpKind::WithdrawFixed,
        OpKind::Charge,
        OpKind::WithdrawAll,
    ];
    let realised = run_and_assert(&ops);

    assert_eq!(realised.charges_applied, 2);
    assert_eq!(realised.credited, CHARGE_AMOUNT * 2);
    assert_eq!(
        realised.withdrawn,
        CHARGE_AMOUNT * 2,
        "a fixed withdraw plus a terminal drain must move the whole credit"
    );
}

// ── Minimal reproduction of the double-count bug ────────────────────────────

/// Minimal deterministic reproduction of the `refunds` double-count in
/// `withdraw_merchant_funds_for_token` (`merchant.rs:901-913`).
///
/// One charge, one full withdrawal, no refund anywhere. Expected:
/// `withdrawals == amount`, `refunds == 0`. Actual on current source:
/// both equal `amount`, so `computed_balance` is `-amount` before
/// `get_reconciliation_snapshot` clamps it to `0`.
///
/// Marked `#[ignore]` so it does not block CI while the bug is open. Remove the
/// attribute once `merchant.rs` drops the stray `earnings.refunds` write; it
/// then serves as the regression guard.
#[test]
#[ignore = "reproduces open bug: withdraw credits both refunds and withdrawals (merchant.rs:901-913)"]
fn xfail_refunds_double_count_on_withdraw() {
    let mut h = setup(1);
    let mut realised = Realised::default();

    h.apply(OpKind::Charge, &mut realised);
    h.apply(OpKind::WithdrawAll, &mut realised);

    let e = h.earnings();

    assert_eq!(
        e.withdrawals, CHARGE_AMOUNT,
        "the withdrawn amount belongs in the withdrawals bucket"
    );
    assert_eq!(
        e.refunds, 0,
        "no refund was issued, so refunds must remain 0 — but \
         withdraw_merchant_funds_for_token increments it alongside withdrawals"
    );
    assert_eq!(
        h.computed_balance(),
        h.balance(),
        "derived and stored balances must agree after a clean drain"
    );
}

// ── TotalAccounted: blocked, not skipped ────────────────────────────────────

/// The `TotalAccounted` half of the invariant the task asks for.
///
/// Currently unassertable: `src/accounting.rs` is not compiled into the crate
/// (no `mod accounting;` exists), and the inline `pub mod accounting` at
/// `lib.rs:327` stubs all three functions — `add_total_accounted` and
/// `sub_total_accounted` ignore their arguments, and `get_total_accounted`
/// always returns `0`. Every accounting call in the charge and withdraw paths
/// is therefore a no-op against `DataKey::TotalAccounted`.
///
/// Writing `assert_eq!(get_total_accounted(..), 0)` here would pass while
/// proving nothing, which is worse than an explicit gap. To re-enable:
///
/// 1. Add `mod accounting;` to `lib.rs` and delete the inline stub module.
/// 2. Drop the `#[ignore]` and the `return` below.
///
/// The assertion body is written out so it is ready to run.
#[test]
#[ignore = "blocked: src/accounting.rs is not wired into the crate; lib.rs:327 stubs it to a no-op"]
fn ignored_total_accounted_tracks_net_flow() {
    let mut h = setup(3);

    // Guard: while the stub is in place `get_total_accounted` is hardwired to 0,
    // so every assertion below would pass vacuously. Delete this block on
    // purpose when wiring `accounting.rs` in.
    if crate::accounting::get_total_accounted(&h.env, &h.token) == 0 {
        return;
    }

    let mut realised = Realised::default();

    for op in [
        OpKind::Charge,
        OpKind::WithdrawFixed,
        OpKind::Charge,
        OpKind::Charge,
        OpKind::WithdrawAll,
    ] {
        h.apply(op, &mut realised);
    }

    // TotalAccounted is a global anchor: credits raise it, withdrawals and
    // refunds lower it. After a full drain the merchant's contribution is zero.
    assert_eq!(
        crate::accounting::get_total_accounted(&h.env, &h.token),
        realised.credited - realised.withdrawn,
        "TotalAccounted must equal net credited-minus-withdrawn flow"
    );
}
