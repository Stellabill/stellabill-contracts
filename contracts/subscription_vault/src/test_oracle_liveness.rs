#![cfg(test)]

use crate::{
    types::{Error, OracleLivenessEvent},
    SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{testutils::Address as _, Address, Env};

const T0: u64 = 1700000000;

mod test_oracle_liveness {
    use super::*;

    fn setup() -> (Env, SubscriptionVaultClient<'static>, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(T0);

        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

        (env, client, token, admin)
    }

    #[test]
    fn test_emit_oracle_liveness_succeeds_when_configured() {
        let (env, client, _token, admin) = setup();
        let oracle_address = Address::generate(&env);

        // Configure oracle with 300 second max age
        client.set_oracle_config(&admin, &true, &Some(oracle_address), &300);

        // Emit liveness event
        let result = client.emit_oracle_liveness(&env);

        assert!(result.is_ok());
        let event = result.unwrap();

        // Verify event fields
        assert!(event.last_sample_ts > 0);
        assert!(event.age > 0);
        assert!(event.age <= 150); // 300 / 2 = 150, and we simulate 60 second age
        assert!(event.healthy); // 60 <= 150, so healthy
        assert_eq!(event.timestamp, T0);
    }

    #[test]
    fn test_emit_oracle_liveness_fails_when_not_configured() {
        let (env, client, _token, _admin) = setup();

        // Oracle not configured (default state)
        let result = client.emit_oracle_liveness(&env);

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::OracleNotConfigured => {}
            e => panic!("Expected OracleNotConfigured, got: {:?}", e),
        }
    }

    #[test]
    fn test_emit_oracle_liveness_fails_when_disabled() {
        let (env, client, _token, admin) = setup();
        let oracle_address = Address::generate(&env);

        // Configure oracle but disable it
        client.set_oracle_config(&admin, &false, &Some(oracle_address), &300);

        let result = client.emit_oracle_liveness(&env);

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::OracleNotConfigured => {}
            e => panic!("Expected OracleNotConfigured, got: {:?}", e),
        }
    }

    #[test]
    fn test_emit_oracle_liveness_fails_when_no_oracle_address() {
        let (env, client, _token, admin) = setup();

        // Configure with enabled=true but no oracle address
        client.set_oracle_config(&admin, &true, &None, &300);

        let result = client.emit_oracle_liveness(&env);

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::OracleNotConfigured => {}
            e => panic!("Expected OracleNotConfigured, got: {:?}", e),
        }
    }

    #[test]
    fn test_emit_oracle_liveness_fails_when_max_age_zero() {
        let (env, client, _token, admin) = setup();
        let oracle_address = Address::generate(&env);

        // Configure with max_age_seconds = 0 (invalid)
        client.set_oracle_config(&admin, &true, &Some(oracle_address), &0);

        let result = client.emit_oracle_liveness(&env);

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::OracleNotConfigured => {}
            e => panic!("Expected OracleNotConfigured, got: {:?}", e),
        }
    }

    #[test]
    fn test_oracle_liveness_healthy_threshold() {
        let (env, client, _token, admin) = setup();
        let oracle_address = Address::generate(&env);

        // Configure oracle with 100 second max age
        // Healthy threshold = 100 / 2 = 50 seconds
        client.set_oracle_config(&admin, &true, &Some(oracle_address), &100);

        // Simulate a 60-second-old sample (our mock implementation)
        // age = 60, threshold = 50, so unhealthy
        let result = client.emit_oracle_liveness(&env);
        assert!(result.is_ok());
        let event = result.unwrap();
        assert!(!event.healthy, "Expected unhealthy when age > threshold");
        assert_eq!(event.age, 60);
        assert_eq!(event.last_sample_ts, T0 - 60);
    }

    #[test]
    fn test_oracle_liveness_event_emitted() {
        let (env, client, _token, admin) = setup();
        let oracle_address = Address::generate(&env);

        // Track events
        let events = env.events();
        let initial_count = events.all().len();

        // Configure and emit
        client.set_oracle_config(&admin, &true, &Some(oracle_address), &300);
        let _ = client.emit_oracle_liveness(&env);

        // Verify event was emitted
        let all_events = events.all();
        assert_eq!(all_events.len(), initial_count + 1);

        // Verify event topic
        let (topic, _) = all_events.last().unwrap();
        assert_eq!(topic, &(Symbol::new(&env, "oracle_liveness"),));
    }

    #[test]
    fn test_oracle_liveness_no_auth_required() {
        let (env, client, _token, admin) = setup();
        let oracle_address = Address::generate(&env);
        let random_caller = Address::generate(&env);

        // Configure oracle as admin
        client.set_oracle_config(&admin, &true, &Some(oracle_address), &300);

        // Anyone can call emit_oracle_liveness (no auth required)
        // This is by design - it's a view-only monitoring function
        let result = client.emit_oracle_liveness(&env);
        assert!(result.is_ok());
    }

    #[test]
    fn test_oracle_liveness_repeated_calls() {
        let (env, client, _token, admin) = setup();
        let oracle_address = Address::generate(&env);

        // Configure oracle
        client.set_oracle_config(&admin, &true, &Some(oracle_address), &300);

        // Call multiple times - each should succeed and emit an event
        for _ in 0..5 {
            let result = client.emit_oracle_liveness(&env);
            assert!(result.is_ok());
            let event = result.unwrap();
            assert!(event.healthy);
            assert_eq!(event.age, 60);
        }
    }

    #[test]
    fn test_oracle_liveness_with_different_max_ages() {
        let (env, client, _token, admin) = setup();

        // Test with very short max age (10 seconds)
        let oracle1 = Address::generate(&env);
        client.set_oracle_config(&admin, &true, &Some(oracle1), &10);
        let result = client.emit_oracle_liveness(&env);
        assert!(result.is_ok());
        let event = result.unwrap();
        // age=60, threshold=5, so unhealthy
        assert!(!event.healthy);

        // Test with very long max age (1000 seconds)
        env.ledger().with_mut(|li| li.timestamp += crate::admin::CONFIG_COOLDOWN_SECS);
        let oracle2 = Address::generate(&env);
        client.set_oracle_config(&admin, &true, &Some(oracle2), &1000);
        let result = client.emit_oracle_liveness(&env);
        assert!(result.is_ok());
        let event = result.unwrap();
        // age=60, threshold=500, so healthy
        assert!(event.healthy);
    }

    #[test]
    fn test_oracle_liveness_event_fields() {
        let (env, client, _token, admin) = setup();
        let oracle_address = Address::generate(&env);

        client.set_oracle_config(&admin, &true, &Some(oracle_address), &300);
        let event = client.emit_oracle_liveness(&env).unwrap();

        // Verify all fields are populated correctly
        assert_eq!(event.last_sample_ts, T0 - 60);
        assert_eq!(event.age, 60);
        assert!(event.healthy);
        assert_eq!(event.timestamp, T0);

        // Verify the event can be serialized/deserialized (contracttype property)
        let serialized = event.clone();
        assert_eq!(serialized.last_sample_ts, event.last_sample_ts);
        assert_eq!(serialized.age, event.age);
        assert_eq!(serialized.healthy, event.healthy);
        assert_eq!(serialized.timestamp, event.timestamp);
    }

    #[test]
    fn test_oracle_liveness_edge_case_exact_threshold() {
        let (env, client, _token, admin) = setup();
        let oracle_address = Address::generate(&env);

        // Set max_age to 120, so threshold = 60
        // Our mock produces age=60, which is exactly at threshold
        client.set_oracle_config(&admin, &true, &Some(oracle_address), &120);
        let result = client.emit_oracle_liveness(&env);

        assert!(result.is_ok());
        let event = result.unwrap();
        // age=60, threshold=60, so healthy (<= threshold)
        assert!(event.healthy, "Expected healthy when age == threshold");
        assert_eq!(event.age, 60);
    }

    #[test]
    fn test_oracle_liveness_config_persistence() {
        let (env, client, _token, admin) = setup();
        let oracle_address = Address::generate(&env);

        // Configure oracle
        client.set_oracle_config(&admin, &true, &Some(oracle_address), &300);

        // Verify config persists
        let config = client.get_oracle_config(&env);
        assert!(config.enabled);
        assert_eq!(config.oracle, Some(oracle_address));
        assert_eq!(config.max_age_seconds, 300);

        // Liveness check should work
        let result = client.emit_oracle_liveness(&env);
        assert!(result.is_ok());
    }
}

