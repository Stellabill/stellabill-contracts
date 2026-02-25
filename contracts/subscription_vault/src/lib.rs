#![no_std]

// ── Modules ──────────────────────────────────────────────────────────────────
mod admin;
mod charge_core;
mod merchant;
mod queries;
mod state_machine;
mod subscription;
pub mod types;

mod safe_math;

// ── Re-exports (used by tests and external consumers) ────────────────────────
pub use state_machine::{can_transition, get_allowed_transitions, validate_status_transition};
pub use types::*;

pub use queries::compute_next_charge_info;
use soroban_sdk::{contract, contractimpl, Address, Env, Vec};

// ── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    // ── Admin / Config ───────────────────────────────────────────────────

    /// Initialize the contract: set token address, admin, and minimum top-up.
    pub fn init(
        env: Env,
        token: Address,
        token_decimals: u32,
        admin: Address,
        min_topup: i128,
        grace_period: u64,
    ) -> Result<(), Error> {
        admin::do_init(&env, token, token_decimals, admin, min_topup, grace_period)
    }

    /// Update the minimum top-up threshold. Only callable by admin.
    pub fn set_min_topup(env: Env, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin::do_set_min_topup(&env, admin, min_topup)
    }

    /// Get the current minimum top-up threshold.
    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        admin::get_min_topup(&env)
    }

    /// Get the current admin address.
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        admin::do_get_admin(&env)
    }

    /// Rotate admin to a new address. Only callable by current admin.
    ///
    /// # Security
    ///
    /// - Immediate effect — old admin loses access instantly.
    /// - Irreversible without the new admin's cooperation.
    /// - Emits an `admin_rotation` event for audit trail.
    pub fn rotate_admin(env: Env, current_admin: Address, new_admin: Address) -> Result<(), Error> {
        admin::do_rotate_admin(&env, current_admin, new_admin)
    }

    /// **ADMIN ONLY**: Recover stranded funds from the contract.
    ///
    /// Tightly-scoped mechanism for recovering funds that have become
    /// inaccessible through normal operations. Each recovery emits a
    /// `RecoveryEvent` with full audit details.
    pub fn recover_stranded_funds(
        env: Env,
        admin: Address,
        recipient: Address,
        amount: i128,
        reason: RecoveryReason,
    ) -> Result<(), Error> {
        admin::do_recover_stranded_funds(&env, admin, recipient, amount, reason)
    }

    /// Charge a batch of subscriptions in one transaction. Admin only.
    ///
    /// Returns a per-subscription result vector so callers can identify
    /// which charges succeeded and which failed (with error codes).
    pub fn batch_charge(
        env: Env,
        subscription_ids: Vec<u32>,
    ) -> Result<Vec<BatchChargeResult>, Error> {
        admin::do_batch_charge(&env, &subscription_ids)
    }

    pub fn set_grace_period(env: Env, admin: Address, grace_period: u64) -> Result<(), Error> {
        admin::do_set_grace_period(&env, admin, grace_period)
    }

    pub fn get_grace_period(env: Env) -> Result<u64, Error> {
        admin::get_grace_period(&env)
    }

    // ── Subscription lifecycle ───────────────────────────────────────────

    /// Create a new subscription. Caller deposits initial USDC; contract stores agreement.
    ///
    /// # Arguments
    /// * `expiration` - Optional Unix timestamp (seconds). If `Some(ts)`, charges are blocked
    ///                  at or after `ts`. Pass `None` for an open-ended subscription.
    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
        _expiration: Option<u64>,
    ) -> Result<u32, Error> {
        subscription::do_create_subscription(
            &env,
            subscriber,
            merchant,
            amount,
            interval_seconds,
            usage_enabled,
        )
    }

    /// Subscriber deposits more USDC into their prepaid vault.
    ///
    /// Rejects deposits below the configured minimum threshold.
    pub fn deposit_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
    ) -> Result<(), Error> {
        subscription::do_deposit_funds(&env, subscription_id, subscriber, amount)
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

    /// Subscriber withdraws their remaining prepaid_balance after cancellation.
    pub fn withdraw_subscriber_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
    ) -> Result<(), Error> {
        subscription::do_withdraw_subscriber_funds(&env, subscription_id, subscriber)
    }

    /// Pause subscription (no charges until resumed). Allowed from Active.
    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_pause_subscription(&env, subscription_id, authorizer)
    }

    /// Resume a subscription to Active. Allowed from Paused or InsufficientBalance.
    pub fn resume_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        subscription::do_resume_subscription(&env, subscription_id, authorizer)
    }

    // ── Charging ─────────────────────────────────────────────────────────

    /// Charge a subscription for one billing interval.
    ///
    /// This function attempts to charge the subscriber's prepaid balance for the
    /// recurring subscription fee. It enforces:
    /// - The subscription must be in `Active` status
    /// - The billing interval must have elapsed since the last charge
    /// - The prepaid balance must be sufficient to cover the charge amount
    ///
    /// # Preconditions
    ///
    /// - The subscription must exist and be in `Active` status
    /// - `last_payment_timestamp + interval_seconds` must be <= current ledger timestamp
    /// - `prepaid_balance >= amount` (the subscription's recurring charge amount)
    ///
    /// # Behavior
    ///
    /// On success:
    /// - `prepaid_balance` is reduced by `amount`
    /// - `last_payment_timestamp` is updated to current timestamp
    /// - A `SubscriptionChargedEvent` is emitted
    /// - The subscription remains `Active`
    ///
    /// On failure (insufficient balance):
    /// - No changes are made to the subscription's prepaid balance
    /// - Status transitions to `InsufficientBalance`
    /// - An `Error::InsufficientBalance` error is returned
    ///
    /// # Error Cases
    ///
    /// | Error | Condition |
    /// |-------|-----------|
    /// | `NotFound` | Subscription ID does not exist |
    /// | `NotActive` | Subscription is not in `Active` status (Paused, Cancelled, or InsufficientBalance) |
    /// | `IntervalNotElapsed` | Not enough time has passed since last charge |
    /// | `Replay` | This billing period has already been charged |
    /// | `InsufficientBalance` | `prepaid_balance < amount` |
    ///
    /// # Non-Destructive Failure Guarantee
    ///
    /// When a charge fails due to insufficient balance:
    /// - The subscriber's prepaid balance is NOT deducted
    /// - No tokens are transferred to the merchant
    /// - The subscription metadata remains unchanged (except status)
    /// - The failure is atomic - no partial state updates occur
    ///
    /// # Recovery
    ///
    /// If the charge fails due to insufficient balance:
    /// 1. Subscriber calls `deposit_funds` to add more funds
    /// 2. Subscriber calls `resume_subscription` to transition back to `Active`
    /// 3. The next charge attempt will succeed (if balance is sufficient)
    ///
    /// # Gas Efficiency
    ///
    /// The function uses early validation to avoid unnecessary state modifications.
    /// Balance check is performed before any state changes.
    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        charge_core::charge_one(&env, subscription_id, env.ledger().timestamp(), None)
    }

    /// Charge a metered usage amount against the subscription's prepaid balance.
    ///
    /// Designed for integration with an **off-chain usage metering service**:
    /// the service measures consumption, then calls this entrypoint with the
    /// computed `usage_amount` to debit the subscriber's vault.
    ///
    /// # Requirements
    ///
    /// * The subscription must be `Active`.
    /// * `usage_enabled` must be `true` on the subscription.
    /// * `usage_amount` must be positive (`> 0`).
    /// * `prepaid_balance` must be >= `usage_amount`.
    ///
    /// # Behaviour
    ///
    /// On success, `prepaid_balance` is reduced by `usage_amount`.  If the
    /// debit drains the balance to zero the subscription transitions to
    /// `InsufficientBalance` status, signalling that no further charges
    /// (interval or usage) can proceed until the subscriber tops up.
    ///
    /// # Errors
    ///
    /// | Variant | Reason |
    /// |---------|--------|
    /// | `NotFound` | Subscription ID does not exist. |
    /// | `NotActive` | Subscription is not `Active`. |
    /// | `UsageNotEnabled` | `usage_enabled` is `false`. |
    /// | `InvalidAmount` | `usage_amount` is zero or negative. |
    /// | `InsufficientPrepaidBalance` | Prepaid balance cannot cover the debit. |
    pub fn charge_usage(env: Env, subscription_id: u32, usage_amount: i128) -> Result<(), Error> {
        charge_core::charge_usage_one(&env, subscription_id, usage_amount)
    }

    // ── Merchant ─────────────────────────────────────────────────────────

    /// Merchant withdraws accumulated USDC to their wallet.
    pub fn withdraw_merchant_funds(env: Env, merchant: Address, amount: i128) -> Result<(), Error> {
        merchant::withdraw_merchant_funds(&env, merchant, amount)
    }

    // ── Queries ──────────────────────────────────────────────────────────

    /// Read subscription by id.
    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        queries::get_subscription(&env, subscription_id)
    }

    /// Estimate how much a subscriber needs to deposit to cover N future intervals.
    pub fn estimate_topup_for_intervals(
        env: Env,
        subscription_id: u32,
        num_intervals: u32,
    ) -> Result<i128, Error> {
        queries::estimate_topup_for_intervals(&env, subscription_id, num_intervals)
    }

    /// Get estimated next charge info (timestamp + whether charge is expected).
    pub fn get_next_charge_info(env: Env, subscription_id: u32) -> Result<NextChargeInfo, Error> {
        let sub = queries::get_subscription(&env, subscription_id)?;
        Ok(compute_next_charge_info(&sub))
    }

    /// Return subscriptions for a merchant, paginated.
    pub fn get_subscriptions_by_merchant(
        env: Env,
        merchant: Address,
        start: u32,
        limit: u32,
    ) -> Vec<Subscription> {
        queries::get_subscriptions_by_merchant(&env, merchant, start, limit)
    }

    /// Return the total number of subscriptions for a merchant.
    pub fn get_merchant_subscription_count(env: Env, merchant: Address) -> u32 {
        queries::get_merchant_subscription_count(&env, merchant)
    }

    /// Merchant-initiated one-off charge.
    pub fn charge_one_off(
        env: Env,
        subscription_id: u32,
        merchant: Address,
        amount: i128,
    ) -> Result<(), Error> {
        subscription::do_charge_one_off(&env, subscription_id, merchant, amount)
    }

    /// List all subscription IDs for a given subscriber with pagination support.
    ///
    /// This read-only function retrieves subscription IDs owned by a subscriber in a paginated manner.
    /// Subscriptions are returned in order by ID (ascending) for predictable iteration.
    ///
    /// # Arguments
    /// * `subscriber` - The address of the subscriber to query
    /// * `start_from_id` - Inclusive lower bound for pagination (use 0 for the first page)
    /// * `limit` - Maximum number of subscription IDs to return (recommended: 10-100)
    ///
    /// # Returns
    /// A `SubscriptionsPage` containing subscription IDs and pagination metadata
    ///
    /// # Performance Notes
    /// - Time complexity: O(n) where n = total subscriptions in contract
    /// - Space complexity: O(limit)
    /// - Suitable for off-chain indexers and UI pagination
    ///
    /// # Usage Example
    ///
    /// ```ignore
    /// // Get first page
    /// let page = client.list_subscriptions_by_subscriber(&subscriber, &0, &10)?;
    /// println!("Found {} subscriptions", page.subscription_ids.len());
    ///
    /// // Get next page if available
    /// if page.has_next {
    ///     let next_start = page.subscription_ids.last().unwrap() + 1;
    ///     let page2 = client.list_subscriptions_by_subscriber(&subscriber, &next_start, &10)?;
    /// }
    /// ```
    pub fn list_subscriptions_by_subscriber(
        env: Env,
        subscriber: Address,
        start_from_id: u32,
        limit: u32,
    ) -> Result<crate::queries::SubscriptionsPage, Error> {
        crate::queries::list_subscriptions_by_subscriber(&env, subscriber, start_from_id, limit)
    }
}

#[cfg(test)]
mod test;
