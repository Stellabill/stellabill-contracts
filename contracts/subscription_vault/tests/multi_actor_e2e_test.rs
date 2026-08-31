#![cfg(test)]

extern crate alloc;

use soroban_sdk::token::{Client as TokenClient, StellarAssetClient as TokenAdminClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env,
};
use subscription_vault::{SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient};

fn create_token_contract<'a>(
    env: &Env,
    admin: &Address,
) -> (TokenClient<'a>, TokenAdminClient<'a>) {
    let contract_address = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    (
        TokenClient::new(env, &contract_address),
        TokenAdminClient::new(env, &contract_address),
    )
}

fn assert_merchant_reconciliation(
    vault: &SubscriptionVaultClient,
    merchant: &Address,
    token: &TokenClient,
) {
    let balance = vault.get_merchant_balance_by_token(merchant, &token.address);
    let earnings = vault.get_merchant_token_earnings(merchant, &token.address);
    let total_accruals = earnings.accruals.interval
        + earnings.accruals.usage
        + earnings.accruals.one_off;
    let computed_balance = total_accruals - earnings.withdrawals - earnings.refunds;

    assert_eq!(balance, computed_balance);

    let snapshot = vault
        .get_reconciliation_snapshot(merchant)
        .into_iter()
        .find(|entry| entry.token == token.address)
        .expect("merchant token missing from reconciliation snapshot");
    assert_eq!(snapshot.total_accruals, total_accruals);
    assert_eq!(snapshot.total_withdrawals, earnings.withdrawals);
    assert_eq!(snapshot.total_refunds, earnings.refunds);
    assert_eq!(snapshot.computed_balance, computed_balance);
}

#[test]
fn test_multi_actor_e2e_flow() {
    let env = Env::default();
    env.mock_all_auths();

    // 1. SAC Token Setup
    let token_admin = Address::generate(&env);
    let (token, token_admin_client) = create_token_contract(&env, &token_admin);

    // 2. Actor Initialization
    let admin = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Give subscriber some initial tokens
    let initial_mint = 10_000_000_000; // 1000 tokens
    token_admin_client.mint(&subscriber, &initial_mint);

    // Deploy and Init Vault
    let vault_id = env.register(SubscriptionVault, ());
    let vault = SubscriptionVaultClient::new(&env, &vault_id);

    let min_topup = 1_000_000; // 0.1 tokens
    let grace_period = 3 * 24 * 60 * 60; // 3 days

    // Initialize the vault contract
    vault.init(&token.address, &7, &admin, &min_topup, &grace_period);

    // Initialize merchant config
    let redirect_url = soroban_sdk::String::from_str(&env, "https://example.com");
    vault.initialize_merchant_config(&merchant, &merchant, &0, &0x1F, &None, &redirect_url);

    // Pre-assertions
    assert_eq!(token.balance(&subscriber), initial_mint);
    assert_eq!(token.balance(&vault_id), 0);

    // Step 1: `create` subscription
    let amount = 5_000_000; // 0.5 tokens per interval
    let interval_seconds = 30 * 24 * 60 * 60; // 30 days
    let usage_enabled = false;

    let sub_id = vault.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval_seconds,
        &usage_enabled,
        &None,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    let sub_state = vault.get_subscription(&sub_id);
    assert_eq!(sub_state.status, SubscriptionStatus::Active);
    assert_eq!(sub_state.prepaid_balance, 0);

    // Step 2: `deposit` funds
    let deposit_amount = 15_000_000; // Covers 3 intervals
    vault.deposit_funds(&sub_id, &subscriber, &deposit_amount, &None);

    assert_eq!(token.balance(&subscriber), initial_mint - deposit_amount);
    assert_eq!(token.balance(&vault_id), deposit_amount);

    let sub_state = vault.get_subscription(&sub_id);
    assert_eq!(sub_state.prepaid_balance, deposit_amount);
    assert_eq!(vault.get_merchant_balance(&merchant), 0);

    // Step 3: `charge` (Simulating Time Passing)
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + interval_seconds + 1);
    vault.charge_subscription(&sub_id, &None);

    let sub_state = vault.get_subscription(&sub_id);
    assert_eq!(sub_state.prepaid_balance, deposit_amount - amount);
    assert_eq!(vault.get_merchant_balance(&merchant), amount);
    assert_eq!(token.balance(&vault_id), deposit_amount);
    assert_merchant_reconciliation(&vault, &merchant, &token);

    // Second charge
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + interval_seconds + 1);
    vault.charge_subscription(&sub_id, &None);

    let sub_state = vault.get_subscription(&sub_id);
    assert_eq!(sub_state.prepaid_balance, deposit_amount - 2 * amount);
    assert_eq!(vault.get_merchant_balance(&merchant), 2 * amount);
    assert_eq!(token.balance(&vault_id), deposit_amount);
    assert_merchant_reconciliation(&vault, &merchant, &token);

    // Step 4: `withdraw_merchant_funds` (Partial Withdrawal)
    let partial_withdraw = 3_000_000;
    vault.withdraw_merchant_funds(&merchant, &partial_withdraw);

    assert_eq!(token.balance(&merchant), partial_withdraw);
    assert_eq!(
        vault.get_merchant_balance(&merchant),
        2 * amount - partial_withdraw
    );
    assert_eq!(token.balance(&vault_id), deposit_amount - partial_withdraw);
    assert_merchant_reconciliation(&vault, &merchant, &token);

    // Refund part of the merchant's remaining balance. This exercises the
    // refunds leg of the reconciliation equation independently of cancellation.
    let merchant_refund = 1_000_000;
    vault.merchant_refund(&merchant, &subscriber, &token.address, &merchant_refund);
    assert_eq!(
        vault.get_merchant_balance(&merchant),
        2 * amount - partial_withdraw - merchant_refund
    );
    assert_merchant_reconciliation(&vault, &merchant, &token);

    // Step 5: `cancel_subscription` — automatically refunds remaining prepaid balance
    let subscriber_balance_before_cancel = token.balance(&subscriber);
    let vault_balance_before_cancel = token.balance(&vault_id);
    let sub_before_cancel = vault.get_subscription(&sub_id);
    let expected_refund = sub_before_cancel.prepaid_balance;

    vault.cancel_subscription(&sub_id, &subscriber);

    let sub_state = vault.get_subscription(&sub_id);
    assert_eq!(sub_state.status, SubscriptionStatus::Cancelled);
    assert_eq!(sub_state.prepaid_balance, 0);

    // Subscriber received the refund
    assert_eq!(
        token.balance(&subscriber),
        subscriber_balance_before_cancel + expected_refund
    );
    // Vault no longer holds the refunded amount
    assert_eq!(
        token.balance(&vault_id),
        vault_balance_before_cancel - expected_refund
    );

    // Vault balance should now exactly match the merchant's unwithdrawn funds,
    // and the merchant accounting equation must still reconcile.
    assert_eq!(
        token.balance(&vault_id),
        vault.get_merchant_balance(&merchant)
    );
    assert_merchant_reconciliation(&vault, &merchant, &token);
}
