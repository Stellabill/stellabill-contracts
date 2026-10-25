#![cfg(test)]

//! Adversarial coverage for `set_fee_token` in `admin.rs` (issue #1007).
//!
//! The issue's evidence points at the `.unwrap()` calls in
//! `validate_treasury_split` (admin.rs:764/774). Those indices are bounded by
//! `entries.len()`, so they cannot panic; the real gap is that `set_fee_token`
//! itself has **no dedicated adversarial coverage**. This file closes that gap
//! and, for completeness, includes focused boundary tests for
//! `validate_treasury_split` (the flagged lines) further down.
//!
//! `set_fee_token` routes protocol fees through an oracle-converted token at
//! charge time (`charge_core::convert_fee` reads `admin::get_fee_token`). It:
//!
//! 1. `admin.require_auth()` — host-level signature check.
//! 2. `require_admin(env)` + equality check — returns `Error::Unauthorized`
//!    (not `Forbidden`; this is the one admin setter that does its own check
//!    instead of funnelling through `require_admin_auth`).
//! 3. `enforce_config_cooldown(env, "FeeToken")` — six-hour per-key cooldown.
//! 4. `write_config(DataKey::FeeToken, token)` or, for `None`,
//!    `remove_config` (clears **both** storage tiers).
//! 5. Emits `FeeTokenConfiguredEvent`.
//!
//! `set_fee_token` / `get_fee_token` are internal helpers (there is no
//! `#[contractimpl]` entrypoint for either), so — like
//! `test_admin_get_token.rs` — they are exercised inside the vault's storage
//! context via `env.as_contract`.

use crate::admin::{get_fee_token, set_fee_token, CONFIG_COOLDOWN_SECS};
use crate::test_utils::setup::TestEnv;
use crate::types::{DataKey, Error, FeeTokenConfiguredEvent, TreasurySplitEntry};
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    Address, Env, FromVal, Symbol, TryFromVal, Vec,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Non-zero starting ledger time.
///
/// The cooldown sentinel is `prev_ts == 0` ("never mutated"), so a first
/// mutation *at* ledger timestamp 0 does not arm the window. Starting at a
/// non-zero time keeps the cooldown tests meaningful; the timestamp-0 quirk is
/// pinned explicitly by `first_mutation_at_timestamp_zero_does_not_arm_cooldown`.
const T0: u64 = 1_000_000;

fn setup() -> TestEnv {
    let te = TestEnv::default();
    te.env.ledger().set_timestamp(T0);
    te
}

/// Call the internal `set_fee_token` inside the vault's storage context.
fn set(te: &TestEnv, admin: &Address, token: Option<Address>) -> Result<(), Error> {
    te.env.as_contract(&te.client.address, || {
        set_fee_token(&te.env, admin.clone(), token.clone())
    })
}

/// Read the override through the internal getter in the vault's context.
fn get(te: &TestEnv) -> Option<Address> {
    te.env
        .as_contract(&te.client.address, || get_fee_token(&te.env))
}

/// Raw value of the `DataKey::FeeToken` config slot.
fn stored_fee_token(te: &TestEnv) -> Option<Address> {
    te.env.as_contract(&te.client.address, || {
        crate::admin::read_config(&te.env, &DataKey::FeeToken)
    })
}

/// The default settlement token, read through the internal getter.
fn default_token(te: &TestEnv) -> Address {
    te.env
        .as_contract(&te.client.address, || crate::admin::get_token(&te.env))
        .expect("vault is initialised in this test")
}

// ── Default / unset state ─────────────────────────────────────────────────────

/// A freshly initialised vault has no fee-token override.
#[test]
fn default_is_none_after_init() {
    let te = setup();
    assert_eq!(get(&te), None);
    assert_eq!(stored_fee_token(&te), None);
}

/// Reading on a registered-but-uninitialised contract is `None`, not a panic.
#[test]
fn uninitialized_contract_reads_as_none() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let _client = SubscriptionVaultClient::new(&env, &contract_id);

    let got = env.as_contract(&contract_id, || get_fee_token(&env));
    assert_eq!(got, None);
}

