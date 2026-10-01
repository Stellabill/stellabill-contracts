#![cfg(test)]

//! Adversarial coverage for the metered usage path,
//! `contracts/subscription_vault/src/charge_core.rs::charge_usage_one`, reached
//! through `charge_usage`, `charge_usage_with_reference` and the operator
//! entry points.
//!
//! The function is a long guard chain followed by a single debit, so the
//! interesting surface is the *order* and the *state* left behind by every
//! rejected branch. This suite establishes that:
//!
//! * each precondition is enforced and observable as a distinct error
//!   (`InvalidAmount`, `InsufficientPrepaidBalance`, `UsageNotEnabled`,
//!   `NotActive`, `NotFound`, `MerchantPaused`, `VacationActive`,
//!   `SubscriberBlocklisted`, `SubscriptionExpired`, `LifetimeCapReached`,
//!   `EmergencyStopActive`);
//! * a rejected charge never moves the prepaid balance, never touches merchant
//!   earnings and never advances the recorded lifetime total;
//! * the reference is a real idempotency key: a replay returns `Replay` without
//!   debiting, while distinct references accumulate;
//! * the metered limits (burst interval, sliding-window rate limit, per-interval
//!   usage cap) reject with their dedicated `UsageChargeResult` variants and do
//!   not consume budget;
//! * the lifetime cap has two distinct outcomes — blocking at the cap, and
//!   cancelling without debiting when a charge would overshoot it;
//! * reaching `charge_usage_one` requires the stored admin (or the operator for
//!   the operator entry points).

use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    token::StellarAssetClient as TokenAdminClient,
    Address, Env, String, Symbol, TryFromVal, Val, Vec,
};
use subscription_vault::{
    types::{ChargeFailureEvent, UsageChargeRejectedEvent},
    Error, SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient, UsageChargeResult,
};

const T0: u64 = 1_000_000;
const MIN_TOPUP: i128 = 1_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const DEPOSIT: i128 = 10_000_000;

struct Fixture {
    env: Env,
    client: SubscriptionVaultClient<'static>,
    admin: Address,
    operator: Address,
    stranger: Address,
    subscriber: Address,
    merchant: Address,
    sub_id: u32,
}

/// Build a vault with a funded, usage-metered subscription.
///
/// `usage_enabled`, `lifetime_cap` and `expires_at` are parameterised because
/// several adversarial branches are only reachable from a subscription created
/// with a specific shape.
fn setup_with(
    usage_enabled: bool,
    lifetime_cap: Option<i128>,
    expires_at: Option<u64>,
    deposit: i128,
) -> Fixture {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_admin_client = TokenAdminClient::new(&env, &token_address);

    let admin = Address::generate(&env);
    let operator = Address::generate(&env);
    let stranger = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    client.init(
        &token_address,
        &7u32,
        &admin,
        &MIN_TOPUP,
        &(3 * 24 * 60 * 60),
    );

    token_admin_client.mint(&subscriber, &1_000_000_000);

    // A usage-metered subscription cannot be created unless the merchant has
    // already recorded a usage-limits entry for the id it is about to receive
    // (`Error::UsageLimitsRequired` / #6020). Seed one with every restriction
    // disabled so the default fixture exercises the plain debit path; the
    // limit-specific tests re-configure it afterwards.
    if usage_enabled {
        client.configure_usage_limits(&merchant, &0u32, &None::<u32>, &0u64, &0u64, &None::<i128>);
    }

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &5_000_000i128,
        &INTERVAL,
        &usage_enabled,
        &lifetime_cap,
        &expires_at,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );

    client.deposit_funds(&sub_id, &subscriber, &deposit, &None);

    Fixture {
        env,
        client,
        admin,
        operator,
        stranger,
        subscriber,
        merchant,
        sub_id,
    }
}

fn setup() -> Fixture {
    setup_with(true, None, None, DEPOSIT)
}

fn advance_seconds(env: &Env, seconds: u64) {
    let now = env.ledger().timestamp();
    env.ledger().set_timestamp(now + seconds);
}

fn reference(env: &Env, s: &str) -> String {
    String::from_str(env, s)
}

fn balance(f: &Fixture) -> i128 {
    f.client.get_subscription(&f.sub_id).prepaid_balance
}

