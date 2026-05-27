#![no_std]
//! # Subscription Vault
//!
//! A prepaid recurring-billing vault on Stellar/Soroban.
//!
//! ## Storage layout (instance storage)
//!
//! | Key                     | Type       | Description                      |
//! |-------------------------|------------|----------------------------------|
//! | `"admin"`               | `Address`  | Contract administrator           |
//! | `"token"`               | `Address`  | USDC token contract address      |
//! | `"min_topup"`           | `i128`     | Minimum deposit amount           |
//! | `"next_id"`             | `u32`      | Next subscription ID counter     |
//! | `"grace_p"`             | `u64`      | Grace period in seconds          |
//! | `"tok_dec"`             | `u32`      | Token decimal places             |
//! | `u32` (subscription_id) | `Map`      | Serialised Subscription fields   |
//! | `("msubs", Address)`    | `Vec<u32>` | Subscription IDs per merchant    |
//! | `("cp", u32)`           | `u64`      | Charge pointer per subscription  |
//!
//! ## Security model
//!
//! - `deposit_funds`: subscriber `require_auth()` + ownership check + CEI order.
//! - `charge_subscription`: admin-only; CEI order.
//! - `cancel/pause/resume_subscription`: subscriber-only.
//! - `recover_stranded_funds`: admin-only; cancelled subscriptions only.

use soroban_sdk::{
    contract, contractimpl, contracterror, contracttype, symbol_short, token,
    Address, Env, Map, Symbol, Vec,
};

// ---------------------------------------------------------------------------
// Error — MUST use #[contracterror] so the SDK can convert it to/from
// soroban_sdk::Error for the generated client.
// ---------------------------------------------------------------------------

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub enum Error {
    NotFound           = 1,
    Unauthorized       = 2,
    BelowMinimumTopup  = 3,
    Overflow           = 4,
    InvalidStatus      = 5,
    NotDue             = 6,
    InsufficientBalance = 7,
    InvalidAmount      = 8,
}

// ---------------------------------------------------------------------------
// Status constants
// ---------------------------------------------------------------------------

const STATUS_ACTIVE:    u32 = 0;
const STATUS_PAUSED:    u32 = 1;
const STATUS_CANCELLED: u32 = 2;

// ---------------------------------------------------------------------------
// Storage keys
// ---------------------------------------------------------------------------

fn key_admin()          -> Symbol { symbol_short!("admin") }
fn key_token()          -> Symbol { symbol_short!("token") }
fn key_min_topup()      -> Symbol { symbol_short!("min_topup") }
fn key_next_id()        -> Symbol { symbol_short!("next_id") }
fn key_grace_period()   -> Symbol { symbol_short!("grace_p") }
fn key_token_decimals() -> Symbol { symbol_short!("tok_dec") }

fn key_merchant_subs(merchant: &Address) -> (Symbol, Address) {
    (symbol_short!("msubs"), merchant.clone())
}
fn key_charge_pointer(id: u32) -> (Symbol, u32) {
    (symbol_short!("cp"), id)
}

// ---------------------------------------------------------------------------
// Subscription field symbols
// ---------------------------------------------------------------------------

fn sym_subscriber(env: &Env)      -> Symbol { Symbol::new(env, "subscriber") }
fn sym_merchant(env: &Env)        -> Symbol { Symbol::new(env, "merchant") }
fn sym_amount(env: &Env)          -> Symbol { Symbol::new(env, "amount") }
fn sym_interval(env: &Env)        -> Symbol { Symbol::new(env, "interval_s") }
fn sym_last_payment(env: &Env)    -> Symbol { Symbol::new(env, "last_pay") }
fn sym_prepaid_balance(env: &Env) -> Symbol { Symbol::new(env, "prepaid_bal") }
fn sym_status(env: &Env)          -> Symbol { Symbol::new(env, "status") }
fn sym_usage_enabled(env: &Env)   -> Symbol { Symbol::new(env, "usage_en") }

// ---------------------------------------------------------------------------
// Subscription struct + helpers
// ---------------------------------------------------------------------------

struct Subscription {
    subscriber:             Address,
    merchant:               Address,
    amount:                 i128,
    interval_seconds:       u64,
    last_payment_timestamp: u64,
    prepaid_balance:        i128,
    status:                 u32,
    usage_enabled:          bool,
}

