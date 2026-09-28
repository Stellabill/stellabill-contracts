//! # `do_remove_from_blocklist` — adversarial coverage
//!
//! Issue #1030 asks for focused coverage of `blocklist::do_remove_from_blocklist`
//! beyond the happy path. The main `test.rs` blocklist cases assert the two
//! error variants; this module adds the properties they do not pin:
//!
//! | Test | Scenario |
//! |------|----------|
//! | `remove_clears_blocklist_and_entry` | Successful removal clears both the flag and the stored entry |
//! | `remove_missing_is_not_found_and_leaves_no_state` | Removing a never-added address is `NotFound` and emits nothing |
//! | `non_admin_removal_is_forbidden_and_state_is_unchanged` | `Forbidden` leaves the entry byte-for-byte intact and emits no event |
//! | `second_removal_is_not_found_after_success` | Removal is not idempotent: the second call is `NotFound` |
//! | `remove_event_payload_is_exact` | Event carries the caller, subscriber, timestamp and schema version |
//! | `removing_one_subscriber_leaves_others_blocklisted` | Removal is scoped to a single address |
//! | `rejected_then_authorized_removal_succeeds` | A rejected attempt mutates nothing, so the admin can still remove |
//!
//! ## Security assumptions validated
//!
//! * Only the stored admin can remove an entry (`Forbidden` otherwise).
//! * Rejected removals are side-effect free: no storage write, no event.
//! * Removal never emits an event for an address that was not blocklisted.

extern crate std;

use crate::blocklist::BlocklistRemovedEvent;
use crate::types::{DataKey, Error, EVENT_SCHEMA_VERSION};
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
use soroban_sdk::{token, Address, Env, Symbol, TryFromVal};

const T0: u64 = 1_000_000;

// ── helpers ──────────────────────────────────────────────────────────────────

fn setup() -> (
    Env,
    SubscriptionVaultClient<'static>,
    Address, // admin
    token::StellarAssetClient<'static>,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = T0);

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token_id = env.register_stellar_asset_contract_v2(Address::generate(&env));
    let token_admin = token::StellarAssetClient::new(&env, &token_id.address());

    client.init(&token_id.address(), &6, &admin, &1_000_000i128, &0u64);

    (env, client, admin, token_admin)
}

/// Collect every `blocklist_removed` event emitted by `client`.
///
/// The host event buffer is scoped to the whole test invocation, so callers that
/// need "no event" semantics assert on the length of the returned vector.
fn removed_events(env: &Env, client: &SubscriptionVaultClient) -> std::vec::Vec<BlocklistRemovedEvent> {
    env.events()
        .all()
        .iter()
        .filter_map(|(cid, topics, data)| {
            if cid != client.address || topics.len() != 2 {
                return None;
            }
            let topic = Symbol::try_from_val(env, &topics.get(0).unwrap()).ok()?;
            if topic != Symbol::new(env, "blocklist_removed") {
                return None;
            }
            BlocklistRemovedEvent::try_from_val(env, &data).ok()
        })
        .collect()
}

// ── tests ────────────────────────────────────────────────────────────────────

/// A successful removal clears both the O(1) flag and the persisted entry.
#[test]
fn remove_clears_blocklist_and_entry() {
    let (env, client, admin, _tok) = setup();
    let subscriber = Address::generate(&env);

    client.add_to_blocklist(&admin, &subscriber, &None::<String>);
    assert!(client.is_blocklisted(&subscriber));
    assert_eq!(client.get_blocklist_entry(&subscriber).added_by, admin);

    client.remove_from_blocklist(&admin, &subscriber);

    assert!(!client.is_blocklisted(&subscriber), "entry must be gone after removal");
    assert_eq!(removed_events(&env, &client).len(), 1);
}

/// Removing an address that was never blocklisted is a clean `NotFound` with no
/// storage mutation and no phantom event.
#[test]
fn remove_missing_is_not_found_and_leaves_no_state() {
    let (env, client, admin, _tok) = setup();
    let never_added = Address::generate(&env);

    let result = client.try_remove_from_blocklist(&admin, &never_added);

    assert_eq!(result, Err(Ok(Error::NotFound)));
    assert!(!client.is_blocklisted(&never_added));
    assert!(
        removed_events(&env, &client).is_empty(),
        "a rejected removal must not emit blocklist_removed"
    );
}

