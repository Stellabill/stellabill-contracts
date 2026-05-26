#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env};

#[contracttype]
#[derive(Clone, PartialEq, Eq)]
pub enum SubscriptionStatus {
    Active,
    Paused,
    Cancelled,
    InsufficientBalance,
}

#[contracttype]
#[derive(Clone)]
pub struct Subscription {
    pub merchant: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub last_payment_timestamp: u64,
    pub status: SubscriptionStatus,
    pub prepaid_balance: i128,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Subscription(u32),
    MerchantBalance(Address),
}

#[contracttype]
#[derive(Clone)]
pub struct ChargeExecutionResult {
    pub success: bool,
}

#[contracterror]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Error {
    NotFound,
    NotActive,
    IntervalNotElapsed,
    InsufficientBalance,
    Overflow,
    InvalidInterval,
}

const MIN_SUBSCRIPTION_INTERVAL_SECONDS: u64 = 60;
const MAX_SUBSCRIPTION_INTERVAL_SECONDS: u64 = 31_536_000;

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    pub fn version(_env: Env) -> u32 {
        0
    }

    pub fn create_subscription(
        env: Env,
        subscription_id: u32,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        initial_balance: i128,
    ) -> Result<(), Error> {
        validate_interval(interval_seconds)?;
        if amount < 0 || initial_balance < 0 {
            return Err(Error::InvalidInterval);
        }

        let now = env.ledger().timestamp();
        let subscription = Subscription {
            merchant,
            amount,
            interval_seconds,
            last_payment_timestamp: now,
            status: SubscriptionStatus::Active,
            prepaid_balance: initial_balance,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Subscription(subscription_id), &subscription);
        Ok(())
    }

    pub fn charge_subscription(
        env: Env,
        subscription_id: u32,
    ) -> Result<ChargeExecutionResult, Error> {
        let mut subscription = load_subscription(&env, subscription_id)?;

        if subscription.status != SubscriptionStatus::Active {
            return Err(Error::NotActive);
        }

        let next_charge_time = subscription
            .last_payment_timestamp
            .checked_add(subscription.interval_seconds)
            .ok_or(Error::Overflow)?;

        let now = env.ledger().timestamp();
        if now < next_charge_time {
            return Err(Error::IntervalNotElapsed);
        }

        if subscription.prepaid_balance < subscription.amount {
            subscription.status = SubscriptionStatus::InsufficientBalance;
            env.storage()
                .persistent()
                .set(&DataKey::Subscription(subscription_id), &subscription);
            return Err(Error::InsufficientBalance);
        }

        subscription.prepaid_balance = subscription
            .prepaid_balance
            .checked_sub(subscription.amount)
            .ok_or(Error::Overflow)?;
        subscription.last_payment_timestamp = now;

        env.storage()
            .persistent()
            .set(&DataKey::Subscription(subscription_id), &subscription);

        credit_merchant(&env, &subscription.merchant, subscription.amount)?;

        Ok(ChargeExecutionResult { success: true })
    }

    pub fn merchant_balance(env: Env, merchant: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::MerchantBalance(merchant))
            .unwrap_or(0)
    }
}

fn load_subscription(env: &Env, subscription_id: u32) -> Result<Subscription, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Subscription(subscription_id))
        .ok_or(Error::NotFound)
}

fn credit_merchant(env: &Env, merchant: &Address, amount: i128) -> Result<(), Error> {
    let current: i128 = env
        .storage()
        .persistent()
        .get(&DataKey::MerchantBalance(merchant.clone()))
        .unwrap_or(0);
    let updated = current.checked_add(amount).ok_or(Error::Overflow)?;
    env.storage()
        .persistent()
        .set(&DataKey::MerchantBalance(merchant.clone()), &updated);
    Ok(())
}

