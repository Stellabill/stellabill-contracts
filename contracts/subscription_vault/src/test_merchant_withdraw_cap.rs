#![cfg(test)]

use crate::{
    types::{DataKey, Error, DEFAULT_ALLOWED_OPS},
    SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env, Symbol,
};

const INTERVAL: u64 = 30 * 24 * 60 * 60;
const AMOUNT: i128 = 10_000_000;
const DEPOSIT_AMOUNT: i128 = 50_000_000;

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

    // Init with 6 decimals.
    // Default cap will be initialized to 10,000 * 10^6 = 10_000_000_000.
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    (env, client, token, admin)
}

fn create_and_fund_sub(
    env: &Env,
    client: &SubscriptionVaultClient,
    token: &Address,
) -> (u32, Address, Address) {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);

    client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0,
        &DEFAULT_ALLOWED_OPS,
        &None,
        &soroban_sdk::String::from_str(env, "https://example.com"),
    );

    token::StellarAssetClient::new(env, token).mint(&subscriber, &DEPOSIT_AMOUNT);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    client.deposit_funds(&id, &subscriber, &DEPOSIT_AMOUNT, &None);
    (id, subscriber, merchant)
}

#[test]
fn test_default_withdraw_cap_enforced() {
    let (env, client, token, admin) = setup();
    let (_, _, merchant) = create_and_fund_sub(&env, &client, &token);

    // Check initial default cap: 10_000 * 10^6 = 10_000_000_000.
    assert_eq!(client.get_default_merchant_cap(), Some(10_000_000_000));

    // Admin sets default cap to 15_000_000 (15 units).
    client.set_default_merchant_cap(&admin, &Some(15_000_000));
    assert_eq!(client.get_default_merchant_cap(), Some(15_000_000));

    // Seed merchant balance to 20_000_000.
    env.as_contract(&client.address, || {
        env.storage().instance().set(
            &DataKey::MerchantBalance(merchant.clone(), token.clone()),
            &20_000_000i128,
        );
        let token_key = DataKey::MerchantTokens(merchant.clone());
        let mut tokens = soroban_sdk::Vec::new(&env);
        tokens.push_back(token.clone());
        env.storage().instance().set(&token_key, &tokens);
    });

    // Withdrawal of 10_000_000 should succeed (within 15_000_000 cap).
    client.withdraw_merchant_funds(&merchant, &10_000_000);
    assert_eq!(client.get_merchant_balance(&merchant), 10_000_000);

    // Subsequent withdrawal of 6_000_000 should exceed the cap (total 16_000_000 > 15_000_000).
    let res = client.try_withdraw_merchant_funds(&merchant, &6_000_000);
    assert_eq!(res, Err(Ok(Error::WithdrawCapExceeded)));
}

#[test]
fn test_per_merchant_override_enforced() {
    let (env, client, token, admin) = setup();
    let (_, _, merchant) = create_and_fund_sub(&env, &client, &token);

    // Admin sets default cap to 10_000_000.
    client.set_default_merchant_cap(&admin, &Some(10_000_000));
    // Admin overrides this merchant's cap to 30_000_000.
    client.set_merchant_withdraw_cap(&admin, &merchant, &Some(30_000_000));

    assert_eq!(client.get_merchant_withdraw_cap(&merchant), Some(30_000_000));

    // Seed merchant balance to 40_000_000.
    env.as_contract(&client.address, || {
        env.storage().instance().set(
            &DataKey::MerchantBalance(merchant.clone(), token.clone()),
            &40_000_000i128,
        );
        let token_key = DataKey::MerchantTokens(merchant.clone());
        let mut tokens = soroban_sdk::Vec::new(&env);
        tokens.push_back(token.clone());
        env.storage().instance().set(&token_key, &tokens);
    });

    // Withdrawal of 25_000_000 should succeed (above default, within override).
    client.withdraw_merchant_funds(&merchant, &25_000_000);

    // Another withdrawal of 10_000_000 should fail (exceeds override).
    let res = client.try_withdraw_merchant_funds(&merchant, &10_000_000);
    assert_eq!(res, Err(Ok(Error::WithdrawCapExceeded)));
}

#[test]
fn test_rolling_window_rollover() {
    let (env, client, token, admin) = setup();
    let (_, _, merchant) = create_and_fund_sub(&env, &client, &token);

    client.set_merchant_withdraw_cap(&admin, &merchant, &Some(10_000_000));

    env.as_contract(&client.address, || {
        env.storage().instance().set(
            &DataKey::MerchantBalance(merchant.clone(), token.clone()),
            &30_000_000i128,
        );
        let token_key = DataKey::MerchantTokens(merchant.clone());
        let mut tokens = soroban_sdk::Vec::new(&env);
        tokens.push_back(token.clone());
        env.storage().instance().set(&token_key, &tokens);
    });

    // Withdraw 10_000_000 - consumes the entire cap.
    client.withdraw_merchant_funds(&merchant, &10_000_000);

    // Attempt immediately - rejected.
    let res = client.try_withdraw_merchant_funds(&merchant, &1_000_000);
    assert_eq!(res, Err(Ok(Error::WithdrawCapExceeded)));

    // Advance ledger by 24 hours + 1 second (86401 seconds).
    env.ledger().with_mut(|l| l.timestamp += 86401);

    // Withdrawal should now succeed.
    client.withdraw_merchant_funds(&merchant, &10_000_000);
}

