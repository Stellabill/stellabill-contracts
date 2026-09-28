//! Adversarial coverage for the blocklist admin surface — `do_add_to_blocklist`
//! (reached through `add_to_blocklist`).
//!
//! `do_add_to_blocklist` is the only write path onto the blocklist, and every
//! payee-touching money path is gated by `require_not_blocklisted`. That makes
//! the *rejection* behaviour of this function load-bearing: an admin-only guard
//! that silently succeeds, or a duplicate guard that overwrites the audit trail,
//! is a fund-safety and compliance defect rather than a cosmetic one.
//!
//! Coverage:
//!
//! | Test | Scenario |
//! |------|----------|
//! | `admin_add_records_caller_and_ledger_timestamp` | Happy path stores the caller, timestamp and reason |
//! | `add_emits_blocklist_added_event_with_full_payload` | Event topics + payload + schema version |
//! | `duplicate_add_is_rejected_and_preserves_original_audit_trail` | Second add fails without overwriting added_by/added_at/reason |
//! | `duplicate_add_emits_no_second_event` | Rejected duplicate is not observable as an add |
//! | `non_admin_call_is_rejected_and_leaves_no_state` | Unauthorized caller cannot blocklist |
//! | `non_admin_call_emits_no_event` | Rejected caller produces no phantom event |
//! | `add_on_uninitialized_contract_is_rejected` | No admin configured → `NotInitialized`, nothing written |
//! | `none_and_empty_reason_are_stored_distinctly` | `Option<String>` boundary is not collapsed |
//! | `long_reason_is_stored_verbatim` | Reason is not silently truncated |
//! | `blocklist_is_keyed_per_subscriber` | Adding one address does not block another |
//! | `authorizer_is_not_implicitly_blocklisted` | Admin who adds is not itself blocked |
//! | `readd_after_remove_uses_the_new_timestamp_and_reason` | Duplicate guard reads live state, not a tombstone |
//! | `timestamp_is_re_read_on_every_add` | `added_at` follows the ledger, not a cached value |
//!
//! Every rejection path additionally asserts that persisted state is unchanged,
//! so a future refactor that writes before validating is caught.
//!
//! Note on `Env::events()`: the host exposes the events of the *most recent*
//! top-level invocation only, so each event assertion below reads the buffer
//! immediately after the call it describes — before any further contract call,
//! including a read-only one, can reset it.
#![cfg(test)]

use crate::{
    types::{Error, EVENT_SCHEMA_VERSION},
    BlocklistAddedEvent, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    Address, Env, IntoVal, String, Symbol, TryFromVal, Val, Vec,
};

const TOKEN_DECIMALS: u32 = 6;
const MIN_TOPUP: i128 = 1_000_000;
const GRACE_PERIOD: u64 = 7 * 24 * 60 * 60;
const INITIAL_TIMESTAMP: u64 = 1_000_000;
const ADDED_TOPIC: &str = "blocklist_added";

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(INITIAL_TIMESTAMP);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(
        &token,
        &TOKEN_DECIMALS,
        &admin,
        &MIN_TOPUP,
        &GRACE_PERIOD,
    );

    (env, client, admin)
}

/// Every `(topics, data)` pair whose first topic is `blocklist_added`.
fn blocklist_added_events(env: &Env) -> Vec<(Vec<Val>, Val)> {
    let all = env.events().all();
    let want = Symbol::new(env, ADDED_TOPIC);
    let mut out = Vec::new(env);
    for i in 0..all.len() {
        let (_, topics, data): (Address, Vec<Val>, Val) = all.get(i).unwrap();
        let is_added = match topics.get(0) {
            Some(val) => Symbol::try_from_val(env, &val)
                .map(|sym| sym == want)
                .unwrap_or(false),
            None => false,
        };
        if is_added {
            out.push_back((topics, data));
        }
    }
    out
}

fn added_event_count(env: &Env) -> u32 {
    blocklist_added_events(env).len()
}

fn reason(env: &Env, text: &str) -> Option<String> {
    Some(String::from_str(env, text))
}

fn advance_seconds(env: &Env, seconds: u64) {
    env.ledger().set_timestamp(env.ledger().timestamp() + seconds);
}

// ── Happy path ───────────────────────────────────────────────────────────────

