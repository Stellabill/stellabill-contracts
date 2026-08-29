//! Optional oracle integration for cross-currency pricing.
//!
//! Includes a deviation circuit breaker that rejects price spikes exceeding a
//! configurable basis-point threshold relative to the median of recent samples.

use crate::safe_math::{safe_add, safe_div, safe_mul, safe_pow, safe_sub};
use crate::types::{
    DataKey, Error, OracleConfig, OracleDeviationBreakerEvent, OraclePrice,
    OraclePriceHistoryMeta, Subscription,
};
use soroban_sdk::{Address, Env, Symbol, Vec};

const KEY_ORACLE_ENABLED: &str = "oracle_enabled";
const KEY_ORACLE_ADDR: &str = "oracle_addr";
const KEY_ORACLE_MAX_AGE: &str = "oracle_max_age";
const KEY_ORACLE_DEVIATION_BPS: &str = "oracle_deviation_bps";

/// Number of recent price samples retained per token for the deviation check.
const ORACLE_PRICE_HISTORY_SIZE: u32 = 10;

// ── Oracle config ──────────────────────────────────────────────────────────────

pub fn set_oracle_config(
    env: &Env,
    enabled: bool,
    oracle: Option<Address>,
    max_age_seconds: u64,
) -> Result<(), Error> {
    #[cfg(not(feature = "oracle-pricing"))]
    {
        let _ = (env, enabled, oracle, max_age_seconds);
        return Err(Error::InvalidInput);
    }
    #[cfg(feature = "oracle-pricing")]
    {
        if enabled {
            if oracle.is_none() {
                return Err(Error::OracleNotConfigured);
            }
            if max_age_seconds == 0 {
                return Err(Error::InvalidInput);
            }
        }
        let storage = env.storage().instance();
        storage.set(&Symbol::new(env, KEY_ORACLE_ENABLED), &enabled);
        if let Some(ref addr) = oracle {
            storage.set(&Symbol::new(env, KEY_ORACLE_ADDR), addr);
        } else {
            storage.remove(&Symbol::new(env, KEY_ORACLE_ADDR));
        }
        storage.set(&Symbol::new(env, KEY_ORACLE_MAX_AGE), &max_age_seconds);
        Ok(())
    }
}

pub fn get_oracle_config(env: &Env) -> OracleConfig {
    #[cfg(not(feature = "oracle-pricing"))]
    {
        let _ = env;
        return OracleConfig {
            enabled: false,
            oracle: None,
            max_age_seconds: 0,
        };
    }
    #[cfg(feature = "oracle-pricing")]
    {
        let storage = env.storage().instance();
        OracleConfig {
            enabled: storage
                .get(&Symbol::new(env, KEY_ORACLE_ENABLED))
                .unwrap_or(false),
            oracle: storage.get::<_, Address>(&Symbol::new(env, KEY_ORACLE_ADDR)),
            max_age_seconds: storage
                .get(&Symbol::new(env, KEY_ORACLE_MAX_AGE))
                .unwrap_or(0u64),
        }
    }
}

// ── Deviation circuit breaker ──────────────────────────────────────────────────

/// Set the maximum allowed price deviation in basis points.
///
/// A value of `0` means any deviation at all will be rejected (strict mode).
/// When unset (default), the deviation check is skipped entirely.
pub fn set_oracle_deviation_bps(env: &Env, bps: u32) {
    #[cfg(feature = "oracle-pricing")]
    env.storage()
        .instance()
        .set(&Symbol::new(env, KEY_ORACLE_DEVIATION_BPS), &bps);
    #[cfg(not(feature = "oracle-pricing"))]
    let _ = (env, bps);
}

/// Read the deviation threshold.
///
/// Returns `None` when no threshold has been configured (check disabled).
pub fn get_oracle_deviation_bps(env: &Env) -> Option<u32> {
    #[cfg(feature = "oracle-pricing")]
    {
        env.storage()
            .instance()
            .get(&Symbol::new(env, KEY_ORACLE_DEVIATION_BPS))
    }
    #[cfg(not(feature = "oracle-pricing"))]
    {
        let _ = env;
        None
    }
}

/// Read the price history for a given token.
pub fn get_oracle_price_history(env: &Env, token: &Address) -> Vec<i128> {
    #[cfg(feature = "oracle-pricing")]
    {
        load_prices(env, token)
    }
    #[cfg(not(feature = "oracle-pricing"))]
    {
        let _ = (env, token);
        Vec::new(env)
    }
}

/// Load the ring-buffer metadata for `token`.
fn load_history_meta(env: &Env, token: &Address) -> OraclePriceHistoryMeta {
    env.storage()
        .instance()
        .get::<DataKey, OraclePriceHistoryMeta>(&DataKey::OraclePriceHistoryMeta(token.clone()))
        .unwrap_or(OraclePriceHistoryMeta { head: 0, count: 0 })
}

/// Persist updated ring-buffer metadata.
fn save_history_meta(env: &Env, token: &Address, meta: &OraclePriceHistoryMeta) {
    env.storage().instance().set(
        &DataKey::OraclePriceHistoryMeta(token.clone()),
        meta,
    );
}

/// Read all samples currently in the ring buffer for `token` (in insertion order).
fn load_prices(env: &Env, token: &Address) -> Vec<i128> {
    let meta = load_history_meta(env, token);
    let mut out = Vec::new(env);
    if meta.count == 0 {
        return out;
    }
    let n = meta.count.min(ORACLE_PRICE_HISTORY_SIZE);
    // tail = (head - count) mod size  (logical oldest)
    let size = ORACLE_PRICE_HISTORY_SIZE;
    let tail = if meta.count < size {
        0u32
    } else {
        meta.head
    };
    for i in 0..n {
        let slot = (tail + i) % size;
        if let Some(price) = env.storage().instance().get::<DataKey, i128>(
            &DataKey::OraclePriceHistoryEntry(token.clone(), slot),
        ) {
            out.push_back(price);
        }
    }
    out
}

