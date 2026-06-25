use soroban_sdk::{Address, Env, String, Symbol, Vec};

use crate::queries::get_subscription;
use crate::types::{
    DataKey, Error, MetadataDeletedEvent, MetadataSetEvent, MAX_METADATA_KEYS,
    MAX_METADATA_KEY_LENGTH, MAX_METADATA_VALUE_LENGTH,
};

/// Wrapper called by the contract entrypoint.
pub fn do_set_metadata(
    env: &Env,
    authorizer: Address,
    subscription_id: u32,
    key: String,
    value: String,
) -> Result<(), Error> {
    set_metadata(env, subscription_id, &authorizer, key, value)
}

/// Wrapper called by the contract entrypoint.
pub fn do_delete_metadata(
    env: &Env,
    authorizer: Address,
    subscription_id: u32,
    key: String,
) -> Result<(), Error> {
    delete_metadata(env, subscription_id, &authorizer, key)
}

pub fn set_metadata(
    env: &Env,
    subscription_id: u32,
    caller: &Address,
    key: String,
    value: String,
) -> Result<(), Error> {
    caller.require_auth();

    let sub = get_subscription(env, subscription_id)?;
    if caller != &sub.subscriber && caller != &sub.merchant {
        return Err(Error::Forbidden);
    }

    if key.len() > MAX_METADATA_KEY_LENGTH {
        return Err(Error::MetadataKeyTooLong);
    }

    if value.len() > MAX_METADATA_VALUE_LENGTH {
        return Err(Error::MetadataValueTooLong);
    }

    let mut keys: Vec<String> = env
        .storage()
        .persistent()
        .get(&DataKey::MetadataKeys(subscription_id))
        .unwrap_or(Vec::new(env));

    let key_exists = keys.iter().any(|k| k == key);

    if !key_exists {
        if keys.len() >= MAX_METADATA_KEYS {
            return Err(Error::MetadataKeyLimitReached);
        }
        keys.push_back(key.clone());
        env.storage()
            .persistent()
            .set(&DataKey::MetadataKeys(subscription_id), &keys);
    }

    env.storage()
        .persistent()
        .set(&DataKey::Metadata(subscription_id, key.clone()), &value);

    env.events().publish(
        (Symbol::new(env, "metadata_set"), subscription_id),
        MetadataSetEvent {
            subscription_id,
            key,
            authorizer: caller.clone(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

pub fn get_metadata(env: &Env, subscription_id: u32, key: String) -> Result<String, Error> {
    let _ = get_subscription(env, subscription_id)?;

    env.storage()
        .persistent()
        .get(&DataKey::Metadata(subscription_id, key))
        .ok_or(Error::NotFound)
}

pub fn delete_metadata(
    env: &Env,
    subscription_id: u32,
    caller: &Address,
    key: String,
) -> Result<(), Error> {
    caller.require_auth();

    let sub = get_subscription(env, subscription_id)?;
    if caller != &sub.subscriber && caller != &sub.merchant {
        return Err(Error::Forbidden);
    }

    let mut keys: Vec<String> = env
        .storage()
        .persistent()
        .get(&DataKey::MetadataKeys(subscription_id))
        .unwrap_or(Vec::new(env));

    if let Some(idx) = keys.iter().position(|k| k == key) {
        keys.remove(idx.try_into().unwrap());
        env.storage()
            .persistent()
            .set(&DataKey::MetadataKeys(subscription_id), &keys);

        env.storage()
            .persistent()
            .remove(&DataKey::Metadata(subscription_id, key.clone()));

        env.events().publish(
            (Symbol::new(env, "metadata_deleted"), subscription_id),
            MetadataDeletedEvent {
                subscription_id,
                key,
                authorizer: caller.clone(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
    }

    Ok(())
}

pub fn list_metadata_keys(env: &Env, subscription_id: u32) -> Result<Vec<String>, Error> {
    let _ = get_subscription(env, subscription_id)?;

    let keys: Vec<String> = env
        .storage()
        .persistent()
        .get(&DataKey::MetadataKeys(subscription_id))
        .unwrap_or(Vec::new(env));

    Ok(keys)
}