/// A non-admin caller is rejected with `Forbidden`, and the existing entry is
/// left byte-for-byte intact (same admin, reason and timestamp).
#[test]
fn non_admin_removal_is_forbidden_and_state_is_unchanged() {
    let (env, client, admin, _tok) = setup();
    let subscriber = Address::generate(&env);
    let reason = Some(soroban_sdk::String::from_str(&env, "chargeback"));

    client.add_to_blocklist(&admin, &subscriber, &reason);
    let before = client.get_blocklist_entry(&subscriber);

    let intruder = Address::generate(&env);
    let result = client.try_remove_from_blocklist(&intruder, &subscriber);

    assert_eq!(result, Err(Ok(Error::Forbidden)));
    assert!(client.is_blocklisted(&subscriber), "blocklist flag must survive a rejected removal");

    let after = client.get_blocklist_entry(&subscriber);
    assert_eq!(after.added_by, before.added_by);
    assert_eq!(after.added_at, before.added_at);
    assert_eq!(after.reason, before.reason);
    assert!(
        removed_events(&env, &client).is_empty(),
        "a Forbidden removal must not emit blocklist_removed"
    );
}

/// `do_remove_from_blocklist` is not idempotent: the first call succeeds and the
/// second reports `NotFound` because the key no longer exists.
#[test]
fn second_removal_is_not_found_after_success() {
    let (env, client, admin, _tok) = setup();
    let subscriber = Address::generate(&env);

    client.add_to_blocklist(&admin, &subscriber, &None::<String>);
    client.remove_from_blocklist(&admin, &subscriber);

    let second = client.try_remove_from_blocklist(&admin, &subscriber);

    assert_eq!(second, Err(Ok(Error::NotFound)));
    assert!(!client.is_blocklisted(&subscriber));
    assert_eq!(removed_events(&env, &client).len(), 1, "only the first removal emits");
}

/// The emitted event carries the exact caller, subscriber, ledger timestamp and
/// schema version required by indexers.
#[test]
fn remove_event_payload_is_exact() {
    let (env, client, admin, _tok) = setup();
    let subscriber = Address::generate(&env);

    client.add_to_blocklist(&admin, &subscriber, &None::<String>);
    env.ledger().with_mut(|l| l.timestamp = T0 + 60);

    client.remove_from_blocklist(&admin, &subscriber);

    let events = removed_events(&env, &client);
    assert_eq!(events.len(), 1);

    let event = &events[0];
    assert_eq!(event.subscriber, subscriber);
    assert_eq!(event.removed_by, admin);
    assert_eq!(event.timestamp, T0 + 60);
    assert_eq!(event.schema_version, EVENT_SCHEMA_VERSION);
}

/// Removing one subscriber must not disturb any other blocklisted address.
#[test]
fn removing_one_subscriber_leaves_others_blocklisted() {
    let (env, client, admin, _tok) = setup();
    let first = Address::generate(&env);
    let second = Address::generate(&env);
    let first_reason = Some(soroban_sdk::String::from_str(&env, "fraud"));
    let second_reason = Some(soroban_sdk::String::from_str(&env, "abuse"));

    client.add_to_blocklist(&admin, &first, &first_reason);
    client.add_to_blocklist(&admin, &second, &second_reason);

    client.remove_from_blocklist(&admin, &first);

    assert!(!client.is_blocklisted(&first));
    assert!(client.is_blocklisted(&second), "unrelated entry must remain");
    let surviving = client.get_blocklist_entry(&second);
    assert_eq!(surviving.reason, second_reason);

    let events = removed_events(&env, &client);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].subscriber, first);
}

/// A rejected attempt is side-effect free, so the legitimate admin can still
/// perform the removal afterwards.
#[test]
fn rejected_then_authorized_removal_succeeds() {
    let (env, client, admin, _tok) = setup();
    let subscriber = Address::generate(&env);
    let intruder = Address::generate(&env);

    client.add_to_blocklist(&admin, &subscriber, &None::<String>);

    let rejected = client.try_remove_from_blocklist(&intruder, &subscriber);
    assert_eq!(rejected, Err(Ok(Error::Forbidden)));
    assert!(client.is_blocklisted(&subscriber));

    client.remove_from_blocklist(&admin, &subscriber);

    assert!(!client.is_blocklisted(&subscriber));
    assert_eq!(removed_events(&env, &client).len(), 1);
}

/// `DataKey::Blocklist` is the only storage slot involved: the removal helper
/// does not touch the admin/config keys.
#[test]
fn removal_touches_only_the_blocklist_key() {
    let (env, client, admin, _tok) = setup();
    let subscriber = Address::generate(&env);
    let stored_admin_before = env.as_contract(&client.address, || {
        env.storage().instance().get::<_, Address>(&DataKey::Admin).unwrap()
    });

    client.add_to_blocklist(&admin, &subscriber, &None::<String>);
    client.remove_from_blocklist(&admin, &subscriber);

    let stored_admin_after = env.as_contract(&client.address, || {
        env.storage().instance().get::<_, Address>(&DataKey::Admin).unwrap()
    });
    assert_eq!(stored_admin_before, stored_admin_after);
    assert!(!client.is_blocklisted(&subscriber));
}