#[test]
fn test_zero_cap_denies_all() {
    let (env, client, token, admin) = setup();
    let (_, _, merchant) = create_and_fund_sub(&env, &client, &token);

    client.set_merchant_withdraw_cap(&admin, &merchant, &Some(0));

    env.as_contract(&client.address, || {
        env.storage().instance().set(
            &DataKey::MerchantBalance(merchant.clone(), token.clone()),
            &10_000_000i128,
        );
        let token_key = DataKey::MerchantTokens(merchant.clone());
        let mut tokens = soroban_sdk::Vec::new(&env);
        tokens.push_back(token.clone());
        env.storage().instance().set(&token_key, &tokens);
    });

    // Withdrawal of even 1 unit is rejected.
    let res = client.try_withdraw_merchant_funds(&merchant, &1);
    assert_eq!(res, Err(Ok(Error::WithdrawCapExceeded)));
}

#[test]
fn test_unset_cap_allows_unlimited() {
    let (env, client, token, admin) = setup();
    let (_, _, merchant) = create_and_fund_sub(&env, &client, &token);

    // Set default and merchant caps to None (unset).
    client.set_default_merchant_cap(&admin, &None);
    client.set_merchant_withdraw_cap(&admin, &merchant, &None);

    assert_eq!(client.get_merchant_withdraw_cap(&merchant), None);

    env.as_contract(&client.address, || {
        env.storage().instance().set(
            &DataKey::MerchantBalance(merchant.clone(), token.clone()),
            &1_000_000_000_000i128,
        );
        let token_key = DataKey::MerchantTokens(merchant.clone());
        let mut tokens = soroban_sdk::Vec::new(&env);
        tokens.push_back(token.clone());
        env.storage().instance().set(&token_key, &tokens);
    });

    token::StellarAssetClient::new(&env, &token).mint(&client.address, &1_000_000_000_000i128);

    // Unlimited withdrawal succeeds.
    client.withdraw_merchant_funds(&merchant, &500_000_000_000i128);
    assert_eq!(client.get_merchant_balance(&merchant), 500_000_000_000i128);
}

#[test]
fn test_sub_account_withdraw_enforces_cap() {
    let (env, client, token, admin) = setup();
    let (_, _, merchant) = create_and_fund_sub(&env, &client, &token);

    client.set_merchant_withdraw_cap(&admin, &merchant, &Some(10_000_000));

    // Register a sub-account for the merchant.
    let label = Symbol::new(&env, "sub_a");
    client.register_sub_account(&merchant, &label);

    env.as_contract(&client.address, || {
        env.storage().instance().set(
            &DataKey::MerchantSubAccount(merchant.clone(), label.clone()),
            &20_000_000i128,
        );
        let token_key = DataKey::MerchantTokens(merchant.clone());
        let mut tokens = soroban_sdk::Vec::new(&env);
        tokens.push_back(token.clone());
        env.storage().instance().set(&token_key, &tokens);
    });

    // Withdraw 6_000_000 from sub-account. Should succeed.
    client.withdraw_sub_account_funds(&merchant, &label, &token, &6_000_000);

    // Try to withdraw 5_000_000 more from sub-account. Should fail (total 11_000_000 > 10_000_000).
    let res = client.try_withdraw_sub_account_funds(&merchant, &label, &token, &5_000_000);
    assert_eq!(res, Err(Ok(Error::WithdrawCapExceeded)));
}

#[test]
fn test_scheduled_payout_enforces_cap() {
    let (env, client, token, admin) = setup();
    let (_, _, merchant) = create_and_fund_sub(&env, &client, &token);

    client.set_merchant_withdraw_cap(&admin, &merchant, &Some(10_000_000));

    client.set_payout_schedule(&merchant, &3600, &100_000);

    env.as_contract(&client.address, || {
        env.storage().instance().set(
            &DataKey::MerchantBalance(merchant.clone(), token.clone()),
            &15_000_000i128,
        );
        let token_key = DataKey::MerchantTokens(merchant.clone());
        let mut tokens = soroban_sdk::Vec::new(&env);
        tokens.push_back(token.clone());
        env.storage().instance().set(&token_key, &tokens);
    });

    // Flush payouts. The payout tries to withdraw the entire balance (15_000_000).
    // This should fail because 15_000_000 > 10_000_000 cap!
    let res = client.try_flush_payouts(&merchant);
    assert_eq!(res, Err(Ok(Error::WithdrawCapExceeded)));
}
