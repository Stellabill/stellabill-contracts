use crate::types::{DataKey, Error};
use soroban_sdk::{Address, Env};

pub fn get_total_accounted(env: &Env, token: &Address) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalAccounted(token.clone()))
        .unwrap_or(0)
}

pub fn add_total_accounted(env: &Env, token: &Address, amount: i128) -> Result<(), Error> {
    if amount <= 0 {
        return Ok(()); // Or return an error depending on preference. We just ignore 0.
    }
    let current = get_total_accounted(env, token);
    let new_total = current.checked_add(amount).ok_or(Error::Overflow)?;
    env.storage()
        .instance()
        .set(&DataKey::TotalAccounted(token.clone()), &new_total);
    Ok(())
}

pub fn sub_total_accounted(env: &Env, token: &Address, amount: i128) -> Result<(), Error> {
    if amount <= 0 {
        return Ok(());
    }
    let current = get_total_accounted(env, token);
    let new_total = current.checked_sub(amount).ok_or(Error::Underflow)?;
    env.storage()
        .instance()
        .set(&DataKey::TotalAccounted(token.clone()), &new_total);
    Ok(())
}
