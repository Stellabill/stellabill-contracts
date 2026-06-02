use soroban_sdk::{contracttype, Address, Env, String, Symbol};
use crate::types::{DataKey, Error};
use crate::admin::require_admin_auth;

#[contracttype]
#[derive(Clone)]
pub struct BlocklistEntry {
    pub reason: String,
}

#[contracttype]
#[derive(Clone)]
pub struct BlocklistAddedEvent {
    pub subscriber: Address,
    pub reason: String,
}

#[contracttype]
#[derive(Clone)]
pub struct BlocklistRemovedEvent {
    pub subscriber: Address,
}

pub fn is_blocklisted(env: &Env, addr: &Address) -> bool {
    env.storage().persistent().has(&DataKey::Blocklist(addr.clone()))
}

pub fn require_not_blocklisted(env: &Env, addr: &Address) -> Result<(), Error> {
    if is_blocklisted(env, addr) {
        Err(Error::SubscriberBlocklisted)
    } else {
        Ok(())
    }
}

pub fn get_blocklist_entry(env: &Env, addr: Address) -> Result<BlocklistEntry, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Blocklist(addr))
        .ok_or(Error::NotFound)
}

pub fn do_add_to_blocklist(
    env: &Env,
    authorizer: Address,
    subscriber: Address,
    reason: Option<String>,
) -> Result<(), Error> {
    require_admin_auth(env, &authorizer)?;

    if is_blocklisted(env, &subscriber) {
        return Err(Error::SubscriberBlocklisted);
    }

    let reason_str = reason.unwrap_or_else(|| String::from_str(env, ""));
    let entry = BlocklistEntry {
        reason: reason_str.clone(),
    };
    
    env.storage().persistent().set(&DataKey::Blocklist(subscriber.clone()), &entry);

    env.events().publish(
        (Symbol::new(env, "blocklist_added"), subscriber.clone()),
        BlocklistAddedEvent {
            subscriber,
            reason: reason_str,
        },
    );

    Ok(())
}

pub fn do_remove_from_blocklist(
    env: &Env,
    admin: Address,
    subscriber: Address,
) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;

    if !is_blocklisted(env, &subscriber) {
        return Err(Error::NotFound);
    }

    env.storage().persistent().remove(&DataKey::Blocklist(subscriber.clone()));

    env.events().publish(
        (Symbol::new(env, "blocklist_removed"), subscriber.clone()),
        BlocklistRemovedEvent { subscriber },
    );

    Ok(())
}