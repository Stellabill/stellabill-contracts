# Mainnet Admin Rotation Playbook

> **Version:** 1.0  
> **Scope:** Production two-step admin rotation for the Stellabill Subscription Vault contract on Stellar mainnet.  
> **Last Updated:** 2026-07-30  
> **Status:** Authoritative — all planned admin transfers MUST use this flow.

---

## 1. Overview

This runbook governs the **two-step admin rotation** (`propose_admin` → `claim_admin_role`) for the Subscription Vault contract deployed on Stellar mainnet. The two-step flow replaces the older single-step `rotate_admin` for all planned transfers, providing a **7-day safety window** during which the proposal can be cancelled if the wrong address was targeted or the claimant key is compromised.

### Why Two-Step?

| Risk | Single-Step | Two-Step |
|------|-------------|----------|
| Wrong address entered | Irreversible — funds locked | Cancellable during 7-day window |
| Compromised claimant key | N/A (instant) | Attacker cannot claim unless they control the *proposed* address |
| Fat-finger / copy-paste error | Permanent | Detectable and reversible |
| Front-running | Possible | Claimant must match exact proposed address |

### State Machine

```
┌─────────────┐    propose_admin     ┌─────────────────┐
│   Admin A   │ ───────────────────► │ Active Proposal │
└─────────────┘                      │  (7-day TTL)    │
                                     └────────┬────────┘
                                              │
                    ┌─────────────────────────┼─────────────────────────┐
                    ▼                         ▼                         ▼
            ┌─────────────┐          ┌─────────────┐           ┌─────────────┐
            │ Claim by    │          │ Cancel by   │           │ Expire      │
            │ Admin B     │          │ Admin A     │           │ (7d passed) │
            │ (success)   │          │ (rollback)  │           │ (no-op)     │
            └─────────────┘          └─────────────┘           └─────────────┘
```

---

## 2. Pre-Rotation Checklist

Complete **every item** before invoking `propose_admin`. No exceptions.

### 2.1 Identity Verification

- [ ] **Confirm current admin address**: Run `get_admin` and verify against the expected address.
- [ ] **Verify current admin key custody**: Confirm you have signing access (private key, hardware wallet, or multisig threshold).
- [ ] **Validate new admin address**: 
  - [ ] Address is NOT the contract address (`InvalidNewAdmin` guard).
  - [ ] Address is NOT the current admin address (`SelfRotation` guard).
  - [ ] Address format is valid (starts with `G`, 56 chars, checksum passes).
  - [ ] New admin key is generated on a secure, offline device or HSM.
  - [ ] New admin public key has been shared via an **out-of-band verified channel** (e.g. video call, signed PGP message, not just Slack/email).

### 2.2 Operational Readiness

- [ ] **Check cooldown status**: The "Admin" config key has a 6-hour cooldown (`CONFIG_COOLDOWN_SECS = 21_600`). Verify the last admin mutation timestamp is > 6 hours ago, or prepare to wait.
- [ ] **Verify no active proposal**: Run `get_admin_proposal` — must return `None`. If a stale proposal exists, it must be cancelled first.
- [ ] **Confirm network**: RPC URL points to **mainnet** (`https://soroban-rpc.mainnet.stellar.games` or equivalent).
- [ ] **Network passphrase**: `Public Global Stellar Network ; September 2015`.
- [ ] **Gas budget**: Ensure the signing account has sufficient XLM for transaction fees.

### 2.3 Communication Plan

- [ ] **Notify stakeholders** (engineering, security, legal) 24 hours before rotation.
- [ ] **Schedule during low-activity window** (avoid billing cycles, batch charges, major releases).
- [ ] **Prepare incident channel**: Dedicated Slack/Discord channel for real-time coordination during the 7-day claim window.
- [ ] **Escalation contact**: Identify who can sign `cancel_admin_proposal` if the current admin becomes unavailable.

### 2.4 Backup & Audit

