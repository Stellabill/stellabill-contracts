//! Persistent-storage TTL exhaustion and recovery tests for `DataKey::Sub(id)`.
//!
//! # What this covers
//!
//! Subscription records live in **persistent** storage (`env.storage().persistent()`)
//! keyed by [`DataKey::Sub`]. Persistent entries carry a *time-to-live* (TTL): they
//! stay accessible only while `live_until_ledger >= current_ledger`. Every read
//! (`get_subscription`) and write (`write_subscription`) bumps the entry's TTL via
//! `extend_ttl(SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO)` — see
//! `contracts/subscription_vault/src/subscription.rs` and `docs/storage_layout.md`.
//!
//! These tests force the entry past its TTL by advancing the ledger sequence and pin
//! down three properties:
//!
//! 1. **Boundary** — the record is readable at *exactly* its last live ledger
//!    (`created_seq + SUB_TTL_EXTEND_TO`).
//! 2. **Exhaustion** — one ledger later the entry is expired and accessing it raises a
//!    Soroban **host error**, never a silent success returning stale data.
//! 3. **Recovery / second cycle** — touching the record at the last opportunity
//!    re-extends its TTL, so it survives past the original window with its data intact,
//!    and a *second* full TTL cycle eventually expires it again.
//!
//! # Observed host behavior (soroban-sdk 22 / soroban-env-host 22.1.3)
//!
//! Reading an **expired persistent entry** does NOT return `None` (which would surface
//! as a clean `Error::NotFound`). The host aborts with `Error(Storage, InternalError)`,
//! which the test harness surfaces as a Rust panic. `try_get_subscription` does **not**
//! convert this into a recoverable `Err(...)` either — the host error propagates as a
//! panic. The correct, faithful assertion is therefore `catch_unwind` / `#[should_panic]`,
//! not `expect_err`. This is the *safe* outcome: an expired record can never be read as
//! live data; on-chain it would require a `RestoreFootprint` operation before access.
//!
//! # Why the contract instance TTL is extended in these tests
//!
//! Invoking any contract entry-point requires the contract **instance** entry to be live.
//! The instance is created with only the default minimum TTL and is not re-extended on
//! every call, so naively advancing the ledger ~`SUB_TTL_EXTEND_TO` ledgers would expire
//! the instance long before the `Sub` entry — and `get_subscription` would then fail for
//! the wrong reason. [`keep_instance_alive`] bumps only the instance TTL so that the
//! **sole** entry whose expiry governs the outcome under test is `DataKey::Sub(id)`.

#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env,
};
use subscription_vault::{
    Subscription, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
    SUB_TTL_EXTEND_TO,
};

// ── Shared constants ────────────────────────────────────────────────────────────

const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
const MIN_TOPUP: i128 = 1_000_000;
const GRACE: u64 = 7 * 24 * 60 * 60;

/// Ledger sequence at which the subscription is created in every test.
const CREATED_SEQ: u32 = 100;

/// Last ledger at which a freshly written `Sub` entry is still live.
///
/// `write_subscription` extends the entry to `created_seq + SUB_TTL_EXTEND_TO`; the
/// entry is live for `seq <= LIVE_UNTIL` and expired for `seq > LIVE_UNTIL`. This is
/// pinned by the boundary test below.
const LIVE_UNTIL: u32 = CREATED_SEQ + SUB_TTL_EXTEND_TO;

// ── Helpers ─────────────────────────────────────────────────────────────────────

/// Register + initialise the vault with a real SAC token, starting at [`CREATED_SEQ`].
///
/// `max_entry_ttl` is set comfortably above `SUB_TTL_EXTEND_TO` so the contract's
/// `extend_ttl(.., SUB_TTL_EXTEND_TO)` is honoured (not clamped) on write.
fn setup() -> (Env, SubscriptionVaultClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|li| {
        li.sequence_number = CREATED_SEQ;
        li.min_persistent_entry_ttl = 4096;
        li.min_temp_entry_ttl = 4096;
        li.max_entry_ttl = SUB_TTL_EXTEND_TO + 5_000_000;
    });

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    let vault_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &vault_id);
    client.init(&token, &6u32, &admin, &MIN_TOPUP, &GRACE);
    (env, client)
}

/// Create a default active subscription and return its id.
fn create_sub(env: &Env, client: &SubscriptionVaultClient) -> u32 {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    )
}

/// Keep the contract **instance** entry alive far beyond any TTL boundary under test,
/// so that only the `Sub` entry's expiry can cause `get_subscription` to fail.
fn keep_instance_alive(env: &Env, contract: &Address) {
    let big = SUB_TTL_EXTEND_TO + 4_000_000;
    env.as_contract(contract, || {
        env.storage().instance().extend_ttl(big, big);
    });
}

/// Set the ledger sequence number.
fn set_seq(env: &Env, seq: u32) {
    env.ledger().with_mut(|li| li.sequence_number = seq);
}

/// Returns `true` if `get_subscription(id)` completes without raising a host error.
///
/// An expired persistent entry causes the host to abort (surfaced as a panic), so this
/// captures that boundary cleanly without aborting the test process. The default panic
/// hook is silenced for the duration to keep test output free of host backtraces.
fn read_succeeds(client: &SubscriptionVaultClient, id: u32) -> bool {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Subscription = client.get_subscription(&id);
    }));
    std::panic::set_hook(prev);
    result.is_ok()
}

// ── 1. Boundary: live at exactly LIVE_UNTIL, expired one ledger later ────────────