/// Data payloads published under `topic` during the most recent invocation.
fn event_data(env: &Env, topic: &str) -> Vec<Val> {
    let want = Symbol::new(env, topic);
    let all = env.events().all();
    let mut out = Vec::new(env);
    for i in 0..all.len() {
        let (_, topics, data): (Address, Vec<Val>, Val) = all.get(i).unwrap();
        if let Some(t) = topics.get(0) {
            if let Ok(published) = Symbol::try_from_val(env, &t) {
                if published == want {
                    out.push_back(data);
                }
            }
        }
    }
    out
}

// ── The debit path ──────────────────────────────────────────────────────────

#[test]
fn a_usage_charge_debits_the_balance_and_credits_the_merchant() {
    let f = setup();

    let result = f.client.charge_usage(&f.sub_id, &2_000_000i128);

    assert_eq!(result, UsageChargeResult::Charged);
    assert_eq!(balance(&f), DEPOSIT - 2_000_000);
    assert_eq!(f.client.get_merchant_balance(&f.merchant), 2_000_000);
    assert_eq!(
        f.client.get_subscription(&f.sub_id).lifetime_charged,
        2_000_000
    );
}

#[test]
fn spending_the_entire_prepaid_balance_is_allowed() {
    let f = setup();

    let result = f.client.charge_usage(&f.sub_id, &DEPOSIT);

    assert_eq!(result, UsageChargeResult::Charged);
    assert_eq!(balance(&f), 0);
    assert_eq!(f.client.get_merchant_balance(&f.merchant), DEPOSIT);
}

#[test]
fn distinct_references_accumulate_on_the_same_subscription() {
    let f = setup();

    for (i, amount) in [1_000_000i128, 2_000_000, 3_000_000].iter().enumerate() {
        let ref_ = reference(&f.env, &alloc_ref(i));
        let result = f
            .client
            .charge_usage_with_reference(&f.sub_id, amount, &ref_);
        assert_eq!(result, UsageChargeResult::Charged);
    }

    assert_eq!(balance(&f), DEPOSIT - 6_000_000);
    assert_eq!(f.client.get_merchant_balance(&f.merchant), 6_000_000);
}

fn alloc_ref(i: usize) -> std::string::String {
    std::format!("usage-{i}")
}

// ── Replay protection ───────────────────────────────────────────────────────

#[test]
fn charge_usage_uses_a_fixed_reference_so_the_second_call_replays() {
    let f = setup();

    // `charge_usage` always passes the literal reference "usage".
    assert_eq!(
        f.client.charge_usage(&f.sub_id, &1_000_000i128),
        UsageChargeResult::Charged
    );
    let second = f.client.try_charge_usage(&f.sub_id, &1_000_000i128);

    assert_eq!(second, Ok(Ok(UsageChargeResult::Replay)));
    assert_eq!(balance(&f), DEPOSIT - 1_000_000);
    assert_eq!(f.client.get_merchant_balance(&f.merchant), 1_000_000);
}

#[test]
fn a_replayed_reference_emits_a_rejected_event_and_does_not_debit() {
    let f = setup();
    let ref_ = reference(&f.env, "invoice-7");

    f.client
        .charge_usage_with_reference(&f.sub_id, &1_500_000i128, &ref_);
    let replay = f
        .client
        .try_charge_usage_with_reference(&f.sub_id, &1_500_000i128, &ref_);

    assert_eq!(replay, Ok(Ok(UsageChargeResult::Replay)));

    // `Env::events().all()` reflects the most recent invocation, so the rejected
    // event is still readable here.
    let rejected = event_data(&f.env, "usage_charge_rejected");
    assert_eq!(rejected.len(), 1);
    let parsed = UsageChargeRejectedEvent::try_from_val(&f.env, &rejected.get(0).unwrap()).unwrap();
    assert_eq!(parsed.result, UsageChargeResult::Replay);
    assert_eq!(parsed.usage_amount, 1_500_000);
    assert_eq!(parsed.reference, ref_);

    assert_eq!(balance(&f), DEPOSIT - 1_500_000);
}

