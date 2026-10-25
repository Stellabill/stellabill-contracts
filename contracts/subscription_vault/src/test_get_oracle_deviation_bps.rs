//! Adversarial coverage for `oracle::get_oracle_deviation_bps`, the threshold
//! getter defined in the inline `pub mod oracle { .. }` block in
//! `contracts/subscription_vault/src/lib.rs` (the function under test is at
//! `lib.rs:322`; its only writer, `set_oracle_deviation_bps`, is at
//! `lib.rs:316`).
//!
//! ```
//! pub fn get_oracle_deviation_bps(env: &Env) -> Option<u32> {
//!     let key = Symbol::new(env, "oracle_deviation_bps");
//!     env.storage().instance().get(&key)
//! }
//! ```
//!
//! Unlike most getters in this crate there is no `read_config` indirection and
//! no `unwrap_or(default)`: the whole observable contract is "read one
//! **instance**-storage slot and hand back `Option<u32>`". So the properties
//! pinned here are exactly the ones that a one-line change could silently
//! break, and the ones a `Symbol`-keyed raw instance slot exposes that a
//! `DataKey`-keyed / `read_config` getter would not.
//!
//! * **Exactness** — every `u32` written is returned bit-for-bit, including the
//!   two values that a "0 means unset" style default would collapse: `0` and
//!   `u32::MAX`.
//! * **`Some(0) != None`** — the slot carries *three* distinct meanings, not
//!   two: `None` = check disabled, `Some(0)` = strict mode (any deviation
//!   rejected), `Some(n > 0)` = tolerance `n` bps. Collapsing `Some(0)` into
//!   `None` would silently disable the circuit breaker; collapsing `None` into
//!   `Some(0)` would silently hard-lock it. Asserted from both sides.
//! * **Purity** — a read never materializes the key, never emits an event and
//!   never records an authorization entry; reading `None` leaves the storage
//!   slot absent.
//! * **Tier confinement** — only *instance* storage is consulted. A value
//!   planted in persistent storage under the same key is invisible, and a
//!   persistent reader cannot see an instance write. This is the exact
//!   confusion that `read_config` (used by e.g. `get_buyout_premium_bps`)
//!   makes *reachable* by checking persistent first and instance second; this
//!   getter deliberately does not.
//! * **Type safety** — a value of a different type under the same key aborts
//!   the read (`ConversionError`) instead of being coerced, so a threshold can
//!   never be silently reinterpreted as a different number.
//! * **Contract isolation** — instance storage is per-contract, so a threshold
//!   written on one deployment is invisible on another.
//!
//! ## Authorization
//!
//! The function under test is a **view with no `Address` parameter and no
//! `Result`**: there is no caller to reject, and the ABI cannot distinguish an
//! admin from a stranger. The tests therefore pin the property that actually
//! holds — *no auth is consumed and no event is emitted, for any caller* — and
//! prove it against `env.mock_auths(&[])` (strict mode: any authorization
//! requirement would abort) plus `env.auths()` (no entries recorded).
//!
//! ## Documented divergence (pinned here, deliberately not changed)
//!
//! The doc comment on `set_oracle_deviation_bps` (`lib.rs:308-315`) promises
//! that the threshold gates charges: "If the deviation exceeds this threshold,
//! the charge is rejected with `Error::OracleDeviationTooHigh`". In the live
//! crate nothing reads the threshold on the charge path:
//!
//! | consumer | status |
//! |---|---|
//! | `oracle::resolve_charge_amount` (`lib.rs:208`) | never calls `get_oracle_deviation_bps` |
//! | `charge_core::charge_subscription` | reads `oracle::get_oracle_config` (`charge_core.rs:213`), never the deviation slot |
//! | `oracle::set_oracle_deviation_bps` (`lib.rs:316`) | **no auth check, not exposed through `#[contractimpl]`** |
//! | `src/oracle.rs` (`check_deviation_and_record`) | the real breaker, but `src/oracle.rs` is **not declared as a module** — dead code |
//! | `src/test.rs:8519-8940` | the only end-to-end breaker tests, but `src/test.rs` is **not declared as a module** — never compiled |
//!
//! So on `main` the threshold is inert: configuring `Some(0)` — the strictest
//! possible setting — does not reject a single charge. The tests at the bottom
//! of this file pin that as the current behaviour instead of asserting the
//! documented one, so the day the breaker is wired up the suite fails loudly.
//! No public contract is changed here: no entrypoint is added, removed or
//! re-signed.

