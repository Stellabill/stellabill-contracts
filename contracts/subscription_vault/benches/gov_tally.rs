//! Governance quorum tally benchmark — quantifies voter-count scaling.
//!
//! # Purpose
//! Measures the CPU-instruction cost of `calculate_quorum` (the tally
//! function inside `governance::do_execute_proposal`) at voter counts
//! {10, 100, 1000}.  The tally iterates every recorded vote and maps
//! each back to the current guardian weight set, yielding O(voters) cost
//! that is dominated by persistent-storage reads and Soroban Map lookups.
//!
//! # Scenarios
//! 1. **All-yes** — every guardian votes yes; maximum tally work.
//! 2. **All-no** — every guardian votes no; exercises the `else` branch.
//! 3. **All-abstain** — zero votes cast; best-case / no-op baseline.
//! 4. **Mixed** — half yes, half no; realistic split scenario.
//!
//! # Edge cases
//! - Unusually large yes-weights (u32::MAX per guardian) to stress the
//!   `checked_add` accumulation path.
//! - Guardian removal mid-proposal: votes from removed guardians are
//!   silently skipped, so the tally cost does not decrease.
//!
//! # Output
//! Prints a CSV table to stdout:
//! ```text
//! Scenario,Voters,CPU_Cost,Cost_Per_Voter,VotesFor,VotesAgainst
//! ```
//!
//! # Security notes
//! - The tally is deterministic and idempotent; re-running after guardian
//!   removal or addition yields the same result for the same snapshot.
//! - All arithmetic uses `checked_add` with saturating-to-u32::MAX
//!   semantics, preventing overflow panics even with malicious weights.

#![cfg(test)]

use soroban_sdk::{testutils::Address as _, Address, Env, Map, Vec};
use subscription_vault::{calculate_quorum, DataKey, Proposal, ProposalKind};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Voter counts to benchmark.
const VOTER_COUNTS: &[u32] = &[10, 100, 1000];

/// Default guardian weight for uniform seeding.
const DEFAULT_WEIGHT: u32 = 100;

/// ── Helpers ──────────────────────────────────────────────────────────────────

/// Seed `count` guardians into persistent storage with uniform `weight`
/// and return their addresses in a Vec.
fn seed_guardians(env: &Env, count: u32, weight: u32) -> Vec<Address> {
    let mut guardians: Map<Address, u32> = Map::new(env);
    let mut addresses: Vec<Address> = Vec::new(env);
    for _ in 0..count {
        let addr = Address::generate(env);
        guardians.set(addr.clone(), weight);
        addresses.push_back(addr);
    }
    env.storage().persistent().set(&DataKey::Guardians, &guardians);
    addresses
}

/// Build a `Proposal` with votes from the first `yes_count` guardians
/// (voted `true`) and the next `no_count` guardians (voted `false`).
/// Remaining guardians in the passed slice abstain.
fn build_proposal(env: &Env, guardians: &Vec<Address>, yes_count: u32, no_count: u32) -> Proposal {
    let mut votes: Map<Address, bool> = Map::new(env);
    let end_yes = yes_count.min(guardians.len() as u32);
    for i in 0..end_yes {
        votes.set(guardians.get(i).unwrap(), true);
    }
    let end_no = (yes_count.saturating_add(no_count)).min(guardians.len() as u32);
    for i in end_yes..end_no {
        votes.set(guardians.get(i).unwrap(), false);
    }

    Proposal {
        id: 0,
        kind: ProposalKind::RotateAdmin,
        target: Address::generate(env),
        target2: None,
        target3: 0,
        quorum_bps: 5_000,
        votes,
        eta: 0,
        submitted_at: 0,
        executed: false,
    }
}

/// Measure one call to `calculate_quorum` and return (cpu_cost, votes_for, votes_against).
fn measure_tally(env: &Env, proposal: &Proposal) -> (u64, u32, u32) {
    env.budget().reset_default();
    let (votes_for, votes_against) = calculate_quorum(env, proposal);
    let cpu_cost = env.budget().cpu_instruction_cost();
    (cpu_cost, votes_for, votes_against)
}

// ── Bench ─────────────────────────────────────────────────────────────────────

