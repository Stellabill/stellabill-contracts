#![cfg(test)]

use crate::{Error, OraclePrice, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger as _},
    token, Address, Env, IntoVal, Symbol,
};

// Dummy oracle contract to mock pricing
#[contract]
pub struct MockOracle;

#[contractimpl]
impl MockOracle {
    pub fn latest_price(env: Env) -> OraclePrice {
        let storage = env.storage().instance();
        OraclePrice {
            price: storage.get(&Symbol::new(&env, "price")).unwrap_or(0i128),
            timestamp: storage.get(&Symbol::new(&env, "timestamp")).unwrap_or(0u64),
        }
    }

    pub fn set_price(env: Env, price: i128, timestamp: u64) {
        let storage = env.storage().instance();
        storage.set(&Symbol::new(&env, "price"), &price);
        storage.set(&Symbol::new(&env, "timestamp"), &timestamp);
    }
}

const INTERVAL: u64 = 30 * 24 * 60 * 60;

fn setup_env() -> (
    Env,
    SubscriptionVaultClient<'static>,
    Address,
    Address,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();

    // Register vault
    let vault_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &vault_id);

    // Setup token
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000, &(7 * 24 * 60 * 60));

    // Register Oracle
    let oracle_id = env.register(MockOracle, ());

    (env, client, token, admin, oracle_id)
}

#[test]
fn test_oracle_staleness_and_missing_data() {
    let (env, client, _token, admin, oracle_id) = setup_env();
    let current_time = 1_000_000;
    env.ledger().set_timestamp(current_time);

    // Setup Oracle with missing timestamp data (timestamp = 0)
    env.invoke_contract::<()>(
        &oracle_id,
        &Symbol::new(&env, "set_price"),
        (1_000_000i128, 0u64).into_val(&env),
    );

    // Configure vault with Oracle config, max age 3600
    client.set_oracle_config(&admin, &true, &Some(oracle_id.clone()), &3600);

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &10_000_000,
        &INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );

    // This should fail due to OraclePriceUnavailable (timestamp is 0)
    let res1 = client.try_charge_subscription(&sub_id);
    assert_eq!(res1, Err(Ok(Error::OraclePriceUnavailable)));

    // Setup Oracle with stale data: timestamp is older than max_age_seconds
    let old_timestamp = current_time - 4000; // > 3600
    env.invoke_contract::<()>(
        &oracle_id,
        &Symbol::new(&env, "set_price"),
        (1_000_000i128, old_timestamp).into_val(&env),
    );

    // This should fail due to OraclePriceStale
    let res2 = client.try_charge_subscription(&sub_id);
    assert_eq!(res2, Err(Ok(Error::OraclePriceStale)));

    // Setup Oracle with fresh price: price logic uses safe math
    env.invoke_contract::<()>(
        &oracle_id,
        &Symbol::new(&env, "set_price"),
        (1_000_000i128, current_time).into_val(&env),
    );

    // Mint tokens to subscriber and deposit
    // Try to deposit and see if the mock token works
    let token_client = token::StellarAssetClient::new(&env, &_token);
    token_client.mint(&subscriber, &20_000_000);
    client.deposit_funds(&sub_id, &subscriber, &20_000_000);

    // Advance past the first interval boundary so charge_subscription is allowed.
    env.ledger().set_timestamp(current_time + INTERVAL + 1);
    // Re-set fresh oracle price at the new timestamp.
    env.invoke_contract::<()>(
        &oracle_id,
        &Symbol::new(&env, "set_price"),
        (1_000_000i128, current_time + INTERVAL + 1).into_val(&env),
    );

    // Now it should pass with the fresh price (it will deduct balance)
    let res3 = client.try_charge_subscription(&sub_id);
    assert!(res3.is_ok());
}

#[test]
fn test_oracle_rate_change_mid_interval() {
    let (env, client, _token, admin, oracle_id) = setup_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);
    let current_time = 1_000_000;
    env.ledger().set_timestamp(current_time);

    client.set_oracle_config(&admin, &true, &Some(oracle_id.clone()), &3600);

    let quote_amount = 5_000_000i128; // $5 USD
    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &quote_amount,
        &INTERVAL,
        &false,
        &None,
        &None::<u64>,
    );

    let token_client = token::StellarAssetClient::new(&env, &_token);
    token_client.mint(&subscriber, &100_000_000); // Plenty of token balance
    client.deposit_funds(&sub_id, &subscriber, &100_000_000);

    // Price = 1 USDC / token (ignoring decimals in this specific check logic, using 1:1)
    // Actually the oracle resolves: token_amount = ceil(quote_amount * 10^decimals / oracle_price)
    // token decimals = 6
    // Charge 1: advance past the first interval boundary first.
    env.ledger().set_timestamp(current_time + INTERVAL + 1);
    let initial_price = 1_000_000i128; // oracle returns 1.0 (with whatever decimal it assumes, here it divides)
    // Re-set price with fresh timestamp (max_age=3600, INTERVAL is 30 days).
    env.invoke_contract::<()>(
        &oracle_id,
        &Symbol::new(&env, "set_price"),
        (initial_price, current_time + INTERVAL + 1).into_val(&env),
    );
    client.charge_subscription(&sub_id);
    let sub_info = client.get_subscription(&sub_id);
    assert_eq!(sub_info.prepaid_balance, 95_000_000);

    // Advance time and change rate mid-interval
    env.ledger().set_timestamp(current_time + 2 * INTERVAL + 1); // Time for next charge

    // Price drops, now 1 quote = 0.5 token in oracle context. Meaning we need TWICE the tokens.
    // Price = 500_000. token_amount = ceil( 5_000_000 * 1_000_000 / 500_000 ) = 10_000_000
    env.invoke_contract::<()>(
        &oracle_id,
        &Symbol::new(&env, "set_price"),
        (500_000i128, current_time + 2 * INTERVAL + 1).into_val(&env),
    );

    client.charge_subscription(&sub_id);
    let sub_info2 = client.get_subscription(&sub_id);
    assert_eq!(sub_info2.prepaid_balance, 85_000_000); // 95M - 10M
}
