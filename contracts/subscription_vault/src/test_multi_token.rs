#![cfg(test)]

//! Multi-token allowlist tests.
//!
//! Covers: allowlist management, per-token subscription/deposit/charge/withdraw
//! flows, token confusion prevention, and active-subscription edge cases.

use crate::{Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env,
};

const INTERVAL: u64 = 30 * 24 * 60 * 60;
const T0: u64 = 1_700_000_000;
const DEPOSIT: i128 = 50_000_000;
const AMOUNT: i128 = 10_000_000;

// ── helpers ───────────────────────────────────────────────────────────────────

struct MultiTokenEnv {
    env: Env,
    client: SubscriptionVaultClient<'static>,
    admin: Address,
    default_token: Address,
}

impl MultiTokenEnv {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(T0);

        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let default_token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        client.init(&default_token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

        MultiTokenEnv { env, client, admin, default_token }
    }

    fn mint(&self, token: &Address, to: &Address, amount: i128) {
        soroban_sdk::token::StellarAssetClient::new(&self.env, token).mint(to, &amount);
    }

    fn new_token(&self) -> Address {
        self.env
            .register_stellar_asset_contract_v2(self.admin.clone())
            .address()
    }
}

// ── allowlist management ──────────────────────────────────────────────────────

#[test]
fn test_add_token_appears_in_list() {
    let t = MultiTokenEnv::new();
    let token_b = t.new_token();
    t.client.add_accepted_token(&t.admin, &token_b, &6);

    let list = t.client.list_accepted_tokens();
    assert!(list.iter().any(|e| e.token == token_b));
}