#[test]
fn admin_add_records_caller_and_ledger_timestamp() {
    let (env, client, admin) = setup();
    let subscriber = Address::generate(&env);

    assert!(!client.is_blocklisted(&subscriber));

    client.add_to_blocklist(&admin, &subscriber, &reason(&env, "chargeback fraud"));

    assert!(client.is_blocklisted(&subscriber));
    let entry = client.get_blocklist_entry(&subscriber);
    assert_eq!(entry.subscriber, subscriber);
    assert_eq!(entry.added_by, admin);
    assert_eq!(entry.added_at, INITIAL_TIMESTAMP);
    assert_eq!(entry.reason, reason(&env, "chargeback fraud"));
}

#[test]
fn add_emits_blocklist_added_event_with_full_payload() {
    let (env, client, admin) = setup();
    let subscriber = Address::generate(&env);

    client.add_to_blocklist(&admin, &subscriber, &reason(&env, "abuse"));

    let events = blocklist_added_events(&env);
    assert_eq!(events.len(), 1, "exactly one blocklist_added event");

    let (topics, data) = events.get(0).unwrap();
    let second_topic = topics
        .get(1)
        .map(|val| Address::try_from_val(&env, &val).unwrap());
    assert!(
        second_topic == Some(subscriber.clone()),
        "second topic must be the blocklisted address"
    );

    let parsed = BlocklistAddedEvent::try_from_val(&env, &data).unwrap();
    assert_eq!(parsed.subscriber, subscriber);
    assert_eq!(parsed.added_by, admin);
    assert_eq!(parsed.timestamp, INITIAL_TIMESTAMP);
    assert_eq!(parsed.reason, reason(&env, "abuse"));
    assert_eq!(parsed.schema_version, EVENT_SCHEMA_VERSION);
}

// ── Duplicate guard ──────────────────────────────────────────────────────────

#[test]
fn duplicate_add_is_rejected_and_preserves_original_audit_trail() {
    let (env, client, admin) = setup();
    let subscriber = Address::generate(&env);

    client.add_to_blocklist(&admin, &subscriber, &reason(&env, "first reason"));
    let original = client.get_blocklist_entry(&subscriber);

    // A later attempt, with a different reason at a different ledger time,
    // must not be able to rewrite what the auditor will read.
    advance_seconds(&env, 5_000);
    assert_eq!(
        client.try_add_to_blocklist(&admin, &subscriber, &reason(&env, "second reason")),
        Err(Ok(Error::InvalidInput))
    );

    let after = client.get_blocklist_entry(&subscriber);
    assert_eq!(after.subscriber, original.subscriber);
    assert_eq!(after.added_by, original.added_by);
    assert_eq!(after.added_at, INITIAL_TIMESTAMP);
    assert_eq!(
        after.reason,
        reason(&env, "first reason"),
        "rejected duplicate must not rewrite the recorded reason"
    );
    assert!(client.is_blocklisted(&subscriber));
}

#[test]
fn duplicate_add_emits_no_second_event() {
    let (env, client, admin) = setup();
    let subscriber = Address::generate(&env);

    client.add_to_blocklist(&admin, &subscriber, &None);
    assert_eq!(added_event_count(&env), 1, "the accepted add emits one event");

    // `Env::events()` exposes only the most recent top-level invocation, so a
    // zero count here means the rejected call emitted nothing at all.
    let _ = client.try_add_to_blocklist(&admin, &subscriber, &None);
    assert_eq!(
        added_event_count(&env),
        0,
        "a rejected duplicate add must not be observable as an event"
    );
}

// ── Authorization ────────────────────────────────────────────────────────────

#[test]
fn non_admin_call_is_rejected_and_leaves_no_state() {
    let (env, client, _admin) = setup();
    let stranger = Address::generate(&env);
    let subscriber = Address::generate(&env);

    assert_eq!(
        client.try_add_to_blocklist(&stranger, &subscriber, &None),
        Err(Ok(Error::Forbidden))
    );

    assert!(
        !client.is_blocklisted(&subscriber),
        "an unauthorized caller must not be able to blocklist anyone"
    );
    assert!(
        matches!(
            client.try_get_blocklist_entry(&subscriber),
            Err(Ok(Error::NotFound))
        ),
        "no entry may be written by a rejected caller"
    );
}

