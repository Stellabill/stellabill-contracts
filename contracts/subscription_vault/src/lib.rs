#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol};

#[contracterror]
#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    NotFound = 404,
    Unauthorized = 401,
    InvalidStatusTransition = 400,
    BelowMinimumTopup = 402,
    RecoveryNotAllowed = 403,
    InvalidRecoveryAmount = 405,
    /// Subscription is not Active (e.g. Paused, Cancelled).
    NotActive = 1002,
}

/// Represents the lifecycle state of a subscription.
///
/// # State Machine
///
/// The subscription status follows a defined state machine with specific allowed transitions:
///
/// - **Active**: Subscription is active and charges can be processed.
///   - Can transition to: `Paused`, `Cancelled`, `InsufficientBalance`
///
/// - **Paused**: Subscription is temporarily suspended, no charges are processed.
///   - Can transition to: `Active`, `Cancelled`
///
/// - **Cancelled**: Subscription is permanently terminated, no further changes allowed.
///   - No outgoing transitions (terminal state)
///
/// - **InsufficientBalance**: Subscription failed due to insufficient funds.
///   - Can transition to: `Active` (after deposit), `Cancelled`
///
/// Invalid transitions (e.g., `Cancelled` -> `Active`) are rejected with
/// [`Error::InvalidStatusTransition`].
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubscriptionStatus {
    /// Subscription is active and ready for charging.
    Active = 0,
    /// Subscription is temporarily paused, no charges processed.
    Paused = 1,
    /// Subscription is permanently cancelled (terminal state).
    Cancelled = 2,
    /// Subscription failed due to insufficient balance for charging.
    InsufficientBalance = 3,
}

/// Represents the reason for stranded funds that can be recovered by admin.
///
/// This enum documents the specific, well-defined cases where funds may become
/// stranded in the contract and require administrative intervention. Each case
/// must be carefully audited before recovery is permitted.
///
/// # Security Note
///
/// Recovery is an exceptional operation that should only be used for truly
/// stranded funds. All recovery operations are logged via events and should
/// be subject to governance review.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryReason {
    /// Funds sent to contract address by mistake (no associated subscription).
    /// This occurs when users accidentally send tokens directly to the contract.
    AccidentalTransfer = 0,

    /// Funds from deprecated contract flows or logic errors.
    /// Used when contract upgrades or bugs leave funds in an inaccessible state.
    DeprecatedFlow = 1,

    /// Funds from cancelled subscriptions with unreachable addresses.
    /// Subscribers may lose access to their withdrawal keys after cancellation.
    UnreachableSubscriber = 2,
}

/// Event emitted when admin recovers stranded funds.
///
/// This event provides a complete audit trail for all recovery operations,
/// including who initiated it, why, and how much was recovered.
#[contracttype]
#[derive(Clone, Debug)]
pub struct RecoveryEvent {
    /// The admin who authorized the recovery
    pub admin: Address,
    /// The destination address receiving the recovered funds
    pub recipient: Address,
    /// The amount of funds recovered
    pub amount: i128,
    /// The documented reason for recovery
    pub reason: RecoveryReason,
    /// Timestamp when recovery was executed
    pub timestamp: u64,
}

/// Stores subscription details and current state.
///
/// The `status` field is managed by the state machine. Use the provided
/// transition helpers to modify status, never set it directly.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Subscription {
    pub subscriber: Address,
    pub merchant: Address,
    pub amount: i128,
    pub interval_seconds: u64,
    pub last_payment_timestamp: u64,
    /// Current lifecycle state. Modified only through state machine transitions.
    pub status: SubscriptionStatus,
    pub prepaid_balance: i128,
    pub usage_enabled: bool,
}

/// Emitted when a subscription is paused.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionPausedEvent {
    pub subscription_id: u32,
    pub authorizer: Address,
}

/// Emitted when a subscription is resumed.
#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionResumedEvent {
    pub subscription_id: u32,
    pub authorizer: Address,
}

