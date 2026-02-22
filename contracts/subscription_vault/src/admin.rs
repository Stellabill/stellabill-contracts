//! Admin and config: init, min_topup, batch_charge.
//!
//! **PRs that only change admin or batch behavior should edit this file only.**

use crate::charge_core::charge_one;
use crate::types::{BatchChargeResult, Error};
use soroban_sdk::{Address, Env, Symbol, Vec};

pub fn do_init(
    env: &Env,
    token: Address,
    admin: Address,
    min_topup: i128,
) -> Result<(), Error> {
    env.storage()
        .instance()
        .set(&Symbol::new(env, "token"), &token);
    env.storage()
        .instance()
        .set(&Symbol::new(env, "admin"), &admin);
    env.storage()
        .instance()
        .set(&Symbol::new(env, "min_topup"), &min_topup);
    Ok(())
}

pub fn require_admin(env: &Env) -> Result<Address, Error> {
    env.storage()
        .instance()
        .get(&Symbol::new(env, "admin"))
        .ok_or(Error::Unauthorized)
}

pub fn do_set_min_topup(env: &Env, admin: Address, min_topup: i128) -> Result<(), Error> {
    admin.require_auth();
    let stored = require_admin(env)?;
    if admin != stored {
        return Err(Error::Unauthorized);
    }
    env.storage()
        .instance()
        .set(&Symbol::new(env, "min_topup"), &min_topup);
    Ok(())
}

pub fn get_min_topup(env: &Env) -> Result<i128, Error> {
    env.storage()
        .instance()
        .get(&Symbol::new(env, "min_topup"))
        .ok_or(Error::NotFound)
}

pub fn do_batch_charge(
    env: &Env,
    subscription_ids: &Vec<u32>,
) -> Result<Vec<BatchChargeResult>, Error> {
    let auth_admin = require_admin(env)?;
    auth_admin.require_auth();

    let mut results = Vec::new(env);
    for id in subscription_ids.iter() {
        let r = charge_one(env, id);
        let res = match &r {
            Ok(()) => BatchChargeResult {
                success: true,
                error_code: 0,
            },
            Err(e) => BatchChargeResult {
                success: false,
                error_code: e.clone().to_code(),
            },
        };
        results.push_back(res);
    }
    Ok(results)
}
