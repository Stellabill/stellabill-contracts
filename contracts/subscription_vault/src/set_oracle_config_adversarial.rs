#![cfg(test)]

//! Adversarial coverage for `set_oracle_config` (issue #1105).
//!
//! The public entrypoint lives in `lib.rs` (`#[contractimpl] impl
//! SubscriptionVault`), which verifies admin auth via
//! `admin::require_admin_auth` and then delegates to the inline
//! `crate::oracle::set_oracle_config` in `lib.rs`. The implementation:
//!
//! 1. Rejects `OracleKind::FixedRate` with `fixed_denominator == 0`
//!    (`Error::InvalidInput`).
//! 2. Enforces the per-key admin config cooldown (`enforce_config_cooldown`,
//!    `admin.rs`, `CONFIG_COOLDOWN_SECS = 6h`) → `Error::CooldownActive`.
//! 3. Writes the full `OracleConfig` under `DataKey::Oracle` and publishes
//!    `oracle_config_updated`.
//!
//! Everything else (enabled/oracle consistency, `max_age_seconds`,
//! `window_secs`, `fixed_numerator`) is **not** validated by the current
//! implementation; tests below document that behavior explicitly and flag
//! the cases that look like latent bugs rather than changing production code.
//!
//! State preservation after every rejection is asserted by comparing the
//! full `OracleConfig` returned by `get_oracle_config` (struct equality)
//! before and after the failing call.

use crate::types::OracleConfigUpdatedEvent;
use crate::{Error, OracleConfig, OracleKind, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    Address, Env,
};

/// Ledger timestamp used for the initial setup (same convention as
/// `test_admin_rotation_two_step.rs`).
const T0: u64 = 1_000_000;

/// Per-key admin config cooldown (`admin.rs` `CONFIG_COOLDOWN_SECS`): 6 hours.
const CONFIG_COOLDOWN_SECS: u64 = 6 * 60 * 60;

struct TestSetup {
    env: Env,
    client: SubscriptionVaultClient<'static>,
    admin: Address,
}

fn setup() -> TestSetup {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    client.init(&token, &6u32, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    TestSetup { env, client, admin }
}

fn advance_seconds(setup: &TestSetup, seconds: u64) {
    let t = setup.env.ledger().timestamp();
    setup.env.ledger().set_timestamp(t + seconds);
}

/// Snapshot helper: the full stored config, used to prove rejected calls
/// leave state untouched.
fn stored_config(setup: &TestSetup) -> OracleConfig {
    setup.client.get_oracle_config()
}

fn assert_rejected_leaves_state_unchanged<T>(
    setup: &TestSetup,
    call: impl FnOnce() -> Result<
        Result<T, soroban_sdk::ConversionError>,
        Result<Error, soroban_sdk::InvokeError>,
    >,
) {
    let before = stored_config(setup);
    let result = call();
    assert!(result.is_err(), "expected the call to be rejected");
    let after = stored_config(setup);
    assert_eq!(before, after, "rejected call must not mutate stored config");
}

// ── 1. Valid calls: every OracleKind variant, enabled/disabled ──────────────

#[test]
fn test_valid_spot_enabled_with_oracle_round_trips_all_fields() {
    let setup = setup();
    let oracle = Address::generate(&setup.env);

    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &Some(oracle.clone()),
        &300u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );

    let cfg = stored_config(&setup);
    assert!(cfg.enabled);
    assert_eq!(cfg.oracle, Some(oracle));
    assert_eq!(cfg.max_age_seconds, 300);
    assert_eq!(cfg.kind, OracleKind::Spot);
    assert_eq!(cfg.window_secs, 0);
    assert_eq!(cfg.fixed_numerator, 0);
    assert_eq!(cfg.fixed_denominator, 1);
}