#![cfg(test)]

use crate::oracle::{get_oracle_deviation_bps, set_oracle_deviation_bps};
use crate::test_utils::fixtures;
use crate::types::{DataKey, Error};
use crate::{SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _, MockAuth, MockAuthInvoke},
    token, Address, BytesN, Env, IntoVal, Symbol, Vec,
};

const T0: u64 = 1_000_000;
const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const GRACE_PERIOD: u64 = 7 * 24 * 60 * 60;

/// The literal both the getter and the setter key their slot with
/// (`lib.rs:317` and `lib.rs:323`). Restated here so a rename on either side
/// shows up as a test failure rather than as a silently orphaned slot.
const KEY_LITERAL: &str = "oracle_deviation_bps";

/// The exact storage slot the getter reads: a raw `Symbol` in **instance**
/// storage, with no `DataKey` wrapper and no tenant component.
fn slot(env: &Env) -> Symbol {
    Symbol::new(env, KEY_LITERAL)
}

// ── Fixtures ──────────────────────────────────────────────────────────────────

/// Registers the vault, initialized, with a real Stellar Asset Contract so the
/// charge path can move funds. Returns `(env, contract_id, admin, token)`.
fn setup() -> (Env, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = T0);

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();

    client.init(&token, &6, &admin, &1_000_000i128, &GRACE_PERIOD);
    (env, contract_id, admin, token)
}

/// The getter under test. It touches instance storage, which is only reachable
/// from inside the contract's own context, so every call is wrapped in
/// `env.as_contract`.
fn read(env: &Env, contract_id: &Address) -> Option<u32> {
    env.as_contract(contract_id, || get_oracle_deviation_bps(env))
}

/// The only writer of the slot, wrapped in the contract context for the same
/// reason.
fn write(env: &Env, contract_id: &Address, bps: u32) {
    env.as_contract(contract_id, || set_oracle_deviation_bps(env, bps));
}

/// True when the slot is physically present, independent of its value.
fn present(env: &Env, contract_id: &Address) -> bool {
    env.as_contract(contract_id, || env.storage().instance().has(&slot(env)))
}

/// Plant a raw value of an arbitrary type in the slot, bypassing the setter.
fn plant<T>(env: &Env, contract_id: &Address, value: &T)
where
    T: soroban_sdk::IntoVal<Env, soroban_sdk::Val>,
{
    env.as_contract(contract_id, || {
        env.storage().instance().set(&slot(env), value)
    })
}

// ── Default / absence ─────────────────────────────────────────────────────────

#[test]
fn returns_none_when_never_configured() {
    let (env, contract_id, _, _) = setup();
    assert!(
        !present(&env, &contract_id),
        "precondition: slot must be absent"
    );
    assert_eq!(read(&env, &contract_id), None);
}

#[test]
fn returns_none_on_an_uninitialized_contract() {
    // `get_oracle_deviation_bps` is a pure read with no `Result`, so it cannot
    // report `NotInitialized` the way `try_revoke_merchant` does. Reading a
    // vault that was registered but never `init`ed must be the same
    // `None`, not a panic and not a phantom default.
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    assert_eq!(
        client.try_get_admin(),
        Err(Ok(Error::NotInitialized)),
        "precondition: the vault must not be initialized"
    );

    assert_eq!(read(&env, &contract_id), None);
}