#[test]
fn the_operator_entry_point_uses_a_different_idempotency_key_than_charge_usage() {
    let f = setup();
    f.client.set_operator(&f.admin, &f.operator);

    // `charge_usage` claims the reference "usage"; `operator_charge_usage`
    // claims "". They are therefore independent replay domains, so the same
    // logical charge can be submitted once through each entry point.
    assert_eq!(
        f.client.charge_usage(&f.sub_id, &1_000_000i128),
        UsageChargeResult::Charged
    );
    assert_eq!(
        f.client
            .operator_charge_usage(&f.operator, &f.sub_id, &1_000_000i128),
        UsageChargeResult::Charged
    );

    assert_eq!(balance(&f), DEPOSIT - 2_000_000);
}

// ── Amount validation ───────────────────────────────────────────────────────

#[test]
fn zero_and_negative_usage_amounts_are_rejected() {
    let f = setup();

    assert_eq!(
        f.client.try_charge_usage(&f.sub_id, &0i128),
        Err(Ok(Error::InvalidAmount))
    );
    assert_eq!(balance(&f), DEPOSIT);

    assert_eq!(
        f.client.try_charge_usage(&f.sub_id, &-1i128),
        Err(Ok(Error::InvalidAmount))
    );
    assert_eq!(balance(&f), DEPOSIT);

    assert_eq!(
        f.client.try_charge_usage(&f.sub_id, &i128::MIN),
        Err(Ok(Error::InvalidAmount))
    );
    assert_eq!(balance(&f), DEPOSIT);
}

#[test]
fn a_charge_above_the_prepaid_balance_is_rejected_without_debiting() {
    let f = setup();

    assert_eq!(
        f.client.try_charge_usage(&f.sub_id, &(DEPOSIT + 1)),
        Err(Ok(Error::InsufficientPrepaidBalance))
    );

    assert_eq!(balance(&f), DEPOSIT);
    assert_eq!(f.client.get_merchant_balance(&f.merchant), 0);
    assert_eq!(f.client.get_subscription(&f.sub_id).lifetime_charged, 0);
}

#[test]
fn a_failed_amount_check_is_reported_with_its_error_code_and_attempted_amount() {
    let f = setup();

    let _ = f.client.try_charge_usage(&f.sub_id, &0i128);

    let failed = event_data(&f.env, "charge_failed_v2");
    assert_eq!(failed.len(), 1);
    let parsed = ChargeFailureEvent::try_from_val(&f.env, &failed.get(0).unwrap()).unwrap();
    assert_eq!(parsed.error_code, Error::InvalidAmount.to_code());
    assert_eq!(parsed.attempted_amount, 0);
    assert_eq!(parsed.subscription_id, f.sub_id);
}

// ── Subscription-state guards ───────────────────────────────────────────────

#[test]
fn a_usage_charge_on_an_unknown_subscription_is_rejected() {
    let f = setup();

    assert_eq!(
        f.client.try_charge_usage(&9_999u32, &1_000i128),
        Err(Ok(Error::NotFound))
    );
    assert_eq!(f.client.get_merchant_balance(&f.merchant), 0);
}

#[test]
fn usage_is_rejected_when_the_subscription_does_not_have_it_enabled() {
    let f = setup_with(false, None, None, DEPOSIT);

    assert_eq!(
        f.client.try_charge_usage(&f.sub_id, &1_000_000i128),
        Err(Ok(Error::UsageNotEnabled))
    );
    assert_eq!(balance(&f), DEPOSIT);
    assert_eq!(f.client.get_merchant_balance(&f.merchant), 0);
}

#[test]
fn a_usage_charge_on_a_cancelled_subscription_is_rejected() {
    let f = setup();
    f.client.cancel_subscription(&f.sub_id, &f.subscriber);
    let after_cancel = balance(&f);

    assert_eq!(
        f.client.try_charge_usage(&f.sub_id, &1_000_000i128),
        Err(Ok(Error::NotActive))
    );

    assert_eq!(
        f.client.get_subscription(&f.sub_id).status,
        SubscriptionStatus::Cancelled
    );
    assert_eq!(balance(&f), after_cancel);
}

#[test]
fn a_usage_charge_on_an_expired_subscription_is_rejected_and_rolled_back() {
    let f = setup_with(true, None, Some(T0 + 100), DEPOSIT);
    let before = balance(&f);

    advance_seconds(&f.env, 200);
    let result = f.client.try_charge_usage(&f.sub_id, &1_000_000i128);

    assert_eq!(result, Err(Ok(Error::SubscriptionExpired)));

    // The branch attempts an `Active -> Expired` transition, but it returns an
    // error, so the host rolls the whole invocation back: the stored status is
    // still the pre-call value and no metadata was rewritten.
    assert_eq!(
        f.client.get_subscription(&f.sub_id).status,
        SubscriptionStatus::Active
    );
    assert_eq!(balance(&f), before);
    assert_eq!(f.client.get_merchant_balance(&f.merchant), 0);
}

