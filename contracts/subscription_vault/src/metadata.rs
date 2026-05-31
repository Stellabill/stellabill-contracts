#![allow(dead_code)]

use crate::queries::get_subscription;
use crate::types::{DataKey, Error, MetadataDeletedEvent, MetadataSetEvent, MAX_METADATA_KEYS, MAX_METADATA_KEY_LENGTH, MAX_METADATA_VALUE_LENGTH};
use soroban_sdk::{Address, Env, String, Symbol, Vec};

pub fn do_set_metadata(
    env: &Env,
    subscription_id: u32,
    authorizer: &Address,
    key: String,
    value: String,
) -> Result<(), Error> {
    if key.len() > MAX_METADATA_KEY_LENGTH as u32 {
        return Err(Error::MetadataKeyTooLong);
    }
    if value.len() > MAX_METADATA_VALUE_LENGTH as u32 {
        return Err(Error::MetadataValueTooLong);
    }

    let sub = get_subscription(env, subscription_id)?;
    if authorizer != &sub.subscriber && authorizer != &sub.merchant {
        return Err(Error::Unauthorized);
    }
    authorizer.require_auth();

    let metadata_keys_key = DataKey::MetadataKeys(subscription_id);
    let mut keys: Vec<String> = env
        .storage()
        .persistent()
        .get(&metadata_keys_key)
        .unwrap_or(Vec::new(env));

    let key_exists = keys.iter().any(|k| k == key);

    if !key_exists && keys.len() >= MAX_METADATA_KEYS as u32 {
        return Err(Error::MetadataKeyLimitReached);
    }

    if !key_exists {
        keys.push_back(key.clone());
        env.storage().persistent().set(&metadata_keys_key, &keys);
    }

    let metadata_key = DataKey::Metadata(subscription_id, key.clone());
    env.storage().persistent().set(&metadata_key, &value);

    env.events().publish(
        (Symbol::new(env, "metadata_set"), subscription_id),
        MetadataSetEvent {
            subscription_id,
            key,
            authorizer: authorizer.clone(),
        },
    );

    Ok(())
}

pub fn do_get_metadata(env: &Env, subscription_id: u32, key: String) -> Result<String, Error> {
    let _ = get_subscription(env, subscription_id)?;
    let metadata_key = DataKey::Metadata(subscription_id, key);
    env.storage()
        .persistent()
        .get(&metadata_key)
        .ok_or(Error::NotFound)
}

pub fn do_delete_metadata(
    env: &Env,
    subscription_id: u32,
    authorizer: &Address,
    key: String,
) -> Result<(), Error> {
    let sub = get_subscription(env, subscription_id)?;
    if authorizer != &sub.subscriber && authorizer != &sub.merchant {
        return Err(Error::Unauthorized);
    }
    authorizer.require_auth();

    let metadata_key = DataKey::Metadata(subscription_id, key.clone());
    if !env.storage().persistent().has(&metadata_key) {
        return Err(Error::NotFound);
    }
    env.storage().persistent().remove(&metadata_key);

    let metadata_keys_key = DataKey::MetadataKeys(subscription_id);
    let keys: Vec<String> = env
        .storage()
        .persistent()
        .get(&metadata_keys_key)
        .unwrap_or(Vec::new(env));

    let mut new_keys = Vec::new(env);
    for k in keys.iter() {
        if k != key {
            new_keys.push_back(k);
        }
    }
    env.storage()
        .persistent()
        .set(&metadata_keys_key, &new_keys);

    env.events().publish(
        (Symbol::new(env, "metadata_deleted"), subscription_id),
        MetadataDeletedEvent {
            subscription_id,
            key,
            authorizer: authorizer.clone(),
        },
    );

    Ok(())
}

pub fn do_list_metadata_keys(env: &Env, subscription_id: u32) -> Result<Vec<String>, Error> {
    let _ = get_subscription(env, subscription_id)?;
    let metadata_keys_key = DataKey::MetadataKeys(subscription_id);
    Ok(env
        .storage()
        .persistent()
        .get(&metadata_keys_key)
        .unwrap_or(Vec::new(env)))
}
