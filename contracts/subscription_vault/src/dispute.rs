//! Dispute / chargeback workflow for contested subscription charges.
//!
//! Provides a two-step dispute workflow (open_dispute, respond_dispute,
//! resolve_dispute) that mirrors payment-card chargeback semantics.
//!
//! # Flow
//!
//! 1. **Subscriber opens a dispute** – the disputed amount is moved from the
//!    merchant's balance into a [`DataKey::DisputeEscrow(u64)`] bucket.
//! 2. **Admin responds** (optional) – during the [`DISPUTE_WINDOW_SECS`] window the
//!    admin may respond with evidence, moving the dispute to `Responded` status.
//! 3. **Admin resolves** – after a response (or after the window elapses) the admin
//!    routes the escrowed funds to either the subscriber or the merchant.
//!
//! # Security
//!
//! - `open_dispute`: subscriber must authorise and match the subscription's
//!   `subscriber` field.
//! - `respond_dispute` and `resolve_dispute`: admin-only.
//! - Double-open detection via [`DataKey::SubscriptionDispute(u32)`].
//! - CEI ordering: state is written before external token transfers.
//! - Escrow invariant: `sum(escrow balances) + merchant_balance == original_merchant_balance`
//!   at all times after a dispute is opened and before it is resolved.

use crate::admin;
use crate::merchant;
use crate::queries;
use crate::types::{
    DataKey, Dispute, DisputeOpenedEvent, DisputeRespondedEvent, DisputeResolvedEvent,
    DisputeStatus, Error, DISPUTE_WINDOW_SECS,
};
use soroban_sdk::{token, Address, BytesN, Env, Symbol};