- [ ] **Export contract snapshot**: Run `export_contract_snapshot` and archive the output.
- [ ] **Document current admin nonce**: Run `get_admin_nonce(current_admin, 1)` and record the value.
- [ ] **Event monitoring**: Ensure indexer/explorer is monitoring for `admin_proposal_created`, `admin_proposal_claimed`, `admin_proposal_cancelled`.

---

## 3. Step-by-Step Execution

### Step 1: Propose New Admin

**Actor:** Current admin (Admin A)

**Action:** Call `propose_admin` with the verified new admin address.

```bash
stellar contract invoke \
  --id CONTRACT_ID \
  --source CURRENT_ADMIN_SECRET \
  --rpc-url https://soroban-rpc.mainnet.stellar.games \
  --network-passphrase "Public Global Stellar Network ; September 2015" \
  -- \
  propose_admin \
  --current_admin $(stellar address from-secret CURRENT_ADMIN_SECRET) \
  --new_admin NEW_ADMIN_ADDRESS
```

**Effects:**
- Instance storage writes an `AdminProposal` record with `proposed_at = now`, `expires_at = now + 604_800` (7 days).
- Emits `admin_proposal_created` event with `old_admin`, `new_admin`, `expires_at`, `timestamp`.
- Rejects if:
  - Caller is not the stored admin → `Unauthorized`
  - `new_admin == contract_address` → `InvalidNewAdmin`
  - A proposal already exists → `ProposalAlreadyExists`

**Verification:**

```bash
stellar contract invoke \
  --id CONTRACT_ID \
  --rpc-url https://soroban-rpc.mainnet.stellar.games \
  --network-passphrase "Public Global Stellar Network ; September 2015" \
  --source SOME_SOURCE \
  -- \
  get_admin_proposal
```

Expected (trimmed):
```json
{
  "new_admin": "GNEWADMIN...",
  "proposed_at": 1753872000,
  "expires_at": 1754476800
}
```

**Security note:** The proposal is stored in **instance storage** (not persistent). It has the same TTL as other instance data and will be cleaned up naturally if the contract instance TTL expires.

---

### Step 2: Verify Proposal (Off-Chain)

**Actor:** Both Admin A and Admin B (independently)

**Actions:**
1. **Confirm `new_admin` address** matches the out-of-band verified public key.
2. **Check `proposed_at` and `expires_at`** are reasonable (should be exactly 7 days apart).
3. **Monitor the `admin_proposal_created` event** on-chain via:
   - Stellar Expert block explorer
   - Custom indexer listening for topic `"admin_proposal_created"`
   - RPC `getEvents` query

**Red flags — cancel immediately if:**
- `new_admin` does not match the agreed address.
- `proposed_at` is in the future or far in the past (indicates clock skew or replay).
- Event was emitted by an unexpected `old_admin` (contract may have been rotated already).

---

### Step 3A: Claim (by New Admin)

**Actor:** New admin (Admin B)

**Window:** Must be called **before `expires_at`**.

**Action:** Call `claim_admin_role`.

```bash
stellar contract invoke \
  --id CONTRACT_ID \
  --source NEW_ADMIN_SECRET \
  --rpc-url https://soroban-rpc.mainnet.stellar.games \
  --network-passphrase "Public Global Stellar Network ; September 2015" \
  -- \
  claim_admin_role \
  --claimant $(stellar address from-secret NEW_ADMIN_SECRET)
```

**Effects:**
- `claimant.require_auth()` must pass.
- Proposal is validated: `now <= expires_at` and `claimant == proposal.new_admin`.
- Proposal is **removed** from storage.
- `DataKey::Admin` is **atomically updated** to the claimant.
- Emits `admin_proposal_claimed` event with `old_admin`, `new_admin`, `timestamp`.

**Rejection conditions:**
| Condition | Error | State After |
|-----------|-------|-------------|
| No proposal exists | `ProposalNotFound` | Unchanged |
| `now > expires_at` | `ProposalExpired` | Proposal **cleaned up**, admin unchanged |
| `claimant != new_admin` | `InvalidClaimant` | Unchanged |
| `claimant` fails `require_auth()` | Host-level panic | Unchanged |

**Post-Claim Verification:**