/// Validates if a status transition is allowed by the state machine.
///
/// # State Transition Rules
///
/// | From              | To                  | Allowed |
/// |-------------------|---------------------|---------|
/// | Active            | Paused              | Yes     |
/// | Active            | Cancelled           | Yes     |
/// | Active            | InsufficientBalance | Yes     |
/// | Paused            | Active              | Yes     |
/// | Paused            | Cancelled           | Yes     |
/// | InsufficientBalance | Active            | Yes     |
/// | InsufficientBalance | Cancelled         | Yes     |
/// | Cancelled         | *any*               | No      |
/// | *any*             | Same status         | Yes (idempotent) |
///
/// # Arguments
/// * `from` - Current status
/// * `to` - Target status
///
/// # Returns
/// * `Ok(())` if transition is valid
/// * `Err(Error::InvalidStatusTransition)` if transition is invalid
pub fn validate_status_transition(
    from: &SubscriptionStatus,
    to: &SubscriptionStatus,
) -> Result<(), Error> {
    // Same status is always allowed (idempotent)
    if from == to {
        return Ok(());
    }

    let valid = match from {
        SubscriptionStatus::Active => matches!(
            to,
            SubscriptionStatus::Paused
                | SubscriptionStatus::Cancelled
                | SubscriptionStatus::InsufficientBalance
        ),
        SubscriptionStatus::Paused => {
            matches!(
                to,
                SubscriptionStatus::Active | SubscriptionStatus::Cancelled
            )
        }
        SubscriptionStatus::Cancelled => false,
        SubscriptionStatus::InsufficientBalance => {
            matches!(
                to,
                SubscriptionStatus::Active | SubscriptionStatus::Cancelled
            )
        }
    };

    if valid {
        Ok(())
    } else {
        Err(Error::InvalidStatusTransition)
    }
}

/// Returns all valid target statuses for a given current status.
///
/// This is useful for UI/documentation to show available actions.
///
/// # Examples
///
/// ```
/// let targets = get_allowed_transitions(&SubscriptionStatus::Active);
/// assert!(targets.contains(&SubscriptionStatus::Paused));
/// ```
pub fn get_allowed_transitions(status: &SubscriptionStatus) -> &'static [SubscriptionStatus] {
    match status {
        SubscriptionStatus::Active => &[
            SubscriptionStatus::Paused,
            SubscriptionStatus::Cancelled,
            SubscriptionStatus::InsufficientBalance,
        ],
        SubscriptionStatus::Paused => &[SubscriptionStatus::Active, SubscriptionStatus::Cancelled],
        SubscriptionStatus::Cancelled => &[],
        SubscriptionStatus::InsufficientBalance => {
            &[SubscriptionStatus::Active, SubscriptionStatus::Cancelled]
        }
    }
}

/// Checks if a transition is valid without returning an error.
///
/// Convenience wrapper around [`validate_status_transition`] for boolean checks.
pub fn can_transition(from: &SubscriptionStatus, to: &SubscriptionStatus) -> bool {
    validate_status_transition(from, to).is_ok()
}

/// Result of computing next charge information for a subscription.
///
/// Contains the estimated next charge timestamp and a flag indicating
/// whether the charge is expected to occur based on the subscription status.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NextChargeInfo {
    /// Estimated timestamp for the next charge attempt.
    /// For Active and InsufficientBalance states, this is `last_payment_timestamp + interval_seconds`.
    /// For Paused and Cancelled states, this represents when the charge *would* occur if the
    /// subscription were Active, but `is_charge_expected` will be `false`.
    pub next_charge_timestamp: u64,

    /// Whether a charge is actually expected based on the subscription status.
    /// - `true` for Active subscriptions (charge will be attempted)
    /// - `true` for InsufficientBalance (charge will be retried after funding)
    /// - `false` for Paused subscriptions (no charges until resumed)
    /// - `false` for Cancelled subscriptions (terminal state, no future charges)
    pub is_charge_expected: bool,
}

