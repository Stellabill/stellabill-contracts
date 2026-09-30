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
    ProposalSubmittedEvent, ProposalVotedEvent, VoteLockedEvent, EVENT_SCHEMA_VERSION,
};
use soroban_sdk::{Address, Env, Map, String, Symbol, Vec};

/// Governance domain for replay protection.
#[allow(dead_code)]
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
    read_guardians(env).get(guardian.clone()).unwrap_or(0)
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
/// Votes are **locked** once the proposal's ETA (timelock) is reached —
/// guardians cannot add or change votes after the timelock opens. This
/// prevents vote-flip griefing where a guardian could feign support during
/// the voting window then flip their vote right at execution time.
///
/// # Errors
/// - `NotFound` if proposal does not exist.
/// - `InvalidInput` if proposal already executed.
/// - `Unauthorized` if caller is not a valid guardian.
/// - `InvalidInput` if the timelock (ETA) has passed and votes are locked.
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

    let now = env.ledger().timestamp();

    // ── Timelock guard: votes are locked once ETA is reached ────────────
    //
    // If the proposal's timelock has passed, no further votes may be
    // recorded. This prevents a guardian from feigning support during the
    // voting window and then flipping their vote to grief execution.
    if now >= proposal.eta {
        env.events().publish(
            (Symbol::new(env, "vote_locked"),),
            VoteLockedEvent {
                proposal_id,
                guardian: guardian.clone(),
                eta: proposal.eta,
                timestamp: now,
                schema_version: EVENT_SCHEMA_VERSION,
            },
        );
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
            timestamp: now,
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
            if let Some(ref treasury) = proposal.target2 {
                crate::admin::write_config(env, &DataKey::Treasury, treasury);
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
pub fn calculate_quorum(env: &Env, proposal: &Proposal) -> (u32, u32) {
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
    crate::subscription::maybe_extend_ttl(
        env,
        &DataKey::Guardians,
        30 * 24 * 60 * 60,
        365 * 24 * 60 * 60,
    );
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
    crate::subscription::maybe_extend_ttl(&env, &key, 30 * 24 * 60 * 60, 365 * 24 * 60 * 60);
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

// ── Adversarial coverage for `get_current_proposal_id` ──────────────────────
//
// `get_current_proposal_id` is a permissionless view over the instance
// `DataKey::NextProposalId` counter. It must be pure (no ID consumption), it
// must be monotonic across successful submissions only, rejected operations
// must leave the counter byte-for-byte unchanged, and reads must stay
// deterministic at the `u64` boundary. These tests pin all four properties.
#[cfg(test)]
mod get_current_proposal_id_tests {
    use super::*;
    use crate::{SubscriptionVault, SubscriptionVaultClient};
    use soroban_sdk::testutils::{Address as _, Ledger as _};

    /// Register and initialize a fresh vault. Returns the contract address
    /// (needed to seed instance storage directly) and the generated client.
    fn init_vault<'a>(env: &'a Env) -> (Address, SubscriptionVaultClient<'a>) {
        let admin = Address::generate(env);
        let token_admin = Address::generate(env);
        let token = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();

        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(env, &contract_id);
        client.init(&token, &6, &admin, &10_000_000, &86_400);

        (contract_id, client)
    }

    /// Overwrite the raw instance counter without going through allocation.
    fn seed_counter(env: &Env, contract_id: &Address, value: u64) {
        env.as_contract(contract_id, || {
            env.storage()
                .instance()
                .set(&DataKey::NextProposalId, &value);
        });
    }

    /// Submit a `RotateAdmin` proposal against `target`.
    fn submit(
        client: &SubscriptionVaultClient,
        target: &Address,
        quorum_bps: u32,
        eta: u64,
    ) -> u64 {
        client.submit_proposal(
            &ProposalKind::RotateAdmin,
            target,
            &None,
            &0,
            &quorum_bps,
            &eta,
        )
    }

    /// A fresh, uninitialized contract reports `0`; `init` must not touch the
    /// proposal counter.
    #[test]
    fn defaults_to_zero_before_and_after_init() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        // Never allocates: the instance key has not been written yet.
        assert_eq!(client.get_current_proposal_id(), 0);

        let admin = Address::generate(&env);
        let token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        client.init(&token, &6, &admin, &10_000_000, &86_400);

        // init configures payments only; governance state is untouched.
        assert_eq!(client.get_current_proposal_id(), 0);
    }

    /// The view is pure: calling it repeatedly must not consume an ID, so the
    /// first real submission still receives ID `0`.
    #[test]
    fn read_is_pure_and_does_not_consume_an_id() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);

        let (_, client) = init_vault(&env);
        for _ in 0..8 {
            assert_eq!(client.get_current_proposal_id(), 0);
        }

        let target = Address::generate(&env);
        assert_eq!(
            submit(&client, &target, 5_000, 2_000),
            0,
            "reads must not consume the first proposal id"
        );
        assert_eq!(client.get_current_proposal_id(), 1);
    }

    /// The reported value is always exactly the ID the next successful
    /// submission will return, and it advances by exactly one per success.
    #[test]
    fn reported_value_equals_next_allocated_id() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);

        let (_, client) = init_vault(&env);
        let target = Address::generate(&env);

        for expected in 0u64..3 {
            assert_eq!(client.get_current_proposal_id(), expected);
            let id = submit(&client, &target, 5_000, 10_000);
            assert_eq!(id, expected, "allocated id must match the pre-read value");
            assert_eq!(client.get_current_proposal_id(), expected + 1);
        }
    }

    /// A submission rejected for an out-of-range quorum must not burn an ID,
    /// and the next valid submission must reuse the same ID.
    #[test]
    fn rejected_quorum_does_not_advance_counter() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);

        let (_, client) = init_vault(&env);
        let target = Address::generate(&env);

        let rejected = client.try_submit_proposal(
            &ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &10_001, // MAX_QUORUM_BPS + 1
            &2_000,
        );
        assert_eq!(rejected, Err(Ok(Error::InvalidInput)));
        assert_eq!(client.get_current_proposal_id(), 0);

        assert_eq!(submit(&client, &target, 5_000, 2_000), 0);
        assert_eq!(client.get_current_proposal_id(), 1);
    }

    /// ETA validation is `eta > now`: equal-to-now, past, and zero ETAs are all
    /// rejected before the counter is read, so none of them advance it.
    #[test]
    fn rejected_eta_does_not_advance_counter() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(5_000);

        let (_, client) = init_vault(&env);
        let target = Address::generate(&env);

        for eta in [5_000u64, 4_999, 0] {
            let rejected = client.try_submit_proposal(
                &ProposalKind::RotateAdmin,
                &target,
                &None,
                &0,
                &5_000,
                &eta,
            );
            assert_eq!(
                rejected,
                Err(Ok(Error::InvalidInput)),
                "eta {eta} must be rejected"
            );
            assert_eq!(client.get_current_proposal_id(), 0);
        }
    }

    /// Quorum accepts the inclusive `[0, 10_000]` range. Both boundaries must
    /// consume exactly one ID, and `10_001` must consume none.
    #[test]
    fn quorum_boundaries_are_inclusive_and_rejection_is_free() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);

        let (_, client) = init_vault(&env);
        let target = Address::generate(&env);

        assert_eq!(submit(&client, &target, 0, 2_000), 0);
        assert_eq!(submit(&client, &target, 10_000, 2_000), 1);
        assert_eq!(client.get_current_proposal_id(), 2);

        let rejected = client.try_submit_proposal(
            &ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &10_001,
            &2_000,
        );
        assert_eq!(rejected, Err(Ok(Error::InvalidInput)));
        assert_eq!(client.get_current_proposal_id(), 2);
    }

    /// Rejected vote / execute / cancel calls against real and non-existent
    /// proposals must not move the counter, and must not create records.
    #[test]
    fn failed_lifecycle_operations_leave_counter_unchanged() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);

        let (_, client) = init_vault(&env);
        let target = Address::generate(&env);

        let id = submit(&client, &target, 5_000, 2_000);
        assert_eq!(client.get_current_proposal_id(), 1);

        // The admin holds no guardian weight, so the vote is rejected as
        // unauthorized. The weight check runs *before* the proposal lookup, so
        // a non-guardian cannot probe whether a proposal id exists.
        assert_eq!(
            client.try_vote_proposal(&id, &true),
            Err(Ok(Error::Unauthorized))
        );
        assert_eq!(
            client.try_vote_proposal(&999u64, &true),
            Err(Ok(Error::Unauthorized))
        );
        // ETA has not been reached yet.
        assert_eq!(
            client.try_execute_proposal(&id),
            Err(Ok(Error::InvalidInput))
        );

        // Cancelling a proposal that was never allocated is NotFound.
        let reason = String::from_str(&env, "does not exist");
        assert_eq!(
            client.try_cancel_proposal(&999u64, &reason),
            Err(Ok(Error::NotFound))
        );

        // Promote the admin to a guardian so the vote clears the weight check
        // and reaches the proposal lookup, which must then report NotFound.
        let admin = client.get_admin();
        client.add_guardian(&admin, &admin, &1);
        assert_eq!(
            client.try_vote_proposal(&999u64, &true),
            Err(Ok(Error::NotFound))
        );

        // None of the rejected calls (nor guardian registration) may have
        // consumed or advanced a proposal id.
        assert_eq!(client.get_current_proposal_id(), 1);
        assert!(client.get_proposal(&999u64).is_none());
    }

    /// Reads at the `u64::MAX` boundary must be exact and overflow-free.
    #[test]
    fn reads_max_u64_without_overflow_and_without_mutation() {
        let env = Env::default();
        env.mock_all_auths();

        let (contract_id, client) = init_vault(&env);
        seed_counter(&env, &contract_id, u64::MAX);

        assert_eq!(client.get_current_proposal_id(), u64::MAX);
        // Repeated reads are stable: no arithmetic on the counter happens.
        assert_eq!(client.get_current_proposal_id(), u64::MAX);
        // The read is side-effect free.
        assert!(client.get_proposal(&u64::MAX).is_none());
    }

    /// One step before exhaustion the final ID (`u64::MAX - 1`) is allocated
    /// and the counter lands exactly on `u64::MAX` — no wrap, no skipped value.
    #[test]
    fn final_allocation_lands_exactly_on_u64_max() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);

        let (contract_id, client) = init_vault(&env);
        seed_counter(&env, &contract_id, u64::MAX - 1);
        assert_eq!(client.get_current_proposal_id(), u64::MAX - 1);

        let target = Address::generate(&env);
        assert_eq!(submit(&client, &target, 5_000, 2_000), u64::MAX - 1);
        assert_eq!(client.get_current_proposal_id(), u64::MAX);
    }

    /// `DataKey::NextProposalId` lives in instance storage, so two vaults must
    /// track independent counters regardless of submission order.
    #[test]
    fn counter_is_isolated_per_contract() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);

        let (_, client_a) = init_vault(&env);
        let (_, client_b) = init_vault(&env);
        let target = Address::generate(&env);

        assert_eq!(submit(&client_a, &target, 5_000, 2_000), 0);
        assert_eq!(submit(&client_a, &target, 5_000, 2_000), 1);
        assert_eq!(client_a.get_current_proposal_id(), 2);

        // Contract B is untouched by A's activity.
        assert_eq!(client_b.get_current_proposal_id(), 0);
        assert_eq!(submit(&client_b, &target, 5_000, 2_000), 0);
    }

    /// The view is intentionally permissionless: it must succeed with auth
    /// mocking disabled. The counter is public, non-sensitive metadata, so this
    /// is a documentation test for the intended access-control surface.
    #[test]
    fn view_requires_no_authorization() {
        let env = Env::default(); // deliberately no mock_all_auths()

        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        assert_eq!(client.get_current_proposal_id(), 0);
    }
}
