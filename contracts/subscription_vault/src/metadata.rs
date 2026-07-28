//! Bounded per-subscription metadata key-value store.
//!
//! Provides a lightweight mechanism to associate off-chain references (invoice IDs,
//! customer IDs, tags) with subscriptions without affecting financial state.
//!
//! See `docs/subscription_metadata.md` for design, limits, and safe-usage guidelines.

use crate::queries::get_subscription;
use crate::types::{
    Error, MetadataDeletedEvent, MetadataSetEvent, SubscriptionStatus, MAX_METADATA_KEYS,
    MAX_METADATA_KEY_LENGTH, MAX_METADATA_VALUE_LENGTH,
};
use soroban_sdk::{Env, String, Symbol, Vec};

/// Storage key for the list of metadata keys for a subscription.
fn metadata_keys_key(env: &Env, subscription_id: u32) -> (Symbol, u32) {
    (Symbol::new(env, "mk"), subscription_id)
}

/// Storage key for a single metadata value.
fn metadata_value_key(env: &Env, subscription_id: u32, key: &String) -> (Symbol, u32, String) {
    (Symbol::new(env, "mv"), subscription_id, key.clone())
}

/// Set or update a metadata key-value pair on a subscription.
///
/// Authorization: subscriber or merchant of the subscription.
/// Does not affect financial state (balances, status, charges).
///
/// # Limits
/// - Max keys per subscription: [`MAX_METADATA_KEYS`] (10)
/// - Max key length: [`MAX_METADATA_KEY_LENGTH`] (32 bytes)
/// - Max value length: [`MAX_METADATA_VALUE_LENGTH`] (256 bytes)
pub fn do_set_metadata(
    env: &Env,
    subscription_id: u32,
    authorizer: &soroban_sdk::Address,
    key: String,
    value: String,
) -> Result<(), Error> {
    authorizer.require_auth();

    let sub = get_subscription(env, subscription_id)?;
    if *authorizer != sub.subscriber && *authorizer != sub.merchant {
        return Err(Error::Forbidden);
    }
    if sub.status == SubscriptionStatus::Cancelled {
        return Err(Error::NotActive);
    }

    // Validate key/value sizes
    if key.is_empty() || key.len() > MAX_METADATA_KEY_LENGTH {
        return Err(Error::MetadataKeyTooLong);
    }
    if value.len() > MAX_METADATA_VALUE_LENGTH {
        return Err(Error::MetadataValueTooLong);
    }

    let storage = env.storage().instance();
    let keys_key = metadata_keys_key(env, subscription_id);
    let mut keys: Vec<String> = storage.get(&keys_key).unwrap_or(Vec::new(env));

    // Check if key already exists
    let mut found = false;
    for i in 0..keys.len() {
        if keys.get(i).unwrap() == key {
            found = true;
            break;
        }
    }

    if !found {
        // New key: check limit
        if keys.len() >= MAX_METADATA_KEYS {
            return Err(Error::MetadataKeyLimitReached);
        }
        keys.push_back(key.clone());
        storage.set(&keys_key, &keys);
    }

    // Store the value
    let val_key = metadata_value_key(env, subscription_id, &key);
    storage.set(&val_key, &value);

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

/// Delete a metadata key from a subscription.
///
/// Authorization: subscriber or merchant of the subscription.
pub fn do_delete_metadata(
    env: &Env,
    subscription_id: u32,
    authorizer: &soroban_sdk::Address,
    key: String,
) -> Result<(), Error> {
    authorizer.require_auth();

    let sub = get_subscription(env, subscription_id)?;
    if *authorizer != sub.subscriber && *authorizer != sub.merchant {
        return Err(Error::Forbidden);
    }

    let storage = env.storage().instance();
    let keys_key = metadata_keys_key(env, subscription_id);
    let mut keys: Vec<String> = storage.get(&keys_key).unwrap_or(Vec::new(env));

    // Find and remove the key
    let mut found_idx: Option<u32> = None;
    for i in 0..keys.len() {
        if keys.get(i).unwrap() == key {
            found_idx = Some(i);
            break;
        }
    }

    match found_idx {
        Some(idx) => {
            keys.remove(idx);
            storage.set(&keys_key, &keys);

            // Remove the value
            let val_key = metadata_value_key(env, subscription_id, &key);
            storage.remove(&val_key);

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
        None => Err(Error::NotFound),
    }
}

/// Get a metadata value by key.
pub fn do_get_metadata(env: &Env, subscription_id: u32, key: String) -> Result<String, Error> {
    // Verify subscription exists
    get_subscription(env, subscription_id)?;

    let val_key = metadata_value_key(env, subscription_id, &key);
    env.storage()
        .instance()
        .get(&val_key)
        .ok_or(Error::NotFound)
}

/// List all metadata keys for a subscription.
pub fn do_list_metadata_keys(env: &Env, subscription_id: u32) -> Result<Vec<String>, Error> {
    // Verify subscription exists
    get_subscription(env, subscription_id)?;

    let keys_key = metadata_keys_key(env, subscription_id);
    Ok(env
        .storage()
        .instance()
        .get(&keys_key)
        .unwrap_or(Vec::new(env)))
}