#[test]
fn test_valid_twap_enabled_round_trips_window() {
    let setup = setup();
    let oracle = Address::generate(&setup.env);

    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &Some(oracle.clone()),
        &600u64,
        &OracleKind::Twap,
        &1800u64,
        &0u128,
        &0u128,
    );

    let cfg = stored_config(&setup);
    assert!(cfg.enabled);
    assert_eq!(cfg.oracle, Some(oracle));
    assert_eq!(cfg.max_age_seconds, 600);
    assert_eq!(cfg.kind, OracleKind::Twap);
    assert_eq!(cfg.window_secs, 1800);
}

#[test]
fn test_valid_fixed_rate_round_trips_ratio() {
    let setup = setup();
    // FixedRate does not read from an oracle address; None is accepted.
    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &None::<Address>,
        &0u64,
        &OracleKind::FixedRate,
        &0u64,
        &1_070_000_000u128, // 1.07 scaled to 10^7
        &1_000_000_000u128,
    );

    let cfg = stored_config(&setup);
    assert!(cfg.enabled);
    assert_eq!(cfg.oracle, None);
    assert_eq!(cfg.kind, OracleKind::FixedRate);
    assert_eq!(cfg.fixed_numerator, 1_070_000_000u128);
    assert_eq!(cfg.fixed_denominator, 1_000_000_000u128);
}

#[test]
fn test_valid_disabled_config_with_neither_oracle_nor_max_age() {
    let setup = setup();

    setup.client.set_oracle_config(
        &setup.admin,
        &false,
        &None::<Address>,
        &0u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );

    let cfg = stored_config(&setup);
    assert!(!cfg.enabled);
    assert_eq!(cfg.oracle, None);
    assert_eq!(cfg.max_age_seconds, 0);
}

#[test]
fn test_valid_disabled_config_still_persists_oracle_address() {
    let setup = setup();
    let oracle = Address::generate(&setup.env);

    // Disabled but address supplied: stored verbatim (pass-through).
    setup.client.set_oracle_config(
        &setup.admin,
        &false,
        &Some(oracle.clone()),
        &120u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );

    let cfg = stored_config(&setup);
    assert!(!cfg.enabled);
    assert_eq!(cfg.oracle, Some(oracle));
    assert_eq!(cfg.max_age_seconds, 120);
}

// ── 2. Authorization ────────────────────────────────────────────────────────

#[test]
fn test_non_admin_caller_rejected_with_forbidden_and_state_unchanged() {
    let setup = setup();
    let stranger = Address::generate(&setup.env);

    assert_rejected_leaves_state_unchanged(&setup, || {
        setup.client.try_set_oracle_config(
            &stranger,
            &true,
            &None::<Address>,
            &60u64,
            &OracleKind::Spot,
            &0u64,
            &0u128,
            &1u128,
        )
    });
    // Exact error: `require_admin_auth` compares against the stored admin and
    // returns Forbidden on mismatch (auth itself passes under mock).
    let result = setup.client.try_set_oracle_config(
        &stranger,
        &true,
        &None::<Address>,
        &60u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );
    assert_eq!(result, Err(Ok(Error::Forbidden)));
}

#[test]
fn test_call_without_any_authorization_fails_at_host_level() {
    // With no auth mocked, the entrypoint's `admin.require_auth()` cannot be
    // satisfied, so the call traps at the host level (outer Err) before any
    // contract logic runs.
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    let result = client.try_set_oracle_config(
        &admin,
        &false,
        &None::<Address>,
        &0u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );
    assert!(
        matches!(result, Err(Err(_))),
        "missing auth must surface as a host error, got {:?}",
        result
    );
}

#[test]
fn test_old_admin_rejected_after_two_step_rotation() {
    let setup = setup();
    let new_admin = Address::generate(&setup.env);

    setup.client.propose_admin(&setup.admin, &new_admin);
    setup.client.claim_admin_role(&new_admin);
    assert_eq!(setup.client.get_admin(), new_admin);

    // The old admin is no longer the stored admin: rejected, state untouched.
    assert_rejected_leaves_state_unchanged(&setup, || {
        setup.client.try_set_oracle_config(
            &setup.admin,
            &true,
            &None::<Address>,
            &60u64,
            &OracleKind::Spot,
            &0u64,
            &0u128,
            &1u128,
        )
    });
    let result = setup.client.try_set_oracle_config(
        &setup.admin,
        &true,
        &None::<Address>,
        &60u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );
    assert_eq!(result, Err(Ok(Error::Forbidden)));

    // The new admin succeeds (cooldown is per-key and this key was never
    // written during this test).
    setup.client.set_oracle_config(
        &new_admin,
        &false,
        &None::<Address>,
        &0u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );
    assert!(!stored_config(&setup).enabled);
}