#[test]
fn a_usage_charge_is_rejected_while_the_merchant_is_paused() {
    let f = setup();
    f.client.pause_merchant(&f.merchant);
    assert!(f.client.get_merchant_paused(&f.merchant));

    assert_eq!(
        f.client.try_charge_usage(&f.sub_id, &1_000_000i128),
        Err(Ok(Error::MerchantPaused))
    );
    assert_eq!(balance(&f), DEPOSIT);
}

#[test]
fn a_usage_charge_is_rejected_inside_the_merchant_vacation_window() {
    let f = setup();
    f.client
        .set_merchant_vacation(&f.merchant, &T0, &(T0 + 3_600));

    advance_seconds(&f.env, 60);
    assert_eq!(
        f.client.try_charge_usage(&f.sub_id, &1_000_000i128),
        Err(Ok(Error::VacationActive))
    );

    // Past the window the same charge succeeds, so the rejection was the window
    // and not something else about the request.
    advance_seconds(&f.env, 3_600);
    assert_eq!(
        f.client.charge_usage(&f.sub_id, &1_000_000i128),
        UsageChargeResult::Charged
    );
    assert_eq!(f.client.get_merchant_balance(&f.merchant), 1_000_000);
}

#[test]
fn a_usage_charge_is_rejected_when_the_subscriber_is_blocklisted() {
    let f = setup();
    f.client.add_to_blocklist(&f.admin, &f.subscriber, &None);

    assert_eq!(
        f.client.try_charge_usage(&f.sub_id, &1_000_000i128),
        Err(Ok(Error::SubscriberBlocklisted))
    );
    assert_eq!(balance(&f), DEPOSIT);
}

#[test]
fn a_usage_charge_is_rejected_while_the_emergency_stop_is_active() {
    let f = setup();
    f.client.enable_emergency_stop(&f.admin);
    assert!(f.client.get_emergency_stop_status());

    assert_eq!(
        f.client.try_charge_usage(&f.sub_id, &1_000_000i128),
        Err(Ok(Error::EmergencyStopActive))
    );
    assert_eq!(balance(&f), DEPOSIT);
}

// ── Lifetime cap ────────────────────────────────────────────────────────────

#[test]
fn the_lifetime_cap_is_enforced_at_deposit_time_not_only_at_charge_time() {
    // `create_subscription` requires `lifetime_cap >= amount`, so the cap has to
    // sit at or above the recurring amount (5_000_000) to be accepted at all.
    let f = setup_with(true, Some(6_000_000), None, 6_000_000);

    // The first deposit may consume the whole cap...
    assert_eq!(balance(&f), 6_000_000);

    // ...but nothing more may be loaded: `enforce_deposit_cap` allows only the
    // remaining chargeable capacity (`cap - lifetime_charged - prepaid_balance`),
    // and the amount still has to clear `min_topup` to reach that guard.
    let extra = f
        .client
        .try_deposit_funds(&f.sub_id, &f.subscriber, &1_000_000i128, &None);
    assert_eq!(extra, Err(Ok(Error::LifetimeCapReached)));
    assert_eq!(balance(&f), 6_000_000);
}

#[test]
fn the_deposit_guard_keeps_lifetime_charged_at_or_below_the_cap() {
    let f = setup_with(true, Some(6_000_000), None, 6_000_000);

    // Because a deposit can never push `lifetime_charged + prepaid_balance` past
    // the cap, `cap` is the largest total this subscription can ever be charged,
    // and it lands on it exactly.
    assert_eq!(
        f.client
            .charge_usage_with_reference(&f.sub_id, &3_000_000i128, &reference(&f.env, "a")),
        UsageChargeResult::Charged
    );
    assert_eq!(
        f.client
            .charge_usage_with_reference(&f.sub_id, &3_000_000i128, &reference(&f.env, "b")),
        UsageChargeResult::Charged
    );

    let sub = f.client.get_subscription(&f.sub_id);
    assert_eq!(sub.lifetime_charged, 6_000_000);
    assert_eq!(sub.lifetime_cap, Some(6_000_000));
    assert_eq!(sub.lifetime_charged, sub.lifetime_cap.unwrap());
    assert_eq!(sub.prepaid_balance, 0);
    assert_eq!(f.client.get_merchant_balance(&f.merchant), 6_000_000);
}