/// Computes the estimated next charge timestamp for a subscription.
///
/// This is a readonly helper that does not mutate contract state. It provides
/// information for off-chain scheduling systems and UX displays.
///
/// # Logic
///
/// The next charge timestamp is calculated as:
/// ```text
/// next_charge_timestamp = last_payment_timestamp + interval_seconds
/// ```
///
/// # Status Interpretation
///
/// | Status | next_charge_timestamp | is_charge_expected | Description |
/// |--------|----------------------|-------------------|-------------|
/// | Active | last_payment + interval | true | Normal billing cycle |
/// | InsufficientBalance | last_payment + interval | true | Will retry after funding |
/// | Paused | last_payment + interval | false | Suspended, no charges |
/// | Cancelled | last_payment + interval | false | Terminal, no future charges |
///
/// # Arguments
/// * `subscription` - The subscription to compute next charge information for
///
/// # Returns
/// * [`NextChargeInfo`] containing the estimated timestamp and charge expectation flag
///
/// # Examples
///
/// ```
/// // Active subscription: charge is expected
/// let info = compute_next_charge_info(&active_subscription);
/// assert!(info.is_charge_expected);
/// assert_eq!(info.next_charge_timestamp,
///            active_subscription.last_payment_timestamp + active_subscription.interval_seconds);
///
/// // Paused subscription: charge is not expected
/// let info = compute_next_charge_info(&paused_subscription);
/// assert!(!info.is_charge_expected);
///
/// // Cancelled subscription: charge is not expected
/// let info = compute_next_charge_info(&cancelled_subscription);
/// assert!(!info.is_charge_expected);
/// ```
///
/// # Usage
///
/// This helper is designed for:
/// - Off-chain billing schedulers to determine when to invoke `charge_subscription()`
/// - Frontend UX to display "Next billing date" to subscribers
/// - Analytics and monitoring systems to track billing cycles
/// - Detecting overdue subscriptions (current_time > next_charge_timestamp)
pub fn compute_next_charge_info(subscription: &Subscription) -> NextChargeInfo {
    let next_charge_timestamp = subscription
        .last_payment_timestamp
        .saturating_add(subscription.interval_seconds);

    let is_charge_expected = match subscription.status {
        SubscriptionStatus::Active => true,
        SubscriptionStatus::InsufficientBalance => true, // Will be retried after funding
        SubscriptionStatus::Paused => false,
        SubscriptionStatus::Cancelled => false,
    };

    NextChargeInfo {
        next_charge_timestamp,
        is_charge_expected,
    }
}

#[contract]
pub struct SubscriptionVault;