/// Open a dispute against a charge for the given subscription.
///
/// Moves `amount` from the merchant's balance to escrow and records the
/// dispute in `Open` status.
pub fn do_open_dispute(
    env: &Env,
    subscriber: Address,
    subscription_id: u32,
    amount: i128,
    evidence_hash: Option<BytesN<32>>,
) -> Result<u64, Error> {
    subscriber.require_auth();

    // ── Checks ────────────────────────────────────────────────────────────
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    let sub = queries::get_subscription(env, subscription_id)?;
    if sub.subscriber != subscriber {
        return Err(Error::Unauthorized);
    }

    // Prevent double-open: only one active dispute per subscription at a time.
    if env
        .storage()
        .instance()
        .has(&DataKey::SubscriptionDispute(subscription_id))
    {
        return Err(Error::DisputeAlreadyOpen);
    }

    // Verify the merchant has sufficient balance to escrow.
    let token_addr = &sub.token;
    let merchant_balance = merchant::get_merchant_balance_by_token(env, &sub.merchant, token_addr);
    if merchant_balance < amount {
        return Err(Error::InsufficientBalance);
    }

    // ── Effects (state mutations before external interactions) ────────────
    let dispute_id: u64 = next_dispute_id(env);

    // 1. Debit merchant balance
    let new_merchant_balance = merchant_balance
        .checked_sub(amount)
        .ok_or(Error::Underflow)?;
    merchant::set_merchant_balance(env, &sub.merchant, token_addr, &new_merchant_balance);

    // 2. Credit dispute escrow
    env.storage()
        .instance()
        .set(&DataKey::DisputeEscrow(dispute_id), &amount);

    // 3. Record dispute
    let now = env.ledger().timestamp();
    let dispute = Dispute {
        id: dispute_id,
        subscription_id,
        subscriber: subscriber.clone(),
        merchant: sub.merchant.clone(),
        amount,
        opened_at: now,
        status: DisputeStatus::Open,
        evidence_hash: evidence_hash.clone(),
        responded_at: None,
        admin_evidence_hash: None,
    };
    env.storage()
        .persistent()
        .set(&DataKey::Dispute(dispute_id), &dispute);

    // 4. Mark subscription dispute index
    env.storage()
        .instance()
        .set(&DataKey::SubscriptionDispute(subscription_id), &dispute_id);

    // 5. Emit event
    env.events().publish(
        (Symbol::new(env, "dispute_opened"), dispute_id),
        DisputeOpenedEvent {
            dispute_id,
            subscription_id,
            subscriber,
            merchant: sub.merchant,
            amount,
            evidence_hash,
            timestamp: now,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(dispute_id)
}

/// Respond to a dispute on behalf of the merchant side. Admin only.
///
/// Transitions the dispute from `Open` to `Responded`. This signals that the
/// admin has reviewed the dispute and may provide evidence. Once responded,
/// the admin may resolve the dispute in either direction.
pub fn do_respond_dispute(
    env: &Env,
    admin: Address,
    dispute_id: u64,
    evidence_hash: Option<BytesN<32>>,
) -> Result<(), Error> {
    admin::require_admin_auth(env, &admin)?;

    // ── Checks ────────────────────────────────────────────────────────────
    let mut dispute = read_dispute(env, dispute_id)?;

    if dispute.status != DisputeStatus::Open {
        return Err(Error::DisputeAlreadyResponded);
    }

    // ── Effects ───────────────────────────────────────────────────────────
    dispute.status = DisputeStatus::Responded;
    dispute.responded_at = Some(env.ledger().timestamp());
    dispute.admin_evidence_hash = evidence_hash.clone();

    env.storage()
        .persistent()
        .set(&DataKey::Dispute(dispute_id), &dispute);

    env.events().publish(
        (Symbol::new(env, "dispute_responded"), dispute_id),
        DisputeRespondedEvent {
            dispute_id,
            subscription_id: dispute.subscription_id,
            admin_evidence_hash: evidence_hash,
            timestamp: env.ledger().timestamp(),
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Resolve a dispute, routing escrowed funds to either the subscriber or the
/// merchant. Admin only.
///
/// # Resolution rules
///
/// | Dispute status | Window elapsed | Resolution allowed |
/// |:---|:---:|:---|
/// | `Open` | No  | Rejected – admin must respond first (`DisputeNotResponded`) |
/// | `Open` | Yes | Resolved to **subscriber** (auto-resolve) |
/// | `Responded` | Either | Admin may resolve to **subscriber** or **merchant** |
/// | Resolved (any) | — | Rejected (`DisputeAlreadyResolved`) |
pub fn do_resolve_dispute(
    env: &Env,
    admin: Address,
    dispute_id: u64,
    resolve_to_subscriber: bool,
) -> Result<(), Error> {
    admin::require_admin_auth(env, &admin)?;

    // ── Checks ────────────────────────────────────────────────────────────
    let mut dispute = read_dispute(env, dispute_id)?;

    if dispute.status == DisputeStatus::ResolvedToMerchant
        || dispute.status == DisputeStatus::ResolvedToSubscriber
    {
        return Err(Error::DisputeAlreadyResolved);
    }

    let now = env.ledger().timestamp();
    let window_elapsed = now.saturating_sub(dispute.opened_at) >= DISPUTE_WINDOW_SECS;

    if dispute.status == DisputeStatus::Open && !window_elapsed {
        // Admin must respond before the dispute can be resolved (merchant
        // needs a chance to present their side).
        return Err(Error::DisputeNotResponded);
    }

    // For unresponded disputes where window has elapsed, the subscriber wins
    // by default (auto-resolve). For responded disputes, the admin decides.
    let resolution = if dispute.status == DisputeStatus::Open && window_elapsed {
        DisputeStatus::ResolvedToSubscriber
    } else if resolve_to_subscriber {
        DisputeStatus::ResolvedToSubscriber
    } else {
        DisputeStatus::ResolvedToMerchant
    };

    // ── Effects ───────────────────────────────────────────────────────────
    let escrow_amount: i128 = env
        .storage()
        .instance()
        .get(&DataKey::DisputeEscrow(dispute_id))
        .unwrap_or(0);

    if escrow_amount <= 0 {
        return Err(Error::InsufficientBalance);
    }

    // Read subscription for token and party addresses
    let sub = queries::get_subscription(env, dispute.subscription_id)?;
    let token_addr = &sub.token;

    // Clear escrow
    env.storage()
        .instance()
        .remove(&DataKey::DisputeEscrow(dispute_id));

    if resolution == DisputeStatus::ResolvedToMerchant {
        // Return escrowed funds to merchant balance
        let current = merchant::get_merchant_balance_by_token(env, &dispute.merchant, token_addr);
        let new_balance = current
            .checked_add(escrow_amount)
            .ok_or(Error::Overflow)?;
        merchant::set_merchant_balance(env, &dispute.merchant, token_addr, &new_balance);
    } else {
        // Transfer escrowed funds to subscriber
        // Funds leave vault custody — keep accounting consistent.
        crate::accounting::sub_total_accounted(env, token_addr, escrow_amount)?;

        let token_client = token::Client::new(env, token_addr);
        token_client.transfer(
            &env.current_contract_address(),
            &dispute.subscriber,
            &escrow_amount,
        );
    }

    // Update dispute status
    dispute.status = resolution;
    env.storage()
        .persistent()
        .set(&DataKey::Dispute(dispute_id), &dispute);

    // Clear subscription dispute index
    env.storage()
        .instance()
        .remove(&DataKey::SubscriptionDispute(dispute.subscription_id));

    env.events().publish(
        (Symbol::new(env, "dispute_resolved"), dispute_id),
        DisputeResolvedEvent {
            dispute_id,
            subscription_id: dispute.subscription_id,
            resolution,
            timestamp: now,
            schema_version: crate::types::EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Read a dispute record by its ID.
pub fn do_get_dispute(env: &Env, dispute_id: u64) -> Result<Dispute, Error> {
    read_dispute(env, dispute_id)
}

/// Return the active dispute ID for a subscription, if any.
pub fn do_get_subscription_dispute(env: &Env, subscription_id: u32) -> Option<u64> {
    env.storage()
        .instance()
        .get(&DataKey::SubscriptionDispute(subscription_id))
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn read_dispute(env: &Env, dispute_id: u64) -> Result<Dispute, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Dispute(dispute_id))
        .ok_or(Error::DisputeNotFound)
}

fn next_dispute_id(env: &Env) -> u64 {
    let key = DataKey::NextDisputeId;
    let current: u64 = env.storage().instance().get(&key).unwrap_or(0);
    let next = current.wrapping_add(1);
    env.storage().instance().set(&key, &next);
    current
}