/// Writing on an uninitialised contract fails with `NotInitialized` (proving
/// the admin lookup is reached instead of silently succeeding).
#[test]
fn uninitialized_contract_rejects_set_with_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let caller = Address::generate(&env);
    let token = Address::generate(&env);

    let res = env.as_contract(&contract_id, || set_fee_token(&env, caller, Some(token)));
    assert_eq!(res, Err(Error::NotInitialized));
}

// ── Happy paths ───────────────────────────────────────────────────────────────

/// Setting an override round-trips through `get_fee_token` and is stored in the
/// persistent tier (schema ≥ 3), not instance storage.
#[test]
fn set_some_round_trips_and_uses_persistent_tier() {
    let te = setup();
    let fee_token = Address::generate(&te.env);

    assert_eq!(set(&te, &te.admin, Some(fee_token.clone())), Ok(()));

    assert_eq!(get(&te), Some(fee_token.clone()));
    assert_eq!(stored_fee_token(&te), Some(fee_token.clone()));

    te.env.as_contract(&te.client.address, || {
        let storage = te.env.storage();
        assert!(
            storage.persistent().has(&DataKey::FeeToken),
            "override must live in persistent storage"
        );
        assert!(
            !storage.instance().has(&DataKey::FeeToken),
            "write_config must clear the legacy instance copy"
        );
    });
}

/// Clearing with `None` after a successful set removes the key from **both**
/// storage tiers.
#[test]
fn set_none_after_some_clears_both_tiers() {
    let te = setup();
    let fee_token = Address::generate(&te.env);

    assert_eq!(set(&te, &te.admin, Some(fee_token)), Ok(()));
    te.jump(CONFIG_COOLDOWN_SECS);

    assert_eq!(set(&te, &te.admin, None), Ok(()));

    assert_eq!(get(&te), None);
    assert_eq!(stored_fee_token(&te), None);
    te.env.as_contract(&te.client.address, || {
        let storage = te.env.storage();
        assert!(!storage.persistent().has(&DataKey::FeeToken));
        assert!(!storage.instance().has(&DataKey::FeeToken));
    });
}

/// The first mutation on a fresh key always succeeds (cooldown starts at 0).
#[test]
fn first_mutation_always_succeeds() {
    let te = setup();
    let fee_token = Address::generate(&te.env);
    assert_eq!(set(&te, &te.admin, Some(fee_token)), Ok(()));
}

/// Documents a real quirk in `enforce_config_cooldown`: `prev_ts == 0` is the
/// "never mutated" sentinel, so a successful write that lands at ledger
/// timestamp **0** does not arm the cooldown. A second write at the same
/// timestamp is therefore *not* rejected. This is only reachable on a chain
/// whose ledger clock is still 0 (tests / genesis), but pinning it makes the
/// sentinel behaviour explicit and guards against a future change that assumes
/// any successful write arms the window.
#[test]
fn first_mutation_at_timestamp_zero_does_not_arm_cooldown() {
    let te = TestEnv::default();
    te.env.ledger().with_mut(|l| l.timestamp = 0);
    let a = Address::generate(&te.env);
    let b = Address::generate(&te.env);

    assert_eq!(set(&te, &te.admin, Some(a)), Ok(()));

    // Cooldown is not enforced because prev_ts == 0 (the "never mutated"
    // sentinel is indistinguishable from a write at timestamp 0).
    assert_eq!(set(&te, &te.admin, Some(b.clone())), Ok(()));
    assert_eq!(get(&te), Some(b));
}

/// Setting the override to the vault's own default settlement token is
/// accepted verbatim — there is no self-token guard on this setter.
/// (`convert_fee` treats `override == source` as a no-op, so this is a
/// deliberately inert configuration.)
#[test]
fn set_to_default_token_is_allowed_verbatim() {
    let te = setup();
    let default = default_token(&te);

    assert_eq!(set(&te, &te.admin, Some(default.clone())), Ok(()));
    assert_eq!(get(&te), Some(default));
}

