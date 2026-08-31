#![cfg(test)]

extern crate alloc;

use rand::{rngs::StdRng, Rng, SeedableRng};
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient as TokenAdminClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env,
};
use std::fs::OpenOptions;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::vec::Vec;
use subscription_vault::{SubscriptionVault, SubscriptionVaultClient};

const MASTER_SEED: u64 = 0x9360_2026_08_31;
const SEQUENCE_COUNT: usize = 256;

#[derive(Clone, Debug)]
enum Op {
    AdvanceTime(u64),
    Deposit { sub_idx: usize, amount: i128 },
    ChargeInterval { sub_idx: usize },
    ChargeUsage { sub_idx: usize, amount: i128 },
    ChargeOneOff { sub_idx: usize, amount: i128 },
    Withdraw {
        merchant_idx: usize,
        amount: i128,
        withdraw_all: bool,
        withdraw_zero: bool,
    },
    Refund {
        merchant_idx: usize,
        sub_idx: usize,
        amount: i128,
        refund_all: bool,
    },
}

fn seeded_ops(rng: &mut StdRng) -> Vec<Op> {
    let len = rng.gen_range(20..80);
    let mut ops = Vec::with_capacity(len);

    for _ in 0..len {
        let amount = || rng.gen_range(1u64..5_000) as i128;
        let op = match rng.gen_range(0..7) {
            0 => Op::AdvanceTime(rng.gen_range(1u64..10) * 86_400),
            1 => Op::Deposit {
                sub_idx: rng.gen_range(0..5),
                amount: rng.gen_range(100u64..5_000) as i128,
            },
            2 => Op::ChargeInterval {
                sub_idx: rng.gen_range(0..5),
            },
            3 => Op::ChargeUsage {
                sub_idx: rng.gen_range(0..5),
                amount: amount(),
            },
            4 => Op::ChargeOneOff {
                sub_idx: rng.gen_range(0..5),
                amount: amount(),
            },
            5 => Op::Withdraw {
                merchant_idx: rng.gen_range(0..3),
                amount: rng.gen_range(0u64..5_000) as i128,
                withdraw_all: rng.gen(),
                withdraw_zero: rng.gen(),
            },
            _ => Op::Refund {
                merchant_idx: rng.gen_range(0..3),
                sub_idx: rng.gen_range(0..5),
                amount: rng.gen_range(0u64..5_000) as i128,
                refund_all: rng.gen(),
            },
        };
        ops.push(op);
    }

    ops
}

fn setup_env<'a>() -> (
    Env,
    SubscriptionVaultClient<'a>,
    TokenClient<'a>,
    Vec<Address>,
    Vec<Address>,
    Vec<u32>,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000_000);

    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token = TokenClient::new(&env, &token_address);
    let token_admin_client = TokenAdminClient::new(&env, &token_address);

    let admin = Address::generate(&env);
    let vault_id = env.register(SubscriptionVault, ());
    let vault = SubscriptionVaultClient::new(&env, &vault_id);
    vault.init(&token.address, &7, &admin, &100, &(3 * 86_400));

    let mut merchants = Vec::with_capacity(3);
    let redirect_url = soroban_sdk::String::from_str(&env, "https://example.com");
    for _ in 0..3 {
        let merchant = Address::generate(&env);
        vault.initialize_merchant_config(
            &merchant,
            &merchant,
            &0,
            &0x1F,
            &None,
            &redirect_url,
        );
        merchants.push(merchant);
    }

    let mut subscribers = Vec::with_capacity(5);
    let mut sub_ids = Vec::with_capacity(5);
    for i in 0..5 {
        let subscriber = Address::generate(&env);
        token_admin_client.mint(&subscriber, &100_000_000_000);
        subscribers.push(subscriber.clone());

        let merchant = &merchants[i % merchants.len()];
        let sub_id = vault.create_subscription(
            &subscriber,
            merchant,
            &1_000,
            &(30 * 86_400),
            &true,
            &None,
            &None,
            &None::<u32>,
            &None::<soroban_sdk::Symbol>,
        );
        vault.deposit_funds(&sub_id, &subscriber, &50_000, &None);
        sub_ids.push(sub_id);
    }

    (env, vault, token, merchants, subscribers, sub_ids)
}

fn assert_merchant_invariant(
    vault: &SubscriptionVaultClient,
    merchant: &Address,
    token: &TokenClient,
) {
    let balance = vault.get_merchant_balance_by_token(merchant, &token.address);
    let earnings = vault.get_merchant_token_earnings(merchant, &token.address);
    let total_accruals = earnings
        .accruals
        .interval
        .checked_add(earnings.accruals.usage)
        .and_then(|v| v.checked_add(earnings.accruals.one_off))
        .expect("accrual total overflowed in test oracle");
    let computed_balance = total_accruals
        .checked_sub(earnings.withdrawals)
        .and_then(|v| v.checked_sub(earnings.refunds))
        .expect("reconciliation total underflowed in test oracle");

    assert_eq!(
        balance, computed_balance,
        "MerchantBalance invariant failed: balance={} computed={} interval={} usage={} one_off={} withdrawals={} refunds={}",
        balance,
        computed_balance,
        earnings.accruals.interval,
        earnings.accruals.usage,
        earnings.accruals.one_off,
        earnings.withdrawals,
        earnings.refunds,
    );

    let snapshots = vault.get_reconciliation_snapshot(merchant);
    let snapshot = snapshots
        .into_iter()
        .find(|snapshot| snapshot.token == token.address)
        .expect("touched token missing from reconciliation snapshot");

    assert_eq!(snapshot.total_accruals, total_accruals);
    assert_eq!(snapshot.total_withdrawals, earnings.withdrawals);
    assert_eq!(snapshot.total_refunds, earnings.refunds);
    assert_eq!(snapshot.computed_balance, computed_balance);
}