```bash
# 1. Confirm admin changed
stellar contract invoke --id CONTRACT_ID --rpc-url ... --source ... -- get_admin
# Expected: NEW_ADMIN_ADDRESS

# 2. Confirm proposal removed
stellar contract invoke --id CONTRACT_ID --rpc-url ... --source ... -- get_admin_proposal
# Expected: None

# 3. Verify new admin can operate
stellar contract invoke \
  --id CONTRACT_ID \
  --source NEW_ADMIN_SECRET \
  --rpc-url ... \
  --network-passphrase ... \
  -- \
  set_min_topup \
  --admin $(stellar address from-secret NEW_ADMIN_SECRET) \
  --min_topup 1000000
```

**Critical:** The old admin **immediately loses** all privileges. There is no grace period.

---

### Step 3B: Cancel (by Current Admin)

**Actor:** Current admin (Admin A)

**When to use:**
- Wrong address was proposed.
- New admin key is suspected compromised before claim.
- Business decision changes before claim.
- Any doubt whatsoever about the proposal.

**Action:** Call `cancel_admin_proposal`.

```bash
stellar contract invoke \
  --id CONTRACT_ID \
  --source CURRENT_ADMIN_SECRET \
  --rpc-url https://soroban-rpc.mainnet.stellar.games \
  --network-passphrase "Public Global Stellar Network ; September 2015" \
  -- \
  cancel_admin_proposal \
  --admin $(stellar address from-secret CURRENT_ADMIN_SECRET)
```

**Effects:**
- `admin.require_auth()` must pass and match stored admin.
- Proposal is **removed** from storage.
- Emits `admin_proposal_cancelled` event with `admin`, `timestamp`.
- Admin remains unchanged.
- A new proposal can be created immediately (subject to the 6-hour "Admin" cooldown).

**Rejection conditions:**
| Condition | Error |
|-----------|-------|
| No active proposal | `NoActiveProposal` |
| Caller is not stored admin | `Unauthorized` |

---

### Step 3C: Expiry (Passive)

**Actor:** None — automatic.

If the proposal is neither claimed nor cancelled after 7 days, any subsequent `claim_admin_role` call will:
1. Detect `now > expires_at`.
2. Return `ProposalExpired`.
3. **Clean up the stale proposal** from storage.

No explicit cleanup transaction is required. However, if you want to force cleanup without attempting a claim:

```bash
# Any address can trigger the cleanup by attempting a claim that will fail
stellar contract invoke \
  --id CONTRACT_ID \
  --source ANY_SOURCE \
  --rpc-url ... \
  --network-passphrase ... \
  -- \
  claim_admin_role \
  --claimant ANY_ADDRESS
# Will fail with ProposalExpired and remove the stale proposal
```

---

## 4. Rollback Path

### 4.1 Proposal Exists, Not Yet Claimed

**Path:** `cancel_admin_proposal` → Admin A remains admin → re-propose if needed.

**Timeline:** Immediate. No state change other than proposal removal.

### 4.2 Proposal Was Already Claimed

**Path:** Admin B (now the stored admin) must voluntarily create a new proposal back to Admin A or another trusted address.

**Timeline:** 7-day claim window + however long Admin B takes to act.

**Risk:** If Admin B is uncooperative or keys are lost, there is **no recovery path**. This is why:
- New admin should be a **multisig address** or **governance contract**.
- The old admin should verify Admin B's key custody before proposing.

### 4.3 Both Old and New Admin Keys Lost

**Path:** None. Contract admin is permanently locked.

**Mitigation:**
- Use a **multisig admin** (e.g. 2-of-3 or 3-of-5) so single-key loss is survivable.
- Consider a **governance proposal** (`submit_proposal` with `ProposalKind::RotateAdmin`) as a backup rotation mechanism if the admin is a governance contract.

---

## 5. Edge Cases & Contingencies

### 5.1 Claim Window Elapsed (Unclaimed)

**Scenario:** Admin B does not claim within 7 days.