// ── Exactness and boundaries ──────────────────────────────────────────────────

#[test]
fn round_trips_every_documented_and_boundary_value() {
    // Ordered from the tightest to the loosest interpretation of the slot:
    // `0` is strict mode, `10_000` is 100 %, and `u32::MAX` is ~4.29e5 x 100 %.
    const CASES: [u32; 14] = [
        0,
        1,
        2,
        50,
        100,
        500,
        999,
        1_000,
        5_000,
        9_999,
        10_000,
        10_001,
        u32::MAX - 1,
        u32::MAX,
    ];

    let (env, contract_id, _, _) = setup();
    for bps in CASES {
        write(&env, &contract_id, bps);
        assert_eq!(read(&env, &contract_id), Some(bps), "round-trip {bps} bps");
    }
}

#[test]
fn zero_is_a_configured_value_and_is_distinct_from_unset() {
    // The single most consequential boundary in this getter. `Some(0)` means
    // "strict mode, reject any price that is not exactly the median";
    // `None` means "skip the check entirely". A getter that used
    // `get(..).unwrap_or_default()`, or that mapped the stored value, would
    // make one of the two unreachable.
    let (env, contract_id, _, _) = setup();

    assert_eq!(read(&env, &contract_id), None, "unset");
    write(&env, &contract_id, 0);
    assert_eq!(read(&env, &contract_id), Some(0), "configured strict mode");

    assert_ne!(
        read(&env, &contract_id),
        None,
        "Some(0) must not collapse into None (check-disabled)"
    );
    assert_ne!(
        read(&env, &contract_id),
        Some(1),
        "0 must not be rounded or clamped up to a 1 bps floor"
    );
}

#[test]
fn u32_max_is_returned_without_truncation_or_panic() {
    // No ceiling is enforced by the writer or the getter. `u32::MAX` bps is
    // nonsensical as a threshold but must round-trip exactly: silently
    // clamping to `u32::MAX % 10_000`, or wrapping, would make a
    // permissive configuration look strict.
    let (env, contract_id, _, _) = setup();
    write(&env, &contract_id, u32::MAX);
    assert_eq!(read(&env, &contract_id), Some(u32::MAX));

    // And the neighbouring value must stay distinguishable, i.e. the getter
    // really is reading the full 32 bits and not a narrowed prefix.
    write(&env, &contract_id, u32::MAX - 1);
    assert_eq!(read(&env, &contract_id), Some(u32::MAX - 1));
}

#[test]
fn last_write_wins_and_is_deterministic() {
    // The slot is a single scalar: no history, no append semantics. The value
    // after N writes depends only on the final one, never on the call history
    // (a stale value resurfacing after a later write would mean the getter is
    // reading a cache or a shadow key).
    let (env, contract_id, _, _) = setup();

    // A deterministic, non-monotonic sequence: it would expose a
    // max()/clamp() accumulator if the slot were aggregated rather than
    // overwritten.
    for bps in [7u32, u32::MAX, 1, 4_242, 0, 9_999, 0, 9_999] {
        write(&env, &contract_id, bps);
        assert_eq!(read(&env, &contract_id), Some(bps), "after writing {bps}");
    }

    // Reading repeatedly must be stable and must not drift back to an earlier
    // value.
    for _ in 0..5 {
        assert_eq!(read(&env, &contract_id), Some(9_999));
    }
}

// ── Purity ────────────────────────────────────────────────────────────────────

#[test]
fn read_is_pure_and_never_materializes_the_slot() {
    // A read that lazily inserted a default (e.g. `get_or_insert(0)`) would
    // silently convert "disabled" into "strict mode" for every consumer, and
    // would cost a storage write on every view call. Both are observable.
    let (env, contract_id, _, _) = setup();

    for _ in 0..8 {
        assert_eq!(read(&env, &contract_id), None);
    }
    assert!(
        !present(&env, &contract_id),
        "reading None must leave the slot absent, not write a default"
    );

    write(&env, &contract_id, 250);
    for _ in 0..8 {
        assert_eq!(read(&env, &contract_id), Some(250));
    }
    assert!(
        present(&env, &contract_id),
        "a configured slot stays present across repeated reads"
    );
}

