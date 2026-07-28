# Governance Proposal System - Implementation Guide

## Overview

The Stellabill subscription vault now implements quorum-based governance over privileged operations that were previously gated by a single admin signature. This document describes the implementation, security properties, and usage patterns.

## Motivation

Single-point-of-failure risk in `rotate_admin` and `set_protocol_fee` operations creates significant security exposure on mainnet. The governance system mitigates this by requiring multiple guardian signatures (weighted voting with configurable quorum) before privileged actions can execute.

## Architecture

### Guardian Model

Guardians are participants with assigned voting **weights** (non-zero u32 values). Guardians are managed directly by the current admin via:

- `add_guardian(admin, guardian, weight)` — Add or update a guardian's weight
- `remove_guardian(admin, guardian)` — Remove a guardian (set weight to zero)

Guardian removals are **immediate** and retroactively invalidate prior votes during quorum calculation.

### Proposal Lifecycle

```
SUBMITTED → [VOTING PERIOD] → [ETA CHECK] → EXECUTED/CANCELLED
                ↓                   ↓
            vote_proposal()    execute_proposal()
```

1. **Submit** (`submit_proposal`): Create a new proposal with:
   - Type: RotateAdmin, SetProtocolFee, or UpgradeContract
   - Target parameters (new admin, treasury address, fee_bps)
   - Quorum requirement in basis points (0-10000)
   - ETA (execution timestamp threshold)

2. **Vote** (`vote_proposal`): Guardians sign transactions to vote yes/no:
   - Each guardian can vote once per proposal (overwrites prior vote)
   - Votes are time-stamped and included in events
   - Guardian removal mid-vote invalidates their votes at execute time

3. **Execute** (`execute_proposal`):
   - Validates ETA has passed
   - Recalculates quorum considering current (not historical) guardian weights
   - Requires vote percentage ≥ quorum_bps
   - Atomically applies the proposal action and marks as executed
   - Prevents re-execution via immutability flag

4. **Cancel** (`cancel_proposal`): Admin-only operation to remove stale proposals

### Storage Layout

| Key | Tier | Type | Purpose |
|-----|------|------|---------|
| `DataKey::Guardians` | persistent | `Map<Address, u32>` | Guardian weights |
| `DataKey::NextProposalId` | instance | `u64` | Proposal ID counter |
| `DataKey::Proposal(u64)` | persistent | `Proposal` | Proposal records |

Guardians and Proposals use TTL extension (30-day threshold, 365-day target).

### Proposal Structure

```rust
pub struct Proposal {
    pub id: u64,                              // Monotonic ID
    pub kind: ProposalKind,                   // Type of proposal
    pub target: Address,                      // Primary target
    pub target2: Option<Address>,             // Secondary target
    pub target3: u32,                         // Tertiary parameter
    pub quorum_bps: u32,                      // Quorum requirement
    pub votes: Map<Address, bool>,            // Guardian votes
    pub eta: u64,                             // Execution threshold
    pub submitted_at: u64,                    // Submission timestamp
    pub executed: bool,                       // Execution flag
}
```

## Entrypoints

### Proposal Management

#### `submit_proposal(kind, target, target2, target3, quorum_bps, eta) → u64`

Submit a new governance proposal.

- **Arguments:**
  - `kind`: ProposalKind enum (RotateAdmin, SetProtocolFee, UpgradeContract)
  - `target`: Primary target address (new admin or treasury)
  - `target2`: Optional secondary target
  - `target3`: Tertiary parameter (0 or fee_bps)
  - `quorum_bps`: Quorum in basis points (0-10000)
  - `eta`: Timestamp after which execution is allowed

- **Returns:** Proposal ID (u64)

- **Errors:**
  - `InvalidInput` if quorum_bps > 10000
  - `InvalidInput` if eta ≤ current timestamp

#### `vote_proposal(proposal_id, voted_yes) → ()`

Cast a vote on a proposal.

- **Arguments:**
  - `proposal_id`: ID of proposal to vote on
  - `voted_yes`: true for yes, false for no

- **Errors:**
  - `Unauthorized` if caller is not a guardian
  - `NotFound` if proposal doesn't exist
  - `InvalidInput` if proposal already executed

#### `execute_proposal(proposal_id) → ()`

Execute a proposal if quorum is met and ETA passed.

- **Arguments:**
  - `proposal_id`: ID of proposal to execute

- **Errors:**
  - `NotFound` if proposal doesn't exist
  - `InvalidInput` if not yet ETA or quorum not met

#### `cancel_proposal(proposal_id, reason) → ()`

Cancel a proposal (admin only).

- **Arguments:**
  - `proposal_id`: ID of proposal to cancel
  - `reason`: Cancellation reason (emitted in event)

- **Errors:**
  - `Unauthorized` if caller is not admin
  - `NotFound` if proposal doesn't exist
  - `InvalidInput` if already executed

### Guardian Management

#### `add_guardian(admin, guardian, weight) → ()`

Add or update a guardian's voting weight.

- **Arguments:**
  - `admin`: Current admin address (auth required)
  - `guardian`: Guardian address to add/update
  - `weight`: Voting weight (must be > 0)

- **Errors:**
  - `Unauthorized` if caller is not admin
  - `InvalidInput` if weight is zero

#### `remove_guardian(admin, guardian) → ()`

Remove a guardian (set weight to zero).

- **Arguments:**
  - `admin`: Current admin address (auth required)
  - `guardian`: Guardian address to remove

- **Errors:**
  - `Unauthorized` if caller is not admin

#### `get_guardian_weight(guardian) → u32`

Get a guardian's current voting weight (0 if not a guardian).

