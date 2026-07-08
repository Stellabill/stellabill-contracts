//! Oracle adapter abstraction (Issue #477).
//!
//! Provides a pluggable pricing strategy pattern for oracle-based charge resolution.
//! Three strategies are supported:
//!
//! - [`SpotAdapter`]     — one-shot latest price from the configured oracle (default).
//! - [`TwapAdapter`]     — median price across a sliding time window.
//! - [`FixedRateAdapter`] — deterministic ratio; no oracle reads.
//!
//! Callers should not instantiate adapters directly. Use
//! [`dispatch_price`] which reads the [`OracleConfig`] and routes to the
//! correct adapter automatically.

use crate::types::{Error, OracleConfig, OracleKind, OraclePrice};
use soroban_sdk::{Address, Env, Vec};

// ── Scale factor ──────────────────────────────────────────────────────────────

/// Oracle prices are represented with 7 decimal places (e.g. Stellar asset precision).
pub const PRICE_SCALE: u128 = 10_000_000; // 10^7

// ── OracleAdapter trait ───────────────────────────────────────────────────────

/// Generic pricing source abstraction.
///
/// Implementors return a price denominated in `quote` units per 1 unit of
/// `base`, scaled by [`PRICE_SCALE`]. Callers apply this price to compute
/// the token amount from a subscription's subscription-currency amount.
pub trait OracleAdapter {
    /// Return the current price for the `base`/`quote` pair.
    ///
    /// The returned `u128` is the number of `quote` units per 1 `base` unit,
    /// scaled by `10^7` (e.g. `1_000_000 = 0.1`, `10_000_000 = 1.0`).
    ///
    /// # Errors
    ///
    /// - [`Error::OracleNotConfigured`]   — oracle address is absent when required.
    /// - [`Error::OraclePriceUnavailable`] — no price data is available.
    /// - [`Error::OraclePriceStale`]       — available data exceeds `max_age_seconds`.
    /// - [`Error::OraclePriceInvalid`]     — returned price is non-positive.
    /// - [`Error::InvalidInput`]           — adapter configuration is invalid.
    fn quote(
        env: &Env,
        config: &OracleConfig,
        base: &Address,
        quote: &Address,
    ) -> Result<u128, Error>;
}

// ── SpotAdapter ───────────────────────────────────────────────────────────────

/// Reads the most-recent price sample from the configured oracle.
///
/// This preserves the original behaviour of `resolve_charge_amount`. The oracle
/// contract must expose a `latest_price(base, quote) -> OraclePrice` view that
/// is already in use elsewhere in the repository.
pub struct SpotAdapter;

impl OracleAdapter for SpotAdapter {
    fn quote(
        env: &Env,
        config: &OracleConfig,
        _base: &Address,
        _quote: &Address,
    ) -> Result<u128, Error> {
        let oracle_addr = config.oracle.clone().ok_or(Error::OracleNotConfigured)?;

        // Call the oracle contract to fetch the latest price observation.
        let price: OraclePrice = env
            .invoke_contract(
                &oracle_addr,
                &soroban_sdk::Symbol::new(env, "latest_price"),
                soroban_sdk::Vec::new(env),
            );

        validate_price(env, &price, config.max_age_seconds)
    }
}

// ── TwapAdapter ───────────────────────────────────────────────────────────────

/// Reads a sliding window of price observations and returns their **median**.
///
/// Using the median rather than the arithmetic mean makes the adapter resistant
/// to single-block price manipulation: an attacker would need to control more
/// than half of the observations inside the window.
///
/// # Edge cases
/// - One observation → returns that price (equivalent to spot).
/// - Empty window   → returns [`Error::OraclePriceUnavailable`].
/// - Any observation older than `max_age_seconds` → filtered out.
/// - All observations stale after filtering → [`Error::OraclePriceStale`].
pub struct TwapAdapter;