#[test]
fn read_emits_no_events_and_consumes_no_authorization() {
    // There is no caller to reject (the function takes no `Address`), so the
    // observable contract is "nothing is demanded and nothing is recorded".
    // `mock_auths(&[])` is strict mode: any `require_auth` would abort rather
    // than silently succeed.
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    env.mock_auths(&[]);

    env.as_contract(&contract_id, || {
        set_oracle_deviation_bps(&env, 500);
    });
    env.set_auths(&[]);
    let events_before = env.events().all().len();

    let value = read(&env, &contract_id);

    assert_eq!(
        value,
        Some(500),
        "a view is readable with no authorizations"
    );
    assert_eq!(
        env.events().all().len(),
        events_before,
        "reading the threshold must not emit an event"
    );
    assert!(
        env.auths().is_empty(),
        "reading the threshold must not consume an authorization entry"
    );
}

#[test]
fn the_result_does_not_depend_on_who_is_asking() {
    // The threshold is protocol-wide configuration, not per-caller state: the
    // key carries no address component, so there is no second value for a
    // stranger to observe. Pinned because "key by caller" is the natural wrong
    // extension of a view getter and would silently weaken the policy for
    // whoever the contract forgot to thread through.
    let (env, contract_id, admin, _) = setup();
    let stranger = Address::generate(&env);
    assert_ne!(admin, stranger, "precondition: two distinct addresses");
    write(&env, &contract_id, 750);

    // Read three times: with an empty (strict) auth context, with all
    // authorizations mocked, and with the recorded auth log cleared. None of
    // those contexts may change what the slot reports.
    env.mock_auths(&[]);
    let with_stranger = read(&env, &contract_id);
    env.mock_all_auths();
    let with_all_auths = read(&env, &contract_id);
    env.set_auths(&[]);
    let with_none = read(&env, &contract_id);

    assert_eq!(with_stranger, Some(750));
    assert_eq!(with_all_auths, with_stranger);
    assert_eq!(with_none, with_stranger);
}

// ── Storage-tier confinement ──────────────────────────────────────────────────

#[test]
fn persistent_storage_under_the_same_key_is_invisible() {
    // The getter reads **instance** storage only. A value written to
    // persistent storage under the identical key — the mistake a
    // `read_config`-based getter (persistent first, then instance) would
    // happily surface — must not be reported as the live threshold.
    let (env, contract_id, _, _) = setup();

    env.as_contract(&contract_id, || {
        env.storage().persistent().set(&slot(&env), &1_234u32);
    });
    assert_eq!(read(&env, &contract_id), None);

    // ...and an instance write must win over, not merge with, the planted
    // persistent value.
    write(&env, &contract_id, 500);
    assert_eq!(read(&env, &contract_id), Some(500));
    env.as_contract(&contract_id, || {
        assert_eq!(
            env.storage().persistent().get::<_, u32>(&slot(&env)),
            Some(1_234),
            "the planted persistent value must be left alone"
        );
    });
}

#[test]
fn the_threshold_is_not_visible_to_a_persistent_reader() {
    // The mirror image of the test above: the writer must not leak the
    // threshold into persistent storage under the same key, which would make
    // the value survive an instance reset and double as a second, divergent
    // source of truth.
    let (env, contract_id, _, _) = setup();
    write(&env, &contract_id, 300);

    env.as_contract(&contract_id, || {
        assert_eq!(
            env.storage().persistent().get::<_, u32>(&slot(&env)),
            None,
            "set_oracle_deviation_bps must write instance storage only"
        );
    });
}

// ── Type safety ───────────────────────────────────────────────────────────────