// ── Authorization ─────────────────────────────────────────────────────────────

/// A non-admin caller is rejected with `Unauthorized` (this setter does its own
/// comparison rather than using `require_admin_auth`, so it does not return
/// `Forbidden`).
#[test]
fn non_admin_is_rejected_with_unauthorized() {
    let te = setup();
    let stranger = Address::generate(&te.env);
    let fee_token = Address::generate(&te.env);

    assert_eq!(
        set(&te, &stranger, Some(fee_token)),
        Err(Error::Unauthorized)
    );
    // State unchanged.
    assert_eq!(get(&te), None);
}

/// The rejected non-admin call must not arm the cooldown: the real admin can
/// mutate immediately afterwards at the same ledger time.
#[test]
fn rejected_non_admin_does_not_arm_cooldown() {
    let te = setup();
    let stranger = Address::generate(&te.env);
    let fee_token = Address::generate(&te.env);

    assert_eq!(
        set(&te, &stranger, Some(fee_token.clone())),
        Err(Error::Unauthorized)
    );

    // Same timestamp, correct admin: succeeds, proving the failed call never
    // armed the "FeeToken" cooldown.
    assert_eq!(set(&te, &te.admin, Some(fee_token.clone())), Ok(()));
    assert_eq!(get(&te), Some(fee_token));
}

/// Authorization is checked against stored state, not the supplied address:
/// passing the stored admin while it is *not* the signer is the host's job, but
/// a stale admin after rotation is rejected outright.
#[test]
fn stale_admin_after_rotation_is_rejected() {
    let te = setup();
    let old_admin = te.admin.clone();
    let new_admin = Address::generate(&te.env);
    let fee_token = Address::generate(&te.env);

    // Rotate the admin (nonce for the admin-rotation domain starts at 0).
    let nonce = te
        .client
        .get_admin_nonce(&old_admin, &crate::nonce::DOMAIN_ADMIN_ROTATION);
    te.client.rotate_admin(&old_admin, &new_admin, &nonce);

    assert_eq!(
        set(&te, &old_admin, Some(fee_token.clone())),
        Err(Error::Unauthorized),
        "rotated-out admin must not configure the fee token"
    );
    assert_eq!(get(&te), None);

    // The new admin is accepted.
    assert_eq!(set(&te, &new_admin, Some(fee_token.clone())), Ok(()));
    assert_eq!(get(&te), Some(fee_token));
}

// ── Cooldown ──────────────────────────────────────────────────────────────────

/// A second mutation within six hours is rejected and leaves the previous value
/// in place.
#[test]
fn second_mutation_within_cooldown_rejected_and_state_unchanged() {
    let te = setup();
    let first = Address::generate(&te.env);
    let second = Address::generate(&te.env);

    assert_eq!(set(&te, &te.admin, Some(first.clone())), Ok(()));

    te.jump(CONFIG_COOLDOWN_SECS - 1);
    assert_eq!(
        set(&te, &te.admin, Some(second)),
        Err(Error::CooldownActive)
    );
    assert_eq!(get(&te), Some(first), "rejected call must not change state");
}

/// Exactly at the cooldown boundary the mutation is allowed again.
#[test]
fn mutation_at_exact_cooldown_boundary_succeeds() {
    let te = setup();
    let first = Address::generate(&te.env);
    let second = Address::generate(&te.env);

    assert_eq!(set(&te, &te.admin, Some(first)), Ok(()));
    te.jump(CONFIG_COOLDOWN_SECS);

    assert_eq!(set(&te, &te.admin, Some(second.clone())), Ok(()));
    assert_eq!(get(&te), Some(second));
}