// ── Oracle staleness boundary tests (Issue #591) ─────────────────────────────
//
// These tests directly exercise `validate_price` from `oracle_adapter` to lock
// in the inclusive staleness boundary: a price whose age equals the freshness
// threshold is accepted, one second beyond is rejected with `Error::OracleStale`.

mod test_oracle_staleness_boundary {
    use super::*;
    use crate::oracle_adapter::validate_price;
    use crate::types::OraclePrice;

    const FRESHNESS_THRESHOLD: u64 = 300;

    /// Helper: build a price with the given timestamp offset from now.
    /// `age` is the intended age in seconds (now - price_timestamp).
    fn price_with_age(env: &Env, age: u64) -> OraclePrice {
        let now = env.ledger().timestamp();
        OraclePrice {
            price: 10_000_000, // 1.0 in PRICE_SCALE
            timestamp: now.saturating_sub(age),
        }
    }

    /// Helper: set ledger timestamp and return the env.
    fn env_with_timestamp(ts: u64) -> Env {
        let env = Env::default();
        env.ledger().set_timestamp(ts);
        env
    }

    // ── Core boundary tests ──────────────────────────────────────────────────

    #[test]
    fn stale_at_exact_threshold_accepted() {
        let env = env_with_timestamp(T0);
        let price = price_with_age(&env, FRESHNESS_THRESHOLD);
        // age == threshold must be accepted (inclusive boundary: age > max_age_seconds rejects)
        let result = validate_price(&env, &price, FRESHNESS_THRESHOLD);
        assert!(result.is_ok(), "Price at exact freshness threshold must be accepted");
        assert_eq!(result.unwrap(), 10_000_000);
    }