#### `list_guardians() → Vec<(Address, u32)>`

List all guardians and their weights.

### Query Entrypoints

#### `get_current_proposal_id() → u64`

Get the next proposal ID to be allocated.

#### `get_proposal(proposal_id) → Option<Proposal>`

Get proposal details by ID.

## Security Properties

### ✅ Verified Security Assumptions

1. **Quorum Validation**
   - Recalculated at execute time using current guardian weights
   - Guardian removals mid-vote correctly invalidate their votes
   - Double-voting prevented (Map overwrites prior vote)

2. **ETA Protection**
   - Proposals cannot execute before ETA
   - Stale proposals can be cancelled by admin
   - ETA in past rejected at submission

3. **Atomicity**
   - Proposal marked `executed = true` before any external effects
   - Re-execution prevented via immutability flag
   - State mutations are transactional

4. **Replay Protection**
   - Each proposal has unique ID (monotonic allocation)
   - Proposal records are persistent and immutable post-execution
   - No nonce reuse across different proposal types

5. **Access Control**
   - Guardians managed by admin only
   - Proposals executed by any caller (if conditions met)
   - Cancel operation restricted to admin

6. **Guardian Removal**
   - Immediate effect (no grace period)
   - Votes from removed guardians excluded at execute time
   - Prevents timing attacks on vote windows

### ⚠️ Known Limitations

1. **No Vote Delegation**: Guardians cannot delegate votes
2. **No Vote Veto Period**: Guardians cannot recall votes
3. **No Proposal Amendments**: Proposals are immutable after submission
4. **Upgrade Path Reserved**: UpgradeContract proposal kind not yet implemented

## Events

### ProposalSubmittedEvent
Emitted when a proposal is created.
```rust
pub proposal_id: u64,
pub kind: ProposalKind,
pub target: Address,
pub quorum_bps: u32,
pub eta: u64,
pub timestamp: u64,
pub schema_version: u32,
```

### ProposalVotedEvent
Emitted when a guardian votes.
```rust
pub proposal_id: u64,
pub guardian: Address,
pub voted_yes: bool,
pub guardian_weight: u32,
pub timestamp: u64,
pub schema_version: u32,
```

### ProposalExecutedEvent
Emitted when a proposal is executed.
```rust
pub proposal_id: u64,
pub kind: ProposalKind,
pub votes_for: u32,
pub votes_against: u32,
pub total_weight: u32,
pub timestamp: u64,
pub schema_version: u32,
```

### ProposalCancelledEvent
Emitted when a proposal is cancelled.
```rust
pub proposal_id: u64,
pub reason: String,
pub timestamp: u64,
pub schema_version: u32,
```

## Usage Patterns

### Example: Rotating Admin with Governance

```rust
// 1. Assume 3 guardians exist with weights 100, 100, 100 (total 300)
// 2. Submit proposal requiring 67% quorum (200 votes)
let proposal_id = client.submit_proposal(
    ProposalKind::RotateAdmin,
    new_admin_address,
    None,
    0,
    6700,  // 67% quorum
    eta,   // e.g., now + 7 days
)?;

// 3. Guardians vote (at least 2 must vote yes)
client.vote_proposal(proposal_id, true)?;  // Guardian 1
client.vote_proposal(proposal_id, true)?;  // Guardian 2
client.vote_proposal(proposal_id, false)?; // Guardian 3

// 4. After ETA, anyone can execute
client.execute_proposal(proposal_id)?;  // 200 votes > 200 quorum ✓

// 5. Admin is now updated
```

### Example: Handling Guardian Removal Mid-Proposal

```rust
// Scenario: 3 guardians (100 each), 67% quorum needed (200 votes)
client.submit_proposal(..., quorum_bps: 6700, ...)?;

// Guardian 1 and 2 vote yes (200 votes)
client.vote_proposal(proposal_id, true)?;   // Guardian 1
client.vote_proposal(proposal_id, true)?;   // Guardian 2

// Admin removes Guardian 2 mid-vote
client.remove_guardian(admin, guardian2)?;

// Now only Guardian 1 (100) and 3 (100) count
// Guardian 2's vote is ignored
// Quorum check: 100 < 200 required → REJECTED ✗
```

## Test Coverage

The implementation includes 11 core test cases covering:

✅ Guardian addition and removal
✅ Proposal submission (RotateAdmin, SetProtocolFee)
✅ Invalid quorum and ETA validation
✅ Vote casting and updates
✅ Proposal execution without quorum
✅ Execution idempotency
✅ Guardian removal invalidating votes
✅ Proposal cancellation
✅ Double-vote prevention (via Map overwrite)
✅ Guardian listing
✅ Proposal ID counter management

Target coverage: **≥ 95%** of governance code paths.

## Deployment Considerations

1. **Initialization**: No migration needed; governance is opt-in (no guardians by default)
2. **Dual-Mode**: Can run in single-admin mode (no guardians) or multi-guardian mode
3. **Transition Plan**:
   - Deploy with no guardians
   - Admin adds guardians via `add_guardian`
   - Admin submits governance-gated proposals
   - Existing `rotate_admin`/`set_protocol_fee` continue to work in single-admin mode
4. **Future**: Optional migration to require governance for all privileged ops

## Rollback

If governance becomes problematic:
1. Admin can remove all guardians
2. Contract reverts to single-admin mode
3. No data loss; proposals remain as audit trail
4. No contract restart needed

## References

- [Admin Authorization Matrix](admin_authorization_matrix.md)
- [Protocol Invariants](protocol_invariants.md)
- [Reentrancy Protection](reentrancy_hardening.md)
