#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    NotFound = 1,
    Unauthorized = 2,
    InvalidArgument = 3,
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

#[contracttype]
pub enum DataKey {
    NextId,
    Sub(u32),
    DefaultToken,
    Admin,
}

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    pub fn init(env: Env, admin: Address, default_token: Address) {
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::DefaultToken, &default_token);
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
            .get(&DataKey::DefaultToken)
            .ok_or(Error::NotFound)?;

        let id = Self::_next_id(&env);
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
        env.storage().instance().set(&DataKey::Sub(id), &sub);
        Ok(id)
    }

    pub fn create_subscription_with_token(
        env: Env,
        subscriber: Address,
        token: Address,
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

        let id = Self::_next_id(&env);
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
        env.storage().instance().set(&DataKey::Sub(id), &sub);
        Ok(id)
    }

    pub fn get_subscription(env: Env, id: u32) -> Result<Subscription, Error> {
        env.storage()
            .instance()
            .get(&DataKey::Sub(id))
            .ok_or(Error::NotFound)
    }

    pub fn version(_env: Env) -> u32 {
        0
    }

    fn _next_id(env: &Env) -> u32 {
        let id: u32 = env
            .storage()
            .instance()
            .get(&DataKey::NextId)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::NextId, &(id + 1));
        id
    }
}

#[cfg(test)]
mod test;