#[test]
#[should_panic(expected = "ConversionError")]
fn a_negative_i128_planted_under_the_key_aborts_the_read() {
    // The slot holds an untyped `Val`; the getter asks for `u32`. A value of a
    // different type must abort rather than be coerced — otherwise a
    // mis-migrated slot (say a signed basis-point count, or a sentinel like
    // `-1` for "disabled") would be reinterpreted as a huge unsigned
    // threshold and silently hard-lock the circuit breaker.
    let (env, contract_id, _, _) = setup();
    plant(&env, &contract_id, &-1i128);
    let _ = read(&env, &contract_id);
}

#[test]
#[should_panic(expected = "ConversionError")]
fn a_u64_planted_under_the_key_aborts_the_read() {
    // `u64` and `u32` are distinct `Val`s, so a `u64` left in the slot by an
    // older build is not silently narrowed to the low 32 bits — a value that
    // could otherwise be reinterpreted as a far stricter threshold than the one
    // its writer intended.
    let (env, contract_id, _, _) = setup();
    plant(&env, &contract_id, &(u32::MAX as u64));
    let _ = read(&env, &contract_id);
}

// ── Contract isolation ────────────────────────────────────────────────────────

#[test]
fn the_threshold_is_scoped_to_one_contract_instance() {
    // The key is a bare `Symbol` with no tenant component, so isolation rests
    // entirely on instance storage being per-contract. If the slot ever moved
    // to persistent or temporary storage, this test would start to fail — which is
    // the point: a shared slot would let one deployment read or overwrite
    // another's oracle risk policy.
    let env = Env::default();
    env.mock_all_auths();
    let a = env.register(SubscriptionVault, ());
    let b = env.register(SubscriptionVault, ());

    write(&env, &a, 100);
    write(&env, &b, 9_000);

    assert_eq!(read(&env, &a), Some(100));
    assert_eq!(read(&env, &b), Some(9_000));

    // Clearing one leaves the other untouched.
    env.as_contract(&a, || env.storage().instance().remove(&slot(&env)));
    assert_eq!(read(&env, &a), None);
    assert_eq!(read(&env, &b), Some(9_000));
}

// ── No clobbering of neighbouring instance state ──────────────────────────────

#[test]
fn configuring_the_threshold_does_not_disturb_neighbouring_state() {
    // The threshold lives in the same instance map as every other piece of
    // contract-level state. A collision, or a future change to the key's shape
    // or tier, would silently corrupt vault state; pin the neighbours that
    // share the tier.
    let (env, contract_id, admin, token) = setup();
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let (id, _sub, _merchant) = fixtures::create_subscription_detailed(
        &env,
        &client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );

    // `DataKey::TotalAccounted` and `DataKey::ChargeFailureCounter` are
    // *neighbouring* instance-storage entries — same tier, same map, different
    // key shape — so they are the closest thing to a collision. They are
    // planted directly because their owning modules (`src/accounting.rs`,
    // `charge_core.rs`'s auto-pause branch) are awkward to reach through the
    // ABI here; the point is the storage layer, not the writer.
    let neighbour_key = DataKey::TotalAccounted(token.clone());
    let failure_key = DataKey::ChargeFailureCounter(id);
    env.as_contract(&contract_id, || {
        env.storage().instance().set(&neighbour_key, &424_242i128);
        env.storage().instance().set(&failure_key, &3u32);
    });
    let accounted_before: i128 = env
        .as_contract(&contract_id, || {
            env.storage().instance().get::<_, i128>(&neighbour_key)
        })
        .expect("neighbour slot written");
    let failures_before: u32 = env
        .as_contract(&contract_id, || {
            env.storage().instance().get::<_, u32>(&failure_key)
        })
        .expect("neighbour slot written");
    let config_before = client.get_oracle_config();
    let count_before = client.get_subscription_count();
    let sub_before = client.get_subscription(&id);
    assert_eq!(
        accounted_before, 424_242,
        "precondition: neighbour is written"
    );

    write(&env, &contract_id, 12_345);
    write(&env, &contract_id, 0);

    let accounted_after: i128 = env
        .as_contract(&contract_id, || {
            env.storage().instance().get::<_, i128>(&neighbour_key)
        })
        .expect("neighbour slot must survive");
    let failures_after: u32 = env
        .as_contract(&contract_id, || {
            env.storage().instance().get::<_, u32>(&failure_key)
        })
        .expect("neighbour slot must survive");
    assert_eq!(
        accounted_after, accounted_before,
        "the neighbouring instance slot must be untouched"
    );
    assert_eq!(
        failures_after, failures_before,
        "the neighbouring instance slot must be untouched"
    );
    assert_eq!(
        client.get_oracle_config(),
        config_before,
        "the oracle config slot must be untouched"
    );
    assert_eq!(
        client.get_subscription_count(),
        count_before,
        "the subscription counter must be untouched"
    );
    assert_eq!(client.get_admin(), admin, "admin must be untouched");
    assert_eq!(
        client.get_subscription(&id).token,
        token,
        "the subscription's token binding must be untouched"
    );
    assert_eq!(
        client.get_subscription(&id).amount,
        sub_before.amount,
        "the subscription's amount must be untouched"
    );
    assert_eq!(
        read(&env, &contract_id),
        Some(0),
        "the threshold write must have landed"
    );
}

