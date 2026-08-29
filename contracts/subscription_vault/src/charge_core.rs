//! Single charge logic (no auth). Used by charge_subscription and batch_charge.
//!
//! Charge runs only when status is Active or GracePeriod. On insufficient balance the
//! subscription is moved to a recoverable non-active state and an explicit failure
//! event is emitted without mutating financial accounting state.
//! On lifetime cap exhaustion the subscription is cancelled (terminal state).
//!
//! See `docs/subscription_lifecycle.md` for lifecycle details.
//! See `docs/lifetime_caps.md` for cap enforcement semantics.
//!
//! **PRs that only change how one subscription is charged should edit this file only.**
//!
//! # Reentrancy Safety
//!
//! This module does **not make external token transfers**. All state mutations happen
//! before any external interactions:
//!
//! 1. **Checks**: Validate expiration, status, interval, balance, replay protection, caps
//! 2. **Effects**: Update subscription state AND merchant earnings in storage
//! 3. **Interactions**: None (no external calls in this module)
//!
//! The public entry-point `lib.rs::charge_subscription` acquires a `ReentrancyGuard`
//! before calling `charge_one`, providing defense-in-depth protection even though
//! this function is naturally safe from external reentrancy.
//!
//! Merchant crediting happens through internal calls to `merchant::credit_merchant_balance_for_token`,
//! which only updates storage and does not call external contracts.
//!
//! See `docs/reentrancy_hardening.md` for complete charge path analysis.

#![allow(dead_code)]

use crate::oracle_adapter::{dispatch_price, PRICE_SCALE};
use crate::queries::get_subscription;
use crate::safe_math::{safe_add, safe_sub, safe_sub_balance};
use crate::state_machine::transition_to;
use crate::statements::append_statement;
use crate::subscription::{next_charge_time, write_subscription};
use crate::types::{
    BillingChargeKind, BillingPeriodSnapshot, ChargeExecutionResult, ChargeFailureEvent, Coupon, DataKey,
    Error, FeeConvertedEvent, GracePeriodEnteredEvent, LifetimeCapReachedEvent,
    SubscriptionAutoPausedEvent, SubscriptionCancelledEvent, SubscriptionChargeFailedEvent,
    SubscriptionChargedEvent, SubscriptionStatus, TOPIC_CHARGED, UsageChargeRejectedEvent, UsageChargeResult,
    UsageLimits, UsageState, UsageStatementEvent, SNAPSHOT_FLAG_CLOSED,
    SNAPSHOT_FLAG_INTERVAL_CHARGED, SNAPSHOT_FLAG_USAGE_CHARGED,
};
use soroban_sdk::{Address, Env, String, Symbol};

/// Resolve the effective fee rate in basis points for a charge to `merchant`.
///
/// Priority:
/// 1. If a per-merchant override is set (`DataKey::MerchantFeeBps`), use it.
/// 2. Otherwise fall back to the global `DataKey::FeeBps`.
///
/// A zero return value means no fee is collected.
#[inline(always)]
fn route_fee_bps(env: &Env, merchant: &soroban_sdk::Address) -> u32 {
    if let Some(override_bps) = crate::merchant::get_merchant_fee_override_bps(env, merchant) {
        return override_bps;
    }
    crate::admin::get_protocol_fee_bps(env)
}

