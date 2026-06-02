#![cfg(test)]

extern crate alloc;

use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env,
};
use subscription_vault::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient as TokenAdminClient};
use std::vec::Vec;

#[derive(Clone, Debug)]
pub enum Op {
    AdvanceTime(u64),
    Deposit { sub_idx: usize, amount: i128 },
    ChargeInterval { sub_idx: usize },
    ChargeUsage { sub_idx: usize, amount: i128 },
    ChargeOneOff { sub_idx: usize, amount: i128 },
    Withdraw { merchant_idx: usize, amount: i128, withdraw_all: bool, withdraw_zero: bool },
    Refund { merchant_idx: usize, sub_idx: usize, amount: i128, refund_all: bool },
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        (1..10u64).prop_map(|d| Op::AdvanceTime(d * 86400)),
        (0..5usize, 100..5000i128).prop_map(|(s, a)| Op::Deposit { sub_idx: s, amount: a }),
        (0..5usize).prop_map(|s| Op::ChargeInterval { sub_idx: s }),
        (0..5usize, 1..2000i128).prop_map(|(s, a)| Op::ChargeUsage { sub_idx: s, amount: a }),
        (0..5usize, 1..2000i128).prop_map(|(s, a)| Op::ChargeOneOff { sub_idx: s, amount: a }),
        (0..3usize, 0..5000i128, any::<bool>(), any::<bool>()).prop_map(|(m, a, all, z)| Op::Withdraw { merchant_idx: m, amount: a, withdraw_all: all, withdraw_zero: z }),
        (0..3usize, 0..5usize, 0..5000i128, any::<bool>()).prop_map(|(m, s, a, all)| Op::Refund { merchant_idx: m, sub_idx: s, amount: a, refund_all: all }),
    ]
}

fn setup_env<'a>() -> (
    Env,
    SubscriptionVaultClient<'a>,
    TokenClient<'a>,
    TokenAdminClient<'a>,
    Address,
    Vec<Address>,
    Vec<Address>,
    Vec<u32>,
) {
    let env = Env::default();
    env.mock_all_auths();
    
    env.ledger().set_timestamp(1_000_000);

    let token_admin = Address::generate(&env);
    let contract_address = env.register_stellar_asset_contract_v2(token_admin.clone()).address();
    let token = TokenClient::new(&env, &contract_address);
    let token_admin_client = TokenAdminClient::new(&env, &contract_address);

    let admin = Address::generate(&env);
    let vault_id = env.register(SubscriptionVault, ());
    let vault = SubscriptionVaultClient::new(&env, &vault_id);

    let min_topup = 100;
    let grace_period = 3 * 86400;

    vault.init(
        &token.address,
        &7,
        &admin,
        &min_topup,
        &grace_period,
    );

    let mut merchants = Vec::new();
    for _ in 0..3 {
        merchants.push(Address::generate(&env));
    }

    let mut subscribers = Vec::new();
    let mut sub_ids = Vec::new();

    for i in 0..5 {
        let sub = Address::generate(&env);
        token_admin_client.mint(&sub, &100_000_000_000);
        subscribers.push(sub.clone());

        let merchant = &merchants[i % merchants.len()];
        let amount = 1000;
        let interval = 86400 * 30;
        let usage_enabled = true;

        let sub_id = vault.create_subscription(
            &sub,
            merchant,
            &amount,
            &interval,
            &usage_enabled,
            &None,
            &None,
        );
        sub_ids.push(sub_id);
        
        vault.deposit_funds(&sub_id, &sub, &50_000);
    }

    (env, vault, token, token_admin_client, admin, merchants, subscribers, sub_ids)
}

proptest! {
    #![proptest_config(Config {
        cases: 250,
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource("merchant_invariant_failures"))),
        .. Config::default()
    })]

    #[test]
    fn test_merchant_earnings_invariant(ops in prop::collection::vec(op_strategy(), 15..100)) {
        let (env, vault, token, _token_admin, _admin, merchants, subscribers, sub_ids) = setup_env();

        for op in ops {
            match op {
                Op::AdvanceTime(secs) => {
                    env.ledger().set_timestamp(env.ledger().timestamp() + secs);
                }
                Op::Deposit { sub_idx, amount } => {
                    let sub_id = sub_ids[sub_idx];
                    let subscriber = &subscribers[sub_idx];
                    let _ = vault.try_deposit_funds(&sub_id, subscriber, &amount);
                }
                Op::ChargeInterval { sub_idx } => {
                    let sub_id = sub_ids[sub_idx];
                    let _ = vault.try_charge_subscription(&sub_id);
                }
                Op::ChargeUsage { sub_idx, amount } => {
                    let sub_id = sub_ids[sub_idx];
                    let _ = vault.try_charge_usage(&sub_id, &amount);
                }
                Op::ChargeOneOff { sub_idx, amount } => {
                    let sub_id = sub_ids[sub_idx];
                    let merchant = &merchants[sub_idx % merchants.len()];
                    let _ = vault.try_charge_one_off(&sub_id, merchant, &amount);
                }
                Op::Withdraw { merchant_idx, amount, withdraw_all, withdraw_zero } => {
                    let merchant = &merchants[merchant_idx];
                    let mut withdraw_amt = amount;
                    
                    if withdraw_zero {
                        withdraw_amt = 0;
                    } else if withdraw_all {
                        withdraw_amt = vault.get_merchant_balance(merchant);
                    }
                    
                    let _ = vault.try_withdraw_merchant_funds(merchant, &withdraw_amt);
                }
                Op::Refund { merchant_idx, sub_idx, amount, refund_all } => {
                    let merchant = &merchants[merchant_idx];
                    let subscriber = &subscribers[sub_idx];
                    let mut refund_amt = amount;
                    
                    if refund_all {
                        refund_amt = vault.get_merchant_balance(merchant);
                    }
                    
                    let _ = vault.try_merchant_refund(merchant, subscriber, &token.address, &refund_amt);
                }
            }

            for merchant in &merchants {
                let balance = vault.get_merchant_balance(merchant);
                let earnings = vault.get_merchant_token_earnings(merchant, &token.address);

                let accruals = earnings.accruals;
                let total_accruals = accruals.interval + accruals.usage + accruals.one_off;
                
                let computed_balance = total_accruals - earnings.withdrawals - earnings.refunds;

                assert_eq!(
                    balance, computed_balance,
                    "Invariant failed for merchant {:?}!\n\
                     stored balance: {}\n\
                     computed: {}\n\
                     (interval: {}, usage: {}, one_off: {})\n\
                     withdrawals: {}\n\
                     refunds: {}",
                    merchant, balance, computed_balance,
                    accruals.interval, accruals.usage, accruals.one_off,
                    earnings.withdrawals, earnings.refunds
                );
                
                let snapshot = vault.get_reconciliation_snapshot(merchant);
                if let Some(token_snap) = snapshot.into_iter().find(|s| s.token == token.address) {
                    assert_eq!(
                        balance, token_snap.computed_balance,
                        "Reconciliation snapshot 'computed_balance' drifted from actual balance"
                    );
                    assert_eq!(
                        total_accruals, token_snap.total_accruals,
                        "Reconciliation snapshot 'total_accruals' drifted from combined sum"
                    );
                }
            }
        }
    }
}