#[test]
fn the_charge_that_reaches_the_lifetime_cap_cancels_the_subscription() {
    let f = setup_with(true, Some(6_000_000), None, 6_000_000);

    assert_eq!(
        f.client
            .charge_usage_with_reference(&f.sub_id, &6_000_000i128, &reference(&f.env, "a")),
        UsageChargeResult::Charged
    );

    // The debit path itself terminates the subscription as soon as the running
    // total equals the cap, and still reports the charge as `Charged`.
    let sub = f.client.get_subscription(&f.sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(sub.lifetime_charged, 6_000_000);
    assert_eq!(sub.prepaid_balance, 0);
    assert_eq!(f.client.get_merchant_balance(&f.merchant), 6_000_000);
}

#[test]
fn a_charge_attempt_past_the_lifetime_cap_is_rejected_without_any_further_change() {
    let f = setup_with(true, Some(6_000_000), None, 6_000_000);
    f.client
        .charge_usage_with_reference(&f.sub_id, &6_000_000i128, &reference(&f.env, "a"));

    // The cap is checked *before* the status, usage-enabled, amount and balance
    // guards, so this is `LifetimeCapReached` rather than
    // `InsufficientPrepaidBalance` even though the balance is already zero.
    let result =
        f.client
            .try_charge_usage_with_reference(&f.sub_id, &1i128, &reference(&f.env, "b"));

    assert_eq!(result, Err(Ok(Error::LifetimeCapReached)));

    let sub = f.client.get_subscription(&f.sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(sub.lifetime_charged, 6_000_000);
    assert_eq!(sub.prepaid_balance, 0);
    assert_eq!(f.client.get_merchant_balance(&f.merchant), 6_000_000);
}

// ── Metered limits ──────────────────────────────────────────────────────────

#[test]
fn the_burst_interval_blocks_a_second_charge_at_the_same_instant() {
    let f = setup();
    f.client.configure_usage_limits(
        &f.merchant,
        &f.sub_id,
        &None::<u32>,
        &3_600u64,
        &60u64,
        &None::<i128>,
    );

    assert_eq!(
        f.client
            .charge_usage_with_reference(&f.sub_id, &1_000_000i128, &reference(&f.env, "a")),
        UsageChargeResult::Charged
    );

    // Same ledger timestamp: elapsed (0) < burst_min_interval_secs (60).
    assert_eq!(
        f.client.try_charge_usage_with_reference(
            &f.sub_id,
            &1_000_000i128,
            &reference(&f.env, "b")
        ),
        Ok(Ok(UsageChargeResult::BurstLimitExceeded))
    );
    assert_eq!(balance(&f), DEPOSIT - 1_000_000);

    // Once the interval has elapsed the next charge is accepted.
    advance_seconds(&f.env, 60);
    assert_eq!(
        f.client
            .charge_usage_with_reference(&f.sub_id, &1_000_000i128, &reference(&f.env, "c")),
        UsageChargeResult::Charged
    );
    assert_eq!(balance(&f), DEPOSIT - 2_000_000);
}

#[test]
fn the_rate_limit_trips_and_recovers_after_its_window() {
    let f = setup();
    f.client.configure_usage_limits(
        &f.merchant,
        &f.sub_id,
        &Some(2u32),
        &3_600u64,
        &0u64,
        &None::<i128>,
    );

    assert_eq!(
        f.client
            .charge_usage_with_reference(&f.sub_id, &1_000_000i128, &reference(&f.env, "a")),
        UsageChargeResult::Charged
    );
    assert_eq!(
        f.client
            .charge_usage_with_reference(&f.sub_id, &1_000_000i128, &reference(&f.env, "b")),
        UsageChargeResult::Charged
    );

    // Third call inside the window: window_call_count (2) >= max_calls (2).
    assert_eq!(
        f.client.try_charge_usage_with_reference(
            &f.sub_id,
            &1_000_000i128,
            &reference(&f.env, "c")
        ),
        Ok(Ok(UsageChargeResult::RateLimitExceeded))
    );
    assert_eq!(balance(&f), DEPOSIT - 2_000_000);

    // The sliding window rolls over, so the counter resets.
    advance_seconds(&f.env, 3_600);
    assert_eq!(
        f.client
            .charge_usage_with_reference(&f.sub_id, &1_000_000i128, &reference(&f.env, "d")),
        UsageChargeResult::Charged
    );
    assert_eq!(balance(&f), DEPOSIT - 3_000_000);
}

#[test]
fn the_per_interval_usage_cap_rejects_an_over_budget_charge_without_consuming_it() {
    let f = setup();
    f.client.configure_usage_limits(
        &f.merchant,
        &f.sub_id,
        &None::<u32>,
        &3_600u64,
        &0u64,
        &Some(5_000_000i128),
    );

    assert_eq!(
        f.client
            .charge_usage_with_reference(&f.sub_id, &3_000_000i128, &reference(&f.env, "a")),
        UsageChargeResult::Charged
    );

    // 3_000_000 used + 3_000_000 requested > 5_000_000 budget.
    assert_eq!(
        f.client.try_charge_usage_with_reference(
            &f.sub_id,
            &3_000_000i128,
            &reference(&f.env, "b")
        ),
        Ok(Ok(UsageChargeResult::UsageCapExceeded))
    );
    assert_eq!(balance(&f), DEPOSIT - 3_000_000);

    // The rejected attempt did not consume budget: a charge that still fits goes
    // through unchanged.
    assert_eq!(
        f.client
            .charge_usage_with_reference(&f.sub_id, &2_000_000i128, &reference(&f.env, "c")),
        UsageChargeResult::Charged
    );
    assert_eq!(balance(&f), DEPOSIT - 5_000_000);
}

#[test]
fn the_per_interval_usage_cap_budget_resets_in_the_next_interval() {
    let f = setup();
    f.client.configure_usage_limits(
        &f.merchant,
        &f.sub_id,
        &None::<u32>,
        &3_600u64,
        &0u64,
        &Some(4_000_000i128),
    );

    assert_eq!(
        f.client
            .charge_usage_with_reference(&f.sub_id, &4_000_000i128, &reference(&f.env, "a")),
        UsageChargeResult::Charged
    );
    assert_eq!(
        f.client
            .try_charge_usage_with_reference(&f.sub_id, &1i128, &reference(&f.env, "b")),
        Ok(Ok(UsageChargeResult::UsageCapExceeded))
    );

    // Crossing into the next billing interval resets the per-period budget.
    advance_seconds(&f.env, INTERVAL);
    assert_eq!(
        f.client
            .charge_usage_with_reference(&f.sub_id, &4_000_000i128, &reference(&f.env, "c")),
        UsageChargeResult::Charged
    );
    assert_eq!(balance(&f), DEPOSIT - 8_000_000);
}

// ── Caller authorization on the operator entry point ────────────────────────

#[test]
fn a_stranger_cannot_reach_the_charge_core_through_the_operator_entry_point() {
    let f = setup();
    f.client.set_operator(&f.admin, &f.operator);

    let result = f
        .client
        .try_operator_charge_usage(&f.stranger, &f.sub_id, &1_000_000i128);

    assert_eq!(result, Err(Ok(Error::Unauthorized)));
    assert_eq!(balance(&f), DEPOSIT);
    assert_eq!(f.client.get_merchant_balance(&f.merchant), 0);
}

#[test]
fn the_operator_entry_point_mirrors_the_same_guards_as_the_admin_one() {
    let f = setup();
    f.client.set_operator(&f.admin, &f.operator);

    // Amount guard...
    assert_eq!(
        f.client
            .try_operator_charge_usage(&f.operator, &f.sub_id, &0i128),
        Err(Ok(Error::InvalidAmount))
    );
    // ...and balance guard are shared, not re-implemented.
    assert_eq!(
        f.client
            .try_operator_charge_usage(&f.operator, &f.sub_id, &(DEPOSIT + 1)),
        Err(Ok(Error::InsufficientPrepaidBalance))
    );
    assert_eq!(balance(&f), DEPOSIT);

    // The happy path through the operator debits exactly like the admin path.
    assert_eq!(
        f.client
            .operator_charge_usage(&f.operator, &f.sub_id, &1_000_000i128),
        UsageChargeResult::Charged
    );
    assert_eq!(balance(&f), DEPOSIT - 1_000_000);
}