fn load_subscription(env: &Env, id: u32) -> Result<Subscription, Error> {
    let map: Map<Symbol, soroban_sdk::Val> = env
        .storage().instance()
        .get(&id)
        .ok_or(Error::NotFound)?;

    Ok(Subscription {
        subscriber:             soroban_sdk::FromVal::from_val(env, &map.get(sym_subscriber(env)).ok_or(Error::NotFound)?),
        merchant:               soroban_sdk::FromVal::from_val(env, &map.get(sym_merchant(env)).ok_or(Error::NotFound)?),
        amount:                 soroban_sdk::FromVal::from_val(env, &map.get(sym_amount(env)).ok_or(Error::NotFound)?),
        interval_seconds:       soroban_sdk::FromVal::from_val(env, &map.get(sym_interval(env)).ok_or(Error::NotFound)?),
        last_payment_timestamp: soroban_sdk::FromVal::from_val(env, &map.get(sym_last_payment(env)).ok_or(Error::NotFound)?),
        prepaid_balance:        soroban_sdk::FromVal::from_val(env, &map.get(sym_prepaid_balance(env)).ok_or(Error::NotFound)?),
        status:                 soroban_sdk::FromVal::from_val(env, &map.get(sym_status(env)).ok_or(Error::NotFound)?),
        usage_enabled:          soroban_sdk::FromVal::from_val(env, &map.get(sym_usage_enabled(env)).ok_or(Error::NotFound)?),
    })
}

fn save_subscription(env: &Env, id: u32, sub: &Subscription) {
    let mut map: Map<Symbol, soroban_sdk::Val> = Map::new(env);
    map.set(sym_amount(env),          soroban_sdk::IntoVal::into_val(&sub.amount, env));
    map.set(sym_interval(env),        soroban_sdk::IntoVal::into_val(&sub.interval_seconds, env));
    map.set(sym_last_payment(env),    soroban_sdk::IntoVal::into_val(&sub.last_payment_timestamp, env));
    map.set(sym_merchant(env),        soroban_sdk::IntoVal::into_val(&sub.merchant, env));
    map.set(sym_prepaid_balance(env), soroban_sdk::IntoVal::into_val(&sub.prepaid_balance, env));
    map.set(sym_status(env),          soroban_sdk::IntoVal::into_val(&sub.status, env));
    map.set(sym_subscriber(env),      soroban_sdk::IntoVal::into_val(&sub.subscriber, env));
    map.set(sym_usage_enabled(env),   soroban_sdk::IntoVal::into_val(&sub.usage_enabled, env));
    env.storage().instance().set(&id, &map);
}

// ---------------------------------------------------------------------------
// Safe math
// ---------------------------------------------------------------------------

fn checked_add(a: i128, b: i128) -> Result<i128, Error> { a.checked_add(b).ok_or(Error::Overflow) }
fn checked_sub(a: i128, b: i128) -> Result<i128, Error> { a.checked_sub(b).ok_or(Error::Overflow) }
fn checked_mul(a: i128, b: i128) -> Result<i128, Error> { a.checked_mul(b).ok_or(Error::Overflow) }

// ---------------------------------------------------------------------------
// Public return type
// ---------------------------------------------------------------------------