// ── Rejected operations leave the threshold unchanged ─────────────────────────

#[test]
fn threshold_unchanged_after_an_unauthorized_charge() {
    // `charge_subscription` is admin-only. The rejection happens in the host
    // before any vault state is read, so the threshold slot must still hold
    // the exact configured value afterwards — a rejected call must not clear
    // the policy back to "disabled".
    let (env, contract_id, _, _) = setup();
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let (id, _sub, _merchant) = fixtures::create_subscription_detailed(
        &env,
        &client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );

    write(&env, &contract_id, 500);
    env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL + 1);
    env.mock_auths(&[]);

    assert!(
        client
            .try_charge_subscription(&id, &None::<BytesN<32>>)
            .is_err(),
        "an unsigned charge must be rejected"
    );
    assert_eq!(read(&env, &contract_id), Some(500));
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active,
        "the rejected charge must not have changed the subscription either"
    );
}

#[test]
fn threshold_unchanged_after_a_charge_signed_by_a_non_admin() {
    // A well-formed signature from the wrong account is not enough: the stored
    // admin must sign. The stranger's own address, and the threshold, both
    // stay exactly where they were.
    let (env, contract_id, _, _) = setup();
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let (id, _sub, _merchant) = fixtures::create_subscription_detailed(
        &env,
        &client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );

    write(&env, &contract_id, 750);
    env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL + 1);

    let mut args = Vec::new(&env);
    args.push_back(id.into_val(&env));
    args.push_back(None::<BytesN<32>>.into_val(&env));
    let stranger = Address::generate(&env);
    let invoke = MockAuthInvoke {
        contract: &contract_id,
        fn_name: "charge_subscription",
        args,
        sub_invokes: &[],
    };
    env.mock_auths(&[MockAuth {
        address: &stranger,
        invoke: &invoke,
    }]);

    assert!(
        client
            .try_charge_subscription(&id, &None::<BytesN<32>>)
            .is_err(),
        "a charge signed by a non-admin must be rejected"
    );
    assert_eq!(read(&env, &contract_id), Some(750));
    assert_eq!(
        client.get_subscription(&id).status,
        SubscriptionStatus::Active
    );
}

#[test]
fn threshold_unchanged_after_a_rejected_charge_for_a_missing_subscription() {
    // `NotFound` is the other rejection the charge path can produce. It must
    // be as inert with respect to the threshold as the auth failures are.
    let (env, contract_id, _, _) = setup();
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    write(&env, &contract_id, 1_000);

    assert_eq!(
        client.try_charge_subscription(&4_242, &None::<BytesN<32>>),
        Err(Ok(Error::NotFound))
    );
    assert_eq!(read(&env, &contract_id), Some(1_000));
}

