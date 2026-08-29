use crate::types::{
    DataKey, Error, OracleConfig, OracleKind, OracleLivenessEvent, OracleConfigUpdatedEvent,
    Subscription,
};
use crate::admin::{read_config, write_config};
use soroban_sdk::{Address, Env, Symbol};

/// Resolve the charge amount for a subscription, applying oracle pricing when enabled.
///
/// When oracle pricing is disabled or the subscription has no cross-currency amount,
/// the subscription's own `amount` is returned directly (existing behaviour).
pub fn resolve_charge_amount(
    env: &Env,
    _subscription_id: u32,
    sub: &Subscription,
) -> Result<i128, Error> {
    let config = get_oracle_config(env);
    if !config.enabled {
        return Ok(sub.amount);
    }

    // When oracle is enabled, call the adapter to resolve the price.
    // We pass the token address as both base and quote to the adapter to fulfill the API,
    // though the current SpotAdapter implementation delegates to a latest_price() method 
    // that takes no arguments.
    let price = crate::oracle_adapter::dispatch_price(env, &config, &sub.token, &sub.token)?;

    // Calculate token_amount = ceil(quote_amount * 10^7 / price)
    // sub.amount is treated as the quote_amount.
    let scaled_amount = (sub.amount as u128)
        .checked_mul(crate::oracle_adapter::PRICE_SCALE)
        .ok_or(Error::MathOverflow)?;

    let token_amount = scaled_amount
        .checked_add(price.saturating_sub(1))
        .ok_or(Error::MathOverflow)?
        .checked_div(price)
        .ok_or(Error::MathOverflow)?;

    if token_amount > i128::MAX as u128 {
        return Err(Error::MathOverflow);
    }

    Ok(token_amount as i128)
}

/// Persist oracle configuration. Admin only (caller must have verified auth).
#[allow(clippy::too_many_arguments)]
pub fn set_oracle_config(
    env: &Env,
    enabled: bool,
    oracle: Option<Address>,
    max_age: u64,
    kind: OracleKind,
    window_secs: u64,
    fixed_numerator: u128,
    fixed_denominator: u128,
) -> Result<(), Error> {
    // Validate FixedRate denominator eagerly so bad config is rejected.
    if matches!(kind, OracleKind::FixedRate) && fixed_denominator == 0 {
        return Err(Error::InvalidInput);
    }

    let cfg = OracleConfig {
        enabled,
        oracle: oracle.clone(),
        max_age_seconds: max_age,
        kind: kind.clone(),
        window_secs,
        fixed_numerator,
        fixed_denominator,
    };
    write_config(env, &DataKey::Oracle, &cfg);

    env.events().publish(
        (Symbol::new(env, "oracle_config_updated"),),
        OracleConfigUpdatedEvent {
            enabled,
            oracle,
            max_age_seconds: max_age,
            kind,
            window_secs,
            fixed_numerator,
            fixed_denominator,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );
    Ok(())
}

/// Read the stored oracle configuration, defaulting to a disabled Spot config.
pub fn get_oracle_config(env: &Env) -> OracleConfig {
    read_config::<OracleConfig>(env, &DataKey::Oracle).unwrap_or(OracleConfig {
        enabled: false,
        oracle: None,
        max_age_seconds: 0,
        kind: OracleKind::Spot,
        window_secs: 0,
        fixed_numerator: 0,
        fixed_denominator: 1,
    })
}

/// Emit an oracle liveness event for monitoring purposes.
pub fn emit_oracle_liveness(env: &Env) -> Result<OracleLivenessEvent, Error> {
    let config = get_oracle_config(env);

    if !config.enabled || config.oracle.is_none() || config.max_age_seconds == 0 {
        return Err(Error::OracleNotConfigured);
    }

    // In a real implementation this would fetch the latest sample timestamp from the oracle.
    // Since we don't have a standardized way to just get the timestamp without doing a full 
    // quote (or maybe we do if we query the oracle directly), we will do a spot quote.
    // BUT this function is view-only and should not fail if price is stale, only report it.
    
    let now = env.ledger().timestamp();
    let oracle_addr = config.oracle.clone().unwrap();
    let price: crate::types::OraclePrice = env
        .invoke_contract(
            &oracle_addr,
            &soroban_sdk::Symbol::new(env, "latest_price"),
            soroban_sdk::Vec::new(env),
        );

    let last_sample_ts = price.timestamp;
    let age = now.saturating_sub(last_sample_ts);

    let threshold = config.max_age_seconds / 2;
    let healthy = age <= threshold;

    let event = OracleLivenessEvent {
        last_sample_ts,
        age,
        healthy,
        timestamp: now,
        schema_version: crate::types::EVENT_SCHEMA_VERSION,
    };

    env.events().publish(
        (Symbol::new(env, "oracle_liveness"),),
        event.clone(),
    );

    Ok(event)
}