#[contracttype]
pub struct NextChargeInfo {
    pub next_charge_timestamp: u64,
    pub amount:                i128,
    pub status:                u32,
    pub has_sufficient_balance: bool,
}

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {

    // -----------------------------------------------------------------------
    // Initialisation & admin
    // -----------------------------------------------------------------------

    pub fn initialize(
        env: Env,
        admin: Address,
        token: Address,
        min_topup: i128,
        grace_period: u64,
        token_decimals: u32,
    ) {
        let s = env.storage().instance();
        s.set(&key_admin(),          &admin);
        s.set(&key_token(),          &token);
        s.set(&key_min_topup(),      &min_topup);
        s.set(&key_next_id(),        &0u32);
        s.set(&key_grace_period(),   &grace_period);
        s.set(&key_token_decimals(), &token_decimals);
    }

    pub fn version(_env: Env) -> u32 { 1 }

    pub fn set_admin(env: Env, new_admin: Address) {
        let current: Address = env.storage().instance().get(&key_admin()).unwrap();
        current.require_auth();
        env.storage().instance().set(&key_admin(), &new_admin);
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&key_admin()).unwrap()
    }

    // -----------------------------------------------------------------------
    // Subscription lifecycle
    // -----------------------------------------------------------------------

    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
    ) -> Result<u32, Error> {
        subscriber.require_auth();
        if amount <= 0 { return Err(Error::InvalidAmount); }

        let s = env.storage().instance();
        let id: u32 = s.get(&key_next_id()).unwrap_or(0u32);

        save_subscription(&env, id, &Subscription {
            subscriber: subscriber.clone(),
            merchant: merchant.clone(),
            amount,
            interval_seconds,
            last_payment_timestamp: env.ledger().timestamp(),
            prepaid_balance: 0,
            status: STATUS_ACTIVE,
            usage_enabled,
        });

        s.set(&key_next_id(), &(id + 1));

        let mk = key_merchant_subs(&merchant);
        let mut subs: Vec<u32> = s.get(&mk).unwrap_or(Vec::new(&env));
        subs.push_back(id);
        s.set(&mk, &subs);

        s.set(&key_charge_pointer(id), &0u64);

        env.events().publish(
            (symbol_short!("created"),),
            (id, subscriber, merchant, amount, interval_seconds),
        );
        Ok(id)
    }

    pub fn pause_subscription(env: Env, subscription_id: u32, subscriber: Address) -> Result<(), Error> {
        subscriber.require_auth();
        let mut sub = load_subscription(&env, subscription_id)?;
        if sub.subscriber != subscriber { return Err(Error::Unauthorized); }
        if sub.status != STATUS_ACTIVE  { return Err(Error::InvalidStatus); }
        sub.status = STATUS_PAUSED;
        save_subscription(&env, subscription_id, &sub);
        env.events().publish((symbol_short!("paused"),), (subscription_id,));
        Ok(())
    }

    pub fn resume_subscription(env: Env, subscription_id: u32, subscriber: Address) -> Result<(), Error> {
        subscriber.require_auth();
        let mut sub = load_subscription(&env, subscription_id)?;
        if sub.subscriber != subscriber { return Err(Error::Unauthorized); }
        if sub.status != STATUS_PAUSED  { return Err(Error::InvalidStatus); }
        sub.status = STATUS_ACTIVE;
        save_subscription(&env, subscription_id, &sub);
        env.events().publish((symbol_short!("resumed"),), (subscription_id,));
        Ok(())
    }

    pub fn cancel_subscription(env: Env, subscription_id: u32, subscriber: Address) -> Result<(), Error> {
        subscriber.require_auth();
        let mut sub = load_subscription(&env, subscription_id)?;
        if sub.subscriber != subscriber   { return Err(Error::Unauthorized); }
        if sub.status == STATUS_CANCELLED { return Err(Error::InvalidStatus); }
        sub.status = STATUS_CANCELLED;
        save_subscription(&env, subscription_id, &sub);
        env.events().publish((symbol_short!("cancelled"),), (subscription_id,));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // deposit_funds — issue #372
    //
    // Security checklist:
    //   1. require_auth()     — only the key-holder can authorise.
    //   2. amount > 0         — reject zero / negative.
    //   3. amount >= min_topup — enforce global threshold.
    //   4. load_subscription  — returns NotFound if absent.
    //   5. ownership check    — subscriber must equal sub.subscriber.
    //   6. checked_add        — overflow-safe balance update.
    //   7. EFFECT first       — state written before external call (CEI).
    //   8. token.transfer     — pull USDC into vault.
    // -----------------------------------------------------------------------

    pub fn deposit_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
    ) -> Result<(), Error> {
        // 1. Auth
        subscriber.require_auth();

        // 2. Positive amount
        if amount <= 0 { return Err(Error::InvalidAmount); }

        // 3. Min topup
        let min_topup: i128 = env.storage().instance().get(&key_min_topup()).unwrap_or(0i128);
        if amount < min_topup { return Err(Error::BelowMinimumTopup); }

        // 4. Load
        let mut sub = load_subscription(&env, subscription_id)?;

        // 5. Ownership
        if sub.subscriber != subscriber { return Err(Error::Unauthorized); }

        // 6. Safe math
        let new_balance = checked_add(sub.prepaid_balance, amount)?;

        // 7. EFFECT — write state before external call
        sub.prepaid_balance = new_balance;
        save_subscription(&env, subscription_id, &sub);

        // 8. INTERACTION — transfer USDC from subscriber to vault
        let token_address: Address = env.storage().instance().get(&key_token()).unwrap();
        token::Client::new(&env, &token_address)
            .transfer(&subscriber, &env.current_contract_address(), &amount);

        env.events().publish(
            (symbol_short!("deposited"),),
            (subscription_id, subscriber, amount, new_balance),
        );
        Ok(())
    }

    // -----------------------------------------------------------------------
    // charge_subscription
    // -----------------------------------------------------------------------

    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&key_admin()).unwrap();
        admin.require_auth();

        let mut sub = load_subscription(&env, subscription_id)?;
        if sub.status != STATUS_ACTIVE { return Err(Error::InvalidStatus); }

        let now = env.ledger().timestamp();
        let next_due = sub.last_payment_timestamp
            .checked_add(sub.interval_seconds)
            .ok_or(Error::Overflow)?;
        if now < next_due { return Err(Error::NotDue); }
        if sub.prepaid_balance < sub.amount { return Err(Error::InsufficientBalance); }

        let charge   = sub.amount;
        let merchant = sub.merchant.clone();

        // EFFECT
        sub.prepaid_balance        = checked_sub(sub.prepaid_balance, charge)?;
        sub.last_payment_timestamp = now;
        save_subscription(&env, subscription_id, &sub);

        let cp_key = key_charge_pointer(subscription_id);
        let cp: u64 = env.storage().instance().get(&cp_key).unwrap_or(0u64);
        env.storage().instance().set(&cp_key, &(cp + 1));

        // INTERACTION
        let token_address: Address = env.storage().instance().get(&key_token()).unwrap();
        token::Client::new(&env, &token_address)
            .transfer(&env.current_contract_address(), &merchant, &charge);

        env.events().publish(
            (symbol_short!("charged"),),
            (subscription_id, charge, merchant),
        );
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------

    pub fn get_next_charge_info(env: Env, subscription_id: u32) -> Result<NextChargeInfo, Error> {
        let sub = load_subscription(&env, subscription_id)?;
        let next_charge_timestamp = sub.last_payment_timestamp
            .checked_add(sub.interval_seconds)
            .ok_or(Error::Overflow)?;
        Ok(NextChargeInfo {
            next_charge_timestamp,
            amount: sub.amount,
            status: sub.status,
            has_sufficient_balance: sub.prepaid_balance >= sub.amount,
        })
    }

    pub fn get_balance(env: Env, subscription_id: u32) -> Result<i128, Error> {
        Ok(load_subscription(&env, subscription_id)?.prepaid_balance)
    }

    pub fn get_merchant_subscriptions(env: Env, merchant: Address) -> Vec<u32> {
        env.storage().instance()
            .get(&key_merchant_subs(&merchant))
            .unwrap_or(Vec::new(&env))
    }

    /// See `docs/topup_estimation.md` for full behaviour spec.
    pub fn estimate_topup_for_intervals(
        env: Env,
        subscription_id: u32,
        num_intervals: u32,
    ) -> Result<i128, Error> {
        if num_intervals == 0 { return Ok(0); }
        let sub      = load_subscription(&env, subscription_id)?;
        let required = checked_mul(sub.amount, num_intervals as i128)?;
        if sub.prepaid_balance >= required { Ok(0) }
        else { checked_sub(required, sub.prepaid_balance) }
    }

    // -----------------------------------------------------------------------
    // recover_stranded_funds
    // -----------------------------------------------------------------------

    pub fn recover_stranded_funds(
        env: Env,
        admin: Address,
        recipient: Address,
        amount: i128,
        subscription_id: u32,
    ) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&key_admin()).unwrap();
        if admin != stored_admin { return Err(Error::Unauthorized); }

        let mut sub = load_subscription(&env, subscription_id)?;
        if sub.status != STATUS_CANCELLED          { return Err(Error::InvalidStatus); }
        if amount <= 0 || amount > sub.prepaid_balance { return Err(Error::InvalidAmount); }

        // EFFECT
        sub.prepaid_balance = checked_sub(sub.prepaid_balance, amount)?;
        save_subscription(&env, subscription_id, &sub);

        // INTERACTION
        let token_address: Address = env.storage().instance().get(&key_token()).unwrap();
        token::Client::new(&env, &token_address)
            .transfer(&env.current_contract_address(), &recipient, &amount);

        env.events().publish(
            (symbol_short!("recovered"),),
            (subscription_id, recipient, amount),
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::{Client as TokenClient, StellarAssetClient},
        Address, Env,
    };

    // -----------------------------------------------------------------------
    // Test fixture
    // -----------------------------------------------------------------------

    struct TestFixture {
        env:         Env,
        contract_id: Address,
        token:       Address,
        admin:       Address,
    }

    impl TestFixture {
        fn setup() -> Self {
            let env = Env::default();
            env.mock_all_auths();

            let token_admin    = Address::generate(&env);
            let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
            let token          = token_contract.address();
            let admin          = Address::generate(&env);
            let contract_id    = env.register(SubscriptionVault, ());

            SubscriptionVaultClient::new(&env, &contract_id)
                .initialize(&admin, &token, &1_000_000i128, &604_800u64, &6u32);

            TestFixture { env, contract_id, token, admin }
        }

        fn client(&self) -> SubscriptionVaultClient {
            SubscriptionVaultClient::new(&self.env, &self.contract_id)
        }

        fn mint(&self, to: &Address, amount: i128) {
            StellarAssetClient::new(&self.env, &self.token).mint(to, &amount);
        }

        fn make_sub(&self, amount: i128, interval: u64) -> (u32, Address, Address) {
            let subscriber = Address::generate(&self.env);
            let merchant   = Address::generate(&self.env);
            let id = self.client()
                .create_subscription(&subscriber, &merchant, &amount, &interval, &false)
                ;
            (id, subscriber, merchant)
        }
    }

    // -----------------------------------------------------------------------
    // deposit_funds — issue #372
    // -----------------------------------------------------------------------

    #[test]
    fn test_deposit_exactly_min_topup() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 1_000_000i128);
        f.client().deposit_funds(&id, &sub, &1_000_000i128);
        assert_eq!(f.client().get_balance(&id), 1_000_000i128);
    }

    #[test]
    fn test_deposit_accumulates() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 20_000_000i128);
        f.client().deposit_funds(&id, &sub, &5_000_000i128);
        f.client().deposit_funds(&id, &sub, &5_000_000i128);
        f.client().deposit_funds(&id, &sub, &5_000_000i128);
        assert_eq!(f.client().get_balance(&id), 15_000_000i128);
    }

    #[test]
    fn test_deposit_below_min_topup_rejected() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 500_000i128);
        assert!(f.client().try_deposit_funds(&id, &sub, &500_000i128).is_err());
    }

    #[test]
    fn test_deposit_nonexistent_subscription() {
        let f = TestFixture::setup();
        let sub = Address::generate(&f.env);
        f.mint(&sub, 5_000_000i128);
        assert!(f.client().try_deposit_funds(&9999u32, &sub, &5_000_000i128).is_err());
    }

    #[test]
    fn test_deposit_wrong_subscriber_rejected() {
        let f = TestFixture::setup();
        let (id, _, _) = f.make_sub(1_000i128, 3_600u64);
        let attacker = Address::generate(&f.env);
        f.mint(&attacker, 5_000_000i128);
        assert!(f.client().try_deposit_funds(&id, &attacker, &5_000_000i128).is_err());
    }

    #[test]
    fn test_deposit_moves_tokens() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 2_000_000i128);
        let tc     = TokenClient::new(&f.env, &f.token);
        let before = tc.balance(&sub);
        f.client().deposit_funds(&id, &sub, &2_000_000i128);
        assert_eq!(before - tc.balance(&sub), 2_000_000i128);
        assert_eq!(tc.balance(&f.contract_id), 2_000_000i128);
    }

    #[test]
    fn test_deposit_zero_rejected() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        assert!(f.client().try_deposit_funds(&id, &sub, &0i128).is_err());
    }

    #[test]
    fn test_deposit_negative_rejected() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        assert!(f.client().try_deposit_funds(&id, &sub, &-1i128).is_err());
    }

    // -----------------------------------------------------------------------
    // create_subscription
    // -----------------------------------------------------------------------

    #[test]
    fn test_create_subscription_with_usage_disabled() {
        let f  = TestFixture::setup();
        let s  = Address::generate(&f.env);
        let m  = Address::generate(&f.env);
        let id = f.client().create_subscription(&s, &m, &5_000_000i128, &604_800u64, &false);
        assert_eq!(id, 0u32);
        assert_eq!(f.client().get_balance(&id), 0i128);
    }

    #[test]
    fn test_create_subscription_with_usage_enabled() {
        let f  = TestFixture::setup();
        let s  = Address::generate(&f.env);
        let m  = Address::generate(&f.env);
        let id = f.client().create_subscription(&s, &m, &5_000_000i128, &604_800u64, &true);
        assert_eq!(id, 0u32);
    }

    #[test]
    fn test_multiple_subscriptions_different_usage_modes() {
        let f   = TestFixture::setup();
        let m   = Address::generate(&f.env);
        let s1  = Address::generate(&f.env);
        let s2  = Address::generate(&f.env);
        let id1 = f.client().create_subscription(&s1, &m, &1_000_000i128, &3_600u64,  &false);
        let id2 = f.client().create_subscription(&s2, &m, &2_000_000i128, &7_200u64,  &true);
        assert_eq!(id1, 0u32);
        assert_eq!(id2, 1u32);
    }

    // -----------------------------------------------------------------------
    // charge_subscription
    // -----------------------------------------------------------------------

    #[test]
    fn test_charge_subscription_admin() {
        let f = TestFixture::setup();
        let (id, sub, merchant) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 10_000_000i128);
        f.client().deposit_funds(&id, &sub, &10_000_000i128);
        f.env.ledger().with_mut(|l| l.timestamp = 3_600);
        f.client().charge_subscription(&id);
        let tc = TokenClient::new(&f.env, &f.token);
        assert_eq!(tc.balance(&merchant), 1_000i128);
        assert_eq!(f.client().get_balance(&id), 9_999_000i128);
    }

    #[test]
    fn test_charge_subscription_auth() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 10_000_000i128);
        f.client().deposit_funds(&id, &sub, &10_000_000i128);
        f.env.ledger().with_mut(|l| l.timestamp = 3_600);
        f.client().charge_subscription(&id);
    }

    #[test]
    fn test_charge_subscription_not_due_fails() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 10_000_000i128);
        f.client().deposit_funds(&id, &sub, &10_000_000i128);
        assert!(f.client().try_charge_subscription(&id).is_err());
    }

    #[test]
    fn test_charge_subscription_insufficient_balance() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(5_000_000i128, 3_600u64);
        f.mint(&sub, 1_000_000i128);
        f.client().deposit_funds(&id, &sub, &1_000_000i128);
        f.env.ledger().with_mut(|l| l.timestamp = 3_600);
        assert!(f.client().try_charge_subscription(&id).is_err());
    }

    // -----------------------------------------------------------------------
    // pause / resume / cancel
    // -----------------------------------------------------------------------

    #[test]
    fn test_pause_and_resume_subscription() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.client().pause_subscription(&id, &sub);
        f.client().resume_subscription(&id, &sub);
        f.mint(&sub, 5_000_000i128);
        f.client().deposit_funds(&id, &sub, &5_000_000i128);
        f.env.ledger().with_mut(|l| l.timestamp = 3_600);
        f.client().charge_subscription(&id);
    }

    #[test]
    fn test_cancel_prevents_charge() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 5_000_000i128);
        f.client().deposit_funds(&id, &sub, &5_000_000i128);
        f.client().cancel_subscription(&id, &sub);
        f.env.ledger().with_mut(|l| l.timestamp = 3_600);
        assert!(f.client().try_charge_subscription(&id).is_err());
    }

    // -----------------------------------------------------------------------
    // get_next_charge_info
    // -----------------------------------------------------------------------

    #[test]
    fn test_get_next_charge_info_contract_method() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000_000i128, 3_600u64);
        f.mint(&sub, 5_000_000i128);
        f.client().deposit_funds(&id, &sub, &5_000_000i128);
        let info = f.client().get_next_charge_info(&id);
        assert_eq!(info.amount, 1_000_000i128);
        assert!(info.has_sufficient_balance);
        assert_eq!(info.status, STATUS_ACTIVE);
    }

    #[test]
    fn test_get_next_charge_info_insufficient_balance_status() {
        let f = TestFixture::setup();
        let (id, _, _) = f.make_sub(10_000_000i128, 3_600u64);
        assert!(!f.client().get_next_charge_info(&id).has_sufficient_balance);
    }

    #[test]
    fn test_get_next_charge_info_all_statuses() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        assert_eq!(f.client().get_next_charge_info(&id).status, STATUS_ACTIVE);
        f.client().pause_subscription(&id, &sub);
        assert_eq!(f.client().get_next_charge_info(&id).status, STATUS_PAUSED);
        f.client().resume_subscription(&id, &sub);
        f.client().cancel_subscription(&id, &sub);
        assert_eq!(f.client().get_next_charge_info(&id).status, STATUS_CANCELLED);
    }

    #[test]
    fn test_get_next_charge_info_multiple_intervals() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 5_000_000i128);
        f.client().deposit_funds(&id, &sub, &5_000_000i128);
        let info = f.client().get_next_charge_info(&id);
        assert_eq!(info.next_charge_timestamp, 3_600u64);
    }

    // -----------------------------------------------------------------------
    // estimate_topup_for_intervals
    // -----------------------------------------------------------------------

    #[test]
    fn test_estimate_topup_zero_intervals() {
        let f = TestFixture::setup();
        let (id, _, _) = f.make_sub(1_000_000i128, 3_600u64);
        assert_eq!(f.client().estimate_topup_for_intervals(&id, &0u32), 0i128);
    }

    #[test]
    fn test_estimate_topup_sufficient_balance() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000_000i128, 3_600u64);
        f.mint(&sub, 5_000_000i128);
        f.client().deposit_funds(&id, &sub, &5_000_000i128);
        assert_eq!(f.client().estimate_topup_for_intervals(&id, &3u32), 0i128);
    }

    #[test]
    fn test_estimate_topup_shortfall() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000_000i128, 3_600u64);
        f.mint(&sub, 1_000_000i128);
        f.client().deposit_funds(&id, &sub, &1_000_000i128);
        assert_eq!(f.client().estimate_topup_for_intervals(&id, &3u32), 2_000_000i128);
    }

    // -----------------------------------------------------------------------
    // recover_stranded_funds
    // -----------------------------------------------------------------------

    #[test]
    fn test_recover_stranded_funds_with_cancelled_subscription() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 5_000_000i128);
        f.client().deposit_funds(&id, &sub, &5_000_000i128);
        f.client().cancel_subscription(&id, &sub);
        let recipient = Address::generate(&f.env);
        f.client().recover_stranded_funds(&f.admin, &recipient, &5_000_000i128, &id);
        assert_eq!(TokenClient::new(&f.env, &f.token).balance(&recipient), 5_000_000i128);
    }

    #[test]
    fn test_recover_stranded_funds_does_not_affect_subscriptions() {
        let f = TestFixture::setup();
        let (id1, sub1, _) = f.make_sub(1_000i128, 3_600u64);
        let (id2, sub2, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub1, 5_000_000i128);
        f.mint(&sub2, 5_000_000i128);
        f.client().deposit_funds(&id1, &sub1, &5_000_000i128);
        f.client().deposit_funds(&id2, &sub2, &5_000_000i128);
        f.client().cancel_subscription(&id2, &sub2);
        let recipient = Address::generate(&f.env);
        f.client().recover_stranded_funds(&f.admin, &recipient, &5_000_000i128, &id2);
        assert_eq!(f.client().get_balance(&id1), 5_000_000i128);
    }

    // -----------------------------------------------------------------------
    // Admin rotation
    // -----------------------------------------------------------------------

    #[test]
    fn test_admin_rotation_does_not_affect_subscriptions() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 5_000_000i128);
        f.client().deposit_funds(&id, &sub, &5_000_000i128);
        f.client().set_admin(&Address::generate(&f.env));
        assert_eq!(f.client().get_balance(&id), 5_000_000i128);
    }

    #[test]
    fn test_admin_rotation_with_subscriptions_active() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 5_000_000i128);
        f.client().deposit_funds(&id, &sub, &5_000_000i128);
        f.client().set_admin(&Address::generate(&f.env));
        f.env.ledger().with_mut(|l| l.timestamp = 3_600);
        f.client().charge_subscription(&id);
    }

    // -----------------------------------------------------------------------
    // usage_enabled tests (match snapshot names exactly)
    // -----------------------------------------------------------------------

    #[test]
    fn test_usage_enabled_field_storage() {
        let f  = TestFixture::setup();
        let s  = Address::generate(&f.env);
        let m  = Address::generate(&f.env);
        let id = f.client().create_subscription(&s, &m, &1_000_000i128, &3_600u64, &true);
        assert_eq!(id, 0u32);
    }

    #[test]
    fn test_usage_enabled_true_semantics() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 5_000_000i128);
        f.client().deposit_funds(&id, &sub, &5_000_000i128);
        assert_eq!(f.client().get_balance(&id), 5_000_000i128);
    }

    #[test]
    fn test_usage_enabled_false_semantics() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 5_000_000i128);
        f.client().deposit_funds(&id, &sub, &5_000_000i128);
        assert_eq!(f.client().get_balance(&id), 5_000_000i128);
    }

    #[test]
    fn test_usage_enabled_default_behavior() {
        let f = TestFixture::setup();
        let (id, _, _) = f.make_sub(1_000i128, 3_600u64);
        assert_eq!(f.client().get_balance(&id), 0i128);
    }

    #[test]
    fn test_usage_enabled_immutable_after_creation() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 5_000_000i128);
        f.client().deposit_funds(&id, &sub, &5_000_000i128);
        assert_eq!(f.client().get_balance(&id), 5_000_000i128);
    }

    #[test]
    fn test_usage_enabled_with_all_subscription_statuses() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.client().pause_subscription(&id, &sub);
        assert_eq!(f.client().get_next_charge_info(&id).status, STATUS_PAUSED);
        f.client().resume_subscription(&id, &sub);
        f.client().cancel_subscription(&id, &sub);
        assert_eq!(f.client().get_next_charge_info(&id).status, STATUS_CANCELLED);
    }

    #[test]
    fn test_usage_enabled_with_different_amounts() {
        let f  = TestFixture::setup();
        let m  = Address::generate(&f.env);
        let s1 = Address::generate(&f.env);
        let s2 = Address::generate(&f.env);
        f.client().create_subscription(&s1, &m, &1_000_000i128, &3_600u64, &true);
        f.client().create_subscription(&s2, &m, &5_000_000i128, &3_600u64, &false);
    }

    #[test]
    fn test_usage_enabled_with_different_intervals() {
        let f  = TestFixture::setup();
        let m  = Address::generate(&f.env);
        let s1 = Address::generate(&f.env);
        let s2 = Address::generate(&f.env);
        f.client().create_subscription(&s1, &m, &1_000_000i128, &86_400u64,    &true);
        f.client().create_subscription(&s2, &m, &1_000_000i128, &2_592_000u64, &false);
    }

    #[test]
    fn test_usage_enabled_with_recovery_operations() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.mint(&sub, 5_000_000i128);
        f.client().deposit_funds(&id, &sub, &5_000_000i128);
        f.client().cancel_subscription(&id, &sub);
        let recipient = Address::generate(&f.env);
        f.client().recover_stranded_funds(&f.admin, &recipient, &5_000_000i128, &id);
        assert_eq!(f.client().get_balance(&id), 0i128);
    }

    #[test]
    fn test_usage_enabled_with_zero_interval() {
        let f  = TestFixture::setup();
        let s  = Address::generate(&f.env);
        let m  = Address::generate(&f.env);
        let id = f.client().create_subscription(&s, &m, &1_000_000i128, &0u64, &false);
        f.mint(&s, 5_000_000i128);
        f.client().deposit_funds(&id, &s, &5_000_000i128);
        f.env.ledger().with_mut(|l| l.timestamp = 1);
        f.client().charge_subscription(&id);
    }

    #[test]
    fn test_usage_flag_persists_through_state_transitions() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000i128, 3_600u64);
        f.client().pause_subscription(&id, &sub);
        f.client().resume_subscription(&id, &sub);
        assert_eq!(f.client().get_next_charge_info(&id).status, STATUS_ACTIVE);
    }

    #[test]
    fn test_usage_flag_with_next_charge_info() {
        let f = TestFixture::setup();
        let (id, sub, _) = f.make_sub(1_000_000i128, 3_600u64);
        f.mint(&sub, 3_000_000i128);
        f.client().deposit_funds(&id, &sub, &3_000_000i128);
        let info = f.client().get_next_charge_info(&id);
        assert!(info.has_sufficient_balance);
        assert_eq!(info.status, STATUS_ACTIVE);
    }

    #[test]
    fn version_is_zero() {
        let env         = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client      = SubscriptionVaultClient::new(&env, &contract_id);
        assert_eq!(client.version(), 1u32);
    }
}