// ── 3. Boundary and invalid values ──────────────────────────────────────────

#[test]
fn test_fixed_rate_zero_denominator_rejected_invalid_input_and_state_unchanged() {
    let setup = setup();

    assert_rejected_leaves_state_unchanged(&setup, || {
        setup.client.try_set_oracle_config(
            &setup.admin,
            &true,
            &None::<Address>,
            &0u64,
            &OracleKind::FixedRate,
            &0u64,
            &1u128,
            &0u128, // division-by-zero risk
        )
    });
    let result = setup.client.try_set_oracle_config(
        &setup.admin,
        &true,
        &None::<Address>,
        &0u64,
        &OracleKind::FixedRate,
        &0u64,
        &1u128,
        &0u128,
    );
    assert_eq!(result, Err(Ok(Error::InvalidInput)));
}

#[test]
fn test_zero_denominator_accepted_for_kinds_that_ignore_it() {
    let setup = setup();

    // The eager check only guards FixedRate; Spot/TWAP ignore the ratio, so a
    // zero denominator is stored verbatim. Documented pass-through behavior.
    setup.client.set_oracle_config(
        &setup.admin,
        &false,
        &None::<Address>,
        &0u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &0u128,
    );
    let cfg = stored_config(&setup);
    assert_eq!(cfg.fixed_denominator, 0);
    assert_eq!(cfg.kind, OracleKind::Spot);
}

#[test]
fn test_fixed_rate_zero_numerator_is_accepted() {
    let setup = setup();

    // No validation on the numerator: 0/1 is stored as-is. Suspicious for a
    // price ratio (a zero price would make ceil conversion divide-by-zero in
    // resolve_charge_amount), but that is downstream, not this setter.
    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &None::<Address>,
        &0u64,
        &OracleKind::FixedRate,
        &0u64,
        &0u128,
        &1u128,
    );
    let cfg = stored_config(&setup);
    assert_eq!(cfg.fixed_numerator, 0);
    assert_eq!(cfg.fixed_denominator, 1);
}

#[test]
fn test_u128_max_fixed_rate_fields_stored_verbatim() {
    let setup = setup();

    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &None::<Address>,
        &0u64,
        &OracleKind::FixedRate,
        &0u64,
        &u128::MAX,
        &u128::MAX,
    );
    let cfg = stored_config(&setup);
    assert_eq!(cfg.fixed_numerator, u128::MAX);
    assert_eq!(cfg.fixed_denominator, u128::MAX);
    // The setter performs no arithmetic on these fields, so MAX/MAX is stored
    // without overflow. Downstream price math must handle this; out of scope
    // for this setter (see PR notes).
}

#[test]
fn test_max_age_zero_with_enabled_spot_is_accepted_documented_gap() {
    let setup = setup();
    let oracle = Address::generate(&setup.env);

    // DOCUMENTS CURRENT BEHAVIOR (suspicious): with `enabled = true` and
    // Spot pricing, `max_age_seconds = 0` disables staleness rejection in
    // resolve_charge_amount (`if cfg.max_age_seconds > 0`), i.e. stale
    // quotes are accepted forever. The unwired legacy tests in test.rs
    // expect this to be `InvalidInput`; the live implementation does not
    // validate it.
    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &Some(oracle),
        &0u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );
    assert_eq!(stored_config(&setup).max_age_seconds, 0);
}

