#![cfg(test)]
extern crate std;

use super::*;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env};

const T0: u64 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;

struct BillingTestEnv {
    env: Env,
    client: SubscriptionVaultClient<'static>,
    admin: Address,
    token: Address,
}

impl BillingTestEnv {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|l| l.timestamp = T0);
        let admin = Address::generate(&env);
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        let token = env.register_stellar_asset_contract_v2(admin.clone()).address();
        client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
        Self { env, client, admin, token }
    }

    fn mint(&self, to: &Address, amount: &i128) {
        soroban_sdk::token::StellarAssetClient::new(&self.env, &self.token).mint(to, amount);
    }

    fn make_sub(&self, subscriber: &Address, merchant: &Address) -> u32 {
        self.client.create_subscription(
            subscriber, merchant, &1_000_000i128, &INTERVAL, &false, &None::<i128>, &None::<u64>,
        )
    }

    fn finalize(&self, sub_id: u32, period: u32, subscriber: &Address, merchant: &Address, charged: i128, mode: BillingStatementFinalization) {
        let t_end = self.env.ledger().timestamp();
        self.client.finalize_billing_statement(
            &self.admin, &sub_id, &period, subscriber, merchant,
            &(t_end - INTERVAL), &t_end,
            &PeriodStatementAmounts { total_amount_charged: charged, total_usage_units: 0, protocol_fee_amount: 0, net_amount_to_merchant: charged, refund_amount: 0 },
            &STMT_FLAG_INTERVAL_CHARGED,
            &mode,
        );
    }
}

#[test]
fn test_billing_statement_write_and_read() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_sub(&subscriber, &merchant);
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    te.finalize(sub_id, 0, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);
    let stmt = te.client.get_billing_statement(&sub_id, &0);
    assert_eq!(stmt.subscription_id, sub_id);
    assert_eq!(stmt.period_index, 0);
    assert_eq!(stmt.total_amount_charged, 1_000_000);
    assert_eq!(stmt.finalized_by, BillingStatementFinalization::PeriodClosed);
    assert_eq!(stmt.token, te.token);
    assert_eq!(stmt.status_flags, STMT_FLAG_INTERVAL_CHARGED);
}

#[test]
fn test_billing_statement_not_found_returns_error() {
    let te = BillingTestEnv::new();
    let result = te.client.try_get_billing_statement(&99, &0);
    assert!(matches!(result, Err(Ok(Error::NotFound))));
}

#[test]
fn test_billing_statement_upsert_overwrites_same_period() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_sub(&subscriber, &merchant);
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    te.finalize(sub_id, 0, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);
    te.finalize(sub_id, 0, &subscriber, &merchant, 2_000_000, BillingStatementFinalization::Cancellation);
    let stmt = te.client.get_billing_statement(&sub_id, &0);
    assert_eq!(stmt.total_amount_charged, 2_000_000, "overwrite applied");
    assert_eq!(stmt.finalized_by, BillingStatementFinalization::Cancellation);
    let page = te.client.get_bill_stmts_by_sub(&sub_id, &0, &10);
    assert_eq!(page.len(), 1, "no duplicate index entries");
}

#[test]
fn test_bill_stmts_by_sub_pagination() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_sub(&subscriber, &merchant);
    for p in 0u32..5 {
        te.env.ledger().with_mut(|l| l.timestamp = T0 + (p as u64 + 1) * INTERVAL);
        te.finalize(sub_id, p, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);
    }
    let all = te.client.get_bill_stmts_by_sub(&sub_id, &0, &10);
    assert_eq!(all.len(), 5);
    let p2 = te.client.get_bill_stmts_by_sub(&sub_id, &3, &2);
    assert_eq!(p2.len(), 2);
    assert_eq!(p2.get(0).unwrap().period_index, 3);
}

#[test]
fn test_bill_stmts_by_sub_empty() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_sub(&subscriber, &merchant);
    let page = te.client.get_bill_stmts_by_sub(&sub_id, &0, &10);
    assert_eq!(page.len(), 0);
}

#[test]
fn test_bill_stmts_by_merch_rng_filter() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_sub(&subscriber, &merchant);
    for p in 0u32..3 {
        te.env.ledger().with_mut(|l| l.timestamp = T0 + (p as u64 + 1) * INTERVAL);
        te.finalize(sub_id, p, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);
    }
    let mid = T0 + 2 * INTERVAL;
    let results = te.client.get_bill_stmts_by_merch_rng(&merchant, &mid, &mid, &0, &10);
    assert_eq!(results.len(), 1);
    assert_eq!(results.get(0).unwrap().period_index, 1);
}

#[test]
fn test_bill_stmts_by_merch_rng_all() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_sub(&subscriber, &merchant);
    for p in 0u32..4 {
        te.env.ledger().with_mut(|l| l.timestamp = T0 + (p as u64 + 1) * INTERVAL);
        te.finalize(sub_id, p, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);
    }
    let all = te.client.get_bill_stmts_by_merch_rng(&merchant, &0, &u64::MAX, &0, &10);
    assert_eq!(all.len(), 4);
}

