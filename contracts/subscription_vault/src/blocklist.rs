use crate::admin::require_admin_auth;
use crate::types::{DataKey, Error};
use soroban_sdk::{contracttype, Address, Env, String, Symbol};

#[contracttype]
#[derive(Clone)]
pub struct BlocklistEntry {
    pub subscriber: Address,
    pub added_by: Address,
    pub added_at: u64,
    pub reason: Option<String>,
}

#[contracttype]
#[derive(Clone)]
pub struct BlocklistAddedEvent {
    pub subscriber: Address,
    pub added_by: Address,
    pub timestamp: u64,
    pub reason: Option<String>,
    /// Event schema version for backwards-compatible indexer decoding.
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone)]
pub struct BlocklistRemovedEvent {
    pub subscriber: Address,
    pub removed_by: Address,
    pub timestamp: u64,
    /// Event schema version for backwards-compatible indexer decoding.
    pub schema_version: u32,
}

/// Check whether `addr` is currently blocklisted.
///
/// O(1) persistent-storage lookup keyed by `DataKey::Blocklist(addr)`.
pub fn is_blocklisted(env: &Env, addr: &Address) -> bool {
    env.storage()
        .persistent()
        .has(&DataKey::Blocklist(addr.clone()))
}

/// Return `Err(Error::SubscriberBlocklisted)` when `addr` is blocklisted.
///
/// This is the canonical guard used at the top of every mutating entry-point
/// that should be blocked while the address is on the blocklist.
pub fn require_not_blocklisted(env: &Env, addr: &Address) -> Result<(), Error> {
    if is_blocklisted(env, addr) {
        Err(Error::SubscriberBlocklisted)
    } else {
        Ok(())
    }
}

/// Retrieve the full [`BlocklistEntry`] for `addr`.
///
/// Returns `Err(Error::NotFound)` when the address is not on the blocklist.
pub fn get_blocklist_entry(env: &Env, addr: Address) -> Result<BlocklistEntry, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Blocklist(addr))
        .ok_or(Error::NotFound)
}

/// Add `subscriber` to the blocklist. Admin only.
///
/// # Authorization
/// Caller must be the stored contract admin.
///
/// # Duplicate guard
/// Returns `Err(Error::InvalidInput)` when `subscriber` is already blocklisted.
/// This preserves the original entry (added_by, added_at, reason) so the audit
/// trail is not overwritten.
///
/// # Events
/// Emits `("blocklist_added", subscriber) -> BlocklistAddedEvent`.
pub fn do_add_to_blocklist(
    env: &Env,
    authorizer: Address,
    subscriber: Address,
    reason: Option<String>,
) -> Result<(), Error> {
    require_admin_auth(env, &authorizer)?;

    if is_blocklisted(env, &subscriber) {
        return Err(Error::InvalidInput);
    }

    let timestamp = env.ledger().timestamp();
    let entry = BlocklistEntry {
        subscriber: subscriber.clone(),
        added_by: authorizer.clone(),
        added_at: timestamp,
        reason: reason.clone(),
    };
    env.storage()
        .persistent()
        .set(&DataKey::Blocklist(subscriber.clone()), &entry);

    env.events().publish(
        (Symbol::new(env, "blocklist_added"), subscriber.clone()),
        BlocklistAddedEvent {
            subscriber,
            added_by: authorizer,
            timestamp,
            reason,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Remove `subscriber` from the blocklist. Admin only.
///
/// # Authorization
/// Caller must be the stored contract admin.
///
/// # Errors
/// - `Error::Forbidden` — caller is not the stored admin.
/// - `Error::NotFound` — `subscriber` is not currently blocklisted.
///
/// # Events
/// Emits `("blocklist_removed", subscriber) -> BlocklistRemovedEvent`.
pub fn do_remove_from_blocklist(
    env: &Env,
    admin: Address,
    subscriber: Address,
) -> Result<(), Error> {
    require_admin_auth(env, &admin)?;

    if !is_blocklisted(env, &subscriber) {
        return Err(Error::NotFound);
    }

    let timestamp = env.ledger().timestamp();
    env.storage()
        .persistent()
        .remove(&DataKey::Blocklist(subscriber.clone()));

    env.events().publish(
        (Symbol::new(env, "blocklist_removed"), subscriber.clone()),
        BlocklistRemovedEvent {
            subscriber,
            removed_by: admin,
            timestamp,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}