fn run_sequence(seed: u64) {
    let (env, vault, token, merchants, subscribers, sub_ids) = setup_env();
    let mut rng = StdRng::seed_from_u64(seed);

    for op in seeded_ops(&mut rng) {
        match op {
            Op::AdvanceTime(secs) => {
                env.ledger().set_timestamp(env.ledger().timestamp() + secs);
            }
            Op::Deposit { sub_idx, amount } => {
                let _ = vault.try_deposit_funds(
                    &sub_ids[sub_idx],
                    &subscribers[sub_idx],
                    &amount,
                    &None,
                );
            }
            Op::ChargeInterval { sub_idx } => {
                let _ = vault.try_charge_subscription(&sub_ids[sub_idx], &None);
            }
            Op::ChargeUsage { sub_idx, amount } => {
                let _ = vault.try_charge_usage(&sub_ids[sub_idx], &amount);
            }
            Op::ChargeOneOff { sub_idx, amount } => {
                let merchant = &merchants[sub_idx % merchants.len()];
                let _ = vault.try_charge_one_off(&sub_ids[sub_idx], merchant, &amount, &None);
            }
            Op::Withdraw {
                merchant_idx,
                amount,
                withdraw_all,
                withdraw_zero,
            } => {
                let merchant = &merchants[merchant_idx];
                let withdraw_amount = if withdraw_zero {
                    0
                } else if withdraw_all {
                    vault.get_merchant_balance_by_token(merchant, &token.address)
                } else {
                    amount
                };
                let _ = vault.try_withdraw_merchant_funds(merchant, &withdraw_amount);
            }
            Op::Refund {
                merchant_idx,
                sub_idx,
                amount,
                refund_all,
            } => {
                let merchant = &merchants[merchant_idx];
                let refund_amount = if refund_all {
                    vault.get_merchant_balance_by_token(merchant, &token.address)
                } else {
                    amount
                };
                let _ = vault.try_merchant_refund(
                    merchant,
                    &subscribers[sub_idx],
                    &token.address,
                    &refund_amount,
                );
            }
        }

        for merchant in &merchants {
            assert_merchant_invariant(&vault, merchant, &token);
        }
    }
}

#[test]
fn merchant_earnings_invariant_256_seeded_sequences() {
    let mut seed_rng = StdRng::seed_from_u64(MASTER_SEED);

    for sequence in 0..SEQUENCE_COUNT {
        let seed = seed_rng.gen::<u64>();
        let result = catch_unwind(AssertUnwindSafe(|| run_sequence(seed)));

        if let Err(payload) = result {
            let path = format!(
                "{}/../../tests/fixtures/merchant_invariant_failures.txt",
                env!("CARGO_MANIFEST_DIR")
            );
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .expect("failed to persist merchant invariant regression seed");
            writeln!(file, "sequence={} seed={}", sequence, seed)
                .expect("failed to persist merchant invariant regression seed");
            std::panic::resume_unwind(payload);
        }
    }
}

#[test]
fn merchant_invariant_boundary_and_invalid_operations() {
    let (env, vault, token, merchants, subscribers, sub_ids) = setup_env();
    let merchant = &merchants[0];

    // Empty bucket: refund and withdrawal are rejected without changing accounting.
    assert!(vault.try_withdraw_merchant_funds(merchant, &0).is_err());
    assert!(vault
        .try_merchant_refund(merchant, &subscribers[0], &token.address, &1)
        .is_err());
    assert_merchant_invariant(&vault, merchant, &token);

    // Create one accrual, then refund exactly the remaining merchant balance.
    env.ledger().set_timestamp(env.ledger().timestamp() + 31 * 86_400);
    vault.charge_subscription(&sub_ids[0], &None);
    let accrued = vault.get_merchant_balance_by_token(merchant, &token.address);
    assert!(accrued > 0);

    vault.merchant_refund(merchant, &subscribers[0], &token.address, &accrued);
    assert_eq!(vault.get_merchant_balance_by_token(merchant, &token.address), 0);
    assert_merchant_invariant(&vault, merchant, &token);

    // Repeating the same refund is rejected; accounting must remain unchanged.
    assert!(vault
        .try_merchant_refund(merchant, &subscribers[0], &token.address, &accrued)
        .is_err());
    assert_merchant_invariant(&vault, merchant, &token);
}
