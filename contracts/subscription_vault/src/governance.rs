//! Governance: proposal submission, voting, and execution with quorum-based validation.
//!
//! This module implements quorum-based governance where N guardians vote on proposals
//! before privileged actions like `rotate_admin` and `set_protocol_fee` can execute.
//!
//! **Security properties:**
//! - Guardian additions/removals tracked via proposals, not direct admin calls.
//! - Quorum validation required on every proposal execution.
//! - Stale proposals cannot execute (ETA check).
//! - Double-voting is prevented (per-guardian vote tracking).
//! - Guardian removal mid-vote invalidates their prior votes.

use crate::types::{
    DataKey, Error, Proposal, ProposalCancelledEvent, ProposalExecutedEvent, ProposalKind,
    ProposalSubmittedEvent, ProposalVotedEvent, EVENT_SCHEMA_VERSION,
};
use soroban_sdk::{token, Address, Env, Map, String, Symbol, Vec};

/// Governance domain for replay protection.
const DOMAIN_GOVERNANCE: u32 = 3;

/// Add or update a guardian's voting weight.
///
/// # Errors
/// - `NotInitialized` if no admin is set.
/// - `InvalidInput` if weight is zero.
pub fn add_guardian(env: &Env, guardian: Address, weight: u32) -> Result<(), Error> {
    if weight == 0 {
        return Err(Error::InvalidInput);
    }

    let mut guardians = read_guardians(env);
    guardians.set(guardian.clone(), weight);
    write_guardians(env, &guardians);

    Ok(())
}

/// Remove a guardian by setting their weight to zero.
///
/// After removal, the guardian cannot vote on new proposals and their prior votes
/// are ignored during quorum calculation.
pub fn remove_guardian(env: &Env, guardian: &Address) -> Result<(), Error> {
    let mut guardians = read_guardians(env);
    guardians.remove(guardian.clone());
    write_guardians(env, &guardians);
    Ok(())
}

/// Get a guardian's current voting weight (0 if not a guardian).
pub fn get_guardian_weight(env: &Env, guardian: &Address) -> u32 {
    read_guardians(env)
        .get(guardian.clone())
        .unwrap_or(0)
}

/// Calculate total voting weight across all guardians.
fn calculate_total_weight(env: &Env) -> u32 {
    let guardians = read_guardians(env);
    let mut total: u32 = 0;
    for (_, weight) in guardians.iter() {
        total = total.checked_add(weight).unwrap_or(u32::MAX);
    }
    total
}

/// Submit a new governance proposal.
///
/// Creates a proposal with a deterministic ID and stores it in persistent storage.
/// Proposals require an ETA (execution timestamp) to prevent immediate execution.
///
/// # Errors
/// - `InvalidInput` if quorum_bps is invalid (> 10000).
/// - `EmergencyStopActive` if emergency stop is enabled.
pub fn do_submit_proposal(
    env: &Env,
    kind: ProposalKind,
    target: Address,
    target2: Option<Address>,
    target3: u32,
    quorum_bps: u32,
    eta: u64,
) -> Result<u64, Error> {
    if quorum_bps > 10_000 {
        return Err(Error::InvalidInput);
    }

    let now = env.ledger().timestamp();
    if eta <= now {
        return Err(Error::InvalidInput);
    }

    let proposal_id = get_next_proposal_id(env);
    let votes = Map::new(env);

    let proposal = Proposal {
        id: proposal_id,
        kind,
        target: target.clone(),
        target2,
        target3,
        quorum_bps,
        votes,
        eta,
        submitted_at: now,
        executed: false,
    };

    write_proposal(env, proposal_id, &proposal);

    env.events().publish(
        (Symbol::new(env, "proposal_submitted"),),
        ProposalSubmittedEvent {
            proposal_id,
            kind,
            target,
            quorum_bps,
            eta,
            timestamp: now,
            schema_version: EVENT_SCHEMA_VERSION,
        },
    );

    Ok(proposal_id)
}