**Resolution:**
1. Any address can trigger cleanup by calling `claim_admin_role` with any address — it will fail with `ProposalExpired` and remove the stale proposal.
2. Admin A can then create a fresh proposal.

**Prevention:**
- Set calendar reminders at T+5 days and T+6 days.
- Have Admin B confirm readiness to claim within 24 hours of proposal.

### 5.2 Compromised Claimant

**Scenario:** Admin B's private key is compromised **after** proposal but **before** claim.

**Resolution:**
1. Admin A calls `cancel_admin_proposal` immediately.
2. Generate a new secure key pair for Admin B (or choose a different admin).
3. Re-propose with the new address.

**Key insight:** The attacker cannot claim unless they control the **exact proposed address**. They cannot redirect the proposal to their own address.

### 5.3 Repeated Rotations

**Scenario:** Chain of rotations: A → B → C → D.

**Resolution:** Each rotation is independent. After A proposes B and B claims:
- B can immediately propose C (subject to 6-hour cooldown).
- A has no special privileges; they are just a previous admin.

**Test coverage:** `test_two_step_rotation_full_lifecycle` validates A → B → C chains.

### 5.4 Proposal During Emergency Stop

**Scenario:** Contract is in emergency stop mode.

**Resolution:** `propose_admin` is **NOT** gated by emergency stop. Admin rotation is the primary mechanism for handing off control during an incident.

**Post-rotation:** New admin can `disable_emergency_stop` to resume operations.

### 5.5 Old `rotate_admin` Still Exists

**Scenario:** Need to rotate immediately without the 7-day window.

**Resolution:** The legacy `rotate_admin(current_admin, new_admin, nonce)` is still available. It:
- Consumes a nonce for `DOMAIN_ADMIN_ROTATION`.
- Updates admin atomically with no proposal window.
- Is subject to the same `SelfRotation` and `InvalidNewAdmin` guards.
- Is subject to the 6-hour "Admin" cooldown.

**Use only when:**
- Two-step flow is unavailable (e.g. contract upgrade reverted the feature).
- Emergency requires immediate rotation and the signer accepts the irreversibility risk.

---

## 6. Security Assumptions & Invariants

### 6.1 Threat Model

| Threat | Likelihood | Impact | Mitigation |
|--------|-----------|--------|------------|
| Wrong address proposed | Medium | Critical | 7-day window + cancel path |
| Claimant key compromised | Low | High | Cancel + re-propose |
| Front-running claim | Very Low | Medium | Claimant must match exact address |
| Admin key compromised | Low | Critical | Multisig + cooldown + monitoring |
| Proposal storage DoS | Very Low | Low | Instance storage, cleaned up on expiry |
| Replay of proposal tx | Very Low | Low | No nonce consumed for propose; claim requires auth |

### 6.2 Invariants

1. **Single active proposal:** At most one `AdminProposal` exists in instance storage at any time. `ProposalAlreadyExists` prevents overlap.
2. **Claimant identity:** Only the exact `new_admin` address from the proposal can claim. `InvalidClaimant` rejects all others.
3. **Expiry cleanup:** Expired proposals are removed on the next `claim_admin_role` attempt. No stale proposals persist indefinitely.
4. **Atomic admin swap:** Admin update and proposal removal happen in the same transaction. No intermediate state exists.
5. **No backdoor:** Previous admins have no residual privileges after claim. `Unauthorized` on all admin-protected ops.
6. **Cooldown enforcement:** `enforce_config_cooldown("Admin")` is called on both `propose_admin` and `rotate_admin`, preventing rapid-fire rotations.

### 6.3 Multisig Considerations

- Both `propose_admin` and `claim_admin_role` call `require_auth()` on their respective addresses.
- If the admin is a multisig account, the threshold must be met for each call independently.
- **Recommendation:** Use a **2-of-3 multisig** for production admin to balance security and availability.

---

## 7. Events Reference