/// At exactly `created_seq + SUB_TTL_EXTEND_TO` the record is still live and returns
/// its stored data. This pins the precise expiry boundary the other tests rely on.
#[test]
fn readable_at_exact_ttl_boundary() {
    let (env, client) = setup();
    let id = create_sub(&env, &client);
    keep_instance_alive(&env, &client.address);

    set_seq(&env, LIVE_UNTIL);
    let sub = client.get_subscription(&id);
    assert_eq!(sub.amount, AMOUNT, "record must be readable on its last live ledger");
    assert_eq!(sub.status, SubscriptionStatus::Active);
}

/// One ledger past `LIVE_UNTIL` the `Sub` entry is expired. Accessing it raises a host
/// error (`Error(Storage, InternalError)`) — it must NOT silently succeed with stale
/// data. We read at the boundary first (positive control) to prove the entry existed,
/// then advance one ledger and assert the access now aborts.
#[test]
fn expired_entry_access_raises_host_error_not_stale_read() {
    let (env, client) = setup();
    let id = create_sub(&env, &client);
    keep_instance_alive(&env, &client.address);

    // Positive control: live at the boundary.
    set_seq(&env, LIVE_UNTIL);
    assert!(
        read_succeeds(&client, id),
        "entry must be live at its last live ledger"
    );

    // Re-extend instance only (the read above already re-extended the Sub entry, so use
    // a fresh subscription to isolate a clean, un-refreshed expiry instead).
    let (env2, client2) = setup();
    let id2 = create_sub(&env2, &client2);
    keep_instance_alive(&env2, &client2.address);
    set_seq(&env2, LIVE_UNTIL + 1);
    assert!(
        !read_succeeds(&client2, id2),
        "accessing an expired Sub entry must raise a host error, not return stale data"
    );
}

/// Same exhaustion property expressed with `#[should_panic]`, matching the host error
/// kind so a future SDK change that turns this into a clean `NotFound` will surface as a
/// deliberate test failure to be re-evaluated.
#[test]
#[should_panic(expected = "Storage")]
fn expired_entry_get_subscription_panics() {
    let (env, client) = setup();
    let id = create_sub(&env, &client);
    keep_instance_alive(&env, &client.address);

    set_seq(&env, LIVE_UNTIL + 1);
    // Expired persistent access -> Error(Storage, InternalError) -> panic in the host.
    let _ = client.get_subscription(&id);
}

// ── 2. Extension at the last opportunity restores access ─────────────────────────

/// Reading the record at *exactly* its last live ledger calls `extend_subscription_ttl`
/// (remaining TTL is 0, well below `SUB_TTL_THRESHOLD`), bumping `live_until` to
/// `seq + SUB_TTL_EXTEND_TO`. The record must then remain accessible at a ledger that
/// would have been past the *original* window.
#[test]
fn extend_at_last_opportunity_restores_access() {
    let (env, client) = setup();
    let id = create_sub(&env, &client);
    keep_instance_alive(&env, &client.address);

    // Touch at the final live ledger -> re-extends TTL to LIVE_UNTIL + SUB_TTL_EXTEND_TO.
    set_seq(&env, LIVE_UNTIL);
    let _ = client.get_subscription(&id);
    keep_instance_alive(&env, &client.address);

    // Past the original window; only accessible because the read above refreshed the TTL.
    let past_original = LIVE_UNTIL + 1_000;
    assert!(
        past_original > LIVE_UNTIL,
        "sanity: target is past the original live_until"
    );
    set_seq(&env, past_original);
    let sub = client.get_subscription(&id);
    assert_eq!(
        sub.amount, AMOUNT,
        "record must remain readable after a last-opportunity TTL extension"
    );
}

// ── 3. Second TTL cycle preserves data, then expires again ───────────────────────

/// Two full TTL cycles: refresh at the end of cycle one, confirm the data is intact at
/// the end of cycle two, then let the entry expire past the refreshed window.
#[test]
fn second_ttl_cycle_preserves_data_then_expires() {
    let (env, client) = setup();
    let id = create_sub(&env, &client);
    keep_instance_alive(&env, &client.address);

    // Capture original data for an end-to-end equality check after the cycles.
    let original = client.get_subscription(&id);

    // Cycle 1: advance to the (refreshed) last live ledger and read to refresh again.
    // The read at CREATED_SEQ above bumped live_until to CREATED_SEQ + SUB_TTL_EXTEND_TO,
    // i.e. LIVE_UNTIL; touch there to start cycle 2.
    set_seq(&env, LIVE_UNTIL);
    let _ = client.get_subscription(&id);
    keep_instance_alive(&env, &client.address);

    // Cycle 2: still within the refreshed window — data must be byte-for-byte intact.
    let refreshed_live_until = LIVE_UNTIL + SUB_TTL_EXTEND_TO;
    set_seq(&env, refreshed_live_until);
    let after = client.get_subscription(&id);
    assert_eq!(after.subscriber, original.subscriber);
    assert_eq!(after.merchant, original.merchant);
    assert_eq!(after.token, original.token);
    assert_eq!(after.amount, original.amount);
    assert_eq!(after.interval_seconds, original.interval_seconds);
    assert_eq!(after.status, original.status);
    assert_eq!(after.prepaid_balance, original.prepaid_balance);
    assert_eq!(after.start_time, original.start_time);

    // The read at `refreshed_live_until` refreshed once more; use a value past THAT
    // window to confirm the entry still ultimately expires (TTL is finite per cycle).
    keep_instance_alive(&env, &client.address);
    let past_second_refresh = refreshed_live_until + SUB_TTL_EXTEND_TO + 1;
    set_seq(&env, past_second_refresh);
    assert!(
        !read_succeeds(&client, id),
        "after the final refresh window lapses the entry must expire again"
    );
}