impl OracleAdapter for TwapAdapter {
    fn quote(
        env: &Env,
        config: &OracleConfig,
        _base: &Address,
        _quote: &Address,
    ) -> Result<u128, Error> {
        let oracle_addr = config.oracle.clone().ok_or(Error::OracleNotConfigured)?;
        let now = env.ledger().timestamp();
        let window_start = now.saturating_sub(config.window_secs);

        // Fetch all observations from the oracle for this window.
        // The oracle contract must expose `get_observations(since: u64) -> Vec<OraclePrice>`.
        let observations: Vec<OraclePrice> = env.invoke_contract(
            &oracle_addr,
            &soroban_sdk::Symbol::new(env, "get_observations"),
            soroban_sdk::vec![env, soroban_sdk::IntoVal::into_val(&window_start, env)],
        );

        if observations.len() == 0 {
            return Err(Error::OraclePriceUnavailable);
        }

        // Collect and filter valid prices, rejecting stale samples.
        let mut prices: std::vec::Vec<u128> = std::vec::Vec::new();
        for obs in observations.iter() {
            // Reject observations older than `max_age_seconds`.
            if now.saturating_sub(obs.timestamp) > config.max_age_seconds {
                continue;
            }
            // Reject non-positive prices.
            if obs.price <= 0 {
                continue;
            }
            prices.push(obs.price as u128);
        }

        if prices.is_empty() {
            return Err(Error::OraclePriceStale);
        }

        Ok(median(&mut prices))
    }
}

/// Compute the median of a non-empty slice (sorts in-place, allocator-free).
fn median(values: &mut std::vec::Vec<u128>) -> u128 {
    values.sort_unstable();
    let mid = values.len() / 2;
    if values.len() % 2 == 0 {
        // Even count: average the two middle values.
        let lo = values[mid - 1];
        let hi = values[mid];
        (lo / 2) + (hi / 2) + ((lo % 2 + hi % 2) / 2)
    } else {
        values[mid]
    }
}

// ── FixedRateAdapter ─────────────────────────────────────────────────────────

/// Returns a deterministic price computed from a configured numerator/denominator.
///
/// `price = (numerator * PRICE_SCALE) / denominator`
///
/// This adapter performs no oracle reads and ignores staleness. It is suitable
/// for pegged pairs or test environments where a fixed exchange rate is desirable.
///
/// # Errors
/// - [`Error::InvalidInput`] — `denominator == 0`.
pub struct FixedRateAdapter;

impl OracleAdapter for FixedRateAdapter {
    fn quote(
        _env: &Env,
        config: &OracleConfig,
        _base: &Address,
        _quote: &Address,
    ) -> Result<u128, Error> {
        if config.fixed_denominator == 0 {
            return Err(Error::InvalidInput);
        }
        Ok((config.fixed_numerator * PRICE_SCALE) / config.fixed_denominator)
    }
}

// ── Validation helpers ────────────────────────────────────────────────────────

/// Validate a single [`OraclePrice`] observation against the staleness threshold.
pub fn validate_price(
    env: &Env,
    price: &OraclePrice,
    max_age_seconds: u64,
) -> Result<u128, Error> {
    if price.price <= 0 {
        return Err(Error::OraclePriceInvalid);
    }
    let now = env.ledger().timestamp();
    let age = now.saturating_sub(price.timestamp);
    if age > max_age_seconds {
        return Err(Error::OraclePriceStale);
    }
    Ok(price.price as u128)
}

// ── Central dispatch ──────────────────────────────────────────────────────────

/// Route to the correct adapter based on `config.kind` and return the price.
///
/// This is the single call-site that `resolve_charge_amount` should use to
/// obtain the exchange rate. All adapter selection logic lives here.
pub fn dispatch_price(
    env: &Env,
    config: &OracleConfig,
    base: &Address,
    quote: &Address,
) -> Result<u128, Error> {
    match config.kind {
        OracleKind::Spot => SpotAdapter::quote(env, config, base, quote),
        OracleKind::Twap => TwapAdapter::quote(env, config, base, quote),
        OracleKind::FixedRate => FixedRateAdapter::quote(env, config, base, quote),
    }
}
