//! Property-based tests for charge interval accumulation and balance invariants.
//!
//! These tests use a pure-Rust model of the charge logic to verify the fund
//! conservation and balance invariants documented in docs/protocol_invariants.md.
//! The model mirrors the intended charge_subscription / deposit_funds semantics:
//!   - prepaid_balance never goes negative
//!   - last_payment_timestamp is monotonically non-decreasing
//!   - fund conservation: total_deposited == total_charged + remaining_prepaid_balance
//!   - a charge attempt while prepaid_balance < amount yields InsufficientBalance
//!   - interval guard prevents double-charges within the same period
//!
//! When the full contract implementation lands, these invariants should additionally
//! be verified against the live Soroban client (using Env::default() + contract client).

use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Minimal charge model (pure Rust, no Soroban env dependency)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum SubStatus {
    Active,
    InsufficientBalance,
}

#[derive(Debug, Clone)]
struct ChargeModel {
    prepaid_balance: i128,
    last_payment_timestamp: u64,
    interval_seconds: u64,
    amount: i128,
    total_deposited: i128,
    total_charged: i128,
    status: SubStatus,
}

impl ChargeModel {
    fn new(interval_seconds: u64, amount: i128) -> Self {
        Self {
            prepaid_balance: 0,
            last_payment_timestamp: 0,
            interval_seconds,
            amount,
            total_deposited: 0,
            total_charged: 0,
            status: SubStatus::Active,
        }
    }

    fn deposit(&mut self, amount: i128) {
        if amount <= 0 {
            return;
        }
        self.prepaid_balance = self.prepaid_balance.saturating_add(amount);
        self.total_deposited = self.total_deposited.saturating_add(amount);
    }

    /// Returns true if the charge succeeded.
    fn charge(&mut self, now: u64) -> bool {
        if self.status != SubStatus::Active {
            return false;
        }
        // Interval guard: INV-1
        if now < self.last_payment_timestamp.saturating_add(self.interval_seconds) {
            return false;
        }
        // Balance guard: INV-2
        if self.prepaid_balance < self.amount {
            self.status = SubStatus::InsufficientBalance;
            return false;
        }
        self.prepaid_balance -= self.amount;
        self.total_charged += self.amount;
        self.last_payment_timestamp = now;
        true
    }

    fn check_conservation(&self) -> bool {
        self.total_deposited == self.total_charged + self.prepaid_balance
    }
}

// ---------------------------------------------------------------------------
// Operation sequence type for proptest strategies
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    Deposit(i128),
    Charge(u64),
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (1i128..=5_000_000i128).prop_map(Op::Deposit),
        (0u64..=10_000_000u64).prop_map(Op::Charge),
    ]
}

// ---------------------------------------------------------------------------
// Property tests
// ---------------------------------------------------------------------------