#[test]
fn test_billing_statements_isolated_by_subscription() {
    let te = BillingTestEnv::new();
    let merchant = Address::generate(&te.env);
    let sub_a = Address::generate(&te.env);
    let sub_b = Address::generate(&te.env);
    let id_a = te.make_sub(&sub_a, &merchant);
    let id_b = te.make_sub(&sub_b, &merchant);
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    te.finalize(id_a, 0, &sub_a, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);
    te.env.ledger().with_mut(|l| l.timestamp = T0 + 2 * INTERVAL);
    te.finalize(id_a, 1, &sub_a, &merchant, 2_000_000, BillingStatementFinalization::PeriodClosed);
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    te.finalize(id_b, 0, &sub_b, &merchant, 3_000_000, BillingStatementFinalization::Cancellation);
    assert_eq!(te.client.get_bill_stmts_by_sub(&id_a, &0, &10).len(), 2);
    assert_eq!(te.client.get_bill_stmts_by_sub(&id_b, &0, &10).len(), 1);
}

#[test]
fn test_finalize_billing_statement_requires_admin() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_sub(&subscriber, &merchant);
    let stranger = Address::generate(&te.env);
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    let result = te.client.try_finalize_billing_statement(
        &stranger, &sub_id, &0, &subscriber, &merchant,
        &T0, &(T0 + INTERVAL),
        &PeriodStatementAmounts { total_amount_charged: 1_000_000, total_usage_units: 0, protocol_fee_amount: 0, net_amount_to_merchant: 1_000_000, refund_amount: 0 },
        &STMT_FLAG_INTERVAL_CHARGED,
        &BillingStatementFinalization::PeriodClosed,
    );
    assert!(matches!(result, Err(Ok(Error::Unauthorized))));
}

#[test]
fn test_status_flags_round_trip() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_sub(&subscriber, &merchant);
    let flags = STMT_FLAG_INTERVAL_CHARGED | STMT_FLAG_USAGE_CHARGED | STMT_FLAG_ONEOFF_CHARGED;
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    te.client.finalize_billing_statement(
        &te.admin, &sub_id, &0, &subscriber, &merchant,
        &T0, &(T0 + INTERVAL),
        &PeriodStatementAmounts { total_amount_charged: 3_000_000, total_usage_units: 100, protocol_fee_amount: 0, net_amount_to_merchant: 3_000_000, refund_amount: 0 },
        &flags,
        &BillingStatementFinalization::PeriodClosed,
    );
    let stmt = te.client.get_billing_statement(&sub_id, &0);
    assert_eq!(stmt.status_flags, flags);
    assert_eq!(stmt.total_usage_units, 100);
}

#[test]
fn test_period_statements_coexist_with_charge_audit_log() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    te.mint(&subscriber, &100_000_000);
    let sub_id = te.make_sub(&subscriber, &merchant);
    te.client.deposit_funds(&sub_id, &subscriber, &10_000_000);
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL + 1);
    te.client.charge_subscription(&sub_id);
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL + 2);
    te.finalize(sub_id, 0, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);
    let charge_rows = te.client.get_sub_statements_offset(&sub_id, &0, &10, &false);
    assert_eq!(charge_rows.statements.len(), 1);
    let period_stmt = te.client.get_billing_statement(&sub_id, &0);
    assert_eq!(period_stmt.total_amount_charged, 1_000_000);
}

#[test]
fn test_compaction_does_not_remove_period_statements() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    te.mint(&subscriber, &100_000_000);
    let sub_id = te.make_sub(&subscriber, &merchant);
    te.client.deposit_funds(&sub_id, &subscriber, &50_000_000);
    for i in 1u64..=5 {
        te.env.ledger().with_mut(|l| l.timestamp = T0 + i * INTERVAL + 1);
        te.client.charge_subscription(&sub_id);
    }
    for p in 0u32..5 {
        te.env.ledger().with_mut(|l| l.timestamp = T0 + (p as u64 + 1) * INTERVAL + 2);
        te.finalize(sub_id, p, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);
    }
    te.client.compact_billing_statements(&te.admin, &sub_id, &Some(2u32));
    let charge_page = te.client.get_sub_statements_offset(&sub_id, &0, &10, &false);
    assert_eq!(charge_page.statements.len(), 2, "per-charge rows compacted to 2");
    let period_page = te.client.get_bill_stmts_by_sub(&sub_id, &0, &10);
    assert_eq!(period_page.len(), 5, "period statements untouched by compaction");
}

#[test]
fn test_period_statement_high_volume_full_walk() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_sub(&subscriber, &merchant);
    let total = 20u32;
    for p in 0..total {
        te.env.ledger().with_mut(|l| l.timestamp = T0 + (p as u64 + 1) * INTERVAL);
        te.finalize(sub_id, p, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);
    }
    let mut seen = std::vec::Vec::new();
    let mut cur = 0u32;
    loop {
        let page = te.client.get_bill_stmts_by_sub(&sub_id, &cur, &7);
        if page.len() == 0 { break; }
        for i in 0..page.len() { seen.push(page.get(i).unwrap().period_index); }
        cur += page.len();
    }
    assert_eq!(seen.len(), total as usize);
}
