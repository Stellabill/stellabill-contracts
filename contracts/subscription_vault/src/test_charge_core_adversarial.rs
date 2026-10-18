use crate::{
    admin::CachedAdminConfig,
    charge_core::charge_one,
    test_utils::fixtures,
    ChargeExecutionResult, DataKey, Error, SubscriptionStatus, SubscriptionVault,
    SubscriptionVaultClient,
};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, BytesN, Env};

const START: u64 = 1_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const AMOUNT: i128 = 10_000_000;
const PREPAID: i128 = 50_000_000;

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|ledger| ledger.timestamp = START);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, client, token)
}

fn charge_in_contract(
    env: &Env,
    client: &SubscriptionVaultClient,
    subscription_id: u32,
    now: u64,
    idempotency_key: Option<BytesN<32>>,
    admin_config: Option<&CachedAdminConfig>,
) -> Result<ChargeExecutionResult, Error> {
    env.as_contract(&client.address, || {
        charge_one(env, subscription_id, now, idempotency_key, admin_config)
    })
}

fn create_funded_subscription(
    env: &Env,
    client: &SubscriptionVaultClient,
) -> (u32, Address) {
    let (subscription_id, _, merchant) = fixtures::create_subscription_detailed(
        env,
        client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    fixtures::seed_balance(env, client, subscription_id, PREPAID);
    let mut subscription = client.get_subscription(&subscription_id);
    subscription.auto_renew = true;
    env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .set(&DataKey::Sub(subscription_id), &subscription);
    });
    (subscription_id, merchant)
}

#[test]
fn charge_one_charges_and_idempotently_replays_with_cached_admin_config() {
    let (env, client, token) = setup();
    let (subscription_id, merchant) = create_funded_subscription(&env, &client);
    let idempotency_key = BytesN::from_array(&env, &[7; 32]);
    let admin_config = CachedAdminConfig {
        fee_bps: 0,
        treasury: None,
        grace_duration: 7 * 24 * 60 * 60,
        auto_pause_threshold: 0,
    };
    let now = START + INTERVAL;

    assert_eq!(
        charge_in_contract(
            &env,
            &client,
            subscription_id,
            now,
            Some(idempotency_key.clone()),
            Some(&admin_config),
        ),
        Ok(ChargeExecutionResult::Charged)
    );

    let charged_sub = client.get_subscription(&subscription_id);
    let charged_balance = client.get_merchant_balance_by_token(&merchant, &token);
    assert_eq!(charged_sub.prepaid_balance, PREPAID - AMOUNT);
    assert_eq!(charged_sub.last_payment_timestamp, now);
    assert_eq!(charged_sub.lifetime_charged, AMOUNT);
    assert_eq!(charged_balance, AMOUNT);

    assert_eq!(
        charge_in_contract(
            &env,
            &client,
            subscription_id,
            now,
            Some(idempotency_key),
            Some(&admin_config),
        ),
        Ok(ChargeExecutionResult::Charged)
    );
    let replayed_sub = client.get_subscription(&subscription_id);
    assert_eq!(replayed_sub.prepaid_balance, charged_sub.prepaid_balance);
    assert_eq!(
        replayed_sub.last_payment_timestamp,
        charged_sub.last_payment_timestamp
    );
    assert_eq!(replayed_sub.lifetime_charged, charged_sub.lifetime_charged);
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        charged_balance
    );
}

#[test]
fn charge_one_without_optional_inputs_rejects_period_replay_without_mutating_state() {
    let (env, client, token) = setup();
    let (subscription_id, merchant) = create_funded_subscription(&env, &client);
    let now = START + INTERVAL;

    assert_eq!(
        charge_in_contract(&env, &client, subscription_id, now, None, None),
        Ok(ChargeExecutionResult::Charged)
    );
    let charged_sub = client.get_subscription(&subscription_id);
    let charged_balance = client.get_merchant_balance_by_token(&merchant, &token);

    assert_eq!(
        charge_in_contract(&env, &client, subscription_id, now, None, None),
        Err(Error::Replay)
    );

    let after_replay = client.get_subscription(&subscription_id);
    assert_eq!(after_replay.prepaid_balance, charged_sub.prepaid_balance);
    assert_eq!(
        after_replay.last_payment_timestamp,
        charged_sub.last_payment_timestamp
    );
    assert_eq!(after_replay.lifetime_charged, charged_sub.lifetime_charged);
    assert_eq!(after_replay.status, charged_sub.status);
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        charged_balance
    );
}

#[test]
fn charge_one_rejects_timestamp_before_interval_without_mutating_state() {
    let (env, client, token) = setup();
    let (subscription_id, merchant) = create_funded_subscription(&env, &client);
    let before = client.get_subscription(&subscription_id);
    let merchant_balance_before = client.get_merchant_balance_by_token(&merchant, &token);

    assert_eq!(
        charge_in_contract(&env, &client, subscription_id, 0, None, None),
        Err(Error::IntervalNotElapsed)
    );

    let after = client.get_subscription(&subscription_id);
    assert_eq!(after.prepaid_balance, before.prepaid_balance);
    assert_eq!(after.last_payment_timestamp, before.last_payment_timestamp);
    assert_eq!(after.lifetime_charged, before.lifetime_charged);
    assert_eq!(after.status, before.status);
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        merchant_balance_before
    );
    env.as_contract(&client.address, || {
        assert_eq!(
            env.storage()
                .instance()
                .get::<_, u64>(&DataKey::ChargedPeriod(subscription_id)),
            None
        );
        assert_eq!(
            env.storage()
                .instance()
                .get::<_, BytesN<32>>(&DataKey::ChargeSalt(subscription_id)),
            None
        );
    });
}

#[test]
fn charge_one_rejects_unknown_max_id_without_mutating_existing_subscription() {
    let (env, client, token) = setup();
    let (subscription_id, merchant) = create_funded_subscription(&env, &client);
    let before = client.get_subscription(&subscription_id);
    let merchant_balance_before = client.get_merchant_balance_by_token(&merchant, &token);

    assert_eq!(
        charge_in_contract(&env, &client, u32::MAX, START + INTERVAL, None, None),
        Err(Error::NotFound)
    );

    let after = client.get_subscription(&subscription_id);
    assert_eq!(after.prepaid_balance, before.prepaid_balance);
    assert_eq!(after.last_payment_timestamp, before.last_payment_timestamp);
    assert_eq!(after.lifetime_charged, before.lifetime_charged);
    assert_eq!(after.status, before.status);
    assert_eq!(
        client.get_merchant_balance_by_token(&merchant, &token),
        merchant_balance_before
    );
}