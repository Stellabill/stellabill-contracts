# Multi-Token Onboarding Checklist for New Payment Assets

> **Audience:** Contract administrators, billing-engine operators, and integration
> engineers adding a new settlement token to the Stellabill subscription vault.
>
> **Scope:** This checklist covers the complete lifecycle of adding, verifying,
> using, monitoring, and (if necessary) removing a non-default payment asset.

---

## Table of Contents

1. [Prerequisites](#prerequisites)
2. [Step 1: Token Verification &amp; Validation](#step-1-token-verification--validation)
3. [Step 2: Admin Registration — `add_accepted_token`](#step-2-admin-registration--add_accepted_token)
4. [Step 3: Oracle Price Feed Configuration (if applicable)](#step-3-oracle-price-feed-configuration-if-applicable)
5. [Step 4: Plan Template Creation with New Token](#step-4-plan-template-creation-with-new-token)
6. [Step 5: Subscription Creation with New Token](#step-5-subscription-creation-with-new-token)
7. [Step 6: Reconciliation Setup &amp; Monitoring](#step-6-reconciliation-setup--monitoring)
8. [Step 7: Merchant Withdrawal Flow Validation](#step-7-merchant-withdrawal-flow-validation)
9. [Step 8: Indexer &amp; Event Pipeline Updates](#step-8-indexer--event-pipeline-updates)
10. [End-to-End Validation](#end-to-end-validation)
11. [Rollback &amp; Token Removal](#rollback--token-removal)
12. [Security Considerations](#security-considerations)

---

## Prerequisites

Before beginning the onboarding checklist, ensure the following conditions are met:

- [ ] **Token contract is deployed on Stellar** — The asset contract (e.g., EURC,
      ARB, or a custom token) must be live and accessible at a valid Stellar
      address `G...`.
- [ ] **Token decimals are known** — Record the exact `decimals` value the token
      contract reports (e.g., EURC = 6, USDC = 7, native XLM = 7). This value
      is **immutable** after registration and used for all future amount
      normalisation.
- [ ] **Admin key is available** — The current contract admin address (returned by
      `get_admin`) must be able to sign the transaction. Rotation must be
      completed **before** this step if the admin key is compromised or is
      being handed over.
- [ ] **Subscription vault is initialised** — `init` has been called successfully
      and `get_admin` returns a non-zero address.
- [ ] **Emergency stop is DISABLED** — `get_emergency_stop_status` returns `false`.
      The onboarding flow mutates contract state and is blocked when the
      circuit breaker is active.
- [ ] **Blocklist is clear** — Neither the admin nor the intended test
      subscriber/merchant addresses are blocklisted (`is_blocklisted` is `false`).
- [ ] **Token is NOT the contract's own address** — `env.current_contract_address()`
      must differ from the token address. The validation in `add_accepted_token`
      calls `reject_contract_self` which returns `InvalidInput` if the token
      address equals the vault contract.

---

## Step 1: Token Verification &amp; Validation

**Objective:** Confirm the token contract behaves correctly before adding it to
the vault's allowlist. Skipping this step may lead to silent accounting errors.

### 1.1 Verify the token implements the Soroban Token Interface

Confirm the token contract supports at least these standard functions:

| Function | Expected Behaviour |
|----------|-------------------|
| `balance(Address) -> i128` | Returns the on-chain balance for an address. |
| `transfer(from, to, amount)` | Moves tokens between accounts; called by `deposit_funds`, `withdraw_*`, `charge_*`. |
| `decimals() -> u32` | Returns the token's decimal precision. Must match the value you intend to pass to `add_accepted_token`. |

> **Test command (Soroban CLI):**
> ```bash
> soroban contract invoke \
>   --id <TOKEN_CONTRACT_ID> \
>   --fn decimals \
>   --source <ADMIN_KEY>
> ```

### 1.2 Check for non-standard behaviours

| Risk | What to check |
|------|---------------|
| **Fee-on-transfer** | Does the token deduct a fee on `transfer`? If yes, the vault's accounting assumption (`debited = credited`) breaks. **Do not onboard** fee-on-transfer tokens. |
| **Pausable** | Can the token contract be frozen by its issuer? If the token pauses `transfer`, the vault's `deposit_funds` and `withdraw_*` calls will fail. Acceptable only if the off-chain ops team monitors token-pause events. |
| **Blacklist** | Does the token allow its issuer to freeze specific addresses? If the vault contract address is frozen, all token operations fail irreversibly. Accept with caution. |
| **Rebasing / elastic supply** | Does the token's supply adjust periodically (e.g., stETH-like rebasing)? The vault stores fixed `i128` balances and does **not** rebase. **Do not onboard** rebasing tokens. |
| **Missing `decimals` function** | Some early Stellar tokens may not expose `decimals()`. These are incompatible. |

### 1.3 Confirm vault can receive the token

Deploy a throwaway test script or use the Soroban CLI to call `transfer` from a
test account to the vault contract address. Confirm the vault's `balanceOf`
increases.

- [ ] Test transfer succeeds.
- [ ] Test transfer from an unauthorised account fails.

---

## Step 2: Admin Registration — `add_accepted_token`

**Objective:** Add the new token to the vault's allowlist so it can be used for
subscriptions, deposits, charges, and withdrawals.

### 2.1 Execute the registration

**Entrypoint:** `add_accepted_token(admin, token, decimals)`

Parameters:

| Parameter | Description | Source |
|-----------|-------------|--------|
| `admin` | Current contract admin address. Must sign the invocation. | `get_admin()` |
| `token` | Address of the deployed token contract (e.g., `G...`). | Step 1.1 |
| `decimals` | Token's decimal precision (e.g., `6`, `7`). **Must not exceed 19.** | Token contract's `decimals()` |

**CLI example:**
```bash
soroban contract invoke \
  --id <VAULT_CONTRACT_ID> \
  --fn add_accepted_token \
  --source <ADMIN_KEY> \
  --arg <ADMIN_ADDRESS> \
  --arg <TOKEN_ADDRESS> \
  --arg <DECIMALS>
```

**Expected result:** The call succeeds. No error is returned.

### 2.2 Verify post-conditions

- [ ] `list_accepted_tokens()` now includes the new token with the correct decimals.
- [ ] `is_token_accepted(token)` returns `true` (internal helper reachable via
      contract reads).
- [ ] `get_token_subscription_count(token)` returns `0` (no subscriptions yet).

### 2.3 Error scenarios &amp; recovery

| Error | Meaning | Action |
|-------|---------|--------|
| `InvalidTokenDecimals` (8001) | `decimals > 19` or `decimals == 0`. | Correct the decimals value. |
| `InvalidInput` (3002) | Token address is the vault contract itself. | Use a valid external token address. |
| `Unauthorized` (1001) | `admin` did not sign or does not match stored admin. | Verify the signing key. |
| `EmergencyStopActive` (4007) | Circuit breaker is active. | Call `disable_emergency_stop(admin)` first. |

### 2.4 Validate token decimal normalisation

The vault normalises all token amounts to a standard 9-decimal base for
consistent cross-token auditing:

- **6-decimal token (EURC):** Raw amount × 1000 → 9-decimal base.
- **7-decimal token (USDC/USDT):** Raw amount × 100 → 9-decimal base.

**On-chain query:**
```
get_token_reconciliation(token) → TokenLiabilities { normalized_* fields }
```

- [ ] Verify that `normalize_amount(env, token, raw)` produces the expected
      scaled value off-chain.

---

## Step 3: Oracle Price Feed Configuration (if applicable)

**Objective:** If the new token is quoted in a different unit than the vault's
default settlement token, configure on-chain pricing so charges resolve
correctly.

**Skip this step if:**
- The new token is the default token (registered at `init`), OR
- No cross-currency subscription plans will use this token (subscriber pays in
  the same token the subscription is pinned to).

### 3.1 Configure oracle

**Entrypoint:** `set_oracle_config(admin, enabled, oracle, max_age_seconds, kind, window_secs, fixed_numerator, fixed_denominator)`

| Parameter | Value for new token |
|-----------|-------------------|
| `enabled` | `true` |
| `oracle` | Address of the deployed Soroban price oracle (e.g., Band Protocol, Stellar DEX TWAP). |
| `max_age_seconds` | Maximum acceptable age of the latest oracle price before a charge is rejected with `OraclePriceStale`. 300–900 s recommended. |
| `kind` | `Spot` (latest price), `Twap` (time-weighted average), or `FixedRate` (deterministic ratio). |
| `window_secs` | TWAP observation window in seconds. Required only when `kind == Twap`. |
| `fixed_numerator`, `fixed_denominator` | Fixed price ratio (scaled to 10⁷). Required only when `kind == FixedRate`. |

### 3.2 Validate oracle liveness

- [ ] Call `emit_oracle_liveness()` and verify `healthy` is `true`.
- [ ] Simulate a charge on a subscription pinned to the new token and confirm
      `OracleChargeResolvedEvent` is emitted with a non-zero price.

### 3.3 Fallback strategy

When the oracle is stale or unavailable, charges against subscriptions using
the new token will fail with `OraclePriceStale`. Ensure the billing engine has:

- [ ] An alert on `OracleLivenessEvent` with `healthy = false`.
- [ ] A retry policy that re-queues stale subscriptions rather than dropping them.

---

## Step 4: Plan Template Creation with New Token

**Objective:** Create a reusable plan template denominated in the new token so
that subscribers can sign up via `create_subscription_from_plan`.

### 4.1 Merchant creates a token-specific plan

**Entrypoint:** `create_plan_template_with_token(merchant, token, amount, interval_seconds, usage_enabled, lifetime_cap)`

| Parameter | Description |
|-----------|-------------|
| `merchant` | Merchant address. Must sign the invocation. |
| `token` | Newly registered token address (from Step 2). |
| `amount` | Per-interval charge amount in the token's base units (e.g., 10 EURC = `10_000000` for 6-decimal). |
| `interval_seconds` | Billing cadence (e.g., 2592000 = 30 days). Must be ≥ 60 and ≤ 31 536 000. |
| `usage_enabled` | Whether metered usage charges are allowed. |
| `lifetime_cap` | Optional cumulative charge cap (in token base units). |

**CLI example:**
```bash
soroban contract invoke \
  --id <VAULT_CONTRACT_ID> \
  --fn create_plan_template_with_token \
  --source <MERCHANT_KEY> \
  --arg <MERCHANT_ADDRESS> \
  --arg <TOKEN_ADDRESS> \
  --arg <AMOUNT> \
  --arg <INTERVAL_SECONDS> \
  --arg <USAGE_ENABLED> \
  --arg <LIFETIME_CAP>
```

### 4.2 Verify post-conditions

- [ ] `get_plan_template(plan_template_id)` returns a `PlanTemplate` whose
      `token` matches the registered token address.
- [ ] The `PlanTemplateCreatedEvent` is emitted with the correct token, amount,
      and interval.

### 4.3 Optional: Set plan concurrency limits

```bash
soroban contract invoke \
  --id <VAULT_CONTRACT_ID> \
  --fn set_plan_max_active_subs \
  --source <MERCHANT_KEY> \
  --arg <MERCHANT_ADDRESS> \
  --arg <PLAN_TEMPLATE_ID> \
  --arg <MAX_ACTIVE>
```

---

## Step 5: Subscription Creation with New Token

**Objective:** Create a live subscription pinned to the new token and deposit
funds to confirm the end-to-end flow works.

### 5.1 Create a subscription from a plan template

**Entrypoint:** `create_subscription_from_plan(subscriber, plan_template_id)`

```bash
soroban contract invoke \
  --id <VAULT_CONTRACT_ID> \
  --fn create_subscription_from_plan \
  --source <SUBSCRIBER_KEY> \
  --arg <SUBSCRIBER_ADDRESS> \
  --arg <PLAN_TEMPLATE_ID>
```

**Alternative:** Use `create_subscription_with_token` directly:

```bash
soroban contract invoke \
  --id <VAULT_CONTRACT_ID> \
  --fn create_subscription_with_token \
  --source <SUBSCRIBER_KEY> \
  --arg <SUBSCRIBER_ADDRESS> \
  --arg <MERCHANT_ADDRESS> \
  --arg <TOKEN_ADDRESS> \
  --arg <AMOUNT> \
  --arg <INTERVAL_SECONDS> \
  --arg <USAGE_ENABLED> \
  --arg <LIFETIME_CAP>
```

**Expected events:**
- `SubscriptionCreatedEvent` where `token` equals the new token address.
- `CredentialIssuedEvent` (soulbound credential for the subscription).

### 5.2 Verify token pinning

- [ ] Call `get_subscription(subscription_id)` and confirm:
  - `subscription.token == <NEW_TOKEN_ADDRESS>`
  - `subscription.status == Active`
  - `subscription.prepaid_balance == 0`

### 5.3 Deposit funds in the new token

**Entrypoint:** `deposit_funds(subscription_id, subscriber, amount, idem_key)`

The subscriber must hold a balance of the new token and sign the transfer.

- [ ] `FundsDepositedEvent` is emitted with the new token address.
- [ ] `get_subscription(subscription_id).prepaid_balance` reflects the deposit.

### 5.4 Execute a test charge

**Entrypoint:** `charge_subscription(subscription_id, idem_key)`

If the interval has not elapsed, use `charge_one_off(subscription_id, merchant, amount)`
instead for an immediate debit.

- [ ] `SubscriptionChargedEvent` is emitted with the new token address.
- [ ] Merchant's balance for the new token increases:
      `get_merchant_balance_by_token(merchant, token) > 0`.
- [ ] Billing statement is appended: verify via `get_sub_statements_offset`.

### 5.5 Error scenarios

| Error | Likely cause | Action |
|-------|-------------|--------|
| `InvalidInput` (3002) | Token not yet accepted or invalid parameters. | Verify `is_token_accepted(token)`. |
| `InvalidAmount` (3001) | Amount is zero or negative. | Check `amount` is > 0. |
| `InvalidExpiration` (3008) | `expires_at` is in the past. | Use `expires_at > now`. |
| `MaxConcurrentSubscriptionsReached` (6006) | Subscriber or merchant limit hit. | Check `get_subscriber_active_cap` / `get_merchant_max_subs`. |
| `CreditLimitExceeded` (6007) | Subscriber exposure exceeds limit. | Check `get_subscriber_credit_limit`. |

---

## Step 6: Reconciliation Setup &amp; Monitoring

**Objective:** Ensure on-chain accounting for the new token is visible and
auditable via the vault's built-in reconciliation endpoints.

### 6.1 Verify token appears in reconciliation summary

**Entrypoint:** `get_recon_summary(start_token_index=0, limit=50)`

- [ ] The returned `ReconciliationSummaryPage` includes a `TokenLiabilities`
      entry for the new token.

**Expected fields:**
```
TokenLiabilities {
    token:              <NEW_TOKEN_ADDRESS>
    total_prepaid:      0 (or the amount deposited in Step 5.3)
    total_merchant_liabilities: 0 (or the amount charged in Step 5.4)
    contract_balance:   <ACTUAL_VAULT_BALANCE>
    is_balanced:        true
    normalized_prepaid: <9-DECIMAL VALUE>
    normalized_merchant_liab: <9-DECIMAL VALUE>
    normalized_contract_balance: <9-DECIMAL VALUE>
}
```

- [ ] `is_balanced == true` — the accounting equation holds.

### 6.2 Generate a reconciliation proof

**Entrypoint:** `generate_reconciliation_proof(token)`

- [ ] `is_valid == true`.
- [ ] Record the proof's `ledger_sequence` and `timestamp` for auditing.

### 6.3 Check paginated prepaid balance query

**Entrypoint:** `query_prepaid_balances_paginated(PrepaidQueryRequest { token, start_subscription_id: 0, scan_limit: 500 })`

- [ ] Returns a non-zero `subscriptions_count` if deposits were made.

### 6.4 Set up off-chain monitoring alert

Configure a monitoring check that calls `get_recon_summary` periodically and
alerts if any token's `is_balanced` is `false`.

**Recommended cadence:** Every ledger close (5 s) for critical tokens; every
10 minutes for low-volume tokens.

---

## Step 7: Merchant Withdrawal Flow Validation

**Objective:** Verify the merchant can withdraw funds denominated in the new
token.

### 7.1 Check merchant balance

```bash
soroban contract invoke \
  --id <VAULT_CONTRACT_ID> \
  --fn get_merchant_balance_by_token \
  --arg <MERCHANT_ADDRESS> \
  --arg <TOKEN_ADDRESS>
```

- [ ] Returns a balance > 0 (if charges have been processed against this token).

### 7.2 Execute a token-specific withdrawal

**Entrypoint:** `withdraw_merchant_token_funds(merchant, token, amount)`

```bash
soroban contract invoke \
  --id <VAULT_CONTRACT_ID> \
  --fn withdraw_merchant_token_funds \
  --source <MERCHANT_KEY> \
  --arg <MERCHANT_ADDRESS> \
  --arg <TOKEN_ADDRESS> \
  --arg <AMOUNT>
```

### 7.3 Verify post-conditions

- [ ] `MerchantWithdrawalEvent` is emitted with the new token address.
- [ ] `get_merchant_balance_by_token(merchant, token)` decreases by `amount`.
- [ ] The merchant's external wallet balance of the new token increases by `amount`.

### 7.4 Test insufficient balance rejection

Attempt a withdrawal for more than the merchant's balance:

- [ ] Returns `InsufficientBalance` (5001).
- [ ] No state change.

### 7.5 Test non-existent token withdrawal

Attempt `withdraw_merchant_token_funds` with a token not in the accepted list:

- [ ] Returns `InvalidInput` (3002).

---

## Step 8: Indexer &amp; Event Pipeline Updates

**Objective:** Ensure off-chain indexers correctly decode and record events
involving the new token.

### 8.1 Verify event schema compatibility

All events carry a `schema_version` field (currently `2`). Confirm the indexer
can decode:

- `SubscriptionCreatedEvent.token` — now points to the new token address.
- `FundsDepositedEvent.token` — reflects the deposit token.
- `SubscriptionChargedEvent.token` — reflects the charge token.
- `MerchantWithdrawalEvent.token` — reflects the withdrawal token.
- `SubscriptionCancelledEvent.token` — reflects the refund token.
- All `OracleChargeResolvedEvent` fields (if oracle is used).

### 8.2 Update event filters

If the indexer uses topic-based filtering (e.g., "only token = default"), add
the new token address to the allowlist.

### 8.3 Update metrics calculations

| Metric | Impact of new token |
|--------|-------------------|
| **MRR** | Must be summed **per token** and normalised to a common base (9-decimal) for aggregate reports. |
| **TVL** | Sum of `prepaid_balance` across all tokens, normalised. |
| **Merchant revenue** | Already per-token via `get_merchant_total_earnings`. |
| **Churn risk** | `estimate_topup_for_intervals` is token-agnostic (operates on subscription's stored token). |

### 8.4 Update billing engine configuration

If the billing engine maintains an internal allowlist of accepted tokens, add
the new token:

```toml
# billing-engine.toml
[accepted_tokens]
usdc = "CDLZFC3SYJYD...Z7PJQ"
eurc = "GA5ZSE...J3FGH"   # NEW
```

---

## End-to-End Validation

After completing Steps 1–8, run the following full-cycle test.

### Test Scenario

```
1. Admin: add_accepted_token(admin, EURC, 6)                    ✓ (Step 2)
2. Merchant: create_plan_template_with_token(... EURC ...)        ✓ (Step 4)
3. Subscriber: create_subscription_from_plan(sub, plan_id)       ✓ (Step 5)
4. Subscriber: deposit_funds(sub_id, sub, 100_000000)            ✓ (Step 5)
5. Admin: charge_subscription(sub_id)                             ✓ (Step 5)
6. Verify: get_merchant_balance_by_token(merchant, EURC) > 0    ✓ (Step 7)
7. Merchant: withdraw_merchant_token_funds(merchant, EURC, ...) ✓ (Step 7)
8. Verify: get_token_reconciliation(EURC).is_balanced == true   ✓ (Step 6)
```

### Assertions

- [ ] All events carry the correct `token` address.
- [ ] At no point does a charge, deposit, or withdrawal revert with an unexpected error.
- [ ] The reconciliation proof is valid (`is_valid == true`).
- [ ] The default token's `is_balanced` remains `true` (the new token's operations
      must not corrupt the default token's accounting).
- [ ] `get_subscriptions_by_token(EURC, 0, 100)` returns the created subscription.

---

## Rollback &amp; Token Removal

### When removal is appropriate

| Scenario | Action |
|----------|--------|
| Token was added by mistake (zero subscriptions). | Safe to remove immediately. |
| Token is being deprecated but has active subscriptions. | Remove only if you accept that existing subscriptions become *immutable* (no new subscriptions, but existing charges continue). |
| Token contract is compromised. | Activate emergency stop first, then remove. |

### Removal procedure

**Entrypoint:** `remove_accepted_token(admin, token)`

```
┌─────────────────────────────────────────────────────────────┐
│                      IMPORTANT                              │
│                                                             │
│  The default token (registered at init) CANNOT be removed. │
│  Attempting to do so returns Error::InvalidInput (3002).    │
│                                                             │
│  Active subscriptions pinned to the removed token           │
│  continue to function normally. Only NEW subscriptions      │
│  with this token are blocked.                               │
└─────────────────────────────────────────────────────────────┘
```

**CLI example:**
```bash
soroban contract invoke \
  --id <VAULT_CONTRACT_ID> \
  --fn remove_accepted_token \
  --source <ADMIN_KEY> \
  --arg <ADMIN_ADDRESS> \
  --arg <TOKEN_ADDRESS>
```

### Post-removal checklist

- [ ] `list_accepted_tokens()` no longer includes the token.
- [ ] `is_token_accepted(token)` returns `false`.
- [ ] `create_subscription_with_token(sub, merch, token, ...)` returns `InvalidInput`.
- [ ] Existing subscriptions pinned to the token remain readable and chargeable.
- [ ] `get_token_subscription_count(token)` returns the count of surviving subscriptions.
- [ ] `get_token_reconciliation(token)` continues to work (for audit trail).

### Emergency removal with emergency stop

If the token contract is compromised:
1. `enable_emergency_stop(admin)` — halts all financial writes immediately.
2. `remove_accepted_token(admin, token)` — removes from allowlist.
3. Assess whether existing subscriptions should be bulk-cancelled via
   `bulk_cancel_subscriptions(admin, ids, nonce)`.

---

## Security Considerations

### 1. Token confusion prevention (reviewed in Step 5)

Each subscription stores its `token` address at creation time. All deposits,
charges, and withdrawals use that stored token — it is **never inferred from
caller input** after creation. This prevents a malicious caller from tricking
the contract into debiting one token and crediting another.

### 2. Allowlist enforcement (reviewed in Step 2 &amp; 4)

Every entrypoint that accepts a token parameter calls `is_token_accepted`
before proceeding:

| Entrypoint | Enforcement |
|-----------|-------------|
| `create_subscription_with_token` | Rejects with `InvalidInput` if token is not accepted. |
| `create_subscription_from_plan` | Plan template token must be accepted (enforced at plan creation). |
| `create_plan_template_with_token` | Rejects with `InvalidInput` if token is not accepted. |
| `withdraw_merchant_token_funds` | Rejects with `InvalidInput` if token is not accepted. |

### 3. Default token immutability (reviewed in Step 2)

The default token (registered at `init`) **cannot be removed**. This prevents
accidental lockout of the vault's primary settlement asset. The default token
is the fallback used by `create_subscription` (without `_with_token`) and
`withdraw_merchant_funds` (without `_token`).

### 4. Active subscriptions survive token removal (reviewed in Rollback)

Removing a token from the allowlist does **not** affect existing subscriptions
using that token. They remain readable and chargeable. Only *new* subscriptions
with the removed token are blocked. This means:

- **DO NOT** remove a token expecting to force-migrate subscriptions off it.
- **DO** retain the token's decimal configuration (`TokenDecimals` key) in
  storage even after removal; reconciliation queries need it for normalisation.

### 5. Decimal immutability

Once `add_accepted_token` stores the `decimals` value under `TokenDecimals(token)`,
it is **never updated**. If the token contract changes its decimals (extremely
rare), a new token address must be deployed and registered.

### 6. Reentrancy hardening

All multi-token operations that perform external token transfers
(`deposit_funds`, `withdraw_merchant_token_funds`, `merchant_refund`) are
wrapped in a `ReentrancyGuard` and follow the Checks-Effects-Interactions
(CEI) pattern. The guard prevents recursive re-entry during the external
call, while CEI ensures state is persisted before any token leaves the vault.

### 7. Cross-token reconciliation invariant

The vault maintains token-isolated accounting buckets:

Let me create the TODO.md first to track my progress, then the document.

<create_file>
<path>c:/Users/FAUZIYAT/Desktop/stellabill-contracts/TODO.md</path>
<content>
# Multi-Token Onboarding Checklist — Task Progress

## Steps
- [ ] Create `docs/multi_token_onboarding_checklist.md` with the full multi-token onboarding checklist
  - [x] Gather information from the codebase
  - [x] Plan the document structure
  - [ ] Write the document content