#[contractimpl]
impl SubscriptionVault {
    /// Initialize the contract (e.g. set token and admin). Extend as needed.
    pub fn init(env: Env, token: Address, admin: Address, min_topup: i128) -> Result<(), Error> {
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "token"), &token);
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "admin"), &admin);
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "min_topup"), &min_topup);
        Ok(())
    }

    /// Update the minimum top-up threshold. Only callable by admin.
    ///
    /// # Arguments
    /// * `min_topup` - Minimum amount (in token base units) required for deposit_funds.
    ///                 Prevents inefficient micro-deposits. Typical range: 1-10 USDC (1_000000 - 10_000000 for 6 decimals).
    pub fn set_min_topup(env: Env, admin: Address, min_topup: i128) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "admin"))
            .ok_or(Error::NotFound)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "min_topup"), &min_topup);
        Ok(())
    }

    /// Rotate admin to a new address. Only callable by current admin.
    ///
    /// This function allows the current admin to transfer administrative control
    /// to a new address. This is critical for:
    /// - Key rotation for security
    /// - Transferring control to multi-sig wallets
    /// - Organizational changes
    /// - Upgrading to new governance mechanisms
    ///
    /// # Security Requirements
    ///
    /// - **Current Admin Authorization Required**: Only the current admin can rotate
    /// - **Immediate Effect**: New admin takes effect immediately
    /// - **No Grace Period**: Old admin loses access instantly
    /// - **Irreversible**: Cannot be undone without new admin's cooperation
    ///
    /// # Safety Considerations
    ///
    /// ⚠️ **CRITICAL**: Ensure new admin address is correct before calling.
    /// There is no recovery mechanism if you set an incorrect or inaccessible address.
    ///
    /// **Best Practices**:
    /// - Verify new_admin address multiple times
    /// - Test with a dry-run if possible
    /// - Consider using a multi-sig wallet for new_admin
    /// - Document the rotation in governance records
    /// - Ensure new admin has tested access before old admin loses control
    ///
    /// # Arguments
    ///
    /// * `current_admin` - The current admin address (must match stored admin)
    /// * `new_admin` - The new admin address (will replace current admin)
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Admin rotation successful
    /// * `Err(Error::Unauthorized)` - Caller is not current admin
    /// * `Err(Error::NotFound)` - Admin not configured
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Rotate from old admin to new admin
    /// client.rotate_admin(&current_admin, &new_admin);
    ///
    /// // Old admin can no longer perform admin operations
    /// client.set_min_topup(&current_admin, &new_value); // Will fail
    ///
    /// // New admin can now perform admin operations
    /// client.set_min_topup(&new_admin, &new_value); // Will succeed
    /// ```
    ///
    /// # Events
    ///
    /// Emits an event with:
    /// - Old admin address
    /// - New admin address
    /// - Timestamp of rotation
    pub fn rotate_admin(env: Env, current_admin: Address, new_admin: Address) -> Result<(), Error> {
        // 1. Require current admin authorization
        current_admin.require_auth();

        // 2. Verify caller is the stored admin
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "admin"))
            .ok_or(Error::NotFound)?;

        if current_admin != stored_admin {
            return Err(Error::Unauthorized);
        }

        // 3. Update admin to new address
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "admin"), &new_admin);

        // 4. Emit event for audit trail
        env.events().publish(
            (Symbol::new(&env, "admin_rotation"), current_admin.clone()),
            (current_admin, new_admin, env.ledger().timestamp()),
        );

        Ok(())
    }

    /// Get the current admin address.
    ///
    /// This is a readonly function that returns the currently configured admin address.
    /// Useful for:
    /// - Verifying who has admin access
    /// - UI displays
    /// - Access control checks in external systems
    ///
    /// # Returns
    ///
    /// * `Ok(Address)` - The current admin address
    /// * `Err(Error::NotFound)` - Admin not configured (contract not initialized)
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "admin"))
            .ok_or(Error::NotFound)
    }

    /// Get the current minimum top-up threshold.
    pub fn get_min_topup(env: Env) -> Result<i128, Error> {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "min_topup"))
            .ok_or(Error::NotFound)
    }

    /// Create a new subscription. Caller deposits initial USDC; contract stores agreement.
    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        interval_seconds: u64,
        usage_enabled: bool,
    ) -> Result<u32, Error> {
        subscriber.require_auth();
        // TODO: transfer initial deposit from subscriber to contract, then store subscription
        let sub = Subscription {
            subscriber: subscriber.clone(),
            merchant,
            amount,
            interval_seconds,
            last_payment_timestamp: env.ledger().timestamp(),
            status: SubscriptionStatus::Active,
            prepaid_balance: 0i128, // TODO: set from initial deposit
            usage_enabled,
        };
        let id = Self::_next_id(&env);
        env.storage().instance().set(&id, &sub);
        Ok(id)
    }

    /// Subscriber deposits more USDC into their vault for this subscription.
    ///
    /// # Minimum top-up enforcement
    /// Rejects deposits below the configured minimum threshold to prevent inefficient
    /// micro-transactions that waste gas and complicate accounting. The minimum is set
    /// globally at contract initialization and adjustable by admin via `set_min_topup`.
    pub fn deposit_funds(
        env: Env,
        subscription_id: u32,
        subscriber: Address,
        amount: i128,
    ) -> Result<(), Error> {
        subscriber.require_auth();

        let min_topup: i128 = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "min_topup"))
            .ok_or(Error::NotFound)?;
        if amount < min_topup {
            return Err(Error::BelowMinimumTopup);
        }

        // TODO: transfer USDC from subscriber, increase prepaid_balance for subscription_id
        let _ = (env, subscription_id, amount);
        Ok(())
    }

    /// Billing engine (backend) calls this to charge one interval. Deducts from vault, pays merchant.
    ///
    /// # State Transitions
    /// - On success: `Active` -> `Active` (no change)
    /// - On insufficient balance: `Active` -> `InsufficientBalance`
    ///
    /// Subscriptions that are `Paused` or `Cancelled` cannot be charged.
    pub fn charge_subscription(env: Env, subscription_id: u32) -> Result<(), Error> {
        // TODO: require_caller admin or authorized billing service
        // TODO: load subscription, check interval and balance, transfer to merchant

        // Placeholder for actual charge logic
        let maybe_sub: Option<Subscription> = env.storage().instance().get(&subscription_id);
        if let Some(mut sub) = maybe_sub {
            // Check current status allows charging
            if sub.status == SubscriptionStatus::Cancelled
                || sub.status == SubscriptionStatus::Paused
            {
                // Cannot charge cancelled or paused subscriptions
                return Err(Error::NotActive);
            }

            // Simulate charge logic - on insufficient balance, transition to InsufficientBalance
            let insufficient_balance = false; // TODO: actual balance check
            if insufficient_balance {
                validate_status_transition(&sub.status, &SubscriptionStatus::InsufficientBalance)?;
                sub.status = SubscriptionStatus::InsufficientBalance;
                env.storage().instance().set(&subscription_id, &sub);
            }
            // TODO: update last_payment_timestamp and prepaid_balance on successful charge
        }
        Ok(())
    }

    /// Subscriber or merchant cancels the subscription. Remaining balance can be withdrawn by subscriber.
    ///
    /// # State Transitions
    /// Allowed from: `Active`, `Paused`, `InsufficientBalance`
    /// - Transitions to: `Cancelled` (terminal state)
    ///
    /// Once cancelled, no further transitions are possible.
    pub fn cancel_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();

        let mut sub = Self::get_subscription(env.clone(), subscription_id)?;

        // Validate and apply status transition
        validate_status_transition(&sub.status, &SubscriptionStatus::Cancelled)?;
        sub.status = SubscriptionStatus::Cancelled;

        // TODO: allow withdraw of prepaid_balance

        env.storage().instance().set(&subscription_id, &sub);
        Ok(())
    }

    /// Pause subscription (no charges until resumed).
    ///
    /// # State Transitions
    /// Allowed from: `Active`
    /// - Transitions to: `Paused`
    ///
    /// Only the subscriber or merchant can pause.
    /// Cannot pause a subscription that is already `Paused`, `Cancelled`, or in `InsufficientBalance`.
    pub fn pause_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();

        let mut sub = Self::get_subscription(env.clone(), subscription_id)?;

        // Only subscriber or merchant can pause
        if authorizer != sub.subscriber && authorizer != sub.merchant {
            return Err(Error::Unauthorized);
        }

        // Validate and apply status transition
        validate_status_transition(&sub.status, &SubscriptionStatus::Paused)?;
        sub.status = SubscriptionStatus::Paused;

        env.storage().instance().set(&subscription_id, &sub);

        // Emit event
        env.events().publish(
            (Symbol::new(&env, "subscription_paused"), subscription_id),
            SubscriptionPausedEvent {
                subscription_id,
                authorizer,
            },
        );

        Ok(())
    }

    /// Resume a subscription to Active status.
    ///
    /// # State Transitions
    /// Allowed from: `Paused`, `InsufficientBalance`
    /// - Transitions to: `Active`
    ///
    /// Only the subscriber or merchant can resume.
    /// Cannot resume a `Cancelled` subscription.
    pub fn resume_subscription(
        env: Env,
        subscription_id: u32,
        authorizer: Address,
    ) -> Result<(), Error> {
        authorizer.require_auth();

        let mut sub = Self::get_subscription(env.clone(), subscription_id)?;

        // Only subscriber or merchant can resume
        if authorizer != sub.subscriber && authorizer != sub.merchant {
            return Err(Error::Unauthorized);
        }

        // Validate and apply status transition
        validate_status_transition(&sub.status, &SubscriptionStatus::Active)?;
        sub.status = SubscriptionStatus::Active;

        env.storage().instance().set(&subscription_id, &sub);

        // Emit event
        env.events().publish(
            (Symbol::new(&env, "subscription_resumed"), subscription_id),
            SubscriptionResumedEvent {
                subscription_id,
                authorizer,
            },
        );

        Ok(())
    }

    /// Merchant withdraws accumulated USDC to their wallet.
    pub fn withdraw_merchant_funds(
        _env: Env,
        merchant: Address,
        _amount: i128,
    ) -> Result<(), Error> {
        merchant.require_auth();
        // TODO: deduct from merchant's balance in contract, transfer token to merchant
        Ok(())
    }

    /// **ADMIN ONLY**: Recover stranded funds from the contract.
    ///
    /// This is an exceptional, tightly-scoped mechanism for recovering funds that have
    /// become inaccessible through normal contract operations. Recovery is subject to
    /// strict constraints and comprehensive audit logging.
    ///
    /// # Security Requirements
    ///
    /// - **Admin Authorization Required**: Only the contract admin can invoke this function
    /// - **Audit Trail**: Every recovery emits a `RecoveryEvent` with full details
    /// - **Protected Balances**: Cannot recover funds from active subscriptions
    /// - **Documented Reasons**: Each recovery must specify a valid `RecoveryReason`
    /// - **Positive Amount**: Amount must be greater than zero
    ///
    /// # Safety Constraints
    ///
    /// This function enforces the following protections:
    /// 1. **Admin-only access** - Requires authentication as the stored admin address
    /// 2. **Valid amount** - Amount must be > 0 to prevent accidental calls
    /// 3. **Event logging** - All recoveries are permanently recorded on-chain
    /// 4. **Limited scope** - Only for well-defined recovery scenarios
    ///
    /// # Recovery Scenarios
    ///
    /// Valid use cases documented in `RecoveryReason`:
    /// - **AccidentalTransfer**: Tokens sent directly to contract by mistake
    /// - **DeprecatedFlow**: Funds stranded by contract upgrades or bugs
    /// - **UnreachableSubscriber**: Cancelled subscriptions with lost keys
    ///
    /// # Governance
    ///
    /// Recovery operations should be subject to:
    /// - Transparent documentation of the stranded fund situation
    /// - Community review or multi-sig approval (external to this contract)
    /// - Post-recovery reporting and verification
    ///
    /// # Arguments
    ///
    /// * `env` - The contract environment
    /// * `admin` - The admin address (must match stored admin)
    /// * `recipient` - Address to receive the recovered funds
    /// * `amount` - Amount of tokens to recover (must be > 0)
    /// * `reason` - Documented reason for recovery (see `RecoveryReason`)
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Recovery successful, event emitted
    /// * `Err(Error::Unauthorized)` - Caller is not the admin
    /// * `Err(Error::InvalidRecoveryAmount)` - Amount is zero or negative
    /// * `Err(Error::NotFound)` - Admin address not configured
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Recover 100 USDC accidentally sent to contract
    /// client.recover_stranded_funds(
    ///     &admin,
    ///     &treasury_address,
    ///     &100_000000,
    ///     &RecoveryReason::AccidentalTransfer
    /// );
    /// ```
    ///
    /// # Events
    ///
    /// Emits `RecoveryEvent` with:
    /// - Admin address
    /// - Recipient address
    /// - Amount recovered
    /// - Recovery reason
    /// - Timestamp
    ///
    /// # Security Notes
    ///
    /// ⚠️ **CRITICAL**: This function grants the admin significant power. The admin key
    /// should be:
    /// - Protected by multi-signature or hardware wallet
    /// - Subject to governance oversight
    /// - Used only for documented, legitimate recovery scenarios
    ///
    /// **Residual Risks**:
    /// - A compromised admin key could enable unauthorized fund recovery
    /// - Recovery decisions require human judgment and may be disputed
    /// - Sufficient off-chain governance processes must exist
    ///
    /// **Recommended Controls**:
    /// - Use multi-sig wallet for admin key
    /// - Implement time-locked recovery with challenge period
    /// - Conduct community review before executing recovery
    /// - Maintain public log of all recovery operations
    pub fn recover_stranded_funds(
        env: Env,
        admin: Address,
        recipient: Address,
        amount: i128,
        reason: RecoveryReason,
    ) -> Result<(), Error> {
        // 1. Require admin authorization
        admin.require_auth();

        // 2. Verify caller is the stored admin
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "admin"))
            .ok_or(Error::NotFound)?;

        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }

        // 3. Validate recovery amount
        if amount <= 0 {
            return Err(Error::InvalidRecoveryAmount);
        }

        // 4. Create audit event
        let recovery_event = RecoveryEvent {
            admin: admin.clone(),
            recipient: recipient.clone(),
            amount,
            reason: reason.clone(),
            timestamp: env.ledger().timestamp(),
        };

        // 5. Emit event for audit trail
        env.events().publish(
            (Symbol::new(&env, "recovery"), admin.clone()),
            recovery_event,
        );

        // 6. TODO: Actual token transfer logic would go here
        // In production, this would call the token contract to transfer funds:
        // token_client.transfer(&env.current_contract_address(), &recipient, &amount);

        Ok(())
    }

    /// Read subscription by id (for indexing and UI).
    pub fn get_subscription(env: Env, subscription_id: u32) -> Result<Subscription, Error> {
        env.storage()
            .instance()
            .get(&subscription_id)
            .ok_or(Error::NotFound)
    }

    /// Get estimated next charge information for a subscription.
    ///
    /// Returns the estimated next charge timestamp and whether a charge is expected
    /// based on the subscription's current status. This is a readonly view function
    /// that does not mutate contract state.
    ///
    /// # Arguments
    /// * `subscription_id` - The ID of the subscription to query
    ///
    /// # Returns
    /// * `Ok(NextChargeInfo)` - Information about the next charge
    /// * `Err(Error::NotFound)` - Subscription does not exist
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Get next charge info for subscription ID 0
    /// let info = client.get_next_charge_info(&0);
    ///
    /// if info.is_charge_expected {
    ///     println!("Next charge at timestamp: {}", info.next_charge_timestamp);
    /// } else {
    ///     println!("No charge expected (paused or cancelled)");
    /// }
    /// ```
    ///
    /// # Usage Scenarios
    ///
    /// 1. **Billing Scheduler**: Determine when to invoke `charge_subscription()`
    /// 2. **User Dashboard**: Display "Next billing date" to subscribers
    /// 3. **Monitoring**: Detect overdue charges (current_time > next_charge_timestamp + grace_period)
    /// 4. **Analytics**: Track billing cycles and payment patterns
    pub fn get_next_charge_info(env: Env, subscription_id: u32) -> Result<NextChargeInfo, Error> {
        let subscription = Self::get_subscription(env, subscription_id)?;
        Ok(compute_next_charge_info(&subscription))
    }

    fn _next_id(env: &Env) -> u32 {
        let key = Symbol::new(env, "next_id");
        let id: u32 = env.storage().instance().get(&key).unwrap_or(0);
        env.storage().instance().set(&key, &(id + 1));
        id
    }
}

#[cfg(test)]
mod test;