#[test]
fn test_max_age_u64_max_stored_verbatim() {
    let setup = setup();
    let oracle = Address::generate(&setup.env);

    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &Some(oracle),
        &u64::MAX,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );
    assert_eq!(stored_config(&setup).max_age_seconds, u64::MAX);
}

#[test]
fn test_window_secs_zero_and_u64_max_stored_verbatim() {
    let setup = setup();
    let oracle = Address::generate(&setup.env);

    // window_secs has no validation at all, for any kind.
    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &Some(oracle.clone()),
        &60u64,
        &OracleKind::Twap,
        &0u64,
        &0u128,
        &1u128,
    );
    assert_eq!(stored_config(&setup).window_secs, 0);

    advance_seconds(&setup, CONFIG_COOLDOWN_SECS);
    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &Some(oracle),
        &60u64,
        &OracleKind::Twap,
        &u64::MAX,
        &0u128,
        &1u128,
    );
    assert_eq!(stored_config(&setup).window_secs, u64::MAX);
}

#[test]
fn test_enabled_true_with_oracle_none_is_accepted_documented_gap() {
    let setup = setup();

    // DOCUMENTS CURRENT BEHAVIOR (suspicious): enabling oracle pricing with
    // NO oracle address is accepted by the live implementation, but
    // resolve_charge_amount later fails with `OracleNotConfigured` for
    // every charge while enabled. The unwired legacy tests in test.rs
    // expect the setter to reject this with `OracleNotConfigured`. Flagged
    // in the PR; production code intentionally untouched here.
    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &None::<Address>,
        &60u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );
    let cfg = stored_config(&setup);
    assert!(cfg.enabled);
    assert_eq!(cfg.oracle, None);
}

#[test]
fn test_fixed_fields_set_for_non_fixed_kind_are_stored_verbatim() {
    let setup = setup();
    let oracle = Address::generate(&setup.env);

    // Pass-through: fields that Spot ignores are persisted exactly as given.
    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &Some(oracle),
        &60u64,
        &OracleKind::Spot,
        &0u64,
        &777u128,
        &0u128, // even a zero denominator for a kind that ignores it
    );
    let cfg = stored_config(&setup);
    assert_eq!(cfg.kind, OracleKind::Spot);
    assert_eq!(cfg.fixed_numerator, 777);
    assert_eq!(cfg.fixed_denominator, 0);
}

// ── 4. State preservation on rejection (incl. after a successful write) ────

#[test]
fn test_cooldown_rejects_second_call_within_window_and_state_unchanged() {
    let setup = setup();
    let oracle = Address::generate(&setup.env);

    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &Some(oracle.clone()),
        &300u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );
    let first = stored_config(&setup);

    // Second call 1 second later: cooldown is active for the "Oracle" key.
    setup.env.ledger().set_timestamp(T0 + 1);
    let result = setup.client.try_set_oracle_config(
        &setup.admin,
        &false,
        &None::<Address>,
        &0u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );
    assert_eq!(result, Err(Ok(Error::CooldownActive)));
    assert_eq!(stored_config(&setup), first);
}

#[test]
fn test_rejected_call_after_successful_one_preserves_earlier_config() {
    let setup = setup();
    let oracle = Address::generate(&setup.env);

    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &Some(oracle.clone()),
        &300u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );
    let good = stored_config(&setup);

    // A batch of rejections of every flavor, one after another; each must
    // leave `good` intact.
    let stranger = Address::generate(&setup.env);
    let attempt = |setup: &TestSetup, caller: &Address| {
        setup.client.try_set_oracle_config(
            caller,
            &false,
            &None::<Address>,
            &0u64,
            &OracleKind::FixedRate,
            &0u64,
            &1u128,
            &1u128,
        )
    };
    // CooldownActive (the successful write armed the per-key cooldown).
    assert_eq!(
        attempt(&setup, &setup.admin),
        Err(Ok(Error::CooldownActive))
    );
    assert_eq!(stored_config(&setup), good);
    // Forbidden (non-admin), even though FixedRate denominator is valid here.
    assert_eq!(attempt(&setup, &stranger), Err(Ok(Error::Forbidden)));
    assert_eq!(stored_config(&setup), good);
}

