//! Subscriber blocklist management.
//!
//! Admins and merchants can blocklist subscriber addresses to prevent new subscription
//! creation and deposits. Existing subscriptions and balances are preserved.
//!
//! **PRs that only change blocklist behavior should edit this file only.**

use crate::types::{DataKey, Error};
use soroban_sdk::{Address, Env, Symbol};

fn blocklist_key(subscriber: &Address) -> DataKey {
    DataKey::Blocklist(subscriber.clone())
}

/// Check if a subscriber is blocklisted.
pub fn is_blocklisted(env: &Env, subscriber: &Address) -> bool {
    let key = blocklist_key(subscriber);
    env.storage().instance().has(&key)
}

/// Reject subscriber-authored mutating flows while the subscriber is blocklisted.
pub fn require_not_blocklisted(env: &Env, subscriber: &Address) -> Result<(), Error> {
    if is_blocklisted(env, subscriber) {
        return Err(Error::SubscriberBlocklisted);
    }
    Ok(())
}

/// Add a subscriber to the blocklist. Admin or merchant only.
///
/// # Arguments
/// * `authorizer` - The admin or merchant adding the subscriber to the blocklist
/// * `subscriber` - The subscriber address to blocklist
/// * `reason` - Optional reason for blocklisting (stored as metadata)
///
/// # Authorization
/// - Admin can blocklist any subscriber
/// - Merchants can only blocklist subscribers they have active subscriptions with
pub fn do_add_to_blocklist(
    env: &Env,
    authorizer: Address,
    subscriber: Address,
    reason: Option<soroban_sdk::String>,
) -> Result<(), Error> {
    authorizer.require_auth();

    // Check if authorizer is admin
    let is_admin = if let Ok(admin) = crate::admin::require_admin(env) {
        admin == authorizer
    } else {
        false
    };

    // If not admin, verify merchant has subscriptions with this subscriber
    if !is_admin {
        // Merchant authorization: must have at least one subscription with this subscriber
        let has_subscription =
            has_merchant_subscription_with_subscriber(env, &authorizer, &subscriber);
        if !has_subscription {
            return Err(Error::Forbidden);
        }
    }

    let key = blocklist_key(&subscriber);
    if env.storage().instance().has(&key) {
        return Err(Error::InvalidInput);
    }

    let entry = BlocklistEntry {
        subscriber: subscriber.clone(),
        added_by: authorizer.clone(),
        added_at: env.ledger().timestamp(),
        reason: reason.clone(),
    };

    env.storage().instance().set(&key, &entry);

    env.events().publish(
        (Symbol::new(env, "blocklist_added"),),
        BlocklistAddedEvent {
            subscriber,
            added_by: authorizer,
            timestamp: env.ledger().timestamp(),
            reason,
        },
    );

    Ok(())
}

/// Remove a subscriber from the blocklist. Admin only.
pub fn do_remove_from_blocklist(
    env: &Env,
    admin: Address,
    subscriber: Address,
) -> Result<(), Error> {
    crate::admin::require_admin_auth(env, &admin)?;

    let key = blocklist_key(&subscriber);
    if !env.storage().instance().has(&key) {
        return Err(Error::NotFound);
    }

    env.storage().instance().remove(&key);

    env.events().publish(
        (Symbol::new(env, "blocklist_removed"),),
        BlocklistRemovedEvent {
            subscriber,
            removed_by: admin,
            timestamp: env.ledger().timestamp(),
        },
    );

    Ok(())
}

/// Get blocklist entry details for a subscriber.
pub fn get_blocklist_entry(env: &Env, subscriber: Address) -> Result<BlocklistEntry, Error> {
    let key = blocklist_key(&subscriber);
    env.storage().instance().get(&key).ok_or(Error::NotFound)
}

/// Helper to check if a merchant has an active relationship with a subscriber.
///
/// A relationship is considered active for any non-cancelled subscription state.
fn has_merchant_subscription_with_subscriber(
    env: &Env,
    merchant: &Address,
    subscriber: &Address,
) -> bool {
    use crate::types::DataKey;
    use soroban_sdk::Vec;

    let merchant_key = DataKey::MerchantSubs(merchant.clone());
    let ids: Vec<u32> = env
        .storage()
        .instance()
        .get(&merchant_key)
        .unwrap_or(Vec::new(env));

    for i in 0..ids.len() {
        if let Some(sub_id) = ids.get(i) {
            if let Ok(sub) = crate::queries::get_subscription(env, sub_id) {
                if sub.subscriber == *subscriber
                    && sub.status != crate::types::SubscriptionStatus::Cancelled
                {
                    return true;
                }
            }
        }
    }

    false
}

// ── Types ─────────────────────────────────────────────────────────────────────

use soroban_sdk::contracttype;

/// Blocklist entry with metadata.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlocklistEntry {
    pub subscriber: Address,
    pub added_by: Address,
    pub added_at: u64,
    pub reason: Option<soroban_sdk::String>,
}

/// Event emitted when a subscriber is added to the blocklist.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlocklistAddedEvent {
    pub subscriber: Address,
    pub added_by: Address,
    pub timestamp: u64,
    pub reason: Option<soroban_sdk::String>,
}

/// Event emitted when a subscriber is removed from the blocklist.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlocklistRemovedEvent {
    pub subscriber: Address,
    pub removed_by: Address,
    pub timestamp: u64,
}
