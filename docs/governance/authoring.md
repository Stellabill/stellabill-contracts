# Governance Proposal Author Guide

## Overview

Stellabill uses quorum-based governance to gate privileged operations that were formerly controlled by a single admin key. Any privileged action must now be drafted as a **governance proposal**, voted on by a set of **guardians**, and executed only after a **timelock** has elapsed.

This guide explains how to draft, submit, and shepherding proposals through to execution. Use the YAML templates in each section to get the proposal metadata right the first time.

---

## Proposal Lifecycle

```
DRAFT ──→ SUBMITTED ──→ VOTING WINDOW ──→ TIMELOCK ──→ EXECUTED
                          (guardians vote)   (ETA check)
                              │                  │
                              └── vote_locked ───┘
                                    ↓
                              CANCELLED (admin only)
```

| Phase | What happens | Who acts |
|-------|-------------|----------|
| **Draft** | Author writes proposal metadata (kind, target, quorum, ETA) | Author |
| **Submit** | `submit_proposal()` stores the proposal, emits `ProposalSubmittedEvent` | Any guardian |
| **Vote** | Guardians call `vote_proposal()` yes/no before ETA | Guardians |
| **Timelock** | Votes are locked once ETA is reached; no further votes accepted | (automatic) |
| **Execute** | `execute_proposal()` checks quorum, applies action atomically | Anyone |
| **Cancel** | Admin calls `cancel_proposal()` to kill a stale or dangerous proposal | Admin |

---

## Proposal Fields

Every proposal carries these fields:

| Field | Type | Description |
|-------|------|-------------|
| `kind` | `ProposalKind` | `RotateAdmin`, `SetProtocolFee`, or `UpgradeContract` (reserved) |
| `target` | `Address` | Primary target — the new admin address on rotation |
| `target2` | `Option<Address>` | Secondary target — treasury address for fee proposals |
| `target3` | `u32` | Tertiary parameter — fee basis points for fee proposals |
| `quorum_bps` | `u32` | Required approval quorum in basis points (0–10 000) |
| `eta` | `u64` | Ledger timestamp after which execution is allowed (must be **future** at submit time) |

---

## Template: Admin Rotation

Use this when migrating the contract admin to a new address.

```yaml
kind: RotateAdmin
target: GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX
target2: ~
target3: 0
quorum_bps: 6700      # 67% of total guardian weight
eta: <now + 7 days in ledger seconds>
```

**Contract call:**
```rust
client.submit_proposal(
    ProposalKind::RotateAdmin,
    new_admin_address,   // target
    None,                // target2
    0,                   // target3
    6700,                // quorum_bps (67%)
    eta,                 // epoch seconds ≥ now + 7 days
)?;
```

**Execution effect:**
- `DataKey::Admin` is overwritten with `target`.
- The previous admin loses all privileges immediately.
- Guardians are **not** automatically cleared — the new admin inherits the guardian roster.

### Voting-window guidance

| Total guardian weight | `quorum_bps` | Required votes (67 %) |
|----------------------|--------------|----------------------|
| 3 × 100 = 300        | 6700         | 201                  |
| 5 × 100 = 500        | 6700         | 335                  |
| 3 ×  50 + 2 × 100 = 350 | 6700      | 235                  |

Recommend a **minimum 7-day voting window** (set ETA to `now + 604 800` ledger seconds). This gives guardians time to verify the new admin off-chain before the timelock expires.

---

## Template: Treasury / Fee Update

Use this when changing the protocol fee rate or the treasury address that collects it.

```yaml
kind: SetProtocolFee
target: GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX  # ignored for fee-only updates
target2: GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX  # new treasury (omit if unchanged)
target3: 250                     # new fee in basis points (2.50 %)
quorum_bps: 6700                 # 67% quorum
eta: <now + 3 days in ledger seconds>
```

**Contract call:**
```rust
// Fee-only (treasury unchanged):
client.submit_proposal(
    ProposalKind::SetProtocolFee,
    current_admin,          // target — ignored for SetProtocolFee
    None,                   // target2 — None keeps existing treasury
    250,                    // target3 — 2.50 %
    6700,                   // quorum_bps
    eta,                    // epoch seconds
)?;

// Fee + treasury change:
client.submit_proposal(
    ProposalKind::SetProtocolFee,
    current_admin,
    Some(new_treasury_address),
    250,
    6700,
    eta,
)?;
```

**Execution effect:**
- `DataKey::FeeBps` is set to `target3` (0 disables the protocol fee).
- If `target2` is `Some(address)`, `DataKey::Treasury` is set to that address.
- If `target2` is `None`, the treasury address is **not** modified.

### Voting-window guidance

Fee changes affect every merchant's net revenue. Recommend a **minimum 3-day voting window** to give merchants time to object.

---

## Template: Multi-action Proposal

The current framework supports only single-action proposals. To execute multiple privileged actions atomically, submit separate proposals and coordinate their ETA windows.

```yaml
# ── Proposal A: Rotate admin ──
kind: RotateAdmin
target: <new-admin-address>
target2: ~
target3: 0
quorum_bps: 7500      # 75% for sensitive operations
eta: <now + 7 days>

# ── Proposal B: Update fee (same window) ──
kind: SetProtocolFee
target: <new-admin-address>
target2: ~
target3: 150
quorum_bps: 6700
eta: <now + 7 days>
```

