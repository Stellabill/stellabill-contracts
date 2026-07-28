# Emergency-stop view surface audit

**Issue:** #634  
**Branch:** `audit/emergency-stop-view-surface`  
**Scope:** `contracts/subscription_vault/src/queries.rs` and all `pub fn` view entrypoints in `lib.rs`

---

## 1. Background

The wave-2 emergency-stop implementation gates every **mutating** entrypoint
behind `require_not_emergency_stop()`, which returns `Error::EmergencyStopActive`
when `DataKey::EmergencyStop` is `true`.  Read-only ("view") functions were
intentionally left ungated so that subscribers and merchants can still observe
their own state during a stop.

This audit asks: *could any view function return information that meaningfully
helps an attacker bypass the stop?*  Concretely, the threat model is:

- The stop is engaged because a compromised key or protocol bug has been detected.
- An attacker who can read contract state wants to craft a valid mutating
  transaction (e.g. `rotate_admin`, `operator_batch_charge`) that would execute
  the moment the stop is lifted — or to race the admin's remediation window.
- The only operationally-sensitive inputs for such a transaction that are *not*
  already observable on-chain are **replay-protection nonces**, because they
  change with each consumed transaction.

---

## 2. Enumeration of view entrypoints

### 2a. Gated — `Error::EmergencyStopActive` returned while stop is active

| Entrypoint | Reason gated |
|---|---|
| `get_admin_nonce(signer, domain)` | Returns the exact nonce needed to craft a valid `rotate_admin` or `batch_charge` transaction.  An attacker who reads this during the stop window can pre-sign a payload that will execute the instant the stop is lifted, before the admin has completed remediation. |
| `get_operator_nonce(op)` | Identical reasoning for `operator_batch_charge`.  The operator is a least-privilege charge delegate but could still drain subscription balances at scale if the nonce is known. |

**Implementation:** both entrypoints call `require_not_emergency_stop(&env)?`
as their first statement, matching the convention used by mutating entrypoints.

### 2b. Intentionally public — no gate applied

These functions expose information that is already observable by inspecting
historical transactions on-chain, or that must remain accessible for legitimate
UX/auditor use during the stop.

| Entrypoint | Classification | Rationale |
|---|---|---|
| `get_emergency_stop_status` | Public by design | UIs must read this to disable user interactions during a stop. |
| `get_admin` | Intentionally public | The admin address has signed every previous admin-only transaction; it is fully observable on-chain. Hiding it in a query adds no protection. |
| `get_operator` | Intentionally public | Same reasoning as `get_admin`. |
| `get_oracle_config` | Intentionally public | The oracle address and config are observable from the last `set_oracle_config` event. Blocking the query would not prevent an oracle attack. |
| `get_metadata_signed_nonce` | Intentionally public | The metadata-signed domain has no custody or governance privilege; a captured nonce cannot affect fund balances or admin control. |
| `version` | Public | Non-sensitive. |
| `get_subscription_count` | Public | Non-sensitive count. |
| `list_accepted_tokens` | Public | Token addresses are non-sensitive. |
| `get_protocol_fee_bps` | Public | Non-sensitive fee config. |
| `get_auto_pause_threshold` | Public | Non-sensitive threshold. |
| `get_billing_retention` | Public | Non-sensitive retention config. |
| `get_min_topup` | Public | Non-sensitive threshold. |
| `get_subscriber_create_cap` | Public | Non-sensitive. |

### 2c. Subscriber/merchant data — accessible while stopped

These queries return data that belongs to the caller's own position.
Blocking them during a stop would harm legitimate users more than it would
hinder attackers (all such data is in persistent storage events anyway).

| Entrypoint | Notes |
|---|---|
| `get_subscription` | Subscriber reads own record. |
| `get_next_charge_info` | Pure computation on subscription data. |
| `get_cap_info` | Subscription cap state. |
| `estimate_topup_for_intervals` | Pure computation. |
| `get_merchant_balance` / `get_merchant_balance_by_token` | Merchant reads own balance. |
| `get_merchant_token_earnings` | Merchant reads own earnings. |
| `get_merchant_paused` | Needed for UX. |
| `get_merchant_subscription_count` / `get_token_subscription_count` | Counts only. |
| `get_subscriptions_by_merchant` / `get_subscriptions_by_token` | Listing. |
| `list_subscriptions_by_subscriber` | Listing. |
| `get_sub_statements_offset` / `get_sub_statements_cursor` | Billing history. |
| `get_stmt_compacted_aggregate` | Aggregate, non-sensitive. |
| `get_period_snapshot` / `list_period_snapshots` | Non-sensitive. |
| `get_plan_template` / `get_plan_max_active_subs` | Non-sensitive. |
| `get_merchant_max_subs` | Non-sensitive. |
| `get_merchant_config` | Merchant reads own config. |
| `get_payout_schedule` | Merchant reads own schedule. |
| `get_global_cap_default` / `get_merchant_cap_default` | Non-sensitive. |
| `get_subscriber_active_cap` / `get_subscriber_active_count` | Own state. |
| `get_subscriber_credit_limit` / `get_subscriber_exposure` | Own state. |
| `is_blocklisted` / `get_blocklist_entry` | Own status. |
| `get_whitelist_mode` / `is_merchant_approved` | Non-sensitive. |
| `get_tag_allowlist` / `get_merchant_tags` | Non-sensitive. |
| `get_dispute` / `get_subscription_dispute` | Dispute parties read own records. |
| `get_coupon` | Non-sensitive. |
| `get_merchant_multisig_config` | Merchant reads own multisig. |

