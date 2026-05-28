//! Subscription Vault — prepaid USDC subscription billing on Stellar.
//!
//! # Architecture
//! The implementation is split across several modules:
//! - `admin` — initialisation and governance
//! - `subscription` — creation, deposit, cancel, migrate
//! - `charge_core` — interval and usage billing
//! - `merchant` — merchant config and withdrawals
//! - `queries` — read-only queries and reconciliation
//! - `types` — shared types and error codes
//! - `safe_math` — overflow-safe arithmetic helpers

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env};

// ── Error types ──────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    /// Subscription not found.
    NotFound = 1000,
    /// Caller is not the stored admin address.
    Unauthorized = 1001,
}

// ── Storage keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    Subscription(u64),
}

// ── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        lifetime_cap: Option<i128>,
        expires_at: Option<u64>,
    ) -> Result<u32, Error> {
        require_not_emergency_stop(&env)?;
        subscription::do_create_subscription(
            &env,
            subscriber,
            merchant,
            amount,
            interval_seconds,
            usage_enabled,
            lifetime_cap,
            expires_at,
        )
    }

    /// Creates a new subscription using a specific accepted token.
    ///
    /// Works like `create_subscription`, but lets you choose the token instead
    /// of using the default one. The token must already be added to the accepted list.
    ///
    /// Disabled when emergency stop is active.
    ///
    /// # Errors
    /// - `EmergencyStopActive` if paused
    /// - `TokenNotAccepted` if token is not allowed
    /// - `InvalidAmount` / `InvalidInterval` for bad input
    /// - `Blocklisted` or `MerchantPaused` if restricted
    ///
    /// # Returns
    /// The new subscription ID.
    #[allow(clippy::too_many_arguments)]
    pub fn create_subscription_with_token(
        env: Env,
        subscriber: Address,
        merchant: Address,
        token: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        lifetime_cap: Option<i128>,
        expires_at: Option<u64>,
    ) -> Result<u32, Error> {
        require_not_emergency_stop(&env)?;
        subscription::do_create_subscription_with_token(
            &env,
            subscriber,
            merchant,
            token,
            amount,
            interval_seconds,
            usage_enabled,
            lifetime_cap,
            expires_at,
        )
    }

    /// Deposit additional funds into a subscription's prepaid balance.
    ///
    /// **Disabled when emergency stop is active.**
    ///
    /// Transfers tokens from the subscriber to the contract vault, increasing the
    /// subscription's prepaid balance. This allows subscribers to top up their account
    /// before running out of funds.
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — ID of the subscription to fund.
    /// * `subscriber` — Address that will authorize and fund the deposit. Must match
    ///   the subscription's registered subscriber.
    /// * `amount` — Amount to deposit (in token base units). Must be greater than the
    ///   configured minimum top-up threshold.
    ///
    /// # Auth
    ///
    /// `subscriber` must authorize the transaction and must match the subscription's
    /// registered subscriber.
    ///
    /// # Errors
    ///
    /// * [`Error::EmergencyStopActive`] — Emergency stop is currently enabled.
    /// * [`Error::NotFound`] — Subscription does not exist.
    /// * [`Error::Unauthorized`] — `subscriber` does not match the subscription's subscriber.
    /// * [`Error::InvalidAmount`] — `amount` is not greater than the minimum top-up threshold.
    /// * [`Error::InsufficientFunds`] — Subscriber does not have enough token balance.
    ///
    /// # Events
    ///
    /// Emits [`FundsDepositedEvent`] with `subscription_id`, `amount`, and timestamp.
    pub fn deposit_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
    ) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;

        // Acquire reentrancy guard: prevents re-entry during token transfer
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "deposit_funds")?;

        subscription::do_deposit_funds(&env, subscription_id, subscriber, amount)
    }

    /// Creates a reusable plan template for subscriptions.
    ///
    /// Merchants can use this to define pricing once and reuse it across
    /// multiple subscribers. The template stores the amount, interval, usage flag,
    /// and optional lifetime cap.
    ///
    /// # Arguments
    ///
    /// * `merchant` — Address of the merchant creating the plan. Must authorize the transaction.
    /// * `amount` — Billing amount per interval (in token base units).
    /// * `interval_seconds` — Billing interval duration in seconds.
    /// * `usage_enabled` — Whether metered usage charges are allowed for subscriptions
    ///   created from this plan.
    /// * `lifetime_cap` — Optional maximum total amount that may ever be charged.
    ///   `None` means no cap.
    ///
    /// # Auth
    ///
    /// `merchant` must authorize the transaction.
    ///
    /// # Errors
    ///
    /// * [`Error::InvalidAmount`] — `amount` is not valid (e.g., ≤ 0).
    /// * [`Error::InvalidInterval`] — `interval_seconds` is not valid (e.g., 0).
    ///
    /// # Returns
    ///
    /// The newly allocated plan template ID.
    ///
    /// # Events
    ///
    /// Emits `plan_template_created` event with `merchant`, `plan_template_id`, and timestamp.
    pub fn create_plan_template(
        env: Env,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        lifetime_cap: Option<i128>,
    ) -> Result<u32, Error> {
        subscription::do_create_plan_template(
            &env,
            merchant,
            amount,
            interval_seconds,
            usage_enabled,
            lifetime_cap,
        )
    }

    /// Creates a plan template tied to a specific settlement token.
    ///
    /// Same as [`create_plan_template`](Self::create_plan_template), but uses a custom token
    /// instead of the default one. The token must already be added to the accepted list.
    ///
    /// # Arguments
    ///
    /// * `merchant` — Address of the merchant creating the plan. Must authorize the transaction.
    /// * `token` — Settlement token address. Must be in the accepted tokens list.
    /// * `amount` — Billing amount per interval (in token base units).
    /// * `interval_seconds` — Billing interval duration in seconds.
    /// * `usage_enabled` — Whether metered usage charges are allowed.
    /// * `lifetime_cap` — Optional maximum total amount that may ever be charged.
    ///
    /// # Auth
    ///
    /// `merchant` must authorize the transaction.
    ///
    /// # Errors
    ///
    /// * [`Error::TokenNotAccepted`] — `token` is not in the accepted tokens list.
    /// * [`Error::InvalidAmount`] — `amount` is not valid (e.g., ≤ 0).
    /// * [`Error::InvalidInterval`] — `interval_seconds` is not valid (e.g., 0).
    ///
    /// # Returns
    ///
    /// The newly allocated plan template ID.
    ///
    /// # Events
    ///
    /// Emits `plan_template_created` event with `merchant`, `plan_template_id`, and timestamp.
    pub fn create_plan_template_with_token(
        env: Env,
        merchant: Address,
        token: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        lifetime_cap: Option<i128>,
    ) -> Result<u32, Error> {
        subscription::do_create_plan_template_with_token(
            &env,
            merchant,
            token,
            amount,
            interval_seconds,
            usage_enabled,
            lifetime_cap,
        )
    }

    /// Create a subscription from a predefined plan template.
    ///
    /// Reads the plan template identified by `plan_template_id` and creates a new
    /// subscription using its stored parameters. If the plan has a `max_active`
    /// limit (see [`set_plan_max_active_subs`](Self::set_plan_max_active_subs)), this
    /// call enforces it before creating the subscription.
    ///
    /// # Arguments
    ///
    /// * `subscriber` — Address that will fund and own the subscription.
    /// * `plan_template_id` — ID of the plan template to instantiate.
    ///
    /// # Auth
    ///
    /// `subscriber` must authorize the transaction.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — No plan template for `plan_template_id`.
    /// * [`Error::SubscriptionLimitReached`] — ID space exhausted.
    /// * [`Error::PlanMaxActiveSubsReached`] — Subscriber already holds the maximum
    ///   number of concurrent active subscriptions for this plan.
    /// * [`Error::Blocklisted`] — Subscriber is blocklisted.
    /// * [`Error::MerchantPaused`] — The plan's merchant has a blanket pause.
    ///
    /// # Returns
    ///
    /// The newly allocated subscription ID.
    ///
    /// # Events
    ///
    /// Emits [`SubscriptionCreatedEvent`].
    pub fn create_subscription_from_plan(
        env: Env,
        subscriber: Address,
        plan_template_id: u32,
    ) -> Result<u32, Error> {
        require_not_emergency_stop(&env)?;
        subscription::do_create_subscription_from_plan(&env, subscriber, plan_template_id)
    }

    /// Retrieve a plan template by its ID.
    ///
    /// # Arguments
    ///
    /// * `plan_template_id` — ID of the plan template to fetch.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — No plan template for `plan_template_id`.
    pub fn get_plan_template(env: Env, plan_template_id: u32) -> Result<PlanTemplate, Error> {
        subscription::get_plan_template(&env, plan_template_id)
    }

    /// Updates a plan template by creating a new version.
    ///
    /// This does not modify the existing one. Instead, it creates a new version
    /// and keeps the old one intact. Existing subscriptions continue using
    /// their current settings unless migrated.
    ///
    /// # Errors
    /// - `NotFound` if template doesn’t exist
    /// - `Unauthorized` if not the owner
    /// - `InvalidAmount` / `InvalidInterval` for bad input
    ///
    /// # Returns
    /// The new template version ID.
    pub fn update_plan_template(
        env: Env,
        merchant: Address,
        plan_template_id: u32,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        lifetime_cap: Option<i128>,
    ) -> Result<u32, Error> {
        subscription::do_update_plan_template(
            &env,
            merchant,
            plan_template_id,
            amount,
            interval_seconds,
            usage_enabled,
            lifetime_cap,
        )
    }

    /// Sets the max number of active subscriptions a user can have for a plan.
    ///
    /// If `max_active` is `0`, there’s no limit. This is enforced when creating
    /// subscriptions from the plan.
    ///
    /// Only the plan’s merchant can call this.
    ///
    /// # Errors
    /// - `NotFound` if the plan doesn’t exist
    /// - `Unauthorized` if caller is not the merchant
    pub fn set_plan_max_active_subs(
        env: Env,
        merchant: Address,
        plan_template_id: u32,
        max_active: u32,
    ) -> Result<(), Error> {
        subscription::do_set_plan_max_active_subs(&env, merchant, plan_template_id, max_active)
    }

    /// Returns the configured max-active-subscriptions limit for a plan template.
    ///
    /// A value of `0` means no limit is enforced. This is the default when
    /// `set_plan_max_active_subs` has never been called for the given plan.
    pub fn get_plan_max_active_subs(env: Env, plan_template_id: u32) -> u32 {
        queries::get_plan_max_active_subs(&env, plan_template_id)
    }

    /// Migrates an existing subscription to a newer version of the same plan template.
    ///
    /// The subscriber must authorize this call. Migration is only allowed between
    /// plan versions that share the same `template_key`, and only from an older
    /// version to a newer one. The settlement token cannot change as part of
    /// migration, and lifetime caps are validated for compatibility.
    pub fn migrate_subscription_to_plan(
        env: Env,
        subscriber: Address,
        subscription_id: u32,
        new_plan_template_id: u32,
    ) -> Result<(), Error> {
        subscription::do_migrate_subscription_to_plan(
            &env,
            subscriber,
            subscription_id,
            new_plan_template_id,
        )
    }

    /// Set a per-subscriber credit limit for a specific settlement token. Admin only.
    ///
    /// The limit is expressed in token base units and applies across all of the
    /// subscriber's subscriptions using that token. When the aggregate exposure
    /// (prepaid balances plus expected interval liabilities) would exceed this
    /// value, new subscriptions and top-ups are rejected.
    pub fn set_subscriber_credit_limit(
        env: Env,
        admin: Address,
        subscriber: Address,
        token: Address,
        limit: i128,
    ) -> Result<(), Error> {
        subscription::do_set_subscriber_credit_limit(&env, admin, subscriber, token, limit)
    }

    /// Read the configured credit limit for a subscriber and token.
    ///
    /// Returns 0 when no limit is configured, meaning "no limit".
    pub fn get_subscriber_credit_limit(env: Env, subscriber: Address, token: Address) -> i128 {
        subscription::get_subscriber_credit_limit(&env, subscriber, token)
    }

    /// Return the current aggregate exposure for a subscriber and token.
    ///
    /// Exposure is defined as the sum of prepaid balances plus the next-interval
    /// amounts for active subscriptions.
    pub fn get_subscriber_exposure(
        env: Env,
        subscriber: Address,
        token: Address,
    ) -> Result<i128, Error> {
        subscription::get_subscriber_exposure(&env, subscriber, token)
    }

    /// Cancel the subscription. Allowed from Active, Paused, or InsufficientBalance.
    /// Transitions to the terminal `Cancelled` state.
    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_cancel_subscription(&env, subscription_id, authorizer)
    }

    /// Withdraw remaining prepaid balance from a cancelled subscription.
    ///
    /// Only allowed when the subscription is in `Cancelled` status. The subscriber
    /// receives their remaining prepaid balance back to their wallet.
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — ID of the subscription to withdraw from.
    /// * `subscriber` — Address that will receive the funds. Must match the subscription's
    ///   registered subscriber.
    ///
    /// # Auth
    ///
    /// `subscriber` must authorize the transaction and must match the subscription's
    /// registered subscriber.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Subscription does not exist.
    /// * [`Error::Unauthorized`] — `subscriber` does not match the subscription's subscriber.
    /// * [`Error::InvalidStatusTransition`] — Subscription is not in `Cancelled` status.
    /// * [`Error::InsufficientFunds`] — No prepaid balance to withdraw.
    ///
    /// # Events
    ///
    /// Emits `funds_withdrawn` event with `subscription_id`, `amount`, and timestamp.
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// token transfer. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.

    pub fn withdraw_subscriber_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
    ) -> Result<(), Error> {
        // Acquire reentrancy guard: prevents re-entry during token transfer
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "withdraw_subscriber_funds")?;

        subscription::do_withdraw_subscriber_funds(&env, subscription_id, subscriber)
    }

    /// Process a partial refund against a subscription's remaining prepaid balance.
    ///
    /// Only the contract admin may authorize partial refunds. The refunded amount
    /// is debited from the subscription's `prepaid_balance` and transferred back
    /// to the subscriber, following the same CEI pattern as other token flows.
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// token transfer. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    pub fn partial_refund(
        env: Env,
        admin: Address,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
    ) -> Result<(), Error> {
        // Acquire reentrancy guard: prevents re-entry during token transfer
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "partial_refund")?;

        subscription::do_partial_refund(&env, admin, subscription_id, subscriber, amount)
    }

    /// Pauses a subscription so it won’t be charged.
    ///
    /// Can be resumed later.
    ///
    /// # Errors
    /// - `NotFound` if subscription doesn’t exist
    /// - `Unauthorized` if caller is not subscriber or merchant
    /// - `InvalidStatusTransition` if not active
    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_pause_subscription(&env, subscription_id, authorizer)
    }

    /// Resume a paused or underfunded subscription.
    ///
    /// Allowed from `Paused`, `GracePeriod`, or `InsufficientBalance`.
    /// Transitions back to `Active`, enabling future charges.
    ///
    /// Note: resuming from `InsufficientBalance` does **not** automatically trigger a
    /// charge; the next scheduled charge will occur at the next billing engine cycle.
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — Subscription to resume.
    /// * `authorizer` — Must be either the subscriber or the merchant.
    ///
    /// # Auth
    ///
    /// `authorizer` must authorize and must be the subscriber or merchant.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Subscription does not exist.
    /// * [`Error::Unauthorized`] — `authorizer` is neither subscriber nor merchant.
    /// * [`Error::InvalidStatusTransition`] — Subscription is not in a resumable state.
    ///
    /// # Events
    ///
    /// Emits [`SubscriptionResumedEvent`] with `subscription_id` and timestamp.
    pub fn resume_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_resume_subscription(&env, subscription_id, authorizer)
    }

    /// Archive an expired or cancelled subscription to mark it as clean up.
    /// This preserves funds and allows withdrawal but prevents other actions.
    pub fn cleanup_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_cleanup_subscription(&env, subscription_id, authorizer)
    }

    /// Merchant-initiated one-off charge against the subscription's prepaid balance.
    ///
    /// **This function is disabled when the emergency stop is active.**
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// state mutations. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    pub fn charge_one_off(
        env: Env,
        subscription_id: u32,
        merchant: Address,
        amount: i128,
    ) -> Result<(), Error> {
        require_not_emergency_stop(&env)?;

        // Acquire reentrancy guard
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "charge_one_off")?;

        subscription::do_charge_one_off(&env, subscription_id, merchant, amount)
    }

    // ── Charging ──────────────────────────────────────────────────────────────

    /// Charge a subscription for one billing interval.
    ///
    /// **This function is disabled when the emergency stop is active.**
    ///
    /// Enforces strict interval timing and replay protection. Underfunded attempts
    /// move the subscription into a recoverable non-active state and emit a
    /// charge-failed event without mutating financial accounting fields.
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// state mutations. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    pub fn charge_subscription(
        env: Env,
        subscription_id: u32,
    ) -> Result<ChargeExecutionResult, Error> {
        require_not_emergency_stop(&env)?;

        // Acquire reentrancy guard: prevents the same function from being called
        // recursively (e.g., if a malicious token contract tries to call back).
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "charge_subscription")?;

        charge_core::charge_one(&env, subscription_id, env.ledger().timestamp(), None)
    }

    /// Charge a metered usage amount against the subscription's prepaid balance.
    ///
    /// **This function is disabled when the emergency stop is active.**
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// state mutations. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    pub fn charge_usage(
        env: Env,
        subscription_id: u32,
        usage_amount: i128,
    ) -> Result<UsageChargeResult, Error> {
        require_not_emergency_stop(&env)?;

        // Acquire reentrancy guard
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "charge_usage")?;

        charge_core::charge_usage_one(
            &env,
            subscription_id,
            usage_amount,
            String::from_str(&env, "usage"),
        )
    }

    /// Charge a metered usage amount against the subscription's prepaid balance with a reference.
    ///
    /// **This function is disabled when the emergency stop is active.**
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// state mutations. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    pub fn charge_usage_with_reference(
        env: Env,
        subscription_id: u32,
        usage_amount: i128,
        reference: String,
    ) -> Result<UsageChargeResult, Error> {
        require_not_emergency_stop(&env)?;

        // Acquire reentrancy guard
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "charge_usage_with_reference")?;

        charge_core::charge_usage_one(&env, subscription_id, usage_amount, reference)
    }

    /// Configure usage rate limits and caps for a subscription.
    ///
    /// Rate limits protect against runaway usage charges. All parameters are optional;
    /// pass `None` / `0` to disable that constraint.
    ///
    /// # Arguments
    ///
    /// * `merchant` — Must match the subscription's registered merchant.
    /// * `subscription_id` — Target subscription.
    /// * `rate_limit_max_calls` — Maximum number of [`charge_usage`](Self::charge_usage)
    ///   calls allowed within `rate_window_secs`. `None` disables call-count rate limiting.
    /// * `rate_window_secs` — Duration of the rate-limit sliding window in seconds.
    ///   Must be positive when `rate_limit_max_calls` is `Some`.
    /// * `burst_min_interval_secs` — Minimum seconds between any two usage charges
    ///   (burst protection). `0` disables burst protection.
    /// * `usage_cap_units` — Maximum cumulative usage amount (in token base units)
    ///   allowed per billing cycle. `None` disables the cap.
    ///
    /// # Auth
    ///
    /// `merchant` must authorize and must match the subscription's stored merchant.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Subscription does not exist.
    /// * [`Error::Unauthorized`] — `merchant` does not match.
    /// * [`Error::InvalidConfig`] — Inconsistent rate-limit parameters
    ///   (e.g., `rate_limit_max_calls` is `Some` but `rate_window_secs` is 0).
    pub fn configure_usage_limits(
        env: Env,
        merchant: Address,
        subscription_id: u32,
        rate_limit_max_calls: Option<u32>,
        rate_window_secs: u64,
        burst_min_interval_secs: u64,
        usage_cap_units: Option<i128>,
    ) -> Result<(), Error> {
        subscription::do_configure_usage_limits(
            &env,
            merchant,
            subscription_id,
            rate_limit_max_calls,
            rate_window_secs,
            burst_min_interval_secs,
            usage_cap_units,
        )
    }

    // ── Merchant ──────────────────────────────────────────────────────────────

    /// Lets a merchant withdraw earnings (default token) to their wallet.
    ///
    /// Moves funds from the contract balance to the merchant.
    ///
    /// # Arguments
    /// - `merchant`: must be the owner of the balance and authorize the call
    /// - `amount`: how much to withdraw (must be > 0 and within available balance)
    ///
    /// # Errors
    /// - Unauthorized → if auth fails
    /// - InvalidAmount → if amount ≤ 0
    /// - InsufficientFunds → if balance is not enough
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// token transfer. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    pub fn withdraw_merchant_funds(env: Env, merchant: Address, amount: i128) -> Result<(), Error> {
        // Acquire reentrancy guard: prevents re-entry during token transfer
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "withdraw_merchant_funds")?;

        merchant::withdraw_merchant_funds(&env, merchant, amount)
    }

    /// Withdraw earnings for a specific token.
    ///
    /// Useful when the merchant works with multiple tokens.
    ///
    /// # Arguments
    /// - `merchant`: must authorize
    /// - `token`: token to withdraw
    /// - `amount`: amount to withdraw
    ///
    /// # Errors
    /// Same as default withdraw +
    /// - TokenNotAccepted → if token is not supported
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// token transfer. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    pub fn withdraw_merchant_token_funds(
        env: Env,
        merchant: Address,
        token: Address,
        amount: i128,
    ) -> Result<(), Error> {
        // Acquire reentrancy guard: prevents re-entry during token transfer
        let _guard =
            crate::reentrancy::ReentrancyGuard::lock(&env, "withdraw_merchant_token_funds")?;

        merchant::withdraw_merchant_funds_for_token(&env, merchant, token, amount)
    }

    /// Get the merchant's accumulated (uncharged) balance.
    pub fn get_merchant_balance(env: Env, merchant: Address) -> i128 {
        merchant::get_merchant_balance(&env, &merchant)
    }

    /// Token-scoped merchant balance.
    pub fn get_merchant_balance_by_token(env: Env, merchant: Address, token: Address) -> i128 {
        merchant::get_merchant_balance_by_token(&env, &merchant, &token)
    }

    /// Detailed per-token earnings record for a merchant.
    ///
    /// Returns the [`TokenEarnings`] struct containing accruals (broken down by
    /// charge kind), withdrawals, and refunds. The reconciliation invariant
    /// `balance = accruals.total - withdrawals - refunds` must hold at all times.
    pub fn get_merchant_token_earnings(
        env: Env,
        merchant: Address,
        token: Address,
    ) -> crate::types::TokenEarnings {
        merchant::get_merchant_token_earnings(&env, &merchant, &token)
    }

    /// Check if a merchant has enabled a blanket pause.
    pub fn get_merchant_paused(env: Env, merchant: Address) -> bool {
        merchant::get_merchant_paused(&env, merchant)
    }

    /// Pause all subscriptions for a merchant.
    ///
    /// Stops charges and prevents new subscriptions.
    /// Acts like a soft emergency stop for just this merchant.
    ///
    /// # Auth
    /// merchant must authorize
    pub fn pause_merchant(env: Env, merchant: Address) -> Result<(), Error> {
        merchant::pause_merchant(&env, merchant)
    }

    /// Resume merchant activity after a pause.
    ///
    /// # Auth
    /// - merchant must authorize
    pub fn unpause_merchant(env: Env, merchant: Address) -> Result<(), Error> {
        merchant::unpause_merchant(&env, merchant)
    }

    /// Refund a subscriber directly from the merchant’s balance.
    ///
    /// Useful for customer support refunds without cancelling the subscription.
    ///
    /// # Arguments
    /// - `merchant`: must authorize
    /// - `subscriber`: receiver of funds
    /// - `token`: token used
    /// - `amount`: refund amount
    ///
    /// # Errors
    /// - Unauthorized
    /// - InvalidAmount
    /// - InsufficientFunds
    ///
    /// # Reentrancy Protection
    /// This function acquires a reentrancy guard to prevent recursive calls during
    /// token transfer. The guard is automatically released (even on error) via the
    /// Drop trait, guaranteeing cleanup.
    pub fn merchant_refund(
        env: Env,
        merchant: Address,
        subscriber: Address,
        token: Address,
        amount: i128,
    ) -> Result<(), Error> {
        // Acquire reentrancy guard: prevents re-entry during token transfer
        let _guard = crate::reentrancy::ReentrancyGuard::lock(&env, "merchant_refund")?;

        merchant::merchant_refund(&env, merchant, subscriber, token, amount)
    }

    /// Get a reconciliation snapshot for all tokens used by a merchant.
    pub fn get_reconciliation_snapshot(
        env: Env,
        merchant: Address,
    ) -> Vec<crate::types::TokenReconciliationSnapshot> {
        merchant::get_reconciliation_snapshot(&env, &merchant)
    }

    /// Get total earnings per token for a merchant.
    ///
    /// Includes total charged, withdrawn, and current balance.
    pub fn get_merchant_total_earnings(
        env: Env,
        merchant: Address,
    ) -> Vec<(Address, crate::types::TokenEarnings)> {
        merchant::get_merchant_total_earnings(&env, &merchant)
    }

    // ── Queries ──────────────────────────────────────────────────────────────

    /// Get a subscription by ID.
    ///
    /// Returns the full [`Subscription`] data.
    ///
    /// # Errors
    /// - NotFound → if the subscription doesn’t exist
    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        queries::get_subscription(&env, subscription_id)
    }

    /// Estimate how much to top up for future billing cycles.
    ///
    /// Calculates how much is needed to cover `num_intervals`,
    /// taking the current prepaid balance into account.
    /// Returns 0 if already covered.
    ///
    /// # Errors
    /// - NotFound → subscription doesn’t exist
    /// - Overflow → calculation overflow
    pub fn estimate_topup_for_intervals(
        env: Env,
        subscription_id: u32,
        num_intervals: u32,
    ) -> Result<i128, Error> {
        queries::estimate_topup_for_intervals(&env, subscription_id, num_intervals)
    }

    /// Get info about the next charge timing.
    ///
    /// Includes when the next charge is expected and whether it’s due.
    ///
    /// # Errors
    /// NotFound → subscription doesn’t exist.
    pub fn get_next_charge_info(env: Env, subscription_id: u32) -> Result<NextChargeInfo, Error> {
        queries::get_next_charge_info(&env, subscription_id)
    }

    /// Return subscriptions for a merchant, paginated.
    ///
    /// `limit` must be between 1 and [`queries::MAX_SUBSCRIPTION_LIST_PAGE`] inclusive.
    pub fn get_subscriptions_by_merchant(
        env: Env,
        merchant: Address,
        start: u32,
        limit: u32,
    ) -> Result<Vec<Subscription>, Error> {
        queries::get_subscriptions_by_merchant(&env, merchant, start, limit)
    }

    /// Return the total number of subscriptions ever created.
    pub fn get_subscription_count(env: Env) -> u32 {
        env.storage().instance().get(&DataKey::NextId).unwrap_or(0u32)
    }

    /// Return the total number of subscriptions for a merchant.
    pub fn get_merchant_subscription_count(env: Env, merchant: Address) -> u32 {
        queries::get_merchant_subscription_count(&env, merchant)
    }

    /// Return the number of subscription ids indexed for a settlement token (for pagination).
    pub fn get_token_subscription_count(env: Env, token: Address) -> u32 {
        queries::get_token_subscription_count(&env, token)
    }

    /// List subscriptions for a subscriber (cursor-based).
    ///
    /// # Errors
    /// - InvalidPageSize → if limit is invalid
    pub fn list_subscriptions_by_subscriber(
        env: Env,
        subscriber: Address,
        start_from_id: u32,
        limit: u32,
    ) -> Result<crate::queries::SubscriptionsPage, Error> {
        crate::queries::list_subscriptions_by_subscriber(&env, subscriber, start_from_id, limit)
    }

    /// Get lifetime cap information for a subscription.
    ///
    /// Returns a [`CapInfo`] summary suitable for off-chain dashboards and UX displays.
    /// When no cap is configured all cap-related fields return `None` / `false`.
    pub fn get_cap_info(env: Env, subscription_id: u32) -> Result<CapInfo, Error> {
        queries::get_cap_info(&env, subscription_id)
    }

    /// Set or clear the contract-wide default lifetime cap applied to new subscriptions.
    ///
    /// When set, any `create_subscription` call that provides no explicit `lifetime_cap`
    /// inherits this value (unless a per-merchant default takes precedence).
    /// Pass `None` to remove the global default.
    ///
    /// # Auth
    /// Admin only.
    pub fn set_global_cap_default(
        env: Env,
        admin: Address,
        cap: Option<i128>,
    ) -> Result<(), Error> {
        subscription::do_set_global_cap_default(&env, admin, cap)
    }

    /// Return the current contract-wide default lifetime cap, or `None` if unset.
    pub fn get_global_cap_default(env: Env) -> Option<i128> {
        subscription::get_global_cap_default(&env)
    }

    /// Set or clear a per-merchant default lifetime cap for all new subscriptions to this merchant.
    ///
    /// Overrides the global default for subscriptions created against `merchant`.
    /// Pass `None` to fall back to the global default.
    ///
    /// # Auth
    /// Merchant address must authorize.
    pub fn set_merchant_cap_default(
        env: Env,
        merchant: Address,
        cap: Option<i128>,
    ) -> Result<(), Error> {
        subscription::do_set_merchant_cap_default(&env, merchant, cap)
    }

    /// Return the per-merchant default lifetime cap, or `None` if unset.
    pub fn get_merchant_cap_default(env: Env, merchant: Address) -> Option<i128> {
        subscription::get_merchant_cap_default(&env, merchant)
    }

    /// Update the lifetime cap on an existing subscription.
    ///
    /// - Raising or removing the cap is always allowed.
    /// - Lowering the cap below `lifetime_charged` is rejected with `LifetimeCapReached`.
    /// - Setting to `None` removes the cap entirely.
    ///
    /// # Auth
    /// Admin only.
    pub fn update_subscription_cap(
        env: Env,
        admin: Address,
        subscription_id: u32,
        new_cap: Option<i128>,
    ) -> Result<(), Error> {
        subscription::do_update_subscription_cap(&env, admin, subscription_id, new_cap)
    }

    /// Return subscription billing statements using offset/limit pagination.
    ///
    /// When `newest_first` is true (recommended for infinite scroll), offset 0
    /// starts from the most recent statement.
    pub fn get_sub_statements_offset(
        env: Env,
        subscription_id: u32,
        offset: u32,
        limit: u32,
        newest_first: bool,
    ) -> Result<BillingStatementsPage, Error> {
        statements::get_statements_by_subscription_offset(
            &env,
            subscription_id,
            offset,
            limit,
            newest_first,
        )
    }

    /// Return subscription billing statements using cursor pagination.
    ///
    /// - `cursor`: sequence index to start from (inclusive); pass `None` for first page.
    /// - `limit`: maximum number of statements to return.
    /// - `newest_first`: return recent history first when true.
    pub fn get_sub_statements_cursor(
        env: Env,
        subscription_id: u32,
        cursor: Option<u32>,
        limit: u32,
        newest_first: bool,
    ) -> Result<BillingStatementsPage, Error> {
        statements::get_statements_by_subscription_cursor(
            &env,
            subscription_id,
            cursor,
            limit,
            newest_first,
        )
    }

    /// Return a single billing period snapshot by subscription and period index.
    ///
    /// `period_index` is `ledger_timestamp / interval_seconds` for the billing period.
    /// Returns `None` when no charge has been processed for that period.
    pub fn get_period_snapshot(
        env: Env,
        subscription_id: u32,
        period_index: u64,
    ) -> Option<BillingPeriodSnapshot> {
        period_snapshots::get_period_snapshot(&env, subscription_id, period_index)
    }

    /// Return the most-recent billing period snapshots for a subscription, newest first.
    ///
    /// - `limit`: maximum number of snapshots to return.
    pub fn list_period_snapshots(
        env: Env,
        subscription_id: u32,
        limit: u32,
    ) -> Vec<BillingPeriodSnapshot> {
        period_snapshots::list_period_snapshots(&env, subscription_id, limit)
    }