The multi-proposal pattern requires guardians to approve **both** proposals before the shared ETA window closes. If one proposal fails to meet quorum, the admin must cancel it and re-submit.

> **Future work**: A future contract upgrade may add a `MultiAction` proposal kind that bundles several actions into one atomic proposal.

---

## Voting-Window Best Practices

| Parameter | Recommended | Rationale |
|-----------|-------------|-----------|
| `eta` **admin rotation** | ≥ 7 days (`now + 604 800`) | Guardians need time to verify the new admin address |
| `eta` **fee update** | ≥ 3 days (`now + 259 200`) | Merchants affected by fee changes should have time to review |
| `eta` **emergency** | ≥ 1 hour (`now + 3 600`) | Emergency proposals need faster action; see § Emergency Proposal |
| `quorum_bps` **default** | 6700 (67 %) | Majority of weight must approve |
| `quorum_bps` **high-risk** | 7500–10 000 | Admin rotation, treasury change, or large fee swings |
| `quorum_bps` **minimum** | 5100 (51 %) | Simple majority — use sparingly |

---

## Emergency Proposal

When a vulnerability or critical issue requires fast action, use an **emergency** proposal with a short ETA.

```yaml
kind: RotateAdmin
target: GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX  # new emergency admin
target2: ~
target3: 0
quorum_bps: 7500      # super-majority even for emergencies
eta: <now + 1 hour in ledger seconds>
```

**Emergency considerations:**

1. **Short ETA increases risk**: An attacker who compromises one guardian has a narrow window to force a malicious proposal. Use a **higher quorum** (7500+) to mitigate.
2. **Off-chain coordination is critical**: Notify all guardians out-of-band (Signal, Telegram, PagerDuty) before submitting.
3. **Follow-up proposal required**: After the emergency is contained, submit a separate proposal to restore normal admin/fee configuration.
4. **No emergency-stop bypass**: The `emergency_stop` mechanism remains independent and can freeze all operations regardless of governance state. See [`docs/emergency_stop.md`](../emergency_stop.md).

---

## Rejected Proposal Cleanup

If a proposal fails to reach quorum before its ETA:

1. **Votes are locked** — guardians cannot vote after ETA (the contract rejects votes with `vote_locked` event).
2. **Admin must cancel** — call `cancel_proposal(proposal_id, reason)` to mark the proposal as `executed = true` (cancellation reuses the `executed` flag; the proposal cannot be revived).
3. **Resubmit with adjusted parameters** — lower the quorum, extend the ETA window, or coordinate off-chain with guardians.

```rust
// Admin cleans up a stale proposal:
client.cancel_proposal(
    proposal_id,
    String::from_str(&env, "Did not reach quorum; resubmitting with corrected parameters"),
)?;
```

**Partial-quorum edge case:**

If some guardians voted yes but quorum was not met, their votes are **public on-chain** via `ProposalVotedEvent` even though the proposal was never executed. This is useful for governance transparency but means those guardians' positions are permanently recorded.

---

## Guardian Management

Guardians are added or removed by the admin via `add_guardian()` and `remove_guardian()` — **not via governance proposals**.

```rust
// Admin adds a guardian with weight 100:
client.add_guardian(admin, guardian_address, 100)?;

// Admin removes a guardian:
client.remove_guardian(admin, guardian_address)?;
```

**Important:** Removing a guardian retroactively invalidates their votes on pending proposals. The quorum calculation at `execute_proposal()` time uses **current** guardian weights, not the weights at vote time. See [`docs/governance_proposals.md`](../governance_proposals.md) for attack analysis.

---

## Cross-links

| Document | Description |
|----------|-------------|
| [`docs/governance_proposals.md`](../governance_proposals.md) | Full implementation guide, storage layout, events |
| [`docs/admin_authorization_matrix.md`](../admin_authorization_matrix.md) | Which operations are governance-gated |
| [`docs/emergency_stop.md`](../emergency_stop.md) | Emergency pause mechanism (independent of governance) |
| [`docs/timelock_runbook.md`](../timelock_runbook.md) | Timelock mechanics, vote-locking, and ETA validation |
| [`docs/protocol_invariants.md`](../protocol_invariants.md) | System-wide invariants affected by governance actions |
| [`docs/reentrancy_hardening.md`](../reentrancy_hardening.md) | Reentrancy guard interactions with governance |
| `contracts/subscription_vault/src/governance.rs` | Reference source |
| `contracts/subscription_vault/src/types.rs` (`Proposal`, `ProposalKind`) | Type definitions |

---

## Quick Reference: Common Errors

| Error | Likely Cause | Fix |
|-------|-------------|-----|
| `InvalidInput` (`quorum_bps > 10000`) | Quorum exceeds 100 % | Set `quorum_bps ≤ 10000` |
| `InvalidInput` (`eta ≤ now`) | Timelock in the past | Set `eta` to a future timestamp |
| `InvalidInput` (proposal executed) | Already executed or cancelled | Check `get_proposal(id)` — submit a new proposal |
| `Unauthorized` | Caller is not a guardian | Verify weight with `get_guardian_weight()` |
| `InvalidInput` (votes locked) | ETA has passed; votes cannot be changed | Wait for execution or ask admin to cancel |
| `UpgradeContract` | That proposal kind is reserved | Use `RotateAdmin` or `SetProtocolFee` |