#[test]
fn bench_gov_tally_scaling() {
    let mut output = String::from("Scenario,Voters,CPU_Cost,Cost_Per_Voter,VotesFor,VotesAgainst\n");

    let scenarios: &[(&str, u32, u32)] = &[
        ("all-yes", u32::MAX, 0), // yes_count = all, no_count = 0
        ("all-no", 0, u32::MAX),  // no_count = all
        ("all-abstain", 0, 0),     // no votes cast
    ];

    for &count in VOTER_COUNTS {
        let env = Env::default();
        env.budget().reset_unlimited();

        let guardians = seed_guardians(&env, count, DEFAULT_WEIGHT);
        let actual_count = guardians.len() as u32;

        // ── Named scenarios ──────────────────────────────────────────────
        for &(label, yes_req, no_req) in scenarios {
            let yes = if yes_req == u32::MAX { actual_count } else { yes_req };
            let no = if no_req == u32::MAX { actual_count } else { no_req };

            let proposal = build_proposal(&env, &guardians, yes, no);
            env.budget().reset_default();
            let (votes_for, votes_against) = calculate_quorum(&env, &proposal);
            let cpu_cost = env.budget().cpu_instruction_cost();
            let cost_per_voter = if actual_count > 0 {
                cpu_cost / actual_count as u64
            } else {
                0
            };

            output.push_str(&format!(
                "{},{},{},{},{},{}\n",
                label, actual_count, cpu_cost, cost_per_voter, votes_for, votes_against
            ));
        }

        // ── Mixed (half yes, half no) ────────────────────────────────────
        if actual_count >= 2 {
            let yes = actual_count / 2;
            let no = actual_count - yes;
            let proposal = build_proposal(&env, &guardians, yes, no);
            env.budget().reset_default();
            let (votes_for, votes_against) = calculate_quorum(&env, &proposal);
            let cpu_cost = env.budget().cpu_instruction_cost();
            let cost_per_voter = cpu_cost / actual_count as u64;

            output.push_str(&format!(
                "mixed,{},{},{},{},{}\n",
                actual_count, cpu_cost, cost_per_voter, votes_for, votes_against
            ));

            // Correctness assertions
            assert_eq!(votes_for, yes * DEFAULT_WEIGHT, "mixed votes_for mismatch");
            assert_eq!(votes_against, no * DEFAULT_WEIGHT, "mixed votes_against mismatch");
        }
    }

    // ── Edge case: unusually large weights ────────────────────────────────
    for &count in VOTER_COUNTS {
        let env = Env::default();
        env.budget().reset_unlimited();

        let guardians = seed_guardians(&env, count, u32::MAX);
        let actual_count = guardians.len() as u32;

        // All-yes with MAX weight → saturates to u32::MAX
        let proposal = build_proposal(&env, &guardians, actual_count, 0);
        env.budget().reset_default();
        let (votes_for, votes_against) = calculate_quorum(&env, &proposal);
        let cpu_cost = env.budget().cpu_instruction_cost();
        let cost_per_voter = if actual_count > 0 {
            cpu_cost / actual_count as u64
        } else {
            0
        };

        output.push_str(&format!(
            "large-weights,{},{},{},{},{}\n",
            actual_count, cpu_cost, cost_per_voter, votes_for, 0u32
        ));

        // With u32::MAX weights, the first checked_add saturates.
        assert_eq!(votes_for, u32::MAX, "large weights must saturate to u32::MAX");
        assert_eq!(votes_against, 0);
    }

    // ── Edge case: guardian removed, vote skipped ─────────────────────────
    {
        let env = Env::default();
        env.budget().reset_unlimited();

        let guardian_a = Address::generate(&env);
        let guardian_b = Address::generate(&env);

        // Seed only guardian_b as a current guardian
        let mut guardians: Map<Address, u32> = Map::new(&env);
        guardians.set(guardian_b.clone(), 100);
        env.storage().persistent().set(&DataKey::Guardians, &guardians);

        // Create a proposal where guardian_a (removed) voted yes.
        let mut votes: Map<Address, bool> = Map::new(&env);
        votes.set(guardian_a.clone(), true); // removed guardian
        votes.set(guardian_b.clone(), true); // active guardian

        let proposal = Proposal {
            id: 0,
            kind: ProposalKind::RotateAdmin,
            target: Address::generate(&env),
            target2: None,
            target3: 0,
            quorum_bps: 5_000,
            votes,
            eta: 0,
            submitted_at: 0,
            executed: false,
        };

        env.budget().reset_default();
        let (votes_for, votes_against) = calculate_quorum(&env, &proposal);
        let cpu_cost = env.budget().cpu_instruction_cost();

        // 2 votes total in the proposal: 1 removed (skipped) + 1 active (counted)
        let total_votes_in_proposal = 2u32;
        output.push_str(&format!(
            "removed-guardian,{},{},{},{},{}\n",
            total_votes_in_proposal, cpu_cost,
            cpu_cost / total_votes_in_proposal as u64,
            votes_for, votes_against
        ));

        // Only guardian_b's vote counts; guardian_a is ignored.
        assert_eq!(votes_for, 100);
        assert_eq!(votes_against, 0);
    }

    std::println!("{}", output);

    // ── Scaling assertion ────────────────────────────────────────────────
    // Cost per voter should be broadly similar across sizes (sub-linear storage
    // overhead may cause small per-voter cost decrease at larger sizes due to
    // fixed read-amortisation).  We assert that the per-voter cost at 1000 voters
    // is no more than 3× the per-voter cost at 10 voters (generous bound to
    // absorb SDK overhead changes while still catching catastrophic regressions).
    {
        let env = Env::default();
        env.budget().reset_unlimited();
        let guardians_10 = seed_guardians(&env, 10, DEFAULT_WEIGHT);
        let proposal_10 = build_proposal(&env, &guardians_10, 10, 0);
        env.budget().reset_default();
        let (_, _) = calculate_quorum(&env, &proposal_10);
        let cpu_10 = env.budget().cpu_instruction_cost();
        let per_10 = cpu_10 / 10;

        let env = Env::default();
        env.budget().reset_unlimited();
        let guardians_1000 = seed_guardians(&env, 1000, DEFAULT_WEIGHT);
        let proposal_1000 = build_proposal(&env, &guardians_1000, 1000, 0);
        env.budget().reset_default();
        let (_, _) = calculate_quorum(&env, &proposal_1000);
        let cpu_1000 = env.budget().cpu_instruction_cost();
        let per_1000 = cpu_1000 / 1000;

        assert!(
            per_1000 <= per_10.saturating_mul(3),
            "per-voter cost regression: 10-voter={} vs 1000-voter={} (ratio > 3×)",
            per_10,
            per_1000
        );
    }
}
