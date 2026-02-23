//! Admin and config: init, min_topup, batch_charge.
//!
//! **PRs that only change admin or batch behavior should edit this file only.**

use crate::charge_core::charge_one;
use crate::types::{BatchChargeResult, Error, RecoveryEvent, RecoveryReason};
use soroban_sdk::{Address, Env, Symbol, Vec};

pub fn do_init(env: &Env, token: Address, admin: Address, min_topup: i128) -> Result<(), Error> {
    env.storage()
        .instance()
        .set(&Symbol::new(env, "token"), &token);
    env.storage()
        .instance()
        .set(&Symbol::new(env, "admin"), &admin);
    env.storage()
        .instance()
        .set(&Symbol::new(env, "min_topup"), &min_topup);
    env.events().publish(
        (Symbol::new(env, "initialized"),),
        (token, admin, min_topup),
    );
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
    env.events()
        .publish((Symbol::new(env, "min_topup_updated"),), min_topup);
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
        let r = charge_one(env, id, None);
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

pub fn do_get_admin(env: &Env) -> Result<Address, Error> {
    env.storage()
        .instance()
        .get(&Symbol::new(env, "admin"))
        .ok_or(Error::NotFound)
}

pub fn do_rotate_admin(env: &Env, current_admin: Address, new_admin: Address) -> Result<(), Error> {
    current_admin.require_auth();

    let stored_admin: Address = env
        .storage()
        .instance()
        .get(&Symbol::new(env, "admin"))
        .ok_or(Error::NotFound)?;

    if current_admin != stored_admin {
        return Err(Error::Unauthorized);
    }

    env.storage()
        .instance()
        .set(&Symbol::new(env, "admin"), &new_admin);

    env.events().publish(
        (Symbol::new(env, "admin_rotation"), current_admin.clone()),
        (current_admin, new_admin, env.ledger().timestamp()),
    );

    Ok(())
}

pub fn do_recover_stranded_funds(
    env: &Env,
    admin: Address,
    recipient: Address,
    amount: i128,
    reason: RecoveryReason,
) -> Result<(), Error> {
    admin.require_auth();

    let stored_admin: Address = env
        .storage()
        .instance()
        .get(&Symbol::new(env, "admin"))
        .ok_or(Error::NotFound)?;

    if admin != stored_admin {
        return Err(Error::Unauthorized);
    }

    if amount <= 0 {
        return Err(Error::InvalidRecoveryAmount);
    }

    let recovery_event = RecoveryEvent {
        admin: admin.clone(),
        recipient: recipient.clone(),
        amount,
        reason,
        timestamp: env.ledger().timestamp(),
    };

    env.events().publish(
        (Symbol::new(env, "recovery"), admin.clone()),
        recovery_event,
    );

    // TODO: Actual token transfer logic
    // token_client.transfer(&env.current_contract_address(), &recipient, &amount);

    Ok(())
}
