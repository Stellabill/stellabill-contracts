//! Optional oracle integration for cross-currency pricing.

use crate::safe_math::{safe_add, safe_div, safe_mul, safe_pow, safe_sub};
use crate::types::{DataKey, Error, OracleConfig, OraclePrice, Subscription};
use soroban_sdk::{Address, Env, Symbol, Vec};



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
        let config = OracleConfig {
            enabled,
            oracle,
            max_age_seconds,
        };
        env.storage().instance().set(&DataKey::Oracle, &config);
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
        storage.get(&DataKey::Oracle).unwrap_or(OracleConfig {
            enabled: false,
            oracle: None,
            max_age_seconds: 0,
        })
    }
}

/// Resolve token-denominated charge amount.
///
/// With oracle disabled, returns `subscription.amount` as-is.
/// With oracle enabled, interprets `subscription.amount` as quote units and converts
/// to token base units using oracle quote:
///
/// token_amount = ceil(quote_amount * 10^token_decimals / quote_per_token)
pub fn resolve_charge_amount(
    env: &Env,
    subscription_id: u32,
    subscription: &Subscription,
) -> Result<i128, Error> {
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

        let token_decimals =
            crate::admin::get_token_decimals(env, &subscription.token).unwrap_or(6);

        let scale = safe_pow(10i128, token_decimals)?;
        let numerator = safe_mul(subscription.amount, scale)?;
        let ceil_adjust = safe_sub(price.price, 1)?;
        let token_amount = safe_div(safe_add(numerator, ceil_adjust)?, price.price)?;

        if token_amount <= 0 {
            return Err(Error::OraclePriceInvalid);
        }

        env.events().publish(
            (Symbol::new(env, "oracle_charge_resolved"), subscription_id),
            crate::types::OracleChargeResolvedEvent {
                subscription_id,
                quote_amount: subscription.amount,
                token_amount,
                price: price.price,
                price_timestamp: price.timestamp,
                timestamp: env.ledger().timestamp(),
            },
        );

        Ok(token_amount)
    }
}