| Event | Topic | Payload Fields |
|-------|-------|----------------|
| `admin_proposal_created` | `["admin_proposal_created"]` | `old_admin: Address`, `new_admin: Address`, `expires_at: u64`, `timestamp: u64` |
| `admin_proposal_claimed` | `["admin_proposal_claimed"]` | `old_admin: Address`, `new_admin: Address`, `timestamp: u64` |
| `admin_proposal_cancelled` | `["admin_proposal_cancelled"]` | `admin: Address`, `timestamp: u64` |
| `admin_rotated` | `["admin_rotated"]` | `old_admin: Address`, `new_admin: Address`, `timestamp: u64` |

**Indexer query example:**
```bash
curl -X POST https://soroban-rpc.mainnet.stellar.games \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "getEvents",
    "params": {
      "startLedger": 50000000,
      "endLedger": 50001000,
      "filters": [
        {
          "type": "contract",
          "contractIds": ["CONTRACT_ID"],
          "topics": [["admin_proposal_created"]]
        }
      ]
    }
  }'
```

---

## 8. Test Coverage

All scenarios below are covered by the existing test suite. Run before any mainnet operation:

```bash
cd contracts/subscription_vault
cargo test test_admin_rotation_two_step
cargo test test_admin_transfer_auth
cargo test test_admin_rotation_does_not_affect_subscriptions
cargo test test_admin_rotation_with_subscriptions_active
```

### Covered Scenarios

| Scenario | Test Function | File |
|----------|--------------|------|
| Successful propose + claim | `test_propose_admin_success`, `test_claim_admin_role_success` | `test_admin_rotation_two_step.rs` |
| Unauthorized propose | `test_propose_admin_unauthorized` | `test_admin_rotation_two_step.rs` |
| Double propose rejected | `test_propose_admin_twice_rejected` | `test_admin_rotation_two_step.rs` |
| Propose to contract address | `test_propose_admin_to_contract_rejected` | `test_admin_rotation_two_step.rs` |
| Claim by wrong address | `test_claim_admin_role_wrong_claimant` | `test_admin_rotation_two_step.rs` |
| Claim with no proposal | `test_claim_admin_role_no_proposal` | `test_admin_rotation_two_step.rs` |
| Claim after expiry | `test_claim_admin_role_expired` | `test_admin_rotation_two_step.rs` |
| New admin can operate | `test_claim_admin_role_new_admin_can_operate` | `test_admin_rotation_two_step.rs` |
| Cancel success | `test_cancel_admin_proposal_success` | `test_admin_rotation_two_step.rs` |
| Cancel no proposal | `test_cancel_admin_proposal_no_proposal` | `test_admin_rotation_two_step.rs` |
| Cancel unauthorized | `test_cancel_admin_proposal_unauthorized` | `test_admin_rotation_two_step.rs` |
| Cancel then re-propose | `test_cancel_then_repropose_works` | `test_admin_rotation_two_step.rs` |
| Full lifecycle chain | `test_two_step_rotation_full_lifecycle` | `test_admin_rotation_two_step.rs` |
| Event payload verification | `test_admin_proposal_created_event_payload`, etc. | `test_admin_rotation_two_step.rs` |
| Old rotate_admin still works | `test_old_rotate_admin_still_works` | `test_admin_rotation_two_step.rs` |
| Happy path transfer | `happy_path_admin_a_can_transfer_to_admin_b` | `test_admin_transfer_auth.rs` |
| Unauthorized transfer | `unauthorized_admin_b_cannot_initiate_transfer` | `test_admin_transfer_auth.rs` |
| Self-rotation rejected | `self_rotation_rejected` | `test_admin_transfer_auth.rs` |
| Rotation to contract rejected | `rotation_to_contract_address_rejected` | `test_admin_transfer_auth.rs` |
| Multiple unauthorized attempts | `multiple_unauthorized_transfer_calls_all_fail` | `test_admin_transfer_auth.rs` |

### Full Test Run

```bash
cargo test --all
```