#[test]
fn test_cooldown_rejection_after_cooldown_window_succeeds() {
    let setup = setup();
    let oracle = Address::generate(&setup.env);

    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &Some(oracle.clone()),
        &300u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );

    advance_seconds(&setup, CONFIG_COOLDOWN_SECS);
    setup.client.set_oracle_config(
        &setup.admin,
        &false,
        &Some(oracle),
        &600u64,
        &OracleKind::Twap,
        &120u64,
        &0u128,
        &1u128,
    );
    let cfg = stored_config(&setup);
    assert!(!cfg.enabled);
    assert_eq!(cfg.kind, OracleKind::Twap);
    assert_eq!(cfg.max_age_seconds, 600);
    assert_eq!(cfg.window_secs, 120);
}

// ── 5. Idempotence / overwrite ──────────────────────────────────────────────

#[test]
fn test_second_valid_config_fully_overwrites_the_first() {
    let setup = setup();
    let oracle_a = Address::generate(&setup.env);
    let oracle_b = Address::generate(&setup.env);

    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &Some(oracle_a),
        &300u64,
        &OracleKind::Twap,
        &1800u64,
        &42u128,
        &7u128,
    );

    advance_seconds(&setup, CONFIG_COOLDOWN_SECS);
    setup.client.set_oracle_config(
        &setup.admin,
        &false,
        &Some(oracle_b.clone()),
        &0u64,
        &OracleKind::FixedRate,
        &0u64,
        &0u128,
        &9u128,
    );

    // No leftover fields from the first write: every field equals the second.
    let cfg = stored_config(&setup);
    assert_eq!(
        cfg,
        OracleConfig {
            enabled: false,
            oracle: Some(oracle_b),
            max_age_seconds: 0,
            kind: OracleKind::FixedRate,
            window_secs: 0,
            fixed_numerator: 0,
            fixed_denominator: 9,
        }
    );
}

// ── 6. Event emission ───────────────────────────────────────────────────────

#[test]
fn test_oracle_config_updated_event_payload_matches_inputs() {
    let setup = setup();
    let oracle = Address::generate(&setup.env);

    setup.client.set_oracle_config(
        &setup.admin,
        &true,
        &Some(oracle.clone()),
        &300u64,
        &OracleKind::Spot,
        &0u64,
        &0u128,
        &1u128,
    );

    use soroban_sdk::{Symbol, TryFromVal, Val};
    // NOTE: topic comparison must decode the Symbol, not compare raw `Val`
    // payloads: "oracle_config_updated" exceeds the 9-char small-symbol
    // limit, so each `Symbol::new` host object gets a distinct handle even
    // though the text matches.
    let all = setup.env.events().all();
    let mut found = false;
    for i in 0..all.len() {
        let (_, topics, data): (Address, soroban_sdk::Vec<Val>, Val) = all.get(i).unwrap();
        let Some(raw_topic) = topics.get(0) else {
            continue;
        };
        let Ok(decoded) = Symbol::try_from_val(&setup.env, &raw_topic) else {
            continue;
        };
        if decoded.to_string() != "oracle_config_updated" {
            continue;
        }
        let ev = OracleConfigUpdatedEvent::try_from_val(&setup.env, &data).unwrap();
        assert!(ev.enabled);
        assert_eq!(ev.oracle, Some(oracle.clone()));
        assert_eq!(ev.max_age_seconds, 300);
        assert_eq!(ev.kind, OracleKind::Spot);
        assert_eq!(ev.window_secs, 0);
        assert_eq!(ev.fixed_numerator, 0);
        assert_eq!(ev.fixed_denominator, 1);
        assert_eq!(ev.schema_version, crate::types::EVENT_SCHEMA_VERSION);
        assert_eq!(ev.timestamp, setup.env.ledger().timestamp());
        found = true;
    }
    assert!(found, "oracle_config_updated event must be published");
}