/// Setting the *same* value again within the window is still gated — the
/// cooldown is value-independent.
#[test]
fn no_op_reset_within_cooldown_is_still_rejected() {
    let te = setup();
    let fee_token = Address::generate(&te.env);

    assert_eq!(set(&te, &te.admin, Some(fee_token.clone())), Ok(()));
    assert_eq!(
        set(&te, &te.admin, Some(fee_token)),
        Err(Error::CooldownActive)
    );
}

/// A rejected cooldown attempt must not push the deadline forward: the next
/// successful write happens six hours after the last *successful* write.
#[test]
fn rejected_cooldown_attempt_does_not_extend_deadline() {
    let te = setup();
    let first = Address::generate(&te.env);
    let second = Address::generate(&te.env);

    assert_eq!(set(&te, &te.admin, Some(first.clone())), Ok(()));

    // One second later the second write is rejected...
    te.jump(1);
    assert_eq!(
        set(&te, &te.admin, Some(second.clone())),
        Err(Error::CooldownActive)
    );

    // ...and one second before the window measured from the *original* success
    // it is still rejected.
    te.jump(CONFIG_COOLDOWN_SECS - 2);
    assert_eq!(
        set(&te, &te.admin, Some(second.clone())),
        Err(Error::CooldownActive)
    );

    // Exactly +6h from the original success — not from the failed attempt — the
    // write goes through, proving the rejected call did not push the deadline.
    te.jump(1);
    assert_eq!(set(&te, &te.admin, Some(second.clone())), Ok(()));
    assert_eq!(get(&te), Some(second));
}

/// The cooldown is per-key: configuring the fee token does not block an
/// unrelated admin config key at the same ledger time.
#[test]
fn cooldown_is_per_key_not_global() {
    let te = setup();
    let fee_token = Address::generate(&te.env);

    assert_eq!(set(&te, &te.admin, Some(fee_token)), Ok(()));

    // Different key ("MinTopup") at the same timestamp must succeed.
    let res = te.client.try_set_min_topup(&te.admin, &2_000_000i128);
    assert_eq!(res, Ok(Ok(())));
}

/// The cooldown is armed by a successful mutation: an immediate second write
/// is rejected.
#[test]
fn cooldown_armed_only_on_success() {
    let te = setup();
    let first = Address::generate(&te.env);
    let second = Address::generate(&te.env);

    assert_eq!(set(&te, &te.admin, Some(first.clone())), Ok(()));
    assert_eq!(
        set(&te, &te.admin, Some(second)),
        Err(Error::CooldownActive),
        "the first successful write must arm the window"
    );
    assert_eq!(get(&te), Some(first));
}

// ── Purity & isolation from other config ──────────────────────────────────────

/// Repeated reads are pure and stable.
#[test]
fn reads_are_pure_and_idempotent() {
    let te = setup();
    let fee_token = Address::generate(&te.env);
    assert_eq!(set(&te, &te.admin, Some(fee_token.clone())), Ok(()));

    for _ in 0..5 {
        assert_eq!(get(&te), Some(fee_token.clone()));
    }
}

/// Configuring the fee token must not disturb the default settlement token.
#[test]
fn fee_token_override_does_not_change_default_token() {
    let te = setup();
    let default = default_token(&te);
    let fee_token = Address::generate(&te.env);

    assert_eq!(set(&te, &te.admin, Some(fee_token)), Ok(()));

    assert_eq!(default_token(&te), default);
}

// ── Event ─────────────────────────────────────────────────────────────────────