#[test]
fn non_admin_call_emits_no_event() {
    let (env, client, _admin) = setup();
    let stranger = Address::generate(&env);
    let subscriber = Address::generate(&env);

    let _ = client.try_add_to_blocklist(&stranger, &subscriber, &reason(&env, "spoofed"));

    assert_eq!(
        added_event_count(&env),
        0,
        "a rejected caller must not produce a phantom blocklist_added event"
    );
}

#[test]
fn add_on_uninitialized_contract_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let caller = Address::generate(&env);
    let subscriber = Address::generate(&env);

    assert_eq!(
        client.try_add_to_blocklist(&caller, &subscriber, &None),
        Err(Ok(Error::NotInitialized))
    );
    assert_eq!(added_event_count(&env), 0);
}

// ── Input boundaries ─────────────────────────────────────────────────────────

#[test]
fn none_and_empty_reason_are_stored_distinctly() {
    let (env, client, admin) = setup();
    let no_reason = Address::generate(&env);
    let empty_reason = Address::generate(&env);

    client.add_to_blocklist(&admin, &no_reason, &None);
    client.add_to_blocklist(&admin, &empty_reason, &reason(&env, ""));

    assert!(client.get_blocklist_entry(&no_reason).reason.is_none());
    assert!(
        client.get_blocklist_entry(&empty_reason).reason == reason(&env, ""),
        "an explicitly empty reason must stay distinct from an absent one"
    );
}

#[test]
fn long_reason_is_stored_verbatim() {
    let (env, client, admin) = setup();
    let subscriber = Address::generate(&env);
    let long = "r".repeat(512);

    client.add_to_blocklist(&admin, &subscriber, &reason(&env, &long));

    let stored = client.get_blocklist_entry(&subscriber).reason.unwrap();
    assert_eq!(
        stored.len(),
        512,
        "reason must not be silently truncated by the add path"
    );
    assert_eq!(stored, String::from_str(&env, &long));
}

// ── Isolation and re-add semantics ───────────────────────────────────────────

#[test]
fn blocklist_is_keyed_per_subscriber() {
    let (env, client, admin) = setup();
    let left = Address::generate(&env);
    let right = Address::generate(&env);

    client.add_to_blocklist(&admin, &left, &reason(&env, "left"));

    assert!(client.is_blocklisted(&left));
    assert!(
        !client.is_blocklisted(&right),
        "blocklisting one address must not block another"
    );

    client.remove_from_blocklist(&admin, &left);
    assert!(!client.is_blocklisted(&left));
    assert!(!client.is_blocklisted(&right));
}

#[test]
fn authorizer_is_not_implicitly_blocklisted() {
    let (env, client, admin) = setup();
    let subscriber = Address::generate(&env);

    client.add_to_blocklist(&admin, &subscriber, &None);

    assert!(
        !client.is_blocklisted(&admin),
        "the admin who adds must not be caught by its own entry"
    );
}

#[test]
fn readd_after_remove_uses_the_new_timestamp_and_reason() {
    let (env, client, admin) = setup();
    let subscriber = Address::generate(&env);

    client.add_to_blocklist(&admin, &subscriber, &reason(&env, "first"));
    assert_eq!(added_event_count(&env), 1, "first add emits one event");

    client.remove_from_blocklist(&admin, &subscriber);
    assert!(!client.is_blocklisted(&subscriber));

    advance_seconds(&env, 3_600);
    client.add_to_blocklist(&admin, &subscriber, &reason(&env, "second"));
    // Read the event buffer before any further contract call resets it.
    let events_after_readd = added_event_count(&env);

    let entry = client.get_blocklist_entry(&subscriber);
    assert_eq!(entry.added_at, INITIAL_TIMESTAMP + 3_600);
    assert_eq!(entry.reason, reason(&env, "second"));
    assert_eq!(
        events_after_readd, 1,
        "the re-add after a removal emits exactly one fresh event"
    );
}

#[test]
fn timestamp_is_re_read_on_every_add() {
    let (env, client, admin) = setup();
    let first = Address::generate(&env);
    let second = Address::generate(&env);

    client.add_to_blocklist(&admin, &first, &None);
    advance_seconds(&env, 42);
    client.add_to_blocklist(&admin, &second, &None);

    assert_eq!(client.get_blocklist_entry(&first).added_at, INITIAL_TIMESTAMP);
    assert_eq!(
        client.get_blocklist_entry(&second).added_at,
        INITIAL_TIMESTAMP + 42
    );
}