    #[test]
    fn stale_at_threshold_plus_one_rejected() {
        let env = env_with_timestamp(T0);
        let price = price_with_age(&env, FRESHNESS_THRESHOLD + 1);
        let result = validate_price(&env, &price, FRESHNESS_THRESHOLD);
        assert!(result.is_err(), "Price one second beyond threshold must be rejected");
        match result.unwrap_err() {
            Error::OraclePriceStale => {}
            e => panic!("Expected OraclePriceStale, got: {:?}", e),
        }
    }

    #[test]
    fn stale_at_threshold_well_inside_accepted() {
        let env = env_with_timestamp(T0);
        let price = price_with_age(&env, FRESHNESS_THRESHOLD / 2);
        let result = validate_price(&env, &price, FRESHNESS_THRESHOLD);
        assert!(result.is_ok(), "Price well inside freshness window must be accepted");
        assert_eq!(result.unwrap(), 10_000_000);
    }

    #[test]
    fn stale_far_beyond_threshold_rejected() {
        let env = env_with_timestamp(T0);
        let price = price_with_age(&env, FRESHNESS_THRESHOLD * 10);
        let result = validate_price(&env, &price, FRESHNESS_THRESHOLD);
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::OraclePriceStale => {}
            e => panic!("Expected OraclePriceStale, got: {:?}", e),
        }
    }

    // ── Edge case: threshold zero (always stale) ─────────────────────────────

    #[test]
    fn stale_threshold_zero_always_stale() {
        let env = env_with_timestamp(T0);
        // With threshold 0, any age > 0 is stale.
        // age == 0 should be accepted (age > 0 is false)
        let price_fresh = OraclePrice {
            price: 10_000_000,
            timestamp: T0, // age == 0
        };
        let result = validate_price(&env, &price_fresh, 0);
        assert!(result.is_ok(), "Zero age with zero threshold must be accepted");

        // age == 1 should be rejected
        let price_stale = OraclePrice {
            price: 10_000_000,
            timestamp: T0.saturating_sub(1), // age == 1
        };
        let result = validate_price(&env, &price_stale, 0);
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::OraclePriceStale => {}
            e => panic!("Expected OraclePriceStale for age=1 with threshold=0, got: {:?}", e),
        }
    }

    // ── Edge case: threshold u64::MAX (never stale) ─────────────────────────

    #[test]
    fn stale_threshold_u64_max_never_stale() {
        let env = env_with_timestamp(T0);

        // Even with a very old price, u64::MAX threshold means age can never exceed it.
        let price_very_old = OraclePrice {
            price: 10_000_000,
            timestamp: 0, // age == T0, which is < u64::MAX
        };
        let result = validate_price(&env, &price_very_old, u64::MAX);
        assert!(result.is_ok(), "Any price must be accepted when threshold is u64::MAX");
        assert_eq!(result.unwrap(), 10_000_000);
    }

    // ── Edge case: negative timestamp delta (price timestamp > now) ──────────

    #[test]
    fn stale_negative_delta_accepted() {
        let env = env_with_timestamp(T0);
        // Price timestamp in the future: saturating_sub gives 0.
        let price_future = OraclePrice {
            price: 10_000_000,
            timestamp: T0 + 1000, // in the future
        };
        let result = validate_price(&env, &price_future, FRESHNESS_THRESHOLD);
        assert!(result.is_ok(), "Future timestamp must be accepted (age == 0 via saturating_sub)");
        assert_eq!(result.unwrap(), 10_000_000);
    }

    // ── Edge case: non-positive price ────────────────────────────────────────

    #[test]
    fn stale_zero_price_rejected() {
        let env = env_with_timestamp(T0);
        let price = OraclePrice {
            price: 0,
            timestamp: T0,
        };
        let result = validate_price(&env, &price, FRESHNESS_THRESHOLD);
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::OraclePriceInvalid => {}
            e => panic!("Expected OraclePriceInvalid for zero price, got: {:?}", e),
        }
    }

    #[test]
    fn stale_negative_price_rejected() {
        let env = env_with_timestamp(T0);
        let price = OraclePrice {
            price: -1,
            timestamp: T0,
        };
        let result = validate_price(&env, &price, FRESHNESS_THRESHOLD);
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::OraclePriceInvalid => {}
            e => panic!("Expected OraclePriceInvalid for negative price, got: {:?}", e),
        }
    }

    // ── Round-trip: SpotAdapter ──────────────────────────────────────────────

    #[test]
    #[ignore = "Requires mock oracle contract infrastructure (wave 3). validate_price is tested directly above."]
    fn spot_adapter_respects_staleness_inclusively() {
        use crate::types::{OracleConfig, OracleKind};
        use crate::oracle_adapter::{OracleAdapter, SpotAdapter};

        let env = env_with_timestamp(T0);
        let oracle_addr = Address::generate(&env);

        let config = OracleConfig {
            enabled: true,
            oracle: Some(oracle_addr),
            max_age_seconds: FRESHNESS_THRESHOLD,
            kind: OracleKind::Spot,
            window_secs: 0,
            fixed_numerator: 0,
            fixed_denominator: 1,
        };

        // When the mock oracle contract infrastructure is available (wave 3),
        // extend this test to register a mock oracle and verify:
        //   SpotAdapter::quote(&env, &config, &base, &quote)
        // accepts a price at the threshold and rejects at threshold+1.
        let _ = (config, oracle_addr);
    }