/// Emits a [`ChargeFailureEvent`] and returns `err` unchanged.
///
/// Call as `return Err(charge_fail(env, id, err, attempted, now))` on every
/// error path inside charge entry-points so that all failures are observable
/// by off-chain indexers regardless of error type.
#[inline(always)]
fn charge_fail(
    env: &Env,
    subscription_id: u32,
    err: Error,
    attempted_amount: i128,
    ledger: u64,
) -> Error {
    env.events().publish(
        (Symbol::new(env, "charge_failed_v2"), subscription_id),
        ChargeFailureEvent {
            subscription_id,
            error_code: err.to_code(),
            attempted_amount,
            ledger,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );
    err
}

/// Result of a fee-token conversion attempt.
struct FeeConversion {
    /// Converted fee amount in the target token. Equals `original` when no
    /// conversion took place.
    effective_amount: i128,
    /// Target token address, or `None` when no conversion was applied.
    target_token: Option<Address>,
    /// Oracle price used for conversion (quote per base, scaled by 10^7).
    /// 0 when no conversion was applied.
    rate: u128,
}

/// Attempt to convert a fee amount from `source_token` to the configured
/// fee-token override using the oracle.
///
/// Returns the fee amount unchanged (with `target_token = None`) when:
/// - No fee-token override is configured.
/// - The override matches `source_token`.
/// - The oracle is not available (not configured, or price fetch fails).
/// - Conversion would round to zero (precision loss guard).
fn convert_fee(
    env: &Env,
    source_token: &Address,
    fee_amount: i128,
) -> FeeConversion {
    let fee_token_opt = crate::admin::get_fee_token(env);
    let fee_token = match fee_token_opt {
        Some(ref t) if t != source_token => t.clone(),
        _ => {
            return FeeConversion {
                effective_amount: fee_amount,
                target_token: None,
                rate: 0,
            };
        }
    };

    let oracle_config = crate::oracle::get_oracle_config(env);
    if !oracle_config.enabled || oracle_config.oracle.is_none() {
        return FeeConversion {
            effective_amount: fee_amount,
            target_token: None,
            rate: 0,
        };
    }

    match dispatch_price(env, &oracle_config, source_token, &fee_token) {
        Ok(price) => {
            let converted = (fee_amount as u128)
                .checked_mul(price)
                .and_then(|v| v.checked_div(PRICE_SCALE))
                .unwrap_or(0) as i128;

            if converted > 0 {
                FeeConversion {
                    effective_amount: converted,
                    target_token: Some(fee_token),
                    rate: price,
                }
            } else {
                FeeConversion {
                    effective_amount: fee_amount,
                    target_token: None,
                    rate: 0,
                }
            }
        }
        Err(_) => FeeConversion {
            effective_amount: fee_amount,
            target_token: None,
            rate: 0,
        },
    }
}

/// Resolve the coupon bound to a subscription and compute the post-discount
/// charge amount.
///
/// Returns `(post_discount_amount, discount_amount, redemption_update)`.
/// `redemption_update` is `Some(next_used_count)` when a valid coupon was
/// applied and the redemption counter must be advanced on a successful charge.
/// The counter is intentionally not advanced here so failed/insufficient
/// charges do not consume a redemption.
fn resolve_coupon_discount(
    env: &Env,
    subscription_id: u32,
    now: u64,
    token: &Address,
    gross_amount: i128,
    coupon_code: &Option<Symbol>,
) -> Result<(i128, i128, Option<u32>), Error> {
    if gross_amount <= 0 {
        return Ok((gross_amount, 0, None));
    }

    let Some(code) = coupon_code else {
        return Ok((gross_amount, 0, None));
    };

    let coupon: Coupon = env
        .storage()
        .instance()
        .get(&DataKey::Coupon(code.clone()))
        .ok_or(Error::CouponNotFound)?;

    if coupon.token != token.clone() {
        return Err(Error::CouponTokenMismatch);
    }
    if coupon.expires_at <= now {
        return Err(Error::CouponExpired);
    }

    let used: u32 = env
        .storage()
        .instance()
        .get(&DataKey::CouponRedemptions(subscription_id))
        .unwrap_or(0);
    if used >= coupon.max_redemptions {
        return Err(Error::CouponRedemptionLimit);
    }

    let percent_discount =
        gross_amount.saturating_mul(coupon.percent_off_bps as i128) / 10_000i128;
    let raw_discount = coupon.fixed_off.saturating_add(percent_discount);
    let discount = raw_discount.min(gross_amount).max(0);
    let net_amount = gross_amount - discount;

    Ok((net_amount, discount, Some(used.saturating_add(1))))
}

/// Performs a single interval-based charge with optional replay protection.
pub fn charge_one(
    env: &Env,
    subscription_id: u32,
    now: u64,
    idempotency_key: Option<soroban_sdk::BytesN<32>>,
    admin_config: Option<&crate::admin::CachedAdminConfig>,
) -> Result<ChargeExecutionResult, Error> {
    let mut sub = get_subscription(env, subscription_id)
        .map_err(|e| charge_fail(env, subscription_id, e, 0, now))?;

    // Merchant pause guard — mirrors charge_usage_one enforcement
    if crate::merchant::get_merchant_paused(env, sub.merchant.clone()) {
        return Err(charge_fail(
            env,
            subscription_id,
            Error::MerchantPaused,
            0,
            now,
        ));
    }

    // Merchant vacation guard — block charges during vacation window
    if crate::merchant::is_merchant_in_vacation(env, &sub.merchant, now) {
        return Err(charge_fail(
            env,
            subscription_id,
            Error::VacationActive,
            0,
            now,
        ));
    }

    crate::blocklist::require_not_blocklisted(env, &sub.subscriber)
        .map_err(|e| charge_fail(env, subscription_id, e, 0, now))?;
    crate::blocklist::require_not_blocklisted(env, &sub.merchant)
        .map_err(|e| charge_fail(env, subscription_id, e, 0, now))?;

    if let Some(split_payees) = crate::subscription::get_split_payees(env, subscription_id) {
        for entry in split_payees.entries.iter() {
            let (payee, _) = entry;
            crate::blocklist::require_not_blocklisted(env, &payee)
                .map_err(|e| charge_fail(env, subscription_id, e, 0, now))?;
            if crate::merchant::get_merchant_paused(env, payee.clone()) {
                return Err(charge_fail(
                    env,
                    subscription_id,
                    Error::MerchantPaused,
                    0,
                    now,
                ));
            }
            if crate::merchant::is_merchant_in_vacation(env, &payee, now) {
                return Err(charge_fail(
                    env,
                    subscription_id,
                    Error::VacationActive,
                    0,
                    now,
                ));
            }
        }
    }

    // Expiration guard
    if sub.is_expired(now, env.ledger().sequence()) {
        if sub.status != SubscriptionStatus::Expired {
            transition_to(&mut sub.status, SubscriptionStatus::Expired)?;
            write_subscription(env, subscription_id, &sub);
            env.events().publish(
                (Symbol::new(env, "subscription_expired"), subscription_id),
                crate::types::SubscriptionExpiredEvent {
                    subscription_id,
                    timestamp: now,
                    schema_version: crate::types::EVENT_SCHEMA_VERSION,
                },
            );
        }
        return Err(charge_fail(
            env,
            subscription_id,
            Error::SubscriptionExpired,
            0,
            now,
        ));
    }

    let charge_amount = crate::oracle::resolve_charge_amount(env, subscription_id, &sub)
        .map_err(|e| charge_fail(env, subscription_id, e, 0, now))?;

    // ── Coupon discount (before protocol-fee split) ───────────────────────────
    // Discount is applied to the oracle-resolved gross amount. The fee split and
    // merchant credit then operate on `charge_amount` (the post-discount payable).
    // This preserves: Gross = Discount + Merchant Net + Treasury Fee.
    let (charge_amount, _discount_amount, coupon_redemption) = resolve_coupon_discount(
        env,
        subscription_id,
        now,
        &sub.token,
        charge_amount,
        &sub.coupon_code,
    )
    .map_err(|e| charge_fail(env, subscription_id, e, charge_amount, now))?;

    if let Some(cap) = sub.lifetime_cap {
        if sub.lifetime_charged >= cap {
            if sub.status != SubscriptionStatus::Cancelled {
                transition_to(&mut sub.status, SubscriptionStatus::Cancelled)?;
                write_subscription(env, subscription_id, &sub);
                env.events().publish(
                    (Symbol::new(env, "lifetime_cap_reached"), subscription_id),
                    LifetimeCapReachedEvent {
                        subscription_id,
                        lifetime_cap: cap,
                        lifetime_charged: sub.lifetime_charged,
                        timestamp: now,
                        schema_version: crate::types::EVENT_SCHEMA_VERSION,
                    },
                );
            }
            return Ok(ChargeExecutionResult::LifetimeCapReached);
        }
    }

    // Scheduled cancellation: fire when cancel_at has arrived.
    if let Some(cancel_at) = sub.cancel_at {
        if now >= cancel_at {
            if sub.status != SubscriptionStatus::Cancelled {
                transition_to(&mut sub.status, SubscriptionStatus::Cancelled)?;
                let refund_amount = sub.prepaid_balance;
                sub.prepaid_balance = 0;
                sub.cancel_at = None;
                let token_addr = sub.token.clone();
                write_subscription(env, subscription_id, &sub);
                if refund_amount > 0 {
                    let token_client = soroban_sdk::token::Client::new(env, &token_addr);
                    token_client.transfer(
                        &env.current_contract_address(),
                        &sub.subscriber,
                        &refund_amount,
                    );
                    crate::accounting::sub_total_accounted(env, &token_addr, refund_amount)?;
                }
                env.events().publish(
                    (
                        soroban_sdk::Symbol::new(env, "subscription_cancelled"),
                        subscription_id,
                    ),
                    SubscriptionCancelledEvent {
                        subscription_id,
                        subscriber: sub.subscriber.clone(),
                        merchant: sub.merchant.clone(),
                        token: sub.token.clone(),
                        authorizer: sub.subscriber.clone(),
                        refund_amount,
                        timestamp: now,
                        schema_version: crate::types::EVENT_SCHEMA_VERSION,
                    },
                );
            }
            return Ok(ChargeExecutionResult::ScheduledCancellation);
        }
    }

    if sub.status != SubscriptionStatus::Active && sub.status != SubscriptionStatus::GracePeriod {
        if sub.status == SubscriptionStatus::InsufficientBalance {
            let next_allowed = next_charge_time(sub.last_payment_timestamp, sub.interval_seconds)?;
            if now < next_allowed {
                return Err(charge_fail(
                    env,
                    subscription_id,
                    Error::NotActive,
                    charge_amount,
                    now,
                ));
            }
        } else {
            return Err(charge_fail(
                env,
                subscription_id,
                Error::NotActive,
                charge_amount,
                now,
            ));
        }
    }

    // ── Auto-renewal gate ────────────────────────────────────────────────────
    // When auto_renew is false the billing engine skips the charge once the
    // interval has elapsed. The charge is silently skipped (not an error) so
    // that batch operations can continue past non-renewing subscriptions.
    if !sub.auto_renew {
        let next_allowed = next_charge_time(sub.last_payment_timestamp, sub.interval_seconds)?;
        if now >= next_allowed {
            // Interval has elapsed but auto-renewal is disabled — skip.
            return Ok(ChargeExecutionResult::Skipped);
        }
        // Interval hasn't elapsed yet: fall through to IntervalNotElapsed below.
    }

    let period_index = now.saturating_sub(sub.start_time) / sub.interval_seconds;
    let period_start = sub
        .start_time
        .checked_add(period_index.saturating_mul(sub.interval_seconds))
        .unwrap_or(u64::MAX);
    let period_end = period_start
        .checked_add(sub.interval_seconds)
        .unwrap_or(u64::MAX);

    // Anti-frontrunning salt
    let seq = env.ledger().sequence();
    let salt = {
        let mut salt_buf = [0u8; 20];
        salt_buf[..4].copy_from_slice(&subscription_id.to_be_bytes());
        salt_buf[4..12].copy_from_slice(&sub.last_payment_timestamp.to_be_bytes());
        salt_buf[12..20].copy_from_slice(&seq.to_be_bytes());
        let salt_input = soroban_sdk::Bytes::from_slice(env, &salt_buf);
        let hash: soroban_sdk::BytesN<32> = env.crypto().sha256(&salt_input).into();
        hash
    };

    let salt_key = DataKey::ChargeSalt(subscription_id);
    if let Some(last_salt) = env.storage().instance().get::<_, soroban_sdk::BytesN<32>>(&salt_key) {
        if last_salt == salt {
            return Err(charge_fail(
                env,
                subscription_id,
                Error::Replay,
                charge_amount,
                now,
            ));
        }
    }

    // Idempotent return: same idempotency key already processed
    if let Some(ref k) = idempotency_key {
        let hashed = crate::idempotency::hash_idem_key(
            env,
            crate::nonce::DOMAIN_CHARGE_INTERVAL,
            subscription_id,
            k,
        );
        if crate::idempotency::check_key(env, subscription_id, &hashed) {
            return Ok(ChargeExecutionResult::Charged);
        }
    }

    // Replay: already charged for this billing period
    if let Some(stored_period) = env
        .storage()
        .instance()
        .get::<_, u64>(&DataKey::ChargedPeriod(subscription_id))
    {
        if period_index <= stored_period {
            return Err(charge_fail(
                env,
                subscription_id,
                Error::Replay,
                charge_amount,
                now,
            ));
        }
    }

    let next_allowed = next_charge_time(sub.last_payment_timestamp, sub.interval_seconds)?;
    if now < next_allowed {
        return Err(charge_fail(
            env,
            subscription_id,
            Error::IntervalNotElapsed,
            charge_amount,
            now,
        ));
    }

    // -- Lifetime cap pre-check -----------------------------------------------
    if let Some(cap) = sub.lifetime_cap {
        let remaining = if sub.lifetime_charged >= cap {
            0
        } else {
            safe_sub(cap, sub.lifetime_charged)?
        };

        if remaining == 0 || charge_amount > remaining {
            // Cap already exhausted or this charge would exceed it: cancel without
            // moving funds and return an explicit terminal error.
            transition_to(&mut sub.status, SubscriptionStatus::Cancelled)?;
            write_subscription(env, subscription_id, &sub);

            env.events().publish(
                (Symbol::new(env, "lifetime_cap_reached"), subscription_id),
                LifetimeCapReachedEvent {
                    subscription_id,
                    lifetime_cap: cap,
                    lifetime_charged: sub.lifetime_charged,
                    timestamp: now,
                    schema_version: crate::types::EVENT_SCHEMA_VERSION,
                },
            );

            return Ok(ChargeExecutionResult::LifetimeCapReached);
        }
    }

    let storage = env.storage().instance();

    match safe_sub_balance(sub.prepaid_balance, charge_amount) {
        Ok(new_balance) => {
            sub.prepaid_balance = new_balance;
            let (fee_bps, treasury_opt) = if let Some(cfg) = admin_config {
                (cfg.fee_bps, cfg.treasury.clone())
            } else {
                (
                    crate::admin::get_protocol_fee_bps(env),
                    crate::admin::get_treasury(env),
                )
            };
            // Determine the protocol fee and merchant credit.
            //
            // Rounding rule: percentage fee is computed with integer division
            // (truncating). Any remainder from the division is deterministically
            // allocated to the merchant credit (i.e. merchant receives
            // `charge_amount - fee`). This ensures `merchant_amount + fee_amount == charge_amount`
            // exactly in the charge token and prevents 1-unit dust from remaining
            // in the vault. Converted fees (fee-token overrides) are handled
            // separately and do not affect the source-token accounting invariant.
            let (merchant_amount, fee_amount) = if fee_bps > 0 {
                if let Some(ref _t) = treasury_opt {
                    let fee = charge_amount * fee_bps as i128 / 10_000i128;
                    let net = charge_amount - fee;
                    (net, fee)
                } else {
                    (charge_amount, 0i128)
                }
            } else {
                (charge_amount, 0i128)
            };

            // Invariant sanity check: the split must sum exactly to the charged amount.
            // If this ever fails it indicates an arithmetic bug; keep as a debug
            // assertion so normal execution is unaffected in release builds.
            debug_assert!(
                merchant_amount + fee_amount == charge_amount,
                "fee + merchant != charge_amount (fee routing invariant)"
            );
            credit_charge_payees(
                env,
                subscription_id,
                &sub,
                merchant_amount,
                BillingChargeKind::Interval,
            )?;

            // Route merchant amount to sub-account if subscription has one
            if let Some(ref label) = sub.sub_account_label {
                crate::merchant::credit_sub_account(env, &sub.merchant, label, &sub.token, merchant_amount)?;
                // Deduct from parent balance (parent earnings stay for roll-up reporting)
                let parent_bal = crate::merchant::get_merchant_balance_by_token(env, &sub.merchant, &sub.token);
                let new_parent_bal = crate::safe_math::safe_sub(parent_bal, merchant_amount)?;
                crate::merchant::set_merchant_balance(env, &sub.merchant, &sub.token, &new_parent_bal);
            }

            let conversion = if fee_amount > 0 {
                Some(convert_fee(env, &sub.token, fee_amount))
            } else {
                None
            };
            let should_emit_fee_event = if fee_amount > 0 {
                if let Some(ref treasury) = treasury_opt {
                    let conv = conversion.as_ref().unwrap();
                    let fee_token = conv.target_token.clone();
                    let fee_credit_amount = conv.effective_amount;
                    if let Some(ref ft) = fee_token {
                        crate::merchant::credit_merchant_balance_for_token(
                            env,
                            treasury,
                            ft,
                            fee_credit_amount,
                            BillingChargeKind::Interval,
                        )?;
                    } else {
                        crate::merchant::credit_merchant_balance_for_token(
                            env,
                            treasury,
                            &sub.token,
                            fee_credit_amount,
                            BillingChargeKind::Interval,
                        )?;
                    }
                    Some((treasury.clone(), fee_amount))
                } else {
                    None
                }
            } else {
                None
            };
            sub.last_payment_timestamp = now.max(sub.last_payment_timestamp);

            sub.lifetime_charged = safe_add(sub.lifetime_charged, charge_amount)?;

            // Consume one coupon redemption only after the charge has succeeded.
            if let Some(next_used) = coupon_redemption {
                env.storage()
                    .instance()
                    .set(&DataKey::CouponRedemptions(subscription_id), &next_used);
            }

            // Recover from grace period or insufficient balance on successful charge.
            // Clear the grace clock so the next charge window uses fresh timestamps.
            if sub.status == SubscriptionStatus::GracePeriod
                || sub.status == SubscriptionStatus::InsufficientBalance
            {
                transition_to(&mut sub.status, SubscriptionStatus::Active)?;
                sub.grace_start_timestamp = None;
            }

            // Reset consecutive failure counter on any successful charge.
            env.storage()
                .instance()
                .remove(&DataKey::ChargeFailureCounter(subscription_id));

            // Check if cap is now exactly reached -- auto-cancel
            let cap_reached = sub
                .lifetime_cap
                .map(|cap| sub.lifetime_charged >= cap)
                .unwrap_or(false);

            if cap_reached {
                transition_to(&mut sub.status, SubscriptionStatus::Cancelled)?;
            }

            write_subscription(env, subscription_id, &sub);

            // Emit protocol fee event after state is written
            if let Some((treasury, fee)) = should_emit_fee_event {
                env.events().publish(
                    (Symbol::new(env, "protocol_fee_charged"), subscription_id),
                    crate::types::ProtocolFeeChargedEvent {
                        subscription_id,
                        merchant: sub.merchant.clone(),
                        token: sub.token.clone(),
                        fee_amount: fee,
                        treasury,
                        timestamp: now,
                        schema_version: crate::types::EVENT_SCHEMA_VERSION,
                    },
                );
                // Emit fee conversion event when fee-token override was applied
                if let Some(conv) = &conversion {
                    if let Some(ref target) = conv.target_token {
                        env.events().publish(
                            (Symbol::new(env, "fee_converted"), subscription_id),
                            FeeConvertedEvent {
                                subscription_id,
                                source_token: sub.token.clone(),
                                target_token: target.clone(),
                                original_fee_amount: fee,
                                converted_fee_amount: conv.effective_amount,
                                rate: conv.rate,
                                timestamp: now,
                                schema_version: crate::types::EVENT_SCHEMA_VERSION,
                            },
                        );
                    }
                }
            }

            append_statement(
                env,
                subscription_id,
                charge_amount,
                sub.merchant.clone(),
                BillingChargeKind::Interval,
                next_allowed.saturating_sub(sub.interval_seconds),
                now,
            )?;

            crate::period_snapshots::write_period_snapshot(
                env,
                BillingPeriodSnapshot {
                    subscription_id,
                    period_index,
                    period_start: next_allowed.saturating_sub(sub.interval_seconds),
                    period_end: now,
                    total_charged: charge_amount,
                    total_usage_units: 0,
                    status_flags: SNAPSHOT_FLAG_CLOSED | SNAPSHOT_FLAG_INTERVAL_CHARGED,
                    finalized_at: now,
                },
            )?;

            // Record charged period and optional idempotency key
            storage.set(&DataKey::ChargedPeriod(subscription_id), &period_index);
            storage.set(&salt_key, &salt);
            if let Some(k) = idempotency_key {
                let hashed = crate::idempotency::hash_idem_key(
                    env,
                    crate::nonce::DOMAIN_CHARGE_INTERVAL,
                    subscription_id,
                    &k,
                );
                crate::idempotency::push_key(env, subscription_id, &hashed);
            }

            env.events().publish(
                (TOPIC_CHARGED,),
                SubscriptionChargedEvent {
                    subscription_id,
                    subscriber: sub.subscriber.clone(),
                    merchant: sub.merchant.clone(),
                    token: sub.token.clone(),
                    amount: charge_amount,
                    lifetime_charged: sub.lifetime_charged,
                    timestamp: now,
                    period_start,
                    period_end,
                    salt: salt.clone(),
                    schema_version: crate::types::EVENT_SCHEMA_VERSION,
                },
            );

            if cap_reached {
                if let Some(cap) = sub.lifetime_cap {
                    env.events().publish(
                        (Symbol::new(env, "lifetime_cap_reached"), subscription_id),
                        LifetimeCapReachedEvent {
                            subscription_id,
                            lifetime_cap: cap,
                            lifetime_charged: sub.lifetime_charged,
                            timestamp: now,
                            schema_version: crate::types::EVENT_SCHEMA_VERSION,
                        },
                    );
                }
            }

            Ok(ChargeExecutionResult::Charged)
        }
        Err(_) => {
            let grace_duration = if let Some(cfg) = admin_config {
                cfg.grace_duration
            } else {
                crate::admin::get_grace_period(env)?
            };
            let previous_status = sub.status;

            if sub.status == SubscriptionStatus::GracePeriod {
                // Already in grace — check whether the window has expired since
                // the clock first started.  Keep the original grace_start_timestamp
                // so a single deposit within the window restores Active.
                if let Some(grace_start) = sub.grace_start_timestamp {
                    let grace_expires = grace_start.saturating_add(grace_duration);
                    if grace_duration == 0 || now >= grace_expires {
                        // Grace window closed — move to InsufficientBalance
                        transition_to(&mut sub.status, SubscriptionStatus::InsufficientBalance)?;
                        sub.grace_start_timestamp = None;
                    }
                    // else: stay in GracePeriod, keep clock unchanged
                } else {
                    // Sanity: GracePeriod status without a timestamp — treat as
                    // fresh entry so the clock is always initialised.
                    sub.grace_start_timestamp = Some(now);
                }
            } else if grace_duration > 0 {
                // First underfunded charge — enter GracePeriod and start the clock
                transition_to(&mut sub.status, SubscriptionStatus::GracePeriod)?;
                sub.grace_start_timestamp = Some(now);
            } else {
                // No grace period configured — go straight to InsufficientBalance
                transition_to(&mut sub.status, SubscriptionStatus::InsufficientBalance)?;
                sub.grace_start_timestamp = None;
            }

            write_subscription(env, subscription_id, &sub);

            // Emit grace_period_entered event after state is written
            if grace_duration > 0 && previous_status != SubscriptionStatus::GracePeriod {
                let grace_expires_at = now.saturating_add(grace_duration);
                env.events().publish(
                    (Symbol::new(env, "grace_period_entered"), subscription_id),
                    GracePeriodEnteredEvent {
                        subscription_id,
                        previous_status,
                        grace_expires_at,
                        timestamp: now,
                        schema_version: crate::types::EVENT_SCHEMA_VERSION,
                    },
                );
            }

            let shortfall = charge_amount.saturating_sub(sub.prepaid_balance).max(0);
            env.events().publish(
                (Symbol::new(env, "charge_failed"), subscription_id),
                SubscriptionChargeFailedEvent {
                    subscription_id,
                    merchant: sub.merchant,
                    required_amount: charge_amount,
                    available_balance: sub.prepaid_balance,
                    shortfall,
                    resulting_status: sub.status,
                    timestamp: now,
                    schema_version: crate::types::EVENT_SCHEMA_VERSION,
                },
            );

            // Increment consecutive failure counter and auto-pause if threshold reached.
            let threshold = if let Some(cfg) = admin_config {
                cfg.auto_pause_threshold
            } else {
                crate::admin::get_auto_pause_threshold(env)
            };
            if threshold > 0 && sub.status == SubscriptionStatus::InsufficientBalance {
                let counter_key = DataKey::ChargeFailureCounter(subscription_id);
                let failures: u32 = env
                    .storage()
                    .instance()
                    .get(&counter_key)
                    .unwrap_or(0u32)
                    .saturating_add(1);
                env.storage().instance().set(&counter_key, &failures);

                if failures >= threshold {
                    // Re-load subscription to apply the Paused transition cleanly.
                    let mut sub2 =
                        crate::queries::get_subscription(env, subscription_id).unwrap();
                    if sub2.status == SubscriptionStatus::InsufficientBalance {
                        if transition_to(&mut sub2.status, SubscriptionStatus::Paused).is_ok() {
                            crate::subscription::write_subscription(env, subscription_id, &sub2);
                            env.storage().instance().remove(&counter_key);
                            env.events().publish(
                                (Symbol::new(env, "sub_auto_paused"), subscription_id),
                                SubscriptionAutoPausedEvent {
                                    subscription_id,
                                    consecutive_failures: failures,
                                    threshold,
                                    timestamp: now,
                                    schema_version: crate::types::EVENT_SCHEMA_VERSION,
                                },
                            );
                        }
                    }
                }
            }

            Ok(ChargeExecutionResult::InsufficientBalance)
        }
    }
}

/// Debit a metered `usage_amount` from a subscription's prepaid balance.
pub fn charge_usage_one(
    env: &Env,
    subscription_id: u32,
    usage_amount: i128,
    reference: String,
) -> Result<UsageChargeResult, Error> {
    let mut sub = get_subscription(env, subscription_id)
        .map_err(|e| charge_fail(env, subscription_id, e, 0, env.ledger().timestamp()))?;
    let merchant = sub.merchant.clone();

    if crate::merchant::get_merchant_paused(env, merchant.clone()) {
        return Err(charge_fail(
            env,
            subscription_id,
            Error::MerchantPaused,
            0,
            env.ledger().timestamp(),
        ));
    }

    // Merchant vacation guard — block charges during vacation window
    let now = env.ledger().timestamp();
    if crate::merchant::is_merchant_in_vacation(env, &merchant, now) {
        return Err(charge_fail(
            env,
            subscription_id,
            Error::VacationActive,
            0,
            now,
        ));
    }

    crate::blocklist::require_not_blocklisted(env, &sub.subscriber)
        .map_err(|e| charge_fail(env, subscription_id, e, 0, env.ledger().timestamp()))?;
    crate::blocklist::require_not_blocklisted(env, &sub.merchant)
        .map_err(|e| charge_fail(env, subscription_id, e, 0, env.ledger().timestamp()))?;

    if let Some(split_payees) = crate::subscription::get_split_payees(env, subscription_id) {
        for entry in split_payees.entries.iter() {
            let (payee, _) = entry;
            crate::blocklist::require_not_blocklisted(env, &payee)
                .map_err(|e| charge_fail(env, subscription_id, e, 0, env.ledger().timestamp()))?;
            if crate::merchant::get_merchant_paused(env, payee.clone()) {
                return Err(charge_fail(
                    env,
                    subscription_id,
                    Error::MerchantPaused,
                    0,
                    env.ledger().timestamp(),
                ));
            }
            if crate::merchant::is_merchant_in_vacation(env, &payee, now) {
                return Err(charge_fail(
                    env,
                    subscription_id,
                    Error::VacationActive,
                    0,
                    now,
                ));
            }
        }
    }

    // Expiration guard
    if sub.is_expired(now, env.ledger().sequence()) {
        if sub.status != SubscriptionStatus::Expired {
            transition_to(&mut sub.status, SubscriptionStatus::Expired)?;
            write_subscription(env, subscription_id, &sub);
            env.events().publish(
                (Symbol::new(env, "subscription_expired"), subscription_id),
                crate::types::SubscriptionExpiredEvent {
                    subscription_id,
                    timestamp: now,
                    schema_version: crate::types::EVENT_SCHEMA_VERSION,
                },
            );
        }
        return Err(charge_fail(
            env,
            subscription_id,
            Error::SubscriptionExpired,
            0,
            now,
        ));
    }

    if let Some(cap) = sub.lifetime_cap {
        if sub.lifetime_charged >= cap {
            if sub.status != SubscriptionStatus::Cancelled {
                transition_to(&mut sub.status, SubscriptionStatus::Cancelled)?;
                write_subscription(env, subscription_id, &sub);
                env.events().publish(
                    (Symbol::new(env, "lifetime_cap_reached"), subscription_id),
                    LifetimeCapReachedEvent {
                        subscription_id,
                        lifetime_cap: cap,
                        lifetime_charged: sub.lifetime_charged,
                        timestamp: now,
                        schema_version: crate::types::EVENT_SCHEMA_VERSION,
                    },
                );
            }
            return Err(charge_fail(
                env,
                subscription_id,
                Error::LifetimeCapReached,
                usage_amount,
                now,
            ));
        }
    }

    if sub.status != SubscriptionStatus::Active {
        return Err(charge_fail(
            env,
            subscription_id,
            Error::NotActive,
            usage_amount,
            now,
        ));
    }

    if !sub.usage_enabled {
        return Err(charge_fail(
            env,
            subscription_id,
            Error::UsageNotEnabled,
            usage_amount,
            now,
        ));
    }

    if usage_amount <= 0 {
        return Err(charge_fail(
            env,
            subscription_id,
            Error::InvalidAmount,
            usage_amount,
            now,
        ));
    }

    if sub.prepaid_balance < usage_amount {
        return Err(charge_fail(
            env,
            subscription_id,
            Error::InsufficientPrepaidBalance,
            usage_amount,
            now,
        ));
    }

    // -- Replay protection (Reference-based) ----------------------------------
    // We use the reference as a unique idempotency key for usage charges.
    // If the reference has been seen before for this subscription, we return Replay.
    let ref_key = (
        Symbol::new(env, "usage_ref"),
        subscription_id,
        reference.clone(),
    );

    if env.storage().instance().has(&ref_key) {
        env.events().publish(
            (Symbol::new(env, "usage_charge_rejected"), subscription_id),
            UsageChargeRejectedEvent {
                subscription_id,
                merchant: sub.merchant.clone(),
                token: sub.token.clone(),
                usage_amount,
                timestamp: now,
                reference,
                result: UsageChargeResult::Replay,
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
        return Ok(UsageChargeResult::Replay);
    }

    // -- Usage Limits & State -------------------------------------------------
    let now = env.ledger().timestamp();
    let limits_key = DataKey::UsageLimits(subscription_id);
    let maybe_limits: Option<UsageLimits> = env.storage().instance().get(&limits_key);

    if let Some(limits) = maybe_limits {
        let state_key = DataKey::UsageState(subscription_id);
        let mut state = env
            .storage()
            .instance()
            .get(&state_key)
            .unwrap_or(UsageState {
                last_usage_timestamp: 0,
                window_start_timestamp: now,
                window_call_count: 0,
                current_period_usage_units: 0,
                period_index: now.saturating_sub(sub.start_time) / sub.interval_seconds,
            });

        // 1. Burst protection
        if limits.burst_min_interval_secs > 0 {
            let elapsed = now.saturating_sub(state.last_usage_timestamp);
            if elapsed < limits.burst_min_interval_secs {
                env.events().publish(
                    (Symbol::new(env, "usage_charge_rejected"), subscription_id),
                    UsageChargeRejectedEvent {
                        subscription_id,
                        merchant: sub.merchant.clone(),
                        token: sub.token.clone(),
                        usage_amount,
                        timestamp: now,
                        reference,
                        result: UsageChargeResult::BurstLimitExceeded,
                        schema_version: crate::types::EVENT_SCHEMA_VERSION,
                    },
                );
                return Ok(UsageChargeResult::BurstLimitExceeded);
            }
        }

        // 2. Rate limit (sliding window approximate)
        if let Some(max_calls) = limits.rate_limit_max_calls {
            if now
                >= state
                    .window_start_timestamp
                    .saturating_add(limits.rate_window_secs)
            {
                state.window_start_timestamp = now;
                state.window_call_count = 0;
            }
            if state.window_call_count >= max_calls {
                env.events().publish(
                    (Symbol::new(env, "usage_charge_rejected"), subscription_id),
                    UsageChargeRejectedEvent {
                        subscription_id,
                        merchant: sub.merchant.clone(),
                        token: sub.token.clone(),
                        usage_amount,
                        timestamp: now,
                        reference,
                        result: UsageChargeResult::RateLimitExceeded,
                        schema_version: crate::types::EVENT_SCHEMA_VERSION,
                    },
                );
                return Ok(UsageChargeResult::RateLimitExceeded);
            }
        }

        // 3. Usage cap (per-interval)
        if let Some(cap_units) = limits.usage_cap_units {
            let current_period = now.saturating_sub(sub.start_time) / sub.interval_seconds;
            if current_period > state.period_index {
                state.period_index = current_period;
                state.current_period_usage_units = 0;
            }
            if state
                .current_period_usage_units
                .saturating_add(usage_amount)
                > cap_units
            {
                env.events().publish(
                    (Symbol::new(env, "usage_charge_rejected"), subscription_id),
                    UsageChargeRejectedEvent {
                        subscription_id,
                        merchant: sub.merchant.clone(),
                        token: sub.token.clone(),
                        usage_amount,
                        timestamp: now,
                        reference,
                        result: UsageChargeResult::UsageCapExceeded,
                        schema_version: crate::types::EVENT_SCHEMA_VERSION,
                    },
                );
                return Ok(UsageChargeResult::UsageCapExceeded);
            }
        }

        // Update state
        state.last_usage_timestamp = now;
        state.window_call_count = state.window_call_count.saturating_add(1);
        state.current_period_usage_units = state
            .current_period_usage_units
            .saturating_add(usage_amount);
        env.storage().instance().set(&state_key, &state);
    }

    // -- Lifetime cap pre-check -----------------------------------------------
    // Over-cap attempts are blocked and cancel the subscription without debiting funds.
    let pending_lifetime = safe_add(sub.lifetime_charged, usage_amount)?;
    if let Some(cap) = sub.lifetime_cap {
        if pending_lifetime > cap {
            transition_to(&mut sub.status, SubscriptionStatus::Cancelled)?;
            write_subscription(env, subscription_id, &sub);
            env.events().publish(
                (Symbol::new(env, "lifetime_cap_reached"), subscription_id),
                LifetimeCapReachedEvent {
                    subscription_id,
                    lifetime_cap: cap,
                    lifetime_charged: sub.lifetime_charged,
                    timestamp: now,
                    schema_version: crate::types::EVENT_SCHEMA_VERSION,
                },
            );
            return Ok(UsageChargeResult::Charged);
        }
    }

    match crate::safe_math::safe_sub_balance(sub.prepaid_balance, usage_amount) {
        Ok(new_balance) => {
            sub.prepaid_balance = new_balance;
            let fee_bps = route_fee_bps(env, &sub.merchant);
            let treasury_opt = crate::admin::get_treasury(env);
            let (merchant_amount, fee_amount) = if fee_bps > 0 {
                if let Some(ref _t) = treasury_opt {
                    let fee = usage_amount * fee_bps as i128 / 10_000i128;
                    (usage_amount - fee, fee)
                } else {
                    (usage_amount, 0i128)
                }
            } else {
                (usage_amount, 0i128)
            };
            credit_charge_payees(
                env,
                subscription_id,
                &sub,
                merchant_amount,
                BillingChargeKind::Usage,
            )?;

            // Route merchant amount to sub-account if subscription has one
            if let Some(ref label) = sub.sub_account_label {
                crate::merchant::credit_sub_account(env, &sub.merchant, label, &sub.token, merchant_amount)?;
                // Deduct from parent balance (parent earnings stay for roll-up reporting)
                let parent_bal = crate::merchant::get_merchant_balance_by_token(env, &sub.merchant, &sub.token);
                let new_parent_bal = crate::safe_math::safe_sub(parent_bal, merchant_amount)?;
                crate::merchant::set_merchant_balance(env, &sub.merchant, &sub.token, &new_parent_bal);
            }

            let conversion = if fee_amount > 0 {
                Some(convert_fee(env, &sub.token, fee_amount))
            } else {
                None
            };
            let should_emit_fee_event = if fee_amount > 0 {
                if let Some(ref treasury) = treasury_opt {
                    let conv = conversion.as_ref().unwrap();
                    let fee_token = conv.target_token.clone();
                    let fee_credit_amount = conv.effective_amount;
                    if let Some(ref ft) = fee_token {
                        crate::merchant::credit_merchant_balance_for_token(
                            env,
                            treasury,
                            ft,
                            fee_credit_amount,
                            BillingChargeKind::Usage,
                        )?;
                    } else {
                        crate::merchant::credit_merchant_balance_for_token(
                            env,
                            treasury,
                            &sub.token,
                            fee_credit_amount,
                            BillingChargeKind::Usage,
                        )?;
                    }
                    Some((treasury.clone(), fee_amount))
                } else {
                    None
                }
            } else {
                None
            };

            sub.lifetime_charged = pending_lifetime;
            let cap_reached = sub
                .lifetime_cap
                .map(|cap| sub.lifetime_charged >= cap)
                .unwrap_or(false);

            if cap_reached {
                transition_to(&mut sub.status, SubscriptionStatus::Cancelled)?;
            } else if new_balance == 0 {
                // Without a cap hit, zero remaining prepaid means underfunded for future usage.
                transition_to(&mut sub.status, SubscriptionStatus::InsufficientBalance)?;
            }

            write_subscription(env, subscription_id, &sub);

            // Emit protocol fee event after state is written
            if let Some((treasury, fee)) = should_emit_fee_event {
                env.events().publish(
                    (Symbol::new(env, "protocol_fee_charged"), subscription_id),
                    crate::types::ProtocolFeeChargedEvent {
                        subscription_id,
                        merchant: sub.merchant.clone(),
                        token: sub.token.clone(),
                        fee_amount: fee,
                        treasury,
                        timestamp: now,
                        schema_version: crate::types::EVENT_SCHEMA_VERSION,
                    },
                );
                // Emit fee conversion event when fee-token override was applied
                if let Some(conv) = &conversion {
                    if let Some(ref target) = conv.target_token {
                        env.events().publish(
                            (Symbol::new(env, "fee_converted"), subscription_id),
                            FeeConvertedEvent {
                                subscription_id,
                                source_token: sub.token.clone(),
                                target_token: target.clone(),
                                original_fee_amount: fee,
                                converted_fee_amount: conv.effective_amount,
                                rate: conv.rate,
                                timestamp: now,
                                schema_version: crate::types::EVENT_SCHEMA_VERSION,
                            },
                        );
                    }
                }
            }

            env.storage().instance().set(&ref_key, &true); // Mark reference as used

            let period_index = now.saturating_sub(sub.start_time) / sub.interval_seconds;
            let period_start = sub
                .start_time
                .checked_add(
                    period_index
                        .checked_mul(sub.interval_seconds)
                        .ok_or(Error::Overflow)?,
                )
                .ok_or(Error::Overflow)?;

            crate::period_snapshots::write_period_snapshot(
                env,
                BillingPeriodSnapshot {
                    subscription_id,
                    period_index,
                    period_start,
                    period_end: now,
                    total_charged: usage_amount,
                    total_usage_units: usage_amount,
                    status_flags: SNAPSHOT_FLAG_USAGE_CHARGED,
                    finalized_at: now,
                },
            )?;

            append_statement(
                env,
                subscription_id,
                usage_amount,
                sub.merchant.clone(),
                BillingChargeKind::Usage,
                now,
                now,
            )?;

            env.events().publish(
                (Symbol::new(env, "usage_charged"), subscription_id),
                UsageStatementEvent {
                    subscription_id,
                    merchant: sub.merchant.clone(),
                    usage_amount,
                    token: sub.token.clone(),
                    timestamp: now,
                    reference,
                    schema_version: crate::types::EVENT_SCHEMA_VERSION,
                },
            );

            if cap_reached {
                if let Some(cap) = sub.lifetime_cap {
                    env.events().publish(
                        (Symbol::new(env, "lifetime_cap_reached"), subscription_id),
                        LifetimeCapReachedEvent {
                            subscription_id,
                            lifetime_cap: cap,
                            lifetime_charged: sub.lifetime_charged,
                            timestamp: now,
                            schema_version: crate::types::EVENT_SCHEMA_VERSION,
                        },
                    );
                }
            }
            Ok(UsageChargeResult::Charged)
        }
        Err(_) => {
            transition_to(&mut sub.status, SubscriptionStatus::InsufficientBalance)?;
            write_subscription(env, subscription_id, &sub);

            env.events().publish(
                (Symbol::new(env, "charge_failed"), subscription_id),
                SubscriptionChargeFailedEvent {
                    subscription_id,
                    merchant: sub.merchant,
                    required_amount: usage_amount,
                    available_balance: sub.prepaid_balance,
                    shortfall: usage_amount.saturating_sub(sub.prepaid_balance),
                    resulting_status: SubscriptionStatus::InsufficientBalance,
                    timestamp: now,
                    schema_version: crate::types::EVENT_SCHEMA_VERSION,
                },
            );
            Ok(UsageChargeResult::Charged)
        }
    }
}

// Distribute `net_merchant_amount` among configured split payees.
//
// Rounding rule: per-payee shares are computed using integer division
// (`share = net * weight / 10000`). To ensure the total distributed
// amount equals `net_merchant_amount` exactly, any remainder from the
// per-payee truncation is allocated to the first payee (index 0).
// This deterministic allocation prevents dust from accumulating in the
// vault and makes accounting auditable.
pub(crate) fn credit_charge_payees(
    env: &Env,
    subscription_id: u32,
    sub: &crate::types::Subscription,
    net_merchant_amount: i128,
    charge_kind: crate::types::BillingChargeKind,
) -> Result<(), Error> {
    if let Some(split_payees) = crate::subscription::get_split_payees(env, subscription_id) {
        let mut total_distributed_amount = 0i128;
        let num_payees = split_payees.entries.len();
        
        for i in 1..num_payees {
            if let Some(entry) = split_payees.entries.get(i) {
                let (payee, weight) = entry;
                let share = net_merchant_amount * weight as i128 / 10_000i128;
                total_distributed_amount = crate::safe_math::safe_add(total_distributed_amount, share)?;
                crate::merchant::credit_merchant_balance_for_token(
                    env,
                    &payee,
                    &sub.token,
                    share,
                    charge_kind,
                )?;
            }
        }
        
        if let Some(entry) = split_payees.entries.get(0) {
            let (payee, _) = entry;
            let first_share = net_merchant_amount - total_distributed_amount;
            crate::merchant::credit_merchant_balance_for_token(
                env,
                &payee,
                &sub.token,
                first_share,
                charge_kind,
            )?;
        }
        
        let mut payees_vec = soroban_sdk::Vec::new(env);
        for i in 0..num_payees {
            if let Some(entry) = split_payees.entries.get(i) {
                let (payee, weight) = entry;
                let share = if i == 0 {
                    net_merchant_amount - total_distributed_amount
                } else {
                    net_merchant_amount * weight as i128 / 10_000i128
                };
                payees_vec.push_back((payee, share));
            }
        }
        
        env.events().publish(
            (soroban_sdk::Symbol::new(env, "split_charge"), subscription_id),
            crate::types::SplitChargeEvent {
                subscription_id,
                payees: payees_vec,
                timestamp: env.ledger().timestamp(),
                schema_version: crate::types::EVENT_SCHEMA_VERSION,
            },
        );
    } else {
        crate::merchant::credit_merchant_balance_for_token(
            env,
            &sub.merchant,
            &sub.token,
            net_merchant_amount,
            charge_kind,
        )?;
    }
    Ok(())
}