/// Add a new accepted token.
///
/// # Auth
/// - Admin only
///
/// # Errors
/// - Unauthorized
/// - TokenAlreadyAccepted
    pub fn add_accepted_token(
        env: Env,
        admin: Address,
        token: Address,
        decimals: u32,
    ) -> Result<(), Error> {
        admin::add_accepted_token(&env, admin, token, decimals)
    }

    /// Remove a token from accepted list.
    ///
    /// Existing subscriptions are unaffected.
    ///
    /// # Errors
    /// - Unauthorized
    /// - NotFound
    /// - CannotRemoveDefaultToken
    pub fn remove_accepted_token(env: Env, admin: Address, token: Address) -> Result<(), Error> {
        admin::remove_accepted_token(&env, admin, token)
    }

    /// List metadata for all accepted settlement tokens.
    ///
    /// Returns a [`Vec<AcceptedToken>`] with address and decimals for each registered token,
    /// including the primary token.
    pub fn list_accepted_tokens(env: Env) -> Vec<AcceptedToken> {
        admin::list_accepted_tokens(&env)
    }

    /// Return subscriptions for a token, paginated by offset.
    ///
    /// # Arguments
    ///
    /// * `token` — Settlement token to filter by.
    /// * `start` — Starting subscription ID (inclusive).
    /// * `limit` — Maximum number of subscriptions to return. Must be between 1 and
    ///   [`queries::MAX_SUBSCRIPTION_LIST_PAGE`] inclusive.
    ///
    /// # Errors
    ///
    /// * [`Error::InvalidPageSize`] — `limit` is 0 or exceeds [`queries::MAX_SUBSCRIPTION_LIST_PAGE`].
    ///
    /// # Returns
    ///
    /// A [`Vec<Subscription>`] of up to `limit` subscriptions using the specified token,
    /// starting from `start` ID.
    pub fn get_subscriptions_by_token(
        env: Env,
        token: Address,
        start: u32,
        limit: u32,
    ) -> Result<Vec<Subscription>, Error> {
        queries::get_subscriptions_by_token(&env, token, start, limit)
    }

    // ── Reconciliation Queries ─────────────────────────────────────────────────

    /// Returns complete reconciliation data for a single settlement token.
    ///
    /// This computes the accounting equation:
    /// `contract_token_balance = total_prepaid + total_merchant_liabilities + recoverable`
    ///
    /// # Arguments
    ///
    /// * `token` — The settlement token to audit.
    ///
    /// # Returns
    ///
    /// A [`TokenLiabilities`] struct containing:
    /// - `total_prepaid`: Sum of all subscriber prepaid balances
    /// - `total_merchant_liabilities`: Sum of all merchant earnings (accruals - withdrawals - refunds)
    /// - `recoverable_amount`: Stranded funds that can be recovered
    /// - `contract_balance`: Actual token balance held by the contract
    /// - `is_balanced`: Whether the accounting equation validates
    ///
    /// # Auth
    ///
    /// Read-only; no auth required.
    ///
    /// # Complexity
    ///
    /// This scans all subscriptions and merchants. For bounded compute with
    /// pagination, use [`query_prepaid_balances_paginated`](Self::query_prepaid_balances_paginated).
    pub fn get_token_reconciliation(env: Env, token: Address) -> TokenLiabilities {
        queries::get_token_reconciliation(&env, token)
    }

    /// Returns paginated reconciliation summaries for all accepted tokens.
    ///
    /// # Arguments
    ///
    /// * `start_token_index` — Index into the accepted tokens list to start from (0 for first page).
    /// * `limit` — Maximum number of token summaries to return (max 50).
    ///
    /// # Returns
    ///
    /// A [`ReconciliationSummaryPage`] with per-token liability summaries and pagination cursor.
    ///
    /// # Auth
    ///
    /// Read-only; no auth required.
    ///
    /// # Example
    ///
    /// To get all token reconciliations:
    /// 1. Call with `start_token_index = 0`, `limit = 50`
    /// 2. If `next_token_index` is `Some(index)`, call again with that index
    /// 3. Repeat until `next_token_index` is `None`
    pub fn get_recon_summary(
        env: Env,
        start_token_index: u32,
        limit: u32,
    ) -> ReconciliationSummaryPage {
        queries::get_contract_reconciliation_summary(&env, start_token_index, limit)
    }

    /// Generates an auditable proof for off-chain reconciliation verification.
    ///
    /// Creates a snapshot with all data needed to independently validate the accounting
    /// equation without requiring full contract state access.
    ///
    /// # Arguments
    ///
    /// * `token` — The settlement token to generate the proof for.
    ///
    /// # Returns
    ///
    /// A [`ReconciliationProof`] containing:
    /// - Timestamp and ledger sequence for temporal anchoring
    /// - Contract balance, prepaid total, merchant liabilities
    /// - Computed recoverable amount
    /// - Subscription and merchant counts scanned
    /// - Validation flag (`is_valid`)
    ///
    /// # Auth
    ///
    /// Read-only; no auth required.
    ///
    /// # Security
    ///
    /// This function is read-only and cannot modify state. The proof is generated
    /// at the current ledger state and includes the ledger sequence for verification.
    pub fn generate_reconciliation_proof(env: Env, token: Address) -> ReconciliationProof {
        queries::generate_reconciliation_proof(&env, token)
    }

    /// Returns paginated prepaid balance aggregation for a token.
    ///
    /// Provides bounded compute for auditors to incrementally build the total
    /// prepaid balance without iterating unbounded subscription sets.
    ///
    /// # Arguments
    ///
    /// * `request` — A [`PrepaidQueryRequest`] with:
    ///   - `token`: Token to filter by
    ///   - `start_subscription_id`: Starting subscription ID (inclusive)
    ///   - `scan_limit`: Max subscriptions to scan (capped at 500)
    ///
    /// # Returns
    ///
    /// A [`PrepaidQueryResult`] with:
    /// - `partial_total`: Sum of prepaid balances in this scan window
    /// - `subscriptions_count`: Number of subscriptions with non-zero prepaid
    /// - `next_start_id`: Next ID to scan, or `None` if complete
    /// - `has_more`: Whether more subscriptions exist beyond this window
    ///
    /// # Auth
    ///
    /// Read-only; no auth required.
    ///
    /// # Example
    ///
    /// To compute full prepaid total off-chain:
    /// ```rust,ignore
    /// let mut total = 0i128;
    /// let mut start_id = 0u32;
    /// loop {
    ///     let result = query_prepaid_balances_paginated(env, PrepaidQueryRequest {
    ///         token: usdc_token,
    ///         start_subscription_id: start_id,
    ///         scan_limit: 500,
    ///     });
    ///     total += result.partial_total;
    ///     if !result.has_more { break; }
    ///     start_id = result.next_start_id.unwrap();
    /// }
    /// ```
    pub fn query_prepaid_balances_paginated(
        env: Env,
        request: PrepaidQueryRequest,
    ) -> PrepaidQueryResult {
        queries::query_prepaid_balances_paginated(&env, request)
    }

    /// Configure the number of detailed billing statement rows retained per subscription.
    ///
    /// When the statement count exceeds `keep_recent`, older rows are compacted into an
    /// aggregate summary. Compaction is triggered lazily or explicitly via
    /// [`compact_billing_statements`](Self::compact_billing_statements).
    ///
    /// # Arguments
    ///
    /// * `admin` — Must match the stored admin.
    /// * `keep_recent` — Number of recent detailed rows to keep per subscription.
    ///
    /// # Auth
    ///
    /// Admin only.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller is not the stored admin.
    pub fn set_billing_retention(env: Env, admin: Address, keep_recent: u32) -> Result<(), Error> {
        require_admin_auth(&env, &admin)?;
        statements::set_retention_config(&env, keep_recent);
        Ok(())
    }

    /// Read current statement retention config.
    pub fn get_billing_retention(env: Env) -> BillingRetentionConfig {
        statements::get_retention_config(&env)
    }

    /// Return compacted aggregate billing totals for a subscription.
    ///
    /// The aggregate accumulates totals for rows that have been pruned by compaction,
    /// so that historical totals remain available even after individual rows are removed.
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — Subscription to query.
    pub fn get_stmt_compacted_aggregate(
        env: Env,
        subscription_id: u32,
    ) -> BillingStatementAggregate {
        statements::get_compacted_aggregate(&env, subscription_id)
    }

    /// Compact (prune) billing statements for one subscription.
    ///
    /// Removes rows older than the retention window, accumulating their totals into
    /// the aggregate. The compacted totals remain queryable via
    /// [`get_stmt_compacted_aggregate`](Self::get_stmt_compacted_aggregate).
    ///
    /// # Arguments
    ///
    /// * `admin` — Must match the stored admin.
    /// * `subscription_id` — Target subscription.
    /// * `keep_recent_override` — When `Some(n)`, override the global retention config
    ///   for this specific compaction run (does not persist). Use `None` to apply the
    ///   globally configured value.
    ///
    /// # Auth
    ///
    /// Admin only.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller is not the stored admin.
    /// * [`Error::NotFound`] — Subscription does not exist.
    ///
    /// # Returns
    ///
    /// A [`BillingCompactionSummary`] with counts of pruned and kept rows and the
    /// total amount of pruned statements.
    ///
    /// # Events
    ///
    /// Emits [`BillingCompactedEvent`] with compaction stats and updated aggregate totals.
    pub fn compact_billing_statements(
        env: Env,
        admin: Address,
        subscription_id: u32,
        keep_recent_override: Option<u32>,
    ) -> Result<BillingCompactionSummary, Error> {
        require_admin_auth(&env, &admin)?;
        let summary = statements::compact_subscription_statements(
            &env,
            subscription_id,
            keep_recent_override,
        )?;
        let aggregate = statements::get_compacted_aggregate(&env, subscription_id);
        env.events().publish(
            (Symbol::new(&env, "billing_compacted"), subscription_id),
            BillingCompactedEvent {
                admin,
                subscription_id,
                pruned_count: summary.pruned_count,
                kept_count: summary.kept_count,
                total_pruned_amount: summary.total_pruned_amount,
                timestamp: env.ledger().timestamp(),
                aggregate_pruned_count: aggregate.pruned_count,
                aggregate_total_amount: aggregate.total_amount,
                aggregate_oldest_period_start: aggregate.oldest_period_start,
                aggregate_newest_period_end: aggregate.newest_period_end,
            },
        );
        Ok(summary)
    }

    /// Read the currently configured oracle integration settings.
    pub fn get_oracle_config(env: Env) -> OracleConfig {
        oracle::get_oracle_config(&env)
    }

    // ── Metadata ──────────────────────────────────────────────────────────────

    /// Set or update a metadata key-value pair on a subscription.
    ///
    /// Metadata is an arbitrary key-value store attached to a subscription for
    /// off-chain use cases (e.g., plan names, customer notes, external IDs). It does
    /// **not** affect financial state (balances, status, or charges).
    ///
    /// See `docs/subscription_metadata.md` for schema constraints.
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — Target subscription.
    /// * `authorizer` — Must be the subscriber or merchant.
    /// * `key` — Metadata key. Max length: [`MAX_METADATA_KEY_LENGTH`].
    /// * `value` — Metadata value. Max length: [`MAX_METADATA_VALUE_LENGTH`].
    ///
    /// # Auth
    ///
    /// `authorizer` must authorize and must be the subscription's subscriber or merchant.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Subscription does not exist.
    /// * [`Error::Unauthorized`] — `authorizer` is neither subscriber nor merchant.
    /// * [`Error::MetadataKeyTooLong`] — `key` exceeds [`MAX_METADATA_KEY_LENGTH`].
    /// * [`Error::MetadataValueTooLong`] — `value` exceeds [`MAX_METADATA_VALUE_LENGTH`].
    /// * [`Error::MetadataLimitReached`] — Subscription already has [`MAX_METADATA_KEYS`] entries.
    ///
    /// # Events
    ///
    /// Emits [`MetadataSetEvent`] with `subscription_id`, `key`, and timestamp.
    pub fn set_metadata(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
        key: String,
        value: String,
    ) -> Result<(), Error> {
        metadata::do_set_metadata(&env, subscription_id, &authorizer, key, value)
    }

    ///
    /// No-op if the key does not exist (returns `Ok`).
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — Target subscription.
    /// * `authorizer` — Must be the subscriber or merchant.
    /// * `key` — Metadata key to delete.
    ///
    /// # Auth
    ///
    /// `authorizer` must authorize and must be the subscription's subscriber or merchant.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Subscription does not exist.
    /// * [`Error::Unauthorized`] — `authorizer` is neither subscriber nor merchant.
    ///
    /// # Events
    ///
    /// Emits [`MetadataDeletedEvent`] with `subscription_id`, `key`, and timestamp.
    pub fn delete_metadata(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
        key: String,
    ) -> Result<(), Error> {
        metadata::do_delete_metadata(&env, subscription_id, &authorizer, key)
    }

    /// Get a metadata value by key.
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — Target subscription.
    /// * `key` — Metadata key to look up.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Subscription does not exist, or key is not set.
    pub fn get_metadata(env: Env, subscription_id: u32, key: String) -> Result<String, Error> {
        metadata::do_get_metadata(&env, subscription_id, key)
    }

    /// List all metadata keys for a subscription.
    ///
    /// # Arguments
    ///
    /// * `subscription_id` — Target subscription.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Subscription does not exist.
    pub fn list_metadata_keys(env: Env, subscription_id: u32) -> Result<Vec<String>, Error> {
        metadata::do_list_metadata_keys(&env, subscription_id)
    }

    // ── Protocol Fees ──────────────────────────────────────────────────────────

    /// Configure the protocol fee. Admin only.
    ///
    /// fee_bps is in basis points (0..=10_000). 0 disables fee collection.
    /// On each charge: gross == merchant_net + treasury_fee
    ///
    /// See docs/protocol_fees.md for full semantics.
    pub fn set_protocol_fee(
        env: Env,
        admin: Address,
        treasury: Address,
        fee_bps: u32,
    ) -> Result<(), Error> {
        admin::set_protocol_fee(&env, admin, treasury, fee_bps)
    }

    /// Return the current protocol fee basis points (0 = disabled).
    pub fn get_protocol_fee_bps(env: Env) -> u32 {
        admin::get_protocol_fee_bps(&env)
    }

    // ── Blocklist ──────────────────────────────────────────────────────────────

    /// Add a subscriber to the blocklist, preventing them from creating new subscriptions.
    ///
    /// Blocklisted addresses are rejected by [`create_subscription`](Self::create_subscription)
    /// and [`create_subscription_with_token`](Self::create_subscription_with_token).
    /// Existing subscriptions are not automatically cancelled.
    ///
    /// # Arguments
    ///
    /// * `authorizer` — Admin or merchant calling this function.
    /// * `subscriber` — Address to blocklist.
    /// * `reason` — Optional human-readable reason string stored in the blocklist entry.
    ///
    /// # Auth
    ///
    /// `authorizer` must be the admin or a merchant (implementation-defined).
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller lacks permission to blocklist.
    /// * [`Error::AlreadyBlocklisted`] — Address is already on the blocklist.
    ///
    /// # Events
    ///
    /// Emits [`BlocklistAddedEvent`] with `subscriber`, `reason`, and timestamp.
    pub fn add_to_blocklist(
        env: Env,
        authorizer: Address,
        subscriber: Address,
        reason: Option<String>,
    ) -> Result<(), Error> {
        blocklist::do_add_to_blocklist(&env, authorizer, subscriber, reason)
    }

    /// Remove a subscriber from the blocklist.
    ///
    /// After removal the subscriber may create new subscriptions normally.
    ///
    /// # Arguments
    ///
    /// * `admin` — Must match the stored admin.
    /// * `subscriber` — Address to remove from the blocklist.
    ///
    /// # Auth
    ///
    /// Admin only.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — Caller is not the stored admin.
    /// * [`Error::NotFound`] — Address is not on the blocklist.
    ///
    /// # Events
    ///
    /// Emits [`BlocklistRemovedEvent`] with `subscriber` and timestamp.
    pub fn remove_from_blocklist(
        env: Env,
        admin: Address,
        subscriber: Address,
    ) -> Result<(), Error> {
        blocklist::do_remove_from_blocklist(&env, admin, subscriber)
    }

    /// Return the blocklist entry for a subscriber.
    ///
    /// # Arguments
    ///
    /// * `subscriber` — Address to look up.
    ///
    /// # Errors
    ///
    /// * [`Error::NotFound`] — Address is not on the blocklist.
    pub fn get_blocklist_entry(env: Env, subscriber: Address) -> Result<BlocklistEntry, Error> {
        blocklist::get_blocklist_entry(&env, subscriber)
    }

    /// Return `true` if `subscriber` is on the blocklist.
    ///
    /// # Arguments
    ///
    /// * `subscriber` — Address to check.
    pub fn is_blocklisted(env: Env, subscriber: Address) -> bool {
        blocklist::is_blocklisted(&env, &subscriber)
    }

    /// Initialize merchant configuration with payout settings and operational flags.
    ///
    /// Creates a new merchant config record with validation. This is the recommended way
    /// for merchants to set up their configuration before accepting subscriptions.
    ///
    /// # Arguments
    ///
    /// * `merchant` — Must authorize and must be the merchant's address.
    /// * `payout_address` — Address where the merchant receives payouts.
    /// * `fee_bips` — Fee percentage in bips (0-10000). 0 means no fee.
    /// * `allowed_operations` — Bitmap of allowed operations (see OP_* constants).
    /// * `fee_address` — Optional address for platform fee routing.
    /// * `redirect_url` — URL for off-chain callbacks.
    ///
    /// # Auth
    ///
    /// `merchant` must authorize.
    ///
    /// # Errors
    ///
    /// * [`Error::InvalidPayoutAddress`] — Payout address is zero.
    /// * [`Error::InvalidFeeBips`] — Fee exceeds 100%.
    /// * [`Error::InvalidOperations`] — Invalid operation bits.
    /// * [`Error::MustAllowChargeOperation`] — CHARGE operation must be enabled.
    ///
    /// # Events
    ///
    /// Emits [`MerchantConfigInitializedEvent`].
    pub fn initialize_merchant_config(
        env: Env,
        merchant: Address,
        payout_address: Address,
        fee_bips: i32,
        allowed_operations: i32,
        fee_address: Option<Address>,
        redirect_url: String,
    ) -> Result<MerchantConfig, Error> {
        merchant::initialize_merchant_config(
            &env,
            merchant,
            payout_address,
            fee_bips,
            allowed_operations,
            fee_address,
            redirect_url,
        )
    }

    /// Set global configuration for a merchant.
    ///
    /// Stores a [`MerchantConfig`] with optional fee routing, a redirect URL, and a
    /// pause flag. The pause flag here is a configuration-layer pause (distinct from
    /// the operational [`pause_merchant`](Self::pause_merchant) / [`unpause_merchant`](Self::unpause_merchant)
    /// toggle).
    ///
    /// # Arguments
    ///
    /// * `merchant` — Must authorize the transaction.
    /// * `config` — Full MerchantConfig struct.
    ///
    /// # Auth
    ///
    /// `merchant` must authorize.
    ///
    /// # Errors
    ///
    /// * [`Error::Unauthorized`] — `merchant` auth failed.
    /// * Validation errors from config.
    pub fn set_merchant_config(
        env: Env,
        merchant: Address,
        config: MerchantConfig,
    ) -> Result<(), Error> {
        merchant::set_merchant_config(&env, merchant, config)
    }

    /// Update merchant configuration with partial fields.
    ///
    /// Allows updating specific fields without replacing the entire config.
    /// Unchanged fields retain their current values.
    ///
    /// # Arguments
    ///
    /// * `merchant` — Must authorize.
    /// * `new_payout_address` — Optional new payout address.
    /// * `new_fee_bips` — Optional new fee in bips.
    /// * `new_allowed_operations` — Optional new operations bitmap.
    /// * `new_is_active` — Optional active flag.
    /// * `new_fee_address` — Optional new fee address.
    /// * `new_redirect_url` — Optional new redirect URL.
    /// * `new_is_paused` — Optional pause flag.
    ///
    /// # Auth
    ///
    /// `merchant` must authorize.
    ///
    /// # Errors
    ///
    /// * [`Error::ConfigNotFound`] — Config not initialized.
    /// * Validation errors for provided fields.
    ///
    /// # Events
    ///
    /// Emits [`MerchantConfigUpdatedEvent`].
    pub fn update_merchant_config(
        env: Env,
        merchant: Address,
        new_payout_address: Option<Address>,
        new_fee_bips: Option<i32>,
        new_allowed_operations: Option<i32>,
        new_is_active: Option<bool>,
        new_fee_address: Option<Option<Address>>,
        new_redirect_url: Option<String>,
        new_is_paused: Option<bool>,
    ) -> Result<MerchantConfig, Error> {
        merchant::update_merchant_config(
            &env,
            merchant,
            new_payout_address,
            new_fee_bips,
            new_allowed_operations,
            new_is_active,
            new_fee_address,
            new_redirect_url,
            new_is_paused,
        )
    }

    /// Return the global configuration for a merchant.
    ///
    /// Returns `None` if the merchant has never called [`set_merchant_config`](Self::set_merchant_config).
    ///
    /// # Arguments
    ///
    /// * `merchant` — Merchant address to query.
    pub fn get_merchant_config(
        env: Env,
        merchant: Address,
    ) -> Option<crate::types::MerchantConfig> {
        merchant::get_merchant_config(&env, merchant)
    }