Expected output (abridged):
```
running 25 tests
test test_propose_admin_success ... ok
test test_propose_admin_unauthorized ... ok
test test_propose_admin_twice_rejected ... ok
test test_propose_admin_to_contract_rejected ... ok
test test_propose_admin_event_emitted ... ok
test test_propose_admin_during_emergency_stop ... ok
test test_claim_admin_role_success ... ok
test test_claim_admin_role_wrong_claimant ... ok
test test_claim_admin_role_no_proposal ... ok
test test_claim_admin_role_expired ... ok
test test_claim_admin_role_new_admin_can_operate ... ok
test test_claim_admin_role_event_emitted ... ok
test test_claim_admin_role_after_expiry_rejected_without_side_effects ... ok
test test_cancel_admin_proposal_success ... ok
test test_cancel_admin_proposal_no_proposal ... ok
test test_cancel_admin_proposal_unauthorized ... ok
test test_cancel_admin_proposal_event_emitted ... ok
test test_cancel_then_repropose_works ... ok
test test_get_admin_proposal_none_when_not_set ... ok
test test_get_admin_proposal_expired_still_visible ... ok
test test_two_step_rotation_full_lifecycle ... ok
test test_admin_proposal_created_event_payload ... ok
test test_admin_proposal_claimed_event_payload ... ok
test test_admin_proposal_cancelled_event_payload ... ok
test test_old_rotate_admin_still_works ... ok
test test_proposal_cannot_be_claimed_by_old_admin_after_immediate_rotation ... ok

test result: ok. 25 passed; 0 failed
```

---

## 9. Quick Reference Card

### Commands

```bash
# Get current admin
stellar contract invoke --id CONTRACT_ID --rpc-url ... --source ... -- get_admin

# Get active proposal
stellar contract invoke --id CONTRACT_ID --rpc-url ... --source ... -- get_admin_proposal

# Propose new admin
stellar contract invoke --id CONTRACT_ID --source CURRENT_ADMIN_SECRET --rpc-url ... --network-passphrase "Public Global Stellar Network ; September 2015" -- propose_admin --current_admin ADMIN_A --new_admin ADMIN_B

# Claim admin role
stellar contract invoke --id CONTRACT_ID --source NEW_ADMIN_SECRET --rpc-url ... --network-passphrase "Public Global Stellar Network ; September 2015" -- claim_admin_role --claimant ADMIN_B

# Cancel proposal
stellar contract invoke --id CONTRACT_ID --source CURRENT_ADMIN_SECRET --rpc-url ... --network-passphrase "Public Global Stellar Network ; September 2015" -- cancel_admin_proposal --admin ADMIN_A

# Legacy instant rotation (emergency only)
stellar contract invoke --id CONTRACT_ID --source CURRENT_ADMIN_SECRET --rpc-url ... --network-passphrase "Public Global Stellar Network ; September 2015" -- rotate_admin --current_admin ADMIN_A --new_admin ADMIN_B --nonce NONCE
```

### Error Codes

| Error | Code | When |
|-------|------|------|
| `Unauthorized` | 1001 | Caller is not the stored admin |
| `Forbidden` | 1002 | Auth passed but identity mismatch |
| `SelfRotation` | 1004 | `new_admin == current_admin` |
| `NonceAlreadyUsed` | 1005 | Nonce replay on `rotate_admin` |
| `ProposalNotFound` | 14001 | No proposal exists for claiming |
| `ProposalExpired` | 14002 | Claim attempted after 7-day window |
| `InvalidClaimant` | 14003 | Claimant != proposed new_admin |
| `ProposalAlreadyExists` | 14004 | Attempt to propose while one is active |
| `NoActiveProposal` | 14005 | Cancel attempted with no proposal |
| `InvalidNewAdmin` | 3004 | `new_admin == contract_address` |
| `CooldownActive` | 12001 | Admin config mutated within 6 hours |

---

## 10. Related Documentation

- [Admin Rotation Tests](../admin_rotation_tests.md) — Detailed test coverage
- [Admin Authorization Matrix](../admin_authorization_matrix.md) — Per-endpoint auth requirements
- [Recovery](../recovery.md) — Admin recovery of stranded funds
- [Events](../events.md) — Full event schema reference
- [Governance Proposals](../governance_proposals.md) — Quorum-based admin rotation alternative