// ── Documented divergence: the threshold is not wired to the charge path ──────

#[test]
fn strictest_threshold_does_not_block_a_charge_today() {
    // Pinned current behaviour, deliberately not the documented one. With the
    // strictest possible setting (`Some(0)` = reject any price that is not
    // exactly the median) a real, fully funded, admin-authorized charge still
    // succeeds, because the live `charge_core`/`oracle::resolve_charge_amount`
    // never consult the slot.
    //
    // When the circuit breaker is wired up this test fails, which is the
    // intended signal. The threshold value is asserted before and after so the
    // failure message shows that the policy was configured correctly and only
    // the enforcement is missing.
    let (env, contract_id, _, token_addr) = setup();
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token_client = token::StellarAssetClient::new(&env, &token_addr);

    let (id, subscriber, _merchant) = fixtures::create_subscription_detailed(
        &env,
        &client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    token_client.mint(&subscriber, &AMOUNT);
    client.deposit_funds(&id, &subscriber, &AMOUNT, &None::<BytesN<32>>);

    write(&env, &contract_id, 0);
    assert_eq!(
        read(&env, &contract_id),
        Some(0),
        "precondition: strict mode is configured"
    );

    env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL + 1);
    client.charge_subscription(&id, &None::<BytesN<32>>);

    assert_eq!(
        read(&env, &contract_id),
        Some(0),
        "the threshold survived the charge untouched"
    );
    let sub = client.get_subscription(&id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert!(
        sub.prepaid_balance < AMOUNT,
        "precondition: the charge actually moved funds (prepaid_balance = {})",
        sub.prepaid_balance
    );
}

#[test]
fn an_unset_threshold_and_a_configured_one_behave_identically_to_the_charge_path() {
    // The flip side: because nothing reads the slot, `None` and `Some(0)` — the
    // two opposite ends of the documented semantics — produce byte-identical
    // charge outcomes. If the breaker is ever connected, this test is the one
    // that must break.
    let outcomes = [
        run_charge_with_threshold(None),
        run_charge_with_threshold(Some(0)),
        run_charge_with_threshold(Some(9_999)),
    ];

    assert_eq!(
        outcomes[0], outcomes[1],
        "unset must equal strict mode today"
    );
    assert_eq!(
        outcomes[1], outcomes[2],
        "strict mode must equal lenient today"
    );
    for (i, o) in outcomes.iter().enumerate() {
        assert_eq!(o.1, AMOUNT, "charge #{i} must bill exactly one period");
        assert_eq!(o.0, AMOUNT, "charge #{i} must leave one period of runway");
    }
}

/// Deploy a vault, optionally configure the threshold, run one admin charge and
/// return `(prepaid_balance_after, total_billed)`.
fn run_charge_with_threshold(threshold: Option<u32>) -> (i128, i128) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = T0);

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token_addr = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let token_client = token::StellarAssetClient::new(&env, &token_addr);
    client.init(&token_addr, &6, &admin, &1_000_000i128, &GRACE_PERIOD);

    let (id, subscriber, _merchant) = fixtures::create_subscription_detailed(
        &env,
        &client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    // Two periods of runway, so a single successful charge bills exactly one.
    token_client.mint(&subscriber, &(AMOUNT * 2));
    client.deposit_funds(&id, &subscriber, &(AMOUNT * 2), &None::<BytesN<32>>);

    if let Some(bps) = threshold {
        write(&env, &contract_id, bps);
    }
    assert_eq!(
        read(&env, &contract_id),
        threshold,
        "threshold precondition"
    );

    env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL + 1);
    client.charge_subscription(&id, &None::<BytesN<32>>);

    let left = client.get_subscription(&id).prepaid_balance;
    (left, (AMOUNT * 2) - left)
}
