#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol};

pub const MAX_SUBSCRIPTION_ID: u32 = u32::MAX;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    NotFound = 404,
    InvalidArgument = 3,
    SubscriptionLimitReached = 429,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscriptionStatus {
    Active = 0,
    Paused = 1,
    Cancelled = 2,
    InsufficientBalance = 3,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Subscription {
    pub subscriber: Address,
    pub token: Address,
    pub merchant: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub last_payment_timestamp: u64,
    pub status: SubscriptionStatus,
    pub prepaid_balance: i128,
    pub usage_enabled: bool,
    pub expires_at: Option<u64>,
}

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    pub fn init(env: Env, admin: Address, default_token: Address) {
        env.storage().instance().set(&Symbol::new(&env, "admin"), &admin);
        env.storage().instance().set(&Symbol::new(&env, "token"), &default_token);
    }

    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        expires_at: Option<u64>,
    ) -> Result<u32, Error> {
        subscriber.require_auth();

        if amount <= 0 {
            return Err(Error::InvalidArgument);
        }
        if interval_seconds == 0 {
            return Err(Error::InvalidArgument);
        }
        if let Some(ts) = expires_at {
            if ts <= env.ledger().timestamp() {
                return Err(Error::InvalidArgument);
            }
        }

        let token: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "token"))
            .ok_or(Error::NotFound)?;

        let id = Self::_next_id(&env)?;
        let sub = Subscription {
            subscriber,
            token,
            merchant,
            amount,
            interval_seconds,
            last_payment_timestamp: env.ledger().timestamp(),
            status: SubscriptionStatus::Active,
            prepaid_balance: 0,
            usage_enabled,
            expires_at,
        };
        env.storage().instance().set(&id, &sub);
        Ok(id)
    }

    pub fn get_subscription(env: Env, id: u32) -> Result<Subscription, Error> {
        env.storage()
            .instance()
            .get(&id)
            .ok_or(Error::NotFound)
    }

    pub fn get_subscription_count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "next_id"))
            .unwrap_or(0)
    }

    pub fn version(_env: Env) -> u32 {
        0
    }

    fn _next_id(env: &Env) -> Result<u32, Error> {
        let key = Symbol::new(env, "next_id");
        let current: u32 = env.storage().instance().get(&key).unwrap_or(0);
        if current == MAX_SUBSCRIPTION_ID {
            return Err(Error::SubscriptionLimitReached);
        }
        env.storage().instance().set(&key, &(current + 1));
        Ok(current)
    }
}

#[cfg(test)]
mod test;
