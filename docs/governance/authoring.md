# Governance Proposal Authoring Guide

> **Audience:** First-time contributors and maintainers who need to draft, submit, or review a Stellabill governance proposal.
>
> **Source of truth:** `contracts/subscription_vault/src/governance.rs`. This guide describes the implemented behaviour only — nothing in this document invents functionality that does not exist in the contract.

Stellabill gates the most privileged operations (admin rotation, protocol fee / treasury changes) behind **quorum-based governance**. A change is no longer applied by a single admin signature; it must be drafted as a **proposal**, voted on by **guardians**, and executed only after a **timelock** (the proposal's `eta`) has elapsed.

This guide walks you through drafting such a proposal correctly the first time.

---

## Table of Contents

1. [How Governance Works](#1-how-governance-works)
2. [Proposal Lifecycle](#2-proposal-lifecycle)
3. [Core Proposal Fields](#3-core-proposal-fields)
4. [Authoring Workflow (Step by Step)](#4-authoring-workflow-step-by-step)
5. [YAML Frontmatter Template](#5-yaml-frontmatter-template)
6. [Proposal Type: Admin Rotation](#6-proposal-type-admin-rotation)
7. [Proposal Type: Fee Update](#7-proposal-type-fee-update)
8. [Proposal Type: Treasury Change](#8-proposal-type-treasury-change)
9. [Voting Guidance](#9-voting-guidance)
10. [Edge Cases & Best Practices](#10-edge-cases--best-practices)
11. [Security Notes & Pre-Submission Checklist](#11-security-notes--pre-submission-checklist)
12. [Troubleshooting](#12-troubleshooting)
13. [FAQ](#13-faq)
14. [Cross-References](#14-cross-references)

---

## 1. How Governance Works

Governance is implemented in [`governance.rs`](../../contracts/subscription_vault/src/governance.rs) and exposed through six entrypoints in `lib.rs`. Every privileged change you author will end up as a call to `submit_proposal`.

### 1.1 The actors

| Role | Who | Capabilities |
|------|-----|--------------|
| **Author** | Any address | Submits proposals. `submit_proposal` performs **no authentication** — anyone may submit. |
| **Guardian** | Address with non-zero weight in the guardians map | Provides the voting weight that counts toward quorum. |
| **Admin** | Stored `DataKey::Admin` | Adds/removes guardians, cancels proposals, and (in the current implementation) is the only actor able to invoke `vote_proposal`. |
| **Executor** | Any address | Calls `execute_proposal` once ETA has passed and quorum is met. No auth required. |

> **Implementation note — who can vote.** In the current code, `vote_proposal` authenticates the caller against the stored admin (`require_stored_admin_auth`) and then checks that this same address has guardian weight. In practice this means **only the stored admin can cast a vote**, and only if the admin address also has non-zero guardian weight. Non-admin guardians (addresses with weight that are not the admin) cannot call `vote_proposal`. Account for this when choosing weights and when planning who will vote.

### 1.2 The operations

| Entrypoint | Signature | Auth | Emits |
|-----------|-----------|------|-------|
| `submit_proposal` | `(kind, target, target2, target3, quorum_bps, eta) -> u64` | none | `ProposalSubmittedEvent` |
| `vote_proposal` | `(proposal_id, voted_yes)` | stored admin + guardian weight | `ProposalVotedEvent` |
| `execute_proposal` | `(proposal_id)` | none | `ProposalExecutedEvent` |
| `cancel_proposal` | `(proposal_id, reason)` | stored admin | `ProposalCancelledEvent` |
| `add_guardian` | `(admin, guardian, weight)` | admin | — |
| `remove_guardian` | `(admin, guardian)` | admin | — |

Guardians are managed by the admin directly through `add_guardian` / `remove_guardian` — **not** through governance proposals. Removing a guardian sets their weight to `0` and retroactively invalidates their votes: quorum is re-calculated at execution time using only *current* guardians.

---

## 2. Proposal Lifecycle

```
DRAFT ──► SUBMITTED ──► VOTING WINDOW ──► ETA REACHED ──► EXECUTED
             │           (now < eta)       (votes locked)   ▲
             │              │                                 │
             │              └── votes insufficient ──────────┤
             │                                                 │
             └───────── CANCELLED (admin, any time            │
                                before execution) ────────────┘
```

| Phase | What happens | Who acts |
|-------|--------------|----------|
| **Draft** | Author writes the proposal metadata (kind, targets, quorum, ETA) off-chain. | Author |
| **Submit** | `submit_proposal` stores a `Proposal` and returns its ID. `eta` must be in the future and `quorum_bps ≤ 10 000`. | Anyone |
| **Vote** | Guardians cast yes/no votes. Votes may be **changed** while `now < eta`. | Admin (see note in §1.1) |
| **ETA reached** | Votes are **locked** — no further votes are accepted (`vote_locked` event). Execution becomes possible. | automatic |
| **Execute** | `execute_proposal` re-checks quorum against current guardians, applies the action atomically, and marks the proposal executed. | Anyone |
| **Cancel** | `cancel_proposal` marks the proposal as executed (cancellation reuses the `executed` flag). Works before **and** after ETA, but never after execution. | Admin |

There is **no automatic expiry**: a proposal that reaches ETA without quorum stays pending forever unless the admin cancels it. See [Rejected Proposal Cleanup](#102-rejected-proposal-cleanup).

---

## 3. Core Proposal Fields

The on-chain `Proposal` record (`types.rs`) carries exactly these fields — your YAML frontmatter must map onto them:

| Field | Type | Meaning |
|-------|------|---------|
| `id` | `u64` | Monotonic proposal ID, allocated at submission. |
| `kind` | `ProposalKind` | `RotateAdmin` (0), `SetProtocolFee` (1), or `UpgradeContract` (2, reserved). |
| `target` | `Address` | Primary target. New admin for `RotateAdmin`; **ignored** for `SetProtocolFee` (still required as an argument). |
| `target2` | `Option<Address>` | Secondary target. New treasury for `SetProtocolFee`; `None` leaves the treasury unchanged. Unused for `RotateAdmin`. |
| `target3` | `u32` | Tertiary parameter. Fee in basis points for `SetProtocolFee`; `0` otherwise. |
| `quorum_bps` | `u32` | Required approval, in basis points of *total current guardian weight* (0–10 000). |
| `votes` | `Map<Address, bool>` | Per-guardian vote tally. |
| `eta` | `u64` | Ledger timestamp (epoch seconds) at/after which the proposal may execute. **This is the timelock.** |
| `submitted_at` | `u64` | Submission timestamp. |
| `executed` | `bool` | Set by execution *and* cancellation; blocks re-execution/re-cancellation. |

### 3.1 Mapping proposal topics to `ProposalKind`

The task a contributor usually describes ("treasury change") does not always match a `ProposalKind` 1:1:

| Topic | `ProposalKind` | How it is expressed |
|-------|----------------|---------------------|
| Admin rotation | `RotateAdmin` | `target` = new admin. |
| Fee update | `SetProtocolFee` | `target3` = new fee bps; `target2` = `None`. |
| Treasury change | `SetProtocolFee` | `target2` = new treasury; `target3` = **current** fee bps (to keep the fee unchanged). |
| Fee **and** treasury together | `SetProtocolFee` | `target3` = new fee bps **and** `target2` = new treasury (the only multi-part action available). |
| Contract upgrade | `UpgradeContract` | **Reserved.** Execution always fails with `InvalidInput`. Do not author proposals of this kind. |

There is **no** separate `TreasuryChange` or `MultiAction` kind. See [§8](#8-proposal-type-treasury-change) and [§10.1](#101-multi-action-proposals).

---

## 4. Authoring Workflow (Step by Step)

1. **Identify the action.** Confirm the change really needs governance. Refer to the [Admin Authorization Matrix](../admin_authorization_matrix.md) to see which operations are privileged.
2. **Check the current configuration.** Read `get_protocol_fee_bps`, `get_treasury`, and `get_admin` so your proposal preserves everything it is not meant to change (especially for treasury-only changes, §8).
3. **Choose a `ProposalKind`** and fill in the [YAML frontmatter](#5-yaml-frontmatter-template).
4. **Choose quorum.** Decide `quorum_bps` for the change (see [Voting guidance](#9-voting-guidance)). Use a higher quorum for higher-risk changes.
5. **Choose the ETA (timelock).** Pick a `eta` far enough in the future for guardians and stakeholders to review. It must be **strictly greater** than the current ledger timestamp.
6. **Peer review the draft.** At minimum one other guardian/contributor must verify every address, fee value, and the rationale (§11).
7. **Submit.** Call `submit_proposal`. Anyone can submit, but the author is usually the admin or a guardian. Record the returned `proposal_id`.
8. **Vote.** Have the admin (as a guardian) cast votes. Voting must finish **before** ETA — votes are locked afterwards.
9. **Monitor.** Track `ProposalVotedEvent`s; confirm quorum will be met before ETA. If not, arrange more votes or cancel (§10.2).
10. **Execute.** After ETA, anyone may call `execute_proposal`. Because execution is permissionless, the proposal is expected to execute as soon as ETA passes if quorum is met.
11. **Verify.** Read `get_proposal(id)` (now `executed == true`) and confirm the applied config (`get_admin` / `get_protocol_fee_bps` / `get_treasury`).

---

## 5. YAML Frontmatter Template

Every proposal should be drafted as a Markdown document with YAML frontmatter. The frontmatter is a **drafting aid** — the contract only consumes the six `submit_proposal` arguments, but the template below keeps your metadata complete and maps 1:1 onto the call.

```yaml
---
title: "<Short, imperative title of the change>"
summary: "<One or two sentences describing the on-chain change.>"
proposal_kind: "<RotateAdmin | SetProtocolFee>"   # UpgradeContract is reserved
rationale: |
  Why this change is needed, who approved the off-chain decision,
  and what happens if the proposal is rejected or lapses.
actions:
  - type: "<same as proposal_kind>"
    target: "<Address>"        # new admin for RotateAdmin; ignored for SetProtocolFee
    target2: "<Address | null>" # new treasury for SetProtocolFee; null = unchanged
    target3: "<0..10000>"       # fee bps for SetProtocolFee; 0 otherwise
voting_window:
  quorum_bps: <0-10000>        # % of total guardian weight required
  eta: <u64 epoch seconds>     # must be > now; this is the timelock
author: "<github-username or key label>"
created_at: "<YYYY-MM-DD>"
---

# Maps 1:1 to:
# submit_proposal(proposal_kind,
#                 actions[0].target,
#                 actions[0].target2,
#                 actions[0].target3,
#                 voting_window.quorum_bps,
#                 voting_window.eta)
```

### Frontmatter → on-chain mapping

| YAML field | `submit_proposal` argument | Notes |
|------------|----------------------------|-------|
| `proposal_kind` | `kind` | Must be a valid `ProposalKind`. |
| `actions[0].target` | `target` | Required `Address`; ignored for `SetProtocolFee`. |
| `actions[0].target2` | `target2` | `null` → `None`. |
| `actions[0].target3` | `target3` | `u32`. |
| `voting_window.quorum_bps` | `quorum_bps` | Rejected if `> 10 000`. |
| `voting_window.eta` | `eta` | Rejected if `≤ now`. |

`title`, `summary`, `rationale`, `author`, and `created_at` are for human review only — they are **not** stored on-chain. The on-chain audit trail is the proposal record plus its events.

---

## 6. Proposal Type: Admin Rotation

### Purpose

Transfer the contract admin key to a new address. On execution, `DataKey::Admin` is overwritten with `target` and the previous admin loses all privileges immediately.

> **Prefer the two-step runbook for planned transfers.** The contract also ships a two-step `propose_admin` → `claim_admin_role` flow with a 7-day cancellable window. Governance proposals are an *alternative* rotation mechanism — see the [Admin Rotation Runbook](../runbooks/admin_rotation.md) to decide which to use.

### When to use it

- The current admin key is compromised or lost and must be replaced.
- A deliberate governance-driven handover (e.g. to a multisig or governance contract).
- Any rotation that guardians should collectively approve.

### Required fields

| Field | Value |
|-------|-------|
| `kind` | `RotateAdmin` |
| `target` | New admin `Address` |
| `target2` | `None` |
| `target3` | `0` |
| `quorum_bps` | ≥ 6700 for a high-risk handover (see §9.3) |
| `eta` | `now + ≥ 7 days` recommended |

### Example proposal

```yaml
---
title: "Rotate contract admin to the new operations multisig"
summary: "Replace the current admin with the 2-of-3 operations multisig approved by guardians."
proposal_kind: RotateAdmin
rationale: |
  The incumbent admin key has been in service since launch. Guardians approved
  migrating to a 2-of-3 multisig at the monthly ops review. If rejected, the
  current admin stays in place and the migration is rescheduled.
actions:
  - type: RotateAdmin
    target: GABCDE234567890ABCDE234567890ABCDE234567890ABCDE234567890
    target2: null
    target3: 0
voting_window:
  quorum_bps: 6700      # 67% of total guardian weight
  eta: 1893456000       # now + 7 days (7 × 86400)
author: "ops-alice"
created_at: "2026-07-31"
---
```

**Contract call (equivalent):**

```rust
let proposal_id = client.submit_proposal(
    ProposalKind::RotateAdmin,
    new_admin_address,   // target
    None,                // target2
    0,                   // target3
    6700,                // quorum_bps (67%)
    eta,                 // epoch seconds ≥ now + 7 days
)?;
```

**Execution effect:** `DataKey::Admin = target`. The old admin immediately loses all privileges. The guardian roster is **not** touched — the new admin inherits it (and must have guardian weight to vote).

### Common mistakes

- Setting `target` to the **contract address** — governance execution does *not* run the `InvalidNewAdmin` guard that the direct `rotate_admin` path enforces, so the contract would happily lock the admin role forever. Always verify the target.
- Setting `target` to the **current admin** — a no-op that wastes a proposal (the direct path rejects this with `SelfRotation`; governance does not).
- Using a low `quorum_bps` for a change that permanently transfers control.
- Rotating **during a pending proposal you cannot afford to lose**: if the rotation executes before another proposal's ETA, votes for the old admin's guardian weight are re-evaluated against current guardians at the other proposal's execution time.

### Review checklist

- [ ] `target` is a valid `G...` address, is **not** the contract address, and is **not** the current admin.
- [ ] The new admin's key custody was verified out-of-band.
- [ ] The new admin has non-zero guardian weight (or the roster is updated separately) so governance remains operable.
- [ ] `quorum_bps` and `eta` are set for a high-risk change.
- [ ] Stakeholders were notified of the rotation window.

---

## 7. Proposal Type: Fee Update

### Purpose

Change the protocol fee rate charged on every successful charge. On execution, `DataKey::FeeBps = target3` and — only if `target2` is set — `DataKey::Treasury = target2`.

### When to use it

- Raising, lowering, or disabling the protocol fee (`0` disables fee collection).
- Combined with a treasury change in the same proposal (see §8).

### Required fields

| Field | Value |
|-------|-------|
| `kind` | `SetProtocolFee` |
| `target` | Any valid `Address` (required but **ignored** on execution) |
| `target2` | `None` to leave the treasury unchanged |
| `target3` | Fee bps, `0..=10 000` (`250` = 2.50 %) |
| `quorum_bps` | ≥ 6700 for material fee changes |
| `eta` | `now + ≥ 3 days` recommended |

### Example proposal

```yaml
---
title: "Raise protocol fee from 2.50% to 3.00%"
summary: "Set the protocol fee to 300 bps; treasury unchanged."
proposal_kind: SetProtocolFee
rationale: |
  Fee revenue no longer covers infrastructure costs. The 50 bps increase was
  socialised with merchants two weeks ago. If rejected, the current 250 bps
  rate continues.
actions:
  - type: SetProtocolFee
    target: GBAAAA1111111111111111111111111111111111111111111111111111 # ignored
    target2: null              # treasury unchanged
    target3: 300               # 3.00%
voting_window:
  quorum_bps: 6700
  eta: 1893370200              # now + 3 days
author: "ops-bob"
created_at: "2026-07-28"
---
```

**Contract call (equivalent):**

```rust
let proposal_id = client.submit_proposal(
    ProposalKind::SetProtocolFee,
    current_admin,        // target — ignored for SetProtocolFee
    None,                 // target2 — None keeps the existing treasury
    300,                  // target3 — 3.00%
    6700,
    eta,
)?;
```

**Execution effect:** `DataKey::FeeBps = 300`. Because `target2` is `None`, the treasury is untouched.

### Common mistakes

- **Passing a fee value `> 10 000`.** Unlike the direct `set_protocol_fee` path (which validates `fee_bps ≤ 10 000`), governance execution writes `target3` **without validation**. A value such as `50 000` makes `fee = gross × 50 000 / 10 000` exceed the charge amount. Validate your own value.
- Assuming `target` matters for a fee proposal — it is ignored. Don't point it at a treasury and expect it to be used; the treasury lives in `target2`.
- Changing the fee when you only meant to change the treasury (see §8) — always confirm `target3` equals the **current** fee in that case.
- Setting a too-short ETA so merchants/stakeholders cannot react.

### Review checklist

- [ ] `target3` is in `0..=10 000` and matches the intended rate.
- [ ] `target2` is `null` if the treasury should stay unchanged, or the correct new address otherwise.
- [ ] The current fee was read (`get_protocol_fee_bps`) so a treasury-only change doesn't accidentally change the rate.
- [ ] `eta` gives affected merchants time to object.
- [ ] The fee math is correct: `fee = gross × fee_bps / 10 000` (see [Protocol Fees](../protocol_fees.md)).

---

## 8. Proposal Type: Treasury Change

### Purpose

Redirect where protocol fees are credited. Because there is **no dedicated treasury-change kind**, a treasury change is expressed as `SetProtocolFee` with `target2 = Some(new_treasury)` and `target3` set to the **current** fee bps (so the rate itself doesn't move).

### When to use it

- Changing the fee-collection address (e.g. migrating to a new operations wallet).
- Fixing a misconfigured treasury.

### Required fields

| Field | Value |
|-------|-------|
| `kind` | `SetProtocolFee` |
| `target` | Any valid `Address` (required but **ignored**) |
| `target2` | `Some(new_treasury)` — the new treasury |
| `target3` | **Current** fee bps (from `get_protocol_fee_bps`), to preserve the rate |
| `quorum_bps` | ≥ 6700 |
| `eta` | `now + ≥ 3 days` recommended |

### Example proposal

Assume the current fee is `250` bps (2.50 %) and it must not change:

```yaml
---
title: "Move protocol fee treasury to the new ops wallet"
summary: "Route protocol fees to the new operations wallet; fee rate stays 2.50%."
proposal_kind: SetProtocolFee
rationale: |
  The old treasury key is being retired. The destination was verified out-of-band.
  If rejected, fees keep accruing to the current treasury.
actions:
  - type: SetProtocolFee
    target: GBAAAA1111111111111111111111111111111111111111111111111111 # ignored
    target2: GNEWTREASURY999999999999999999999999999999999999999999999 # new treasury
    target3: 250               # current fee preserved
voting_window:
  quorum_bps: 6700
  eta: 1893370200
author: "ops-carla"
created_at: "2026-07-29"
---
```

**Contract call (equivalent):**

```rust
let proposal_id = client.submit_proposal(
    ProposalKind::SetProtocolFee,
    current_admin,                 // target — ignored
    Some(new_treasury_address),    // target2 — new treasury
    250,                           // target3 — CURRENT fee, preserved
    6700,
    eta,
)?;
```

**Execution effect:** `DataKey::Treasury = new_treasury_address`, `DataKey::FeeBps = 250` (unchanged).

> To change **both** fee and treasury atomically, set `target3` to the new fee and `target2` to the new treasury in the same proposal — this is the only multi-part action the framework supports.

### Common mistakes

- **Forgetting `target3`.** A treasury proposal that passes `target3 = 0` disables the protocol fee entirely (surprise!). Always set `target3` to the current fee unless a rate change is intended.
- **Setting the treasury to the contract address.** Governance execution does *not* run the `reject_contract_self` guard that the direct `set_protocol_fee` path enforces — fees would accrue to the contract and become hard to recover. Verify the destination.
- Pointing `target` (not `target2`) at the treasury — `target` is ignored for `SetProtocolFee`; the treasury must be in `target2`.

### Review checklist

- [ ] `target2` is the verified new treasury, not the contract address, and not a typo of the old one.
- [ ] `target3` equals the current fee (or the deliberate new rate).
- [ ] `get_treasury()` was read first to confirm what will change.
- [ ] Fee-collection docs updated alongside if this is a policy change (see [Fee Routing](../fee_routing.md)).

---

## 9. Voting Guidance

### 9.1 Voting window expectations

The **voting window is entirely defined by `eta`** — there is no fixed constant in the contract. Voting is only possible while `now < eta`; at `now >= eta` votes are locked (see §9.5). Choose `eta` so the window is long enough for guardians to verify the proposal off-chain.

| Scenario | Recommended `eta` | Rationale |
|----------|-------------------|-----------|
| Admin rotation | `now + ≥ 7 days` (604 800 s) | Guardians verify the new key out-of-band. |
| Fee / treasury change | `now + ≥ 3 days` (259 200 s) | Merchants affected by the fee should have time to react. |
| Emergency | `now + ≥ 1 hour` (3 600 s) | Faster action — with a *higher* quorum to compensate (§10.3). |

`eta` is a ledger timestamp in epoch seconds, and must be **strictly greater** than the current ledger time at submission — `submit_proposal` rejects `eta <= now` with `InvalidInput`.

### 9.2 Proposal lifecycle recap

```
submitted_at ────────────────► eta ─────────────────────────►
   │  VOTING (votes can change) │  VOTES LOCKED               │
   │                            │  (no more votes accepted)   │
   └── cancel possible ─────────┴── cancel still possible ────┘
                                    execute possible if quorum
```

### 9.3 Quorum / approval requirements

Quorum is a percentage of **total current guardian weight**, expressed in basis points, and is re-checked at execution time:

```
required_votes = total_guardian_weight × quorum_bps / 10 000   (integer floor)
```

Execution succeeds only if `votes_for >= required_votes`, where `votes_for` counts **currently-registered guardians only**. Votes from removed guardians are dropped.

| Guardian weights | Total | `quorum_bps` | Required | Passes |
|------------------|-------|--------------|----------|--------|
| 100, 100, 100 | 300 | 5000 (50 %) | 150 | any two (200) |
| 100, 100, 100 | 300 | 6700 (67 %) | 201 | all three (300) — two (200) is **not** enough |
| 200 × 5 | 1000 | 6700 (67 %) | 670 | four guardians (800) |
| 100, 100, 100 | 300 | 7500 (75 %) | 225 | all three (300) |
| 100, 100, 100 | 300 | 10000 (100 %) | 300 | all three (300) |

> Because of floor division, `6700` with three equal guardians requires **all three** — two out of three (66.7 %) is below 67 %. Budget for this when setting quorum and weights.

Recommended `quorum_bps`: **6700** default, **7500–10000** for high-risk changes (admin rotation, treasury moves, large fee swings), **5100+** absolute floor for simple majority.

### 9.4 Execution timing

- `execute_proposal` requires `now >= eta` and quorum met. It requires **no authentication** — any address can trigger it.
- In practice, a proposal that has quorum is expected to execute **immediately when ETA arrives**, because anyone can call it. Plan for that: once ETA is reached with quorum, there is no way to stop execution (see §9.5).
- A failed execution (quorum not met) reverts cleanly and leaves the proposal pending for a later retry or cancellation — there is no penalty and no state change.

### 9.5 Timelock interaction

- The `eta` is the proposal's **timelock**: it is the earliest execution time and the moment votes lock. There is no separate "timelock contract" involved in the governance path.
- **Vote locking:** once `now >= eta`, `vote_proposal` fails with `InvalidInput` and emits `VoteLockedEvent`. This prevents a guardian from feigning support during the window and then flipping their vote to grief execution.
- **Bypass of other delays:** a governance proposal that executes writes `DataKey::Admin` / `FeeBps` / `Treasury` directly via `write_config`. It intentionally **bypasses**:
  - the 6-hour per-key admin-config cooldown (`enforce_config_cooldown`), and
  - the 48-hour `queue_treasury_change` timelock that the direct `set_protocol_fee` path requires.
  In other words, once a `SetProtocolFee` proposal executes, the fee/treasury change is effective **immediately**, not 48 hours later. See [`admin.rs`](../../contracts/subscription_vault/src/admin.rs) and the [Timelocked Treasury Change](../timelocked_treasury_change.md) document for the non-governance path.
- **Cancellation boundary:** the admin can cancel a proposal at any time **before execution** — even after ETA has passed. After execution there is no undo; the only remedy is a new proposal reversing the change.
- **Emergency stop:** the governance entrypoints are **not** gated by the emergency-stop circuit breaker (`require_not_emergency_stop` is not called on them). Governance continues to function during an emergency stop, consistent with admin rotation being a recovery path. See [Emergency Stop](../emergency_stop.md).

---

## 10. Edge Cases & Best Practices

### 10.1 Multi-action proposals

Each proposal is **single-action**. To change the admin *and* the fee, you must submit separate proposals and coordinate their ETA windows. Only `SetProtocolFee` can bundle two fields (fee + treasury) in one action.

```yaml
# ── Proposal A: rotate admin ──
proposal_kind: RotateAdmin
actions:
  - type: RotateAdmin
    target: <new-admin>
voting_window: { quorum_bps: 7500, eta: <now + 7 days> }

# ── Proposal B: update fee (same window) ──
proposal_kind: SetProtocolFee
actions:
  - type: SetProtocolFee
    target: <any-address>
    target2: null
    target3: 150
voting_window: { quorum_bps: 6700, eta: <now + 7 days> }
```

Guardians must approve **both** before the shared ETA closes. If one fails quorum, cancel and resubmit it independently — the other proposal can still proceed.

> **Future work:** a `MultiAction` proposal kind is not implemented. Do not draft one.

### 10.2 Rejected proposal cleanup

A proposal that reaches ETA without quorum does **not** expire on-chain. It stays pending, votes are locked, and no one can execute it.

1. **Cancel it.** Only the admin can: `cancel_proposal(proposal_id, reason)`. This sets `executed = true` (the same flag used by execution) and emits `ProposalCancelledEvent`. The proposal cannot be revived or re-cancelled.
2. **Resubmit** with adjusted parameters — lower `quorum_bps`, extend `eta`, or coordinate more votes off-chain.
3. **Know the trail is permanent.** `ProposalVotedEvent` records every vote on-chain, including votes on proposals that never executed.

```rust
client.cancel_proposal(
    proposal_id,
    String::from_str(&env, "Did not reach quorum; resubmitting with corrected parameters"),
)?;
```

### 10.3 Emergency proposals

There is no separate "emergency" kind — an emergency proposal is simply one with a short `eta`. Because a short window shrinks the review time:

1. **Raise quorum** for short-ETA proposals (≥ 7500) so a single compromised key cannot push a change through quickly.
2. **Coordinate out-of-band** (Signal/Telegram/PagerDuty) *before* submitting so guardians vote promptly.
3. **Plan a follow-up proposal** to restore normal configuration after the incident is contained.
4. Remember the **emergency stop is a separate lever** — it freezes critical operations without governance, and governance is not blocked by it.

### 10.4 Editing vs creating a new proposal

- **Before submission, edit freely** — the YAML is just a draft.
- **After submission, proposals are immutable.** There is no amendment function. To change any parameter you must cancel the live proposal (admin) and submit a new one.
- **Changing votes** is allowed while `now < eta` (a guardian's vote is overwritten); it is locked afterwards. So a "late change" must happen before ETA, and correcting the proposal itself requires cancel + resubmit.
- **After execution**, the only correction is a brand-new proposal that reverses the change (e.g. rotate the admin back, or set the fee back to the old value).

### 10.5 Guardian roster changes mid-proposal

- Removing a guardian **immediately** zeroes their weight and drops their votes from quorum calculations at execution time. A proposal that looked approved can suddenly fail quorum.
- Adding or updating guardians changes `total_guardian_weight`, which re-scales `required_votes` for **all pending proposals** (the requirement is recomputed from the live roster at each execution). Be aware that an admin adding a heavy new guardian can *lower* the effective quorum bar for a pending proposal.
- If the admin is rotated, the new admin can only vote if they have guardian weight; pending proposals are unaffected otherwise.

### 10.6 Best practices

- **One intent per proposal** — makes quorum review tractable and audit trails clear.
- **Prefer longer ETAs** over shorter ones; the cost of waiting is low, the cost of a wrong execution is not.
- **Publish the draft** (with rationale) where guardians can comment *before* submission.
- **Keep `get_proposal(id)` handy** to inspect exactly what was submitted (the `ProposalSubmittedEvent` does not carry `target2`/`target3`).
- **Document reversibility** in the rationale: how to undo this change if it goes wrong.

---

## 11. Security Notes & Pre-Submission Checklist

Review every item before submitting. These are the highest-frequency failure modes in governance proposals.

- [ ] **Verify all addresses.** Check the checksum, and that no address is the **contract address** — governance execution does *not* run the guards (`InvalidNewAdmin`, `SelfRotation`, `reject_contract_self`) that the direct admin entrypoints enforce. Rotating admin to the contract locks the role forever; setting the treasury to the contract strands fees.
- [ ] **Review treasury destinations carefully.** `target2` is where protocol fees land. Confirm it is the intended wallet, out-of-band verified.
- [ ] **Validate fee values.** `target3` must be `0..=10 000`. The governance path performs **no** fee validation — a value like `50 000` would compute a fee larger than the gross charge. Confirm against `get_protocol_fee_bps()`.
- [ ] **Avoid unnecessary privilege changes.** Don't bundle an admin rotation with a fee tweak just because you can; every privilege transfer is a permanent, high-value change.
- [ ] **Review timelock implications.** Once ETA passes with quorum, execution is permissionless and unstoppable. Make sure the ETA gives enough time to cancel if something looks wrong. Remember governance bypasses the 48-hour treasury timelock and the 6-hour cooldown.
- [ ] **Have proposals peer-reviewed before submission.** A second person should independently re-derive every address, the fee, the quorum math, and the ETA. Use the per-type review checklists in §6–§8.
- [ ] **Check the guardian roster** (`list_guardians`) before choosing quorum, so `required_votes` matches your expectation.
- [ ] **Confirm who can vote.** Because `vote_proposal` is admin-gated in the current implementation, ensure the admin address has guardian weight, or arrange governance to work through it (see §1.1).

---

## 12. Troubleshooting

| Error | Likely cause | Fix |
|-------|--------------|-----|
| `InvalidInput` (`quorum_bps > 10000`) | Quorum exceeds 100 % | Set `quorum_bps ≤ 10 000`. |
| `InvalidInput` (`eta ≤ now`) | Timelock in the past | Set `eta` to a future ledger timestamp. |
| `InvalidInput` (proposal executed) | Proposal already executed *or* already cancelled | Check `get_proposal(id)`; submit a new proposal. |
| `Unauthorized` on `vote_proposal` | Caller is not the stored admin, or admin has no guardian weight | Have the admin (with weight) cast the vote. |
| `InvalidInput` + `vote_locked` event | ETA reached; votes are locked | Wait for execution, or ask the admin to cancel. |
| `InvalidInput` on `execute_proposal` (before ETA) | ETA not reached yet | Wait until `now >= eta`. |
| `InvalidInput` on `execute_proposal` (after ETA) | Quorum not met (`votes_for < required`) | Gather more yes votes (impossible after ETA) or cancel and resubmit. |
| `InvalidInput` on `execute_proposal` (`UpgradeContract`) | Kind is reserved and always fails | Use `RotateAdmin` or `SetProtocolFee`. |
| `Unauthorized` on `cancel_proposal` | Caller is not the stored admin | The admin must cancel. |
| `InvalidInput` / `Forbidden` on `add_guardian` | Not admin, or weight is `0` | Admin only; weight must be `> 0`. |

---

## 13. FAQ

**Q: Who can submit a proposal?**
A: Anyone. `submit_proposal` performs no authentication. The returned `proposal_id` is the reference for all later operations.

**Q: Who can vote?**
A: In the current implementation, `vote_proposal` requires the stored admin's authentication and that the admin has non-zero guardian weight. Non-admin guardians cannot call it (see §1.1).

**Q: Can a vote be changed?**
A: Yes, while `now < eta`. After ETA, votes are locked and `vote_proposal` fails with `vote_locked`.

**Q: Can a proposal be edited after submission?**
A: No. Proposals are immutable. To change anything, the admin must cancel and you must resubmit (§10.4).

**Q: Is there a maximum voting window?**
A: No. `eta` can be arbitrarily far in the future (it only must exceed `now`). Long windows are safe; short windows increase risk.

**Q: What happens to a proposal nobody executes?**
A: Nothing automatic — it stays pending. After ETA the votes are locked, so if quorum was met, anyone can still execute it later; if not, only the admin can clean it up by cancelling (§10.2).

**Q: Does governance respect the 48-hour treasury timelock?**
A: No. Governance execution writes the fee/treasury directly, so the change is effective immediately at execution time. Only the direct admin path (`queue_treasury_change` / `set_protocol_fee`) enforces the 48-hour delay (§9.5).

**Q: Does the emergency stop block governance?**
A: No. Governance entrypoints are not gated by the emergency stop (§9.5). This is intentional — rotation stays available as a recovery path.

**Q: Is `UpgradeContract` usable?**
A: No. The kind is reserved; execution always fails. Do not author proposals of this kind.

**Q: Where can I see what was actually submitted?**
A: `get_proposal(proposal_id)`. Note that `ProposalSubmittedEvent` omits `target2`/`target3`, so indexers and reviewers must read the proposal record for fee/treasury values.

---

## 14. Cross-References

| Document | Why it matters |
|----------|----------------|
| [`docs/governance_proposals.md`](../governance_proposals.md) | Full implementation guide: storage layout, events, security analysis. |
| [`docs/runbooks/admin_rotation.md`](../runbooks/admin_rotation.md) | Operational runbook for the two-step `propose_admin` → `claim_admin_role` flow (the planned-transfer path). |
| [`docs/timelocked_treasury_change.md`](../timelocked_treasury_change.md) | The 48-hour queue/execute timelock on the **non-governance** fee/treasury path. |
| [`docs/emergency_stop.md`](../emergency_stop.md) | Emergency-stop mechanism — independent of governance. |
| [`docs/admin_authorization_matrix.md`](../admin_authorization_matrix.md) | Which operations are privileged / governance-gated. |
| [`docs/protocol_fees.md`](../protocol_fees.md) | How `fee_bps` is applied on each charge. |
| [`docs/fee_routing.md`](../fee_routing.md) | Fee-routing walkthrough with numeric examples. |
| [`docs/events-schema-canonical.md`](../events-schema-canonical.md) | Canonical schema for all governance/timelock events. |
| [`docs/security.md`](../security.md) / [`SECURITY.md`](../../SECURITY.md) | Threat model and security posture (incl. governance takeover). |
| [`contracts/subscription_vault/src/governance.rs`](../../contracts/subscription_vault/src/governance.rs) | Reference implementation. |
| [`contracts/subscription_vault/src/types.rs`](../../contracts/subscription_vault/src/types.rs) | `Proposal`, `ProposalKind`, governance events, `Error` codes. |
| [`contracts/subscription_vault/src/admin.rs`](../../contracts/subscription_vault/src/admin.rs) | `write_config`, cooldown, and the non-governance fee/treasury paths. |