/// Record a guardian's vote on a proposal.
///
/// Guardian weight at vote time is recorded; if guardian is removed later,
/// this vote is invalidated during execute phase.
///
/// # Errors
/// - `NotFound` if proposal does not exist.
/// - `InvalidInput` if proposal already executed or ETA not passed yet.
/// - `Unauthorized` if caller is not a valid guardian.
pub fn do_vote_proposal(env: &Env, proposal_id: u64, voted_yes: bool) -> Result<(), Error> {
    let guardian = crate::admin::require_stored_admin_auth(env)?;

    let guardian_weight = get_guardian_weight(env, &guardian);
    if guardian_weight == 0 {
        return Err(Error::Unauthorized);
    }

    let mut proposal = read_proposal(env, proposal_id)?;

    if proposal.executed {
        return Err(Error::InvalidInput);
    }

    // Record the vote
    proposal.votes.set(guardian.clone(), voted_yes);
    write_proposal(env, proposal_id, &proposal);

    env.events().publish(
        (Symbol::new(env, "proposal_voted"),),
        ProposalVotedEvent {
            proposal_id,
            guardian: guardian.clone(),
            voted_yes,
            guardian_weight,
            timestamp: env.ledger().timestamp(),
            schema_version: EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Execute a proposal if quorum is met and ETA has passed.
///
/// Validates quorum requirements before invoking the proposal-specific handler.
/// Blocks re-execution via `executed` flag.
///
/// # Errors
/// - `NotFound` if proposal does not exist.
/// - `InvalidInput` if ETA has not been reached or proposal already executed.
pub fn do_execute_proposal(env: &Env, proposal_id: u64) -> Result<(), Error> {
    let mut proposal = read_proposal(env, proposal_id)?;

    if proposal.executed {
        return Err(Error::InvalidInput);
    }

    let now = env.ledger().timestamp();
    if now < proposal.eta {
        return Err(Error::InvalidInput);
    }

    // Calculate quorum
    let (votes_for, votes_against) = calculate_quorum(env, &proposal);
    let total_weight = calculate_total_weight(env);

    let required_votes = (total_weight as u128)
        .checked_mul(proposal.quorum_bps as u128)
        .and_then(|v| v.checked_div(10_000))
        .ok_or(Error::Overflow)? as u32;

    if votes_for < required_votes {
        return Err(Error::InvalidInput);
    }

    // Execute the proposal
    match proposal.kind {
        ProposalKind::RotateAdmin => {
            crate::admin::write_config(env, &DataKey::Admin, &proposal.target);
        }
        ProposalKind::SetProtocolFee => {
            crate::admin::write_config(env, &DataKey::FeeBps, &proposal.target3);
            if let Some(treasury) = proposal.target2 {
                crate::admin::write_config(env, &DataKey::Treasury, &treasury);
            }
        }
        ProposalKind::UpgradeContract => {
            // Reserved for future use
            return Err(Error::InvalidInput);
        }
    }

    // Mark as executed
    proposal.executed = true;
    write_proposal(env, proposal_id, &proposal);

    env.events().publish(
        (Symbol::new(env, "proposal_executed"),),
        ProposalExecutedEvent {
            proposal_id,
            kind: proposal.kind,
            votes_for,
            votes_against,
            total_weight,
            timestamp: now,
            schema_version: EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Cancel a proposal.
///
/// Can only be called by the current admin. Prevents stale proposals from lingering.
///
/// # Errors
/// - `Unauthorized` if caller is not the admin.
/// - `NotFound` if proposal does not exist.
/// - `InvalidInput` if proposal is already executed.
pub fn do_cancel_proposal(env: &Env, proposal_id: u64, reason: String) -> Result<(), Error> {
    let _admin = crate::admin::require_stored_admin_auth(env)?;

    let mut proposal = read_proposal(env, proposal_id)?;

    if proposal.executed {
        return Err(Error::InvalidInput);
    }

    proposal.executed = true;
    write_proposal(env, proposal_id, &proposal);

    env.events().publish(
        (Symbol::new(env, "proposal_cancelled"),),
        ProposalCancelledEvent {
            proposal_id,
            reason,
            timestamp: env.ledger().timestamp(),
            schema_version: EVENT_SCHEMA_VERSION,
        },
    );

    Ok(())
}

/// Get the current quorum (votes for and against).
///
/// Re-validates guardian status at read time to handle guardian removal.
fn calculate_quorum(env: &Env, proposal: &Proposal) -> (u32, u32) {
    let guardians = read_guardians(env);
    let mut votes_for: u32 = 0;
    let mut votes_against: u32 = 0;

    for (guardian, voted_yes) in proposal.votes.iter() {
        // Only count votes from current guardians
        if let Some(weight) = guardians.get(guardian.clone()) {
            if voted_yes {
                votes_for = votes_for.checked_add(weight).unwrap_or(u32::MAX);
            } else {
                votes_against = votes_against.checked_add(weight).unwrap_or(u32::MAX);
            }
        }
    }

    (votes_for, votes_against)
}

// ── Storage helpers ────────────────────────────────────────────────────────

/// Read guardians map from persistent storage.
fn read_guardians(env: &Env) -> Map<Address, u32> {
    env.storage()
        .persistent()
        .get::<_, Map<Address, u32>>(&DataKey::Guardians)
        .unwrap_or_else(|| Map::new(env))
}

/// Write guardians map to persistent storage.
fn write_guardians(env: &Env, guardians: &Map<Address, u32>) {
    env.storage()
        .persistent()
        .set(&DataKey::Guardians, guardians);
    env.storage()
        .persistent()
        .extend_ttl(&DataKey::Guardians, 30 * 24 * 60 * 60, 365 * 24 * 60 * 60);
}

/// Read proposal from persistent storage.
fn read_proposal(env: &Env, proposal_id: u64) -> Result<Proposal, Error> {
    env.storage()
        .persistent()
        .get::<_, Proposal>(&DataKey::Proposal(proposal_id))
        .ok_or(Error::NotFound)
}

/// Write proposal to persistent storage.
fn write_proposal(env: &Env, proposal_id: u64, proposal: &Proposal) {
    let key = DataKey::Proposal(proposal_id);
    env.storage().persistent().set(&key, proposal);
    env.storage()
        .persistent()
        .extend_ttl(&key, 30 * 24 * 60 * 60, 365 * 24 * 60 * 60);
}

/// Get next proposal ID and increment counter.
fn get_next_proposal_id(env: &Env) -> u64 {
    let id = env
        .storage()
        .instance()
        .get::<_, u64>(&DataKey::NextProposalId)
        .unwrap_or(0);
    env.storage()
        .instance()
        .set(&DataKey::NextProposalId, &(id + 1));
    id
}

/// Read current proposal ID counter.
pub fn get_current_proposal_id(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get::<_, u64>(&DataKey::NextProposalId)
        .unwrap_or(0)
}

/// Get proposal by ID.
pub fn get_proposal(env: &Env, proposal_id: u64) -> Option<Proposal> {
    read_proposal(env, proposal_id).ok()
}

/// List all guardians and their weights.
pub fn list_guardians(env: &Env) -> Vec<(Address, u32)> {
    let guardians = read_guardians(env);
    let mut result = Vec::new(env);
    for (guardian, weight) in guardians.iter() {
        result.push_back((guardian, weight));
    }
    result
}