### 2d. Governance transparency — intentionally public

Governance information (guardian weights, proposals, current proposal id) is
required to be transparent by the governance design: guardians must be able to
verify quorum calculations.  Blocking these during a stop would impede the
remediation process itself.

| Entrypoint |
|---|
| `get_guardian_weight` |
| `list_guardians` |
| `get_current_proposal_id` |
| `get_proposal` |

### 2e. Reconciliation / auditor tools

These are intentionally available to auditors and off-chain reconciliation
tooling.  Full financial state is recoverable from on-chain events regardless;
blocking these views adds no protection.

| Entrypoint |
|---|
| `get_token_reconciliation` |
| `get_recon_summary` |
| `generate_reconciliation_proof` |
| `query_prepaid_balances_paginated` |
| `emit_oracle_liveness` |

---

## 3. Changes made

### `contracts/subscription_vault/src/lib.rs`

Two entrypoints modified:

```rust
// BEFORE
pub fn get_admin_nonce(env: Env, signer: Address, domain: u32) -> u64 {
    nonce::get_nonce(&env, &signer, domain)
}

pub fn get_operator_nonce(env: Env, op: Address) -> u64 {
    nonce::get_nonce(&env, &op, nonce::DOMAIN_OPERATOR_BATCH_CHARGE)
}
```

```rust
// AFTER
pub fn get_admin_nonce(env: Env, signer: Address, domain: u32) -> Result<u64, Error> {
    require_not_emergency_stop(&env)?;
    Ok(nonce::get_nonce(&env, &signer, domain))
}

pub fn get_operator_nonce(env: Env, op: Address) -> Result<u64, Error> {
    require_not_emergency_stop(&env)?;
    Ok(nonce::get_nonce(&env, &op, nonce::DOMAIN_OPERATOR_BATCH_CHARGE))
}
```

### `contracts/subscription_vault/src/test_emergency_stop_view_surface.rs` (new)

Comprehensive test module covering:
- All gated views return `EmergencyStopActive` while stopped.
- All gated views return correct data when not stopped.
- Stop/resume cycles do not corrupt nonce state.
- All intentionally-public views return data while stopped.
- Pre-init calls return `NotInitialized` or safe defaults.
- Empty-state calls return safe defaults without panicking.

---

## 4. Security notes

### Why only nonce queries are gated

Soroban smart contracts have no private state: everything in storage is
readable by any node operator who can run a simulation.  Gating a read
function prevents it from being called as a contract invocation in a
standard transaction, but it does not prevent a determined attacker from
reading the underlying storage key directly.

The gate on `get_admin_nonce` and `get_operator_nonce` is therefore a
**defence-in-depth measure**, not an absolute control.  Its value is:

1. It prevents automated scripts from polling for nonces in-band during a
   stop, making nonce harvesting slightly harder and more detectable.
2. It signals clearly to integrators that the stop window is not a safe time
   to submit prepared transactions.
3. It is consistent with the principle that the stop should block all
   operations that could advance a privileged state change.

### The `get_admin` and `get_operator` decision

Blocking these would be security theatre: the admin and operator addresses
are embedded in every historical admin-signed transaction on-chain and are
therefore fully public.  Blocking the query view would not prevent an attacker
who already knows the addresses from acting; it would only inconvenience
legitimate callers (e.g. off-chain tooling verifying the admin before calling
`disable_emergency_stop`).

### Timelock / config cooldown note

`enforce_config_cooldown` records a per-key timestamp in persistent storage
(`DataKey::AdminConfigLastChangedAt`).  There is no view function that exposes
these raw timestamps, so they cannot be used to infer when a cooldown will
expire.  This surface is therefore clear.

---

## 5. Test output

Run with:

```bash
cargo test --all -- --include-ignored 2>&1 | tee test_output.txt
```

All tests in `test_emergency_stop_view_surface` must pass.  See the test file
for the full list of cases.