/// Append a new price sample to the ring buffer for `token`.
fn record_price(env: &Env, token: &Address, price: i128) {
    let mut meta = load_history_meta(env, token);
    let slot = meta.head;
    meta.head = (meta.head + 1) % ORACLE_PRICE_HISTORY_SIZE;
    meta.count = meta.count.saturating_add(1);
    save_history_meta(env, token, &meta);
    env.storage().instance().set(
        &DataKey::OraclePriceHistoryEntry(token.clone(), slot),
        &price,
    );
}

/// Compute the median of a sorted price Vec.
/// Panics / returns the lower-middle for even-length lists.
fn median_of_sorted(sorted: &Vec<i128>) -> i128 {
    let len = sorted.len();
    sorted.get(len / 2).unwrap()
}

/// Sort prices using selection sort (N ≤ 10, so this is fine).
fn sort_prices(prices: &Vec<i128>, env: &Env) -> Vec<i128> {
    let n = prices.len();
    let mut sorted = Vec::new(env);
    for i in 0..n {
        sorted.push_back(prices.get(i).unwrap());
    }
    for i in 0..n {
        let mut min_idx = i;
        for j in (i + 1)..n {
            if sorted.get(min_idx).unwrap() > sorted.get(j).unwrap() {
                min_idx = j;
            }
        }
        if min_idx != i {
            let tmp = sorted.get(i).unwrap();
            sorted.set(i, sorted.get(min_idx).unwrap());
            sorted.set(min_idx, tmp);
        }
    }
    sorted
}

/// Check whether `new_price` deviates too far from the median of the last N
/// samples for `token`. If the deviation exceeds the configured threshold (or
/// the threshold is zero), return `Err(Error::OracleDeviationTooHigh)`.
///
/// On success the new price is recorded in the ring buffer.
fn check_deviation_and_record(
    env: &Env,
    token: &Address,
    new_price: i128,
) -> Result<(), Error> {
    let threshold_opt = get_oracle_deviation_bps(env);
    let threshold = match threshold_opt {
        Some(t) => t,
        None => return record_and_ok(env, token, new_price),
    };

    let prices = load_prices(env, token);
    let median = if prices.len() > 0 {
        let sorted = sort_prices(&prices, env);
        median_of_sorted(&sorted)
    } else {
        // Bootstrap: no history yet, always accept
        record_price(env, token, new_price);
        return Ok(());
    };

    let diff = if new_price > median {
        new_price - median
    } else {
        median - new_price
    };
    // deviation_bps = (diff * 10_000) / median
    let numerator = safe_mul(diff, 10_000i128)?;
    let deviation = safe_div(numerator, median)?;

    if (threshold == 0 && new_price != median) || deviation > threshold as i128 {
        let now = env.ledger().timestamp();
        env.events().publish(
            (Symbol::new(env, "oracle_deviation_breaker"),),
            OracleDeviationBreakerEvent {
                token: token.clone(),
                latest_price: new_price,
                median_price: median,
                deviation_bps: deviation as u64,
                threshold_bps: threshold,
                timestamp: now,
            },
        );
        return Err(Error::OracleDeviationTooHigh);
    }

    record_price(env, token, new_price);
    Ok(())
}

// Split out to avoid redundant code in the early-return path.
fn record_and_ok(env: &Env, token: &Address, price: i128) -> Result<(), Error> {
    record_price(env, token, price);
    Ok(())
}

// ── Charge resolution ──────────────────────────────────────────────────────────

/// Resolve token-denominated charge amount.
///
/// With oracle disabled, returns `subscription.amount` as-is.
/// With oracle enabled, interprets `subscription.amount` as quote units and converts
/// to token base units using oracle quote:
///
/// token_amount = ceil(quote_amount * 10^token_decimals / quote_per_token)
pub fn resolve_charge_amount(env: &Env, subscription: &Subscription) -> Result<i128, Error> {
    #[cfg(not(feature = "oracle-pricing"))]
    {
        let _ = env;
        return Ok(subscription.amount);
    }
    #[cfg(feature = "oracle-pricing")]
    {
        let cfg = get_oracle_config(env);
        if !cfg.enabled {
            return Ok(subscription.amount);
        }

        let oracle = cfg.oracle.ok_or(Error::OracleNotConfigured)?;
        let price: OraclePrice =
            env.invoke_contract(&oracle, &Symbol::new(env, "latest_price"), Vec::new(env));

        if price.price <= 0 {
            return Err(Error::OraclePriceInvalid);
        }
        if price.timestamp == 0 {
            return Err(Error::OraclePriceUnavailable);
        }
        if cfg.max_age_seconds > 0 {
            let now = env.ledger().timestamp();
            if now.saturating_sub(price.timestamp) > cfg.max_age_seconds {
                return Err(Error::OraclePriceStale);
            }
        }

        // Deviation circuit breaker — checked before the price is consumed.
        check_deviation_and_record(env, &subscription.token, price.price)?;

        let token_decimals =
            crate::admin::get_token_decimals(env, &subscription.token).unwrap_or(6);

        let scale = safe_pow(10i128, token_decimals)?;
        let numerator = safe_mul(subscription.amount, scale)?;
        let ceil_adjust = safe_sub(price.price, 1)?;
        let token_amount = safe_div(safe_add(numerator, ceil_adjust)?, price.price)?;

        if token_amount <= 0 {
            return Err(Error::OraclePriceInvalid);
        }
        Ok(token_amount)
    }
}