/// A successful call emits `fee_token_configured` carrying the new override and
/// the ledger timestamp. `old_token` is currently always `None` — pinned here
/// because it is a known wart (see PR follow-ups), not because it is desirable.
#[test]
fn success_emits_fee_token_configured_event() {
    let te = setup();
    let fee_token = Address::generate(&te.env);
    let now = te.env.ledger().timestamp();

    assert_eq!(set(&te, &te.admin, Some(fee_token.clone())), Ok(()));

    let mut found = false;
    let events = te.env.events().all();
    for i in 0..events.len() {
        let (_contract, topics, data) = events.get_unchecked(i);
        let topic0 = Symbol::from_val(&te.env, &topics.get_unchecked(0));
        if topic0 == Symbol::new(&te.env, "fee_token_configured") {
            let evt: FeeTokenConfiguredEvent =
                TryFromVal::try_from_val(&te.env, &data).expect("event payload must decode");
            assert_eq!(evt.admin, te.admin);
            assert_eq!(evt.fee_token, Some(fee_token.clone()));
            assert_eq!(evt.new_token, Some(fee_token.clone()));
            assert_eq!(evt.old_token, None);
            assert_eq!(evt.timestamp, now);
            assert_eq!(evt.schema_version, crate::types::EVENT_SCHEMA_VERSION);
            found = true;
        }
    }
    assert!(found, "fee_token_configured event not found");
}

// ── Flagged evidence: validate_treasury_split bounds ──────────────────────────

/// `validate_treasury_split` uses `entries.get(i).unwrap()` on indices bounded
/// by `entries.len()`, so the flagged lines cannot panic. These tests exercise
/// the boundary conditions the issue highlighted.
#[test]
fn treasury_split_empty_is_rejected() {
    let te = setup();
    let entries: Vec<TreasurySplitEntry> = Vec::new(&te.env);
    let res = te.env.as_contract(&te.client.address, || {
        crate::admin::validate_treasury_split(&entries)
    });
    assert_eq!(res, Err(Error::InvalidFeeBips));
}

#[test]
fn treasury_split_zero_bps_entry_is_rejected() {
    let te = setup();
    let mut entries = Vec::new(&te.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: Address::generate(&te.env),
        bps: 0,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: Address::generate(&te.env),
        bps: 10_000,
    });

    let res = te.env.as_contract(&te.client.address, || {
        crate::admin::validate_treasury_split(&entries)
    });
    assert_eq!(res, Err(Error::InvalidFeeBips));
}

#[test]
fn treasury_split_duplicate_beneficiary_is_rejected() {
    let te = setup();
    let dup = Address::generate(&te.env);
    let mut entries = Vec::new(&te.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: dup.clone(),
        bps: 5_000,
    });
    entries.push_back(TreasurySplitEntry {
        beneficiary: dup,
        bps: 5_000,
    });

    let res = te.env.as_contract(&te.client.address, || {
        crate::admin::validate_treasury_split(&entries)
    });
    assert_eq!(res, Err(Error::InvalidFeeBips));
}

#[test]
fn treasury_split_sum_not_ten_thousand_is_rejected() {
    let te = setup();
    let mut entries = Vec::new(&te.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: Address::generate(&te.env),
        bps: 9_999,
    });

    let res = te.env.as_contract(&te.client.address, || {
        crate::admin::validate_treasury_split(&entries)
    });
    assert_eq!(res, Err(Error::InvalidFeeBips));
}

#[test]
fn treasury_split_valid_single_entry_passes() {
    let te = setup();
    let mut entries = Vec::new(&te.env);
    entries.push_back(TreasurySplitEntry {
        beneficiary: Address::generate(&te.env),
        bps: 10_000,
    });

    let res = te.env.as_contract(&te.client.address, || {
        crate::admin::validate_treasury_split(&entries)
    });
    assert_eq!(res, Ok(()));
}

/// Many-entry split: the `O(n²)` duplicate scan must not panic at its upper
/// index bound and must accept a valid, all-distinct configuration.
#[test]
fn treasury_split_many_distinct_entries_passes() {
    let te = setup();
    let mut entries = Vec::new(&te.env);
    for _ in 0..10 {
        entries.push_back(TreasurySplitEntry {
            beneficiary: Address::generate(&te.env),
            bps: 1_000,
        });
    }

    let res = te.env.as_contract(&te.client.address, || {
        crate::admin::validate_treasury_split(&entries)
    });
    assert_eq!(res, Ok(()));
}