fn validate_interval(interval_seconds: u64) -> Result<(), Error> {
    if interval_seconds < MIN_SUBSCRIPTION_INTERVAL_SECONDS
        || interval_seconds > MAX_SUBSCRIPTION_INTERVAL_SECONDS
    {
        return Err(Error::InvalidInterval);
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{testutils::Ledger, Address, BytesN, Env};

    fn account_id(env: &Env, nonce: u8) -> Address {
        let mut bytes = [0u8; 32];
        bytes[0] = nonce;
        Address::from_account_id(&BytesN::from_array(env, &bytes))
    }

    fn set_ledger_timestamp(env: &Env, timestamp: u64) {
        env.ledger().set(Ledger {
            timestamp,
            sequence_number: timestamp as i32,
            protocol_version: 1,
        });
    }

    #[test]
    fn charge_subscription_deducts_balance_and_credits_merchant() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        set_ledger_timestamp(&env, 1);
        let merchant = account_id(&env, 1);
        client
            .create_subscription(1, merchant.clone(), 25, 60, 100)
            .unwrap();

        set_ledger_timestamp(&env, 61);
        let result = client.charge_subscription(1);
        assert_eq!(result.unwrap().success, true);
        assert_eq!(client.merchant_balance(merchant.clone()), 25);

        let subscription: Subscription = env
            .storage()
            .persistent()
            .get(&DataKey::Subscription(1))
            .unwrap();
        assert_eq!(subscription.prepaid_balance, 75);
        assert_eq!(subscription.last_payment_timestamp, 61);
        assert_eq!(subscription.status, SubscriptionStatus::Active);
    }

    #[test]
    fn charge_subscription_rejects_before_interval() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        set_ledger_timestamp(&env, 1);
        let merchant = account_id(&env, 2);
        client
            .create_subscription(2, merchant, 30, 60, 100)
            .unwrap();

        set_ledger_timestamp(&env, 60);
        let err = client.charge_subscription(2).unwrap_err();
        assert_eq!(err, Error::IntervalNotElapsed);
    }

    #[test]
    fn charge_subscription_allows_exact_balance_to_zero() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        set_ledger_timestamp(&env, 1);
        let merchant = account_id(&env, 3);
        client
            .create_subscription(3, merchant.clone(), 50, 60, 50)
            .unwrap();

        set_ledger_timestamp(&env, 61);
        let result = client.charge_subscription(3).unwrap();
        assert_eq!(result.success, true);
        assert_eq!(client.merchant_balance(merchant), 50);

        let subscription: Subscription = env
            .storage()
            .persistent()
            .get(&DataKey::Subscription(3))
            .unwrap();
        assert_eq!(subscription.prepaid_balance, 0);
        assert_eq!(subscription.status, SubscriptionStatus::Active);
    }

    #[test]
    fn charge_subscription_sets_insufficient_balance_when_funds_are_low() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        set_ledger_timestamp(&env, 1);
        let merchant = account_id(&env, 4);
        client
            .create_subscription(4, merchant.clone(), 40, 60, 20)
            .unwrap();

        set_ledger_timestamp(&env, 61);
        let err = client.charge_subscription(4).unwrap_err();
        assert_eq!(err, Error::InsufficientBalance);
        assert_eq!(client.merchant_balance(merchant), 0);

        let subscription: Subscription = env
            .storage()
            .persistent()
            .get(&DataKey::Subscription(4))
            .unwrap();
        assert_eq!(subscription.prepaid_balance, 20);
        assert_eq!(subscription.status, SubscriptionStatus::InsufficientBalance);
    }

    #[test]
    fn charge_subscription_rejects_non_active_status() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        set_ledger_timestamp(&env, 1);
        let merchant = account_id(&env, 5);
        client
            .create_subscription(5, merchant.clone(), 10, 60, 100)
            .unwrap();

        let mut subscription: Subscription = env
            .storage()
            .persistent()
            .get(&DataKey::Subscription(5))
            .unwrap();
        subscription.status = SubscriptionStatus::Paused;
        env.storage()
            .persistent()
            .set(&DataKey::Subscription(5), &subscription);

        set_ledger_timestamp(&env, 61);
        let err = client.charge_subscription(5).unwrap_err();
        assert_eq!(err, Error::NotActive);
    }
}
