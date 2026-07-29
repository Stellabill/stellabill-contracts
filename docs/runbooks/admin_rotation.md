# Mainnet Admin Rotation Playbook

## Overview

This runbook covers the **two-step admin rotation** feature for the Subscription Vault contract on Stellar mainnet. The two-step flow replaces the older single-step `rotate_admin` for all planned admin transfers, providing a safety window during which the proposal can be cancelled if the wrong address was targeted.

### Flow

```
Current Admin  ──propose_admin──►  Proposal (7-day TTL)
                                        │
                  ┌─────────────────────┼─────────────────────┐
                  ▼                     ▼                     ▼
           Claim by target      Cancel by admin         Expire (7d)
           (admin transferred)  (no change)             (no change)
```

## Prerequisites

- Stellar CLI or SDK (e.g. `stellar`, `soroban`) pointed at **mainnet**.
- Current admin private key (or multisig authorization).
- Target admin public key (or multisig address).
- Network passphrase: `Public Global Stellar Network ; September 2015`.

## Step 1: Propose New Admin

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

- Emits event `admin_proposal_created` with `old_admin`, `new_admin`, `expires_at`, `timestamp`.
- Proposal is stored in contract instance storage with a **7-day TTL**.
- If a proposal already exists, the call fails with `ProposalAlreadyExists`.

### Verification

```bash
stellar contract invoke \
  --id CONTRACT_ID \
  --rpc-url https://soroban-rpc.mainnet.stellar.games \
  --network-passphrase "Public Global Stellar Network ; September 2015" \
  --source SOME_SOURCE \
  -- \
  get_admin_proposal
```

Expected output (trimmed):

```json
{"new_admin": "G…", "proposed_at": 1234567890, "expires_at": 1234567890 + 604800}
```

## Step 2: Verify Proposal (Off-Chain)

1. Confirm the `new_admin` address is correct.
2. Check `proposed_at` and `expires_at` timestamps are reasonable.
3. Monitor the `admin_proposal_created` event on-chain (e.g. via Stellar Expert or a block explorer).

## Step 3A: Claim (by New Admin)

Once verified, the **new admin** must call `claim_admin_role` before the proposal expires:

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

- Emits event `admin_proposal_claimed` with `old_admin`, `new_admin`, `timestamp`.
- On success, the proposal is removed from storage and the admin is updated.
- Fails with `ProposalExpired` if the 7-day window has passed.
- Fails with `InvalidClaimant` if the caller is not the proposed `new_admin`.

### Post-Claim Verification

```bash
stellar contract invoke \
  --id CONTRACT_ID \
  --rpc-url https://soroban-rpc.mainnet.stellar.games \
  --network-passphrase "Public Global Stellar Network ; September 2015" \
  --source SOME_SOURCE \
  -- \
  get_admin
```

Confirm the admin address now matches the new admin.

Validate that the new admin can execute admin-protected operations:

```bash
stellar contract invoke \
  --id CONTRACT_ID \
  --source NEW_ADMIN_SECRET \
  --rpc-url https://soroban-rpc.mainnet.stellar.games \
  --network-passphrase "Public Global Stellar Network ; September 2015" \
  -- \
  set_min_topup \
  --admin $(stellar address from-secret NEW_ADMIN_SECRET) \
  --min_topup 1000000
```

## Step 3B: Cancel (by Current Admin)

If the proposal was made in error, the **current admin** can cancel it:

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

- Emits event `admin_proposal_cancelled` with `admin`, `timestamp`.
- After cancellation, a new proposal can be created immediately.

## Step 3C: Expiry

If the proposal is neither claimed nor cancelled after 7 days, it expires. Any subsequent `claim_admin_role` call will fail with `ProposalExpired` and clean up the stale proposal. No action is required by either party after expiry.

## Fallback / Emergency

### Single-Step Rotate (Backward Compatible)

The older `rotate_admin(new_admin)` is still available and performs an instant admin transfer with no proposal window. Use this **only** when:

- The two-step flow is unavailable (e.g. contract upgrade reverted the feature).
- An emergency requires immediate rotation and the signer accepts the risk.

```bash
stellar contract invoke \
  --id CONTRACT_ID \
  --source CURRENT_ADMIN_SECRET \
  --rpc-url https://soroban-rpc.mainnet.stellar.games \
  --network-passphrase "Public Global Stellar Network ; September 2015" \
  -- \
  rotate_admin \
  --current_admin $(stellar address from-secret CURRENT_ADMIN_SECRET) \
  --new_admin NEW_ADMIN_ADDRESS
```

### Re-Proposal After Cancel/Expiry

After a cancellation or expiry, simply re-run **Step 1** with a corrected `new_admin`.

## Security Notes

| Concern | Mitigation |
|---------|-----------|
| Proposed admin is a burned/dead address | Validate address before proposing. Use `get_admin_proposal` to verify. |
| Admin private key compromised | The attacker cannot claim without the proposal being for their address. Cancel+re-propose if you suspect compromise. |
| Proposal front-running | Claimant must be the exact `new_admin` address. An attacker cannot claim on behalf of a different address. |
| Multisig accounts | Both `propose_admin` and `claim_admin_role` require `require_auth()`. Multisig setup works naturally. |
| Contract cannot be its own admin | `propose_admin` and `rotate_admin` both reject `new_admin == contract_address`. |
| Replay attack | Proposal is removed on claim or cleaned up on expiry. Each proposal is single-use. |

## Events Reference

| Event | Topics | Payload |
|-------|--------|---------|
| `admin_proposal_created` | `["admin_proposal_created"]` | `AdminProposalCreatedEvent { old_admin, new_admin, expires_at, timestamp }` |
| `admin_proposal_claimed` | `["admin_proposal_claimed"]` | `AdminProposalClaimedEvent { old_admin, new_admin, timestamp }` |
| `admin_proposal_cancelled` | `["admin_proposal_cancelled"]` | `AdminProposalCancelledEvent { admin, timestamp }` |
| `admin_rotated` | `["admin_rotated"]` | `AdminRotatedEvent { old_admin, new_admin, timestamp }` |

## Rollback Plan

1. **If proposal exists but not yet claimed**: Call `cancel_admin_proposal` from the current admin. Admin remains unchanged.
2. **If proposal was already claimed**: The old admin is no longer admin. To recover, the **new admin** must voluntarily create a proposal back to the old admin (or another trusted address).
3. **If both old and new admin keys are lost**: Contract admin is permanently locked. No recovery path exists. Use a multisig admin to reduce this risk.