#![cfg(test)]

extern crate alloc;

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env,
};
use subscription_vault::{SubscriptionVault, SubscriptionVaultClient, SubscriptionStatus};
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient as TokenAdminClient};

fn create_token_contract<'a>(
    env: &Env,
    admin: &Address,
) -> (TokenClient<'a>, TokenAdminClient<'a>) {
    let contract_address = env.register_stellar_asset_contract_v2(admin.clone()).address();
    (
        TokenClient::new(env, &contract_address),
        TokenAdminClient::new(env, &contract_address),
    )
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
    vault.init(
        &token.address,
        &7,
        &admin,
        &min_topup,
        &grace_period,
    );

    // Pre-assertions
    assert_eq!(token.balance(&subscriber), initial_mint);
    assert_eq!(token.balance(&vault_id), 0);

    // Step 1: `create` subscription
    let amount = 5_000_000; // 0.5 tokens per interval
    let interval_seconds = 30 * 24 * 60 * 60; // 30 days
    let usage_enabled = false;

    let sub_id =     vault.create_subscription(
        &subscriber,
        &merchant,
        &amount,
        &interval_seconds,
        &usage_enabled,
        &None,
        &None::<u64>,
    );

    let sub_state = vault.get_subscription(&sub_id);
    assert_eq!(sub_state.status, SubscriptionStatus::Active);
    assert_eq!(sub_state.prepaid_balance, 0);

    // Step 2: `deposit` funds
    let deposit_amount = 15_000_000; // Covers 3 intervals
    vault.deposit_funds(&sub_id, &subscriber, &deposit_amount);

    assert_eq!(token.balance(&subscriber), initial_mint - deposit_amount);
    assert_eq!(token.balance(&vault_id), deposit_amount);
    
    let sub_state = vault.get_subscription(&sub_id);
    assert_eq!(sub_state.prepaid_balance, deposit_amount);
    assert_eq!(vault.get_merchant_balance(&merchant), 0);

    // Step 3: `charge` (Simulating Time Passing)
    // First charge
    env.ledger().set_timestamp(env.ledger().timestamp() + interval_seconds + 1);
    vault.charge_subscription(&sub_id);

    let sub_state = vault.get_subscription(&sub_id);
    assert_eq!(sub_state.prepaid_balance, deposit_amount - amount);
    assert_eq!(vault.get_merchant_balance(&merchant), amount);
    assert_eq!(token.balance(&vault_id), deposit_amount); // Total tokens in vault remains the same

    // Second charge
    env.ledger().set_timestamp(env.ledger().timestamp() + interval_seconds + 1);
    vault.charge_subscription(&sub_id);

    let sub_state = vault.get_subscription(&sub_id);
    assert_eq!(sub_state.prepaid_balance, deposit_amount - 2 * amount);
    assert_eq!(vault.get_merchant_balance(&merchant), 2 * amount);
    assert_eq!(token.balance(&vault_id), deposit_amount);

    // Step 4: `withdraw_merchant_funds` (Partial Withdrawal)
    let partial_withdraw = 3_000_000;
    vault.withdraw_merchant_funds(&merchant, &partial_withdraw);

    assert_eq!(token.balance(&merchant), partial_withdraw);
    assert_eq!(vault.get_merchant_balance(&merchant), 2 * amount - partial_withdraw);
    assert_eq!(token.balance(&vault_id), deposit_amount - partial_withdraw);

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
    
    // Vault balance should now exactly match the merchant's unwithdrawn funds
    assert_eq!(token.balance(&vault_id), vault.get_merchant_balance(&merchant));
}