// Duplicate stub block removed – implementation retained elsewhere.

    pub fn get_subscriptions_by_merchant(
        env: Env,
        merchant: Address,
        start: u32,
        limit: u32,
    ) -> Result<Vec<crate::types::Subscription>, Error> {
        queries::get_subscriptions_by_merchant(&env, merchant, start, limit)
    }

    /// Returns the total number of subscriptions for a merchant.
    pub fn get_merchant_subscription_count(env: Env, merchant: Address) -> u32 {
        queries::get_merchant_subscription_count(&env, merchant)
    }

    /// Lists subscription IDs for a subscriber with pagination.
    pub fn list_subscriptions_by_subscriber(
        env: Env,
        subscriber: Address,
        start_from_id: u32,
        limit: u32,
    ) -> Result<crate::queries::SubscriptionsPage, Error> {
        queries::list_subscriptions_by_subscriber(&env, subscriber, start_from_id, limit)
    }

    /// Returns the schema version of this contract.
    pub fn version(_env: Env) -> u32 {
        1
    }

    /// Returns the current subscription count.
    ///
    /// This equals the total number of subscriptions ever created,
    /// including cancelled and expired ones.
    pub fn get_subscription_count(env: Env) -> u32 {
        let key = Symbol::new(&env, "next_id");
        env.storage()
            .instance()
            .get(&key)
            .unwrap_or(0u32)
    }

    /// Creates a new subscription and returns its ID.
    ///
    /// # Errors
    ///
    /// Returns `Error::SubscriptionLimitReached` if the ID space is exhausted.
    pub fn create_subscription(env: Env) -> Result<u32, Error> {
        Self::_next_id(&env)
    }

    /// Internal helper to allocate the next subscription ID.
    ///
    /// This function implements overflow-safe ID allocation by checking
    /// the limit before incrementing the counter.
    fn _next_id(env: &Env) -> Result<u32, Error> {
        let key = Symbol::new(env, "next_id");
        let current: u32 = env.storage().instance().get(&key).unwrap_or(0u32);

        if current == MAX_SUBSCRIPTION_ID {
            return Err(Error::SubscriptionLimitReached);
        }

        env.storage().instance().set(&key, &(current + 1));
        Ok(current)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::SubscriptionVaultClient;

    #[test]
    fn version_is_one() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        assert_eq!(client.version(), 0);
    }

    // ── charge_subscription authorization tests ───────────────────────────────
    //
    // Findings recorded per issue #374 investigation:
    // - Admin stored under DataKey::Admin (instance storage).
    // - Stored-admin pattern: load from state, require_auth() — no explicit param.
    // - Error::Unauthorized (1001) returned when admin not set or caller mismatch.
    // - Error::NotFound (1000) returned when subscription_id has no record.
    // - mock_all_auths() satisfies require_auth() for any address in tests.
    // - Storage assertions use env.as_contract() to read persistent storage directly,
    //   confirming no DataKey::Subscription entry was written on rejection.

    #[test]
    fn charge_subscription_admin_not_set_returns_unauthorized_and_no_storage_written() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        // init never called — no admin stored
        let result = client.try_charge_subscription(&0);

        // Error variant
        assert_eq!(result, Err(Ok(Error::Unauthorized)));
        // No subscription entry was written
        assert!(!subscription_exists(&env, &contract_id, 0));
    }

    #[test]
    fn charge_subscription_unknown_id_returns_not_found_and_no_storage_written() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let usdc = Address::generate(&env);
        client.init(&admin, &usdc);

        let result = client.try_charge_subscription(&99);

        // Error variant
        assert_eq!(result, Err(Ok(Error::NotFound)));
        // No subscription entry was written
        assert!(!subscription_exists(&env, &contract_id, 99));
    }

    #[test]
    fn charge_subscription_non_admin_rejected_and_no_storage_written() {
        // init with admin, then call charge_subscription with no mocked auths.
        // set_auths(&[]) clears all mocked authorizations; try_charge_subscription
        // returns Err (host auth failure) without writing any storage.
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let usdc = Address::generate(&env);

        env.mock_all_auths();
        client.init(&admin, &usdc);

        // Drop all mocked auths — subsequent require_auth() calls are unsatisfied.
        env.set_auths(&[]);

        let result = client.try_charge_subscription(&0);

        // Host auth failure — must be an error of some kind.
        assert!(result.is_err());
        // No DataKey::Subscription entry was written.
        assert!(!subscription_exists(&env, &contract_id, 0));
>>>>> main
    }
}