proptest! {
    /// INV-2: prepaid_balance never goes negative across any deposit/charge sequence.
    #[test]
    fn prop_prepaid_balance_never_negative(
        interval in 60u64..=86_400u64,
        amount in 1i128..=1_000_000i128,
        ops in prop::collection::vec(arb_op(), 1..=60),
    ) {
        let mut sub = ChargeModel::new(interval, amount);
        for op in ops {
            match op {
                Op::Deposit(d) => sub.deposit(d),
                Op::Charge(now) => { sub.charge(now); }
            }
            prop_assert!(
                sub.prepaid_balance >= 0,
                "prepaid_balance went negative: {}",
                sub.prepaid_balance
            );
        }
    }

    /// INV-1 (timestamp): last_payment_timestamp is monotonically non-decreasing.
    #[test]
    fn prop_last_payment_timestamp_monotonic(
        interval in 60u64..=86_400u64,
        amount in 1i128..=1_000_000i128,
        charge_times in prop::collection::vec(0u64..=20_000_000u64, 1..=40),
    ) {
        let mut sub = ChargeModel::new(interval, amount);
        sub.deposit(amount.saturating_mul(100)); // ensure ample funds

        let mut prev_ts = 0u64;
        for now in charge_times {
            sub.charge(now);
            prop_assert!(
                sub.last_payment_timestamp >= prev_ts,
                "timestamp moved backward: {} < {}",
                sub.last_payment_timestamp,
                prev_ts
            );
            prev_ts = sub.last_payment_timestamp;
        }
    }

    /// Fund conservation: total_deposited == total_charged + remaining prepaid_balance.
    #[test]
    fn prop_fund_conservation(
        interval in 60u64..=86_400u64,
        amount in 1i128..=100_000i128,
        ops in prop::collection::vec(arb_op(), 1..=60),
    ) {
        let mut sub = ChargeModel::new(interval, amount);
        for op in ops {
            match op {
                Op::Deposit(d) => sub.deposit(d),
                Op::Charge(now) => { sub.charge(now); }
            }
            prop_assert!(
                sub.check_conservation(),
                "Fund conservation violated: deposited={} charged={} balance={}",
                sub.total_deposited,
                sub.total_charged,
                sub.prepaid_balance
            );
        }
    }

    /// A charge when prepaid_balance < amount must set InsufficientBalance
    /// and must NOT move any funds to the merchant.
    #[test]
    fn prop_insufficient_balance_blocks_charge(
        interval in 60u64..=86_400u64,
        amount in 2i128..=1_000_000i128,
        deposit in 1i128..=999_999i128,
        now in 86_400u64..=10_000_000u64,
    ) {
        prop_assume!(deposit < amount);
        let mut sub = ChargeModel::new(interval, amount);
        sub.deposit(deposit);

        let charged = sub.charge(now);

        prop_assert!(!charged, "charge should fail with insufficient balance");
        prop_assert_eq!(sub.status, SubStatus::InsufficientBalance);
        prop_assert_eq!(
            sub.prepaid_balance,
            deposit,
            "prepaid_balance must be unchanged on failed charge"
        );
        prop_assert_eq!(
            sub.total_charged,
            0,
            "no funds should reach merchant on failed charge"
        );
    }

    /// INV-1: A second charge within the same interval must not advance
    /// the balance or timestamp.
    #[test]
    fn prop_interval_guard_prevents_double_charge(
        interval in 60u64..=86_400u64,
        amount in 1i128..=1_000_000i128,
        first_now in 0u64..=10_000_000u64,
    ) {
        let mut sub = ChargeModel::new(interval, amount);
        sub.deposit(amount.saturating_mul(10));

        sub.charge(first_now);
        let balance_snapshot = sub.prepaid_balance;
        let charged_snapshot = sub.total_charged;
        let ts_snapshot = sub.last_payment_timestamp;

        // Retry immediately — interval has not elapsed
        sub.charge(first_now);

        prop_assert_eq!(
            sub.prepaid_balance,
            balance_snapshot,
            "balance changed on within-interval retry"
        );
        prop_assert_eq!(
            sub.total_charged,
            charged_snapshot,
            "funds moved on within-interval retry"
        );
        prop_assert_eq!(
            sub.last_payment_timestamp,
            ts_snapshot,
            "timestamp changed on within-interval retry"
        );
    }

    /// Many small intervals: repeated charges over a long timeline drain balance
    /// predictably and fund conservation holds throughout.
    #[test]
    fn prop_many_intervals_conservation(
        interval in 60u64..=3_600u64,
        amount in 1_000i128..=10_000i128,
        n_charges in 1usize..=50usize,
        initial_deposit in 100_000i128..=1_000_000i128,
    ) {
        let mut sub = ChargeModel::new(interval, amount);
        sub.deposit(initial_deposit);

        let mut t = interval;
        for _ in 0..n_charges {
            sub.charge(t);
            prop_assert!(sub.prepaid_balance >= 0, "negative balance after charge");
            prop_assert!(sub.check_conservation(), "conservation violated");
            t = t.saturating_add(interval);
        }
    }

    /// Alternating deposit/charge: fund conservation holds even when top-ups
    /// arrive between every charge attempt.
    #[test]
    fn prop_alternating_deposit_charge_conservation(
        interval in 60u64..=86_400u64,
        amount in 1i128..=10_000i128,
        topup in 1i128..=50_000i128,
        n_cycles in 1usize..=30usize,
    ) {
        let mut sub = ChargeModel::new(interval, amount);
        let mut t = interval;

        for _ in 0..n_cycles {
            sub.deposit(topup);
            sub.charge(t);
            prop_assert!(sub.prepaid_balance >= 0, "negative balance in cycle");
            prop_assert!(sub.check_conservation(), "conservation violated in cycle");
            t = t.saturating_add(interval);
        }
    }

    /// Large amount vs small balance: InsufficientBalance is returned exactly
    /// when prepaid_balance < charge amount, with zero funds moved.
    #[test]
    fn prop_large_amount_small_balance_no_partial_debit(
        interval in 60u64..=86_400u64,
        amount in 1_000_000i128..=100_000_000i128,
        deposit in 0i128..=999_999i128,
        now in 86_400u64..=10_000_000u64,
    ) {
        prop_assume!(deposit < amount);
        let mut sub = ChargeModel::new(interval, amount);
        sub.deposit(deposit);

        sub.charge(now);

        prop_assert_eq!(
            sub.total_charged,
            0,
            "partial debit must not occur"
        );
        prop_assert!(
            sub.prepaid_balance >= 0,
            "balance went negative on failed charge"
        );
        prop_assert!(sub.check_conservation());
    }
}