#[test]
fn test_add_token_non_admin_rejected() {
    let t = MultiTokenEnv::new();
    let token_b = t.new_token();
    let stranger = Address::generate(&t.env);
    assert_eq!(
        t.client.try_add_accepted_token(&stranger, &token_b, &6),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn test_remove_token_no_longer_in_list() {
    let t = MultiTokenEnv::new();
    let token_b = t.new_token();
    t.client.add_accepted_token(&t.admin, &token_b, &6);
    t.client.remove_accepted_token(&t.admin, &token_b);

    let list = t.client.list_accepted_tokens();
    assert!(!list.iter().any(|e| e.token == token_b));
}

#[test]
fn test_remove_default_token_rejected() {
    let t = MultiTokenEnv::new();
    // Removing the default token must fail.
    assert_eq!(
        t.client.try_remove_accepted_token(&t.admin, &t.default_token),
        Err(Ok(Error::InvalidInput))
    );
}

#[test]
fn test_remove_token_non_admin_rejected() {
    let t = MultiTokenEnv::new();
    let token_b = t.new_token();
    t.client.add_accepted_token(&t.admin, &token_b, &6);
    let stranger = Address::generate(&t.env);
    assert_eq!(
        t.client.try_remove_accepted_token(&stranger, &token_b),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn test_add_token_idempotent_updates_decimals() {
    let t = MultiTokenEnv::new();
    let token_b = t.new_token();
    t.client.add_accepted_token(&t.admin, &token_b, &6);
    // Re-adding with different decimals should succeed (update).
    t.client.add_accepted_token(&t.admin, &token_b, &7);
    let list = t.client.list_accepted_tokens();
    let entry = list.iter().find(|e| e.token == token_b).unwrap();
    assert_eq!(entry.decimals, 7);
}

// ── subscription creation with non-default token ──────────────────────────────

#[test]
fn test_create_subscription_with_accepted_token_succeeds() {
    let t = MultiTokenEnv::new();
    let token_b = t.new_token();
    t.client.add_accepted_token(&t.admin, &token_b, &6);

    let subscriber = Address::generate(&t.env);
    let merchant = Address::generate(&t.env);
    let id = t.client.create_subscription_with_token(
        &subscriber, &merchant, &token_b,
        &AMOUNT, &INTERVAL, &false, &None::<i128>, &None::<u64>,
    );
    let sub = t.client.get_subscription(&id);
    assert_eq!(sub.token, token_b);
}

#[test]
fn test_create_subscription_with_unaccepted_token_rejected() {
    let t = MultiTokenEnv::new();
    let unknown = Address::generate(&t.env);
    let subscriber = Address::generate(&t.env);
    let merchant = Address::generate(&t.env);
    assert_eq!(
        t.client.try_create_subscription_with_token(
            &subscriber, &merchant, &unknown,
            &AMOUNT, &INTERVAL, &false, &None::<i128>, &None::<u64>,
        ),
        Err(Ok(Error::InvalidInput))
    );
}

#[test]
fn test_create_subscription_with_removed_token_rejected() {
    let t = MultiTokenEnv::new();
    let token_b = t.new_token();
    t.client.add_accepted_token(&t.admin, &token_b, &6);
    t.client.remove_accepted_token(&t.admin, &token_b);

    let subscriber = Address::generate(&t.env);
    let merchant = Address::generate(&t.env);
    assert_eq!(
        t.client.try_create_subscription_with_token(
            &subscriber, &merchant, &token_b,
            &AMOUNT, &INTERVAL, &false, &None::<i128>, &None::<u64>,
        ),
        Err(Ok(Error::InvalidInput))
    );
}

// ── deposit / charge / withdraw with non-default token ───────────────────────

#[test]
fn test_deposit_and_charge_use_correct_token() {
    let t = MultiTokenEnv::new();
    let token_b = t.new_token();
    t.client.add_accepted_token(&t.admin, &token_b, &6);

    let subscriber = Address::generate(&t.env);
    let merchant = Address::generate(&t.env);
    t.mint(&token_b, &subscriber, DEPOSIT * 2);

    let id = t.client.create_subscription_with_token(
        &subscriber, &merchant, &token_b,
        &AMOUNT, &INTERVAL, &false, &None::<i128>, &None::<u64>,
    );
    t.client.deposit_funds(&id, &subscriber, &DEPOSIT);

    // Advance past one interval and charge.
    t.env.ledger().set_timestamp(T0 + INTERVAL + 1);
    t.client.charge_subscription(&id);

    // Merchant balance should be in token_b bucket.
    let balance = t.client.get_merchant_balance_by_token(&merchant, &token_b);
    assert_eq!(balance, AMOUNT);
    // Default token bucket must be untouched.
    assert_eq!(t.client.get_merchant_balance_by_token(&merchant, &t.default_token), 0);
}

#[test]
fn test_merchant_withdraw_correct_token_bucket() {
    let t = MultiTokenEnv::new();
    let token_b = t.new_token();
    t.client.add_accepted_token(&t.admin, &token_b, &6);

    let subscriber = Address::generate(&t.env);
    let merchant = Address::generate(&t.env);
    t.mint(&token_b, &subscriber, DEPOSIT * 2);

    let id = t.client.create_subscription_with_token(
        &subscriber, &merchant, &token_b,
        &AMOUNT, &INTERVAL, &false, &None::<i128>, &None::<u64>,
    );
    t.client.deposit_funds(&id, &subscriber, &DEPOSIT);
    t.env.ledger().set_timestamp(T0 + INTERVAL + 1);
    t.client.charge_subscription(&id);

    // Merchant withdraws from token_b bucket.
    t.client.withdraw_merchant_token_funds(&merchant, &token_b, &AMOUNT);
    assert_eq!(t.client.get_merchant_balance_by_token(&merchant, &token_b), 0);
}

#[test]
fn test_merchant_withdraw_wrong_token_bucket_fails() {
    let t = MultiTokenEnv::new();
    let token_b = t.new_token();
    t.client.add_accepted_token(&t.admin, &token_b, &6);

    let subscriber = Address::generate(&t.env);
    let merchant = Address::generate(&t.env);
    t.mint(&token_b, &subscriber, DEPOSIT * 2);

    let id = t.client.create_subscription_with_token(
        &subscriber, &merchant, &token_b,
        &AMOUNT, &INTERVAL, &false, &None::<i128>, &None::<u64>,
    );
    t.client.deposit_funds(&id, &subscriber, &DEPOSIT);
    t.env.ledger().set_timestamp(T0 + INTERVAL + 1);
    t.client.charge_subscription(&id);

    // Trying to withdraw from the default token bucket (which has 0) must fail.
    assert!(t.client
        .try_withdraw_merchant_token_funds(&merchant, &t.default_token, &AMOUNT)
        .is_err());
}

// ── token confusion prevention ────────────────────────────────────────────────

#[test]
fn test_two_subscriptions_different_tokens_isolated_accounting() {
    let t = MultiTokenEnv::new();
    let token_b = t.new_token();
    t.client.add_accepted_token(&t.admin, &token_b, &6);

    let sub_a = Address::generate(&t.env);
    let sub_b = Address::generate(&t.env);
    let merchant = Address::generate(&t.env);

    t.mint(&t.default_token.clone(), &sub_a, DEPOSIT * 2);
    t.mint(&token_b, &sub_b, DEPOSIT * 2);

    let id_a = t.client.create_subscription(
        &sub_a, &merchant, &AMOUNT, &INTERVAL, &false, &None::<i128>, &None::<u64>,
    );
    let id_b = t.client.create_subscription_with_token(
        &sub_b, &merchant, &token_b, &AMOUNT, &INTERVAL, &false, &None::<i128>, &None::<u64>,
    );

    t.client.deposit_funds(&id_a, &sub_a, &DEPOSIT);
    t.client.deposit_funds(&id_b, &sub_b, &DEPOSIT);

    t.env.ledger().set_timestamp(T0 + INTERVAL + 1);
    t.client.charge_subscription(&id_a);
    t.client.charge_subscription(&id_b);

    // Each token bucket holds exactly one charge.
    assert_eq!(t.client.get_merchant_balance_by_token(&merchant, &t.default_token), AMOUNT);
    assert_eq!(t.client.get_merchant_balance_by_token(&merchant, &token_b), AMOUNT);
}

// ── removing token with active subscriptions ──────────────────────────────────

#[test]
fn test_remove_token_with_active_subscription_existing_sub_still_readable() {
    let t = MultiTokenEnv::new();
    let token_b = t.new_token();
    t.client.add_accepted_token(&t.admin, &token_b, &6);

    let subscriber = Address::generate(&t.env);
    let merchant = Address::generate(&t.env);
    t.mint(&token_b, &subscriber, DEPOSIT * 2);

    let id = t.client.create_subscription_with_token(
        &subscriber, &merchant, &token_b,
        &AMOUNT, &INTERVAL, &false, &None::<i128>, &None::<u64>,
    );
    t.client.deposit_funds(&id, &subscriber, &DEPOSIT);

    // Remove token from allowlist.
    t.client.remove_accepted_token(&t.admin, &token_b);

    // Existing subscription is still readable.
    let sub = t.client.get_subscription(&id);
    assert_eq!(sub.token, token_b);
    assert_eq!(sub.prepaid_balance, DEPOSIT);
}

#[test]
fn test_new_subscription_with_removed_token_rejected_after_removal() {
    let t = MultiTokenEnv::new();
    let token_b = t.new_token();
    t.client.add_accepted_token(&t.admin, &token_b, &6);
    t.client.remove_accepted_token(&t.admin, &token_b);

    let subscriber = Address::generate(&t.env);
    let merchant = Address::generate(&t.env);
    assert_eq!(
        t.client.try_create_subscription_with_token(
            &subscriber, &merchant, &token_b,
            &AMOUNT, &INTERVAL, &false, &None::<i128>, &None::<u64>,
        ),
        Err(Ok(Error::InvalidInput))
    );
}
