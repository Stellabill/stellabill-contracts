# New Payment Asset Onboarding

> **Audience:** Contract administrators and integration engineers adding a new
> settlement token to the Stellabill subscription vault.
>
> **Related:** See the full [Multi-Token Onboarding Checklist](../multi_token_onboarding_checklist.md)
> for the complete step-by-step process. This document focuses on per-token
> configuration details and pre-flight vetting.

---

## Pre-Listing Vetting

Before calling `add_accepted_token`, verify the token contract:

### 1. Token Contract Verification

| Check | Tool / Method | Pass Criteria |
|---|---|---|
| Implements Soroban Token Interface | `soroban contract invoke --id <TOKEN> --fn decimals` | Returns a valid `u32` ≤ 19 |
| `transfer` works correctly | Test transfer to vault address | Vault balance increases, no fee deducted |
| No fee-on-transfer | Compare `sent` vs `received` | `sent == received` |
| No pausable transfers (if unacceptable) | Check token source / issuer docs | Transfers are always available, or ops team monitors pause events |
| No rebasing supply | Check token source | Supply only changes on mint/burn, not periodically |

### 2. Risk Classification

| Token Type | Risk | Recommendation |
|---|---|---|
| Major stablecoin (USDC, EURC) | Low | Onboard with standard monitoring |
| Bridged asset (wBTC, wETH) | Medium | Require oracle pricing; monitor bridge events |
| Protocol-native governance token | Medium | Validate transferability; monitor DAO proposals |
| New/unverified token | High | Require security review; consider emergency-stop gating |

---

## Per-Token Configuration

### Decimals & Normalization

The vault normalizes all amounts to a 9-decimal base for cross-token reconciliation:

| Token decimals | Normalization factor | Example |
|---|---|---|
| 6 (EURC) | × 1,000 | 1.00 EURC raw → 1,000.000000000 normalized |
| 7 (USDC, XLM) | × 100 | 1.00 USDC raw → 100.000000000 normalized |
| 18 (wETH) | N/A (rejected) | `decimals > 19` → `InvalidTokenDecimals` |

### Minimum Top-Up (`min_topup`)

The `min_topup` value is a **global** contract parameter (not per-token). It applies
to all `deposit_funds` calls regardless of token. When onboarding a new token with
different decimal precision than the default token, ensure `min_topup` is set to a
value that makes economic sense across all accepted tokens.

**Example:** If `min_topup = 1_000_000` (1.0 USDC at 7 decimals) and EURC (6 decimals)
is added, deposits of `100_000` EURC raw (0.10 EURC) will be rejected with
`BelowMinimumTopup` (5003).

> **Cross-reference:** See [`set_min_topup`](../integration_guide.md) in the
> integration guide and the [`min_topup` storage layout](../storage_layout.md).

### Oracle Wiring (if applicable)

If the new token is not the default settlement token and cross-currency plans will
use it, configure an oracle price feed:

1. **Select oracle kind:** `Spot` (latest price), `Twap` (median over window), or `FixedRate` (deterministic).
2. **Set `max_age_seconds`:** 300–900 s recommended for Spot/Twap.
3. **TWAP window:** Minimum 60 s; 300 s recommended for mainnet.
4. **Verify liveness:** Call `emit_oracle_liveness()` → `healthy == true`.

> **Cross-reference:** See [Oracle Pricing](../oracle_pricing.md) for full adapter details.

### Plan Templates

Create token-specific plan templates so subscribers can use the new asset:

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

---

## Required Admin Entrypoints

| Step | Entrypoint | Auth | Key Validation |
|---|---|---|---|
| Register token | `add_accepted_token(admin, token, decimals)` | Admin | `decimals ≤ 19`, token ≠ vault |
| Set oracle (optional) | `set_oracle_config(admin, enabled, oracle, max_age_seconds, kind, ...)` | Admin | `oracle` address valid, `max_age_seconds > 0` |
| Adjust min_topup | `set_min_topup(admin, min_topup)` | Admin | `min_topup > 0` |
| Create plan template | `create_plan_template_with_token(merchant, token, amount, ...)` | Merchant | Token is accepted |
| Remove token (if needed) | `remove_accepted_token(admin, token)` | Admin | Token ≠ default token |

---

## Post-Launch Monitoring

After the token is live with at least one active subscription:

### Reconciliation Health

Monitor `get_recon_summary()` — verify `is_balanced == true` for the new token.
Set an alert if it ever flips to `false`.

**Recommended cadence:** Every ledger close (5 s) for critical tokens; every
10 minutes for low-volume tokens.

### Oracle Liveness

If oracle pricing is configured, poll `emit_oracle_liveness()` on a schedule.
Alert when `healthy == false` (age > max_age_seconds / 2), providing a window
for intervention before charges fail with `OraclePriceStale`.

### Event Pipeline

Verify the indexer correctly decodes events involving the new token:

- `SubscriptionCreatedEvent.token`
- `FundsDepositedEvent.token`
- `SubscriptionChargedEvent.token`
- `MerchantWithdrawalEvent.token`
- `OracleChargeResolvedEvent`

### Merchant Withdrawal Testing

Periodically execute a test withdrawal for the new token to confirm the
full e2e flow remains operational:

1. `get_merchant_balance_by_token(merchant, token)` returns expected balance.
2. `withdraw_merchant_token_funds(merchant, token, amount)` succeeds.
3. Merchant's external wallet balance increases.

---

## Edge Cases

### Fee-on-Transfer Tokens

**Do NOT onboard.** The vault's accounting assumption (`debited == credited`)
breaks when the token deducts a fee on `transfer`. The reconciliation equation
will permanently show `is_balanced == false`.

### Tokens with Pausable Transfers

Acceptable **only if** the off-chain ops team monitors token-pause events and
has a runbook. When the token pauses transfers, all vault operations
(`deposit_funds`, `withdraw_*`, `charge_*`) will fail for that token.

Implementation: add the token's pause-event topic to the monitoring pipeline.

### Tokens with Non-Standard Decimals

- `decimals == 0` → rejected with `InvalidTokenDecimals` (8001).
- `decimals > 19` → rejected with `InvalidTokenDecimals` (8001).
- Tokens that don't expose `decimals()` → incompatible.

### Rebasing / Elastic Supply Tokens

**Do NOT onboard.** The vault stores fixed `i128` balances and does not rebase.
Silent accounting drift would accumulate over time.

---

## Related Documentation

- [Multi-Token Onboarding Checklist](../multi_token_onboarding_checklist.md) — Full step-by-step walkthrough
- [Multi-Token Architecture](../multi_token.md) — Token registry, isolation, and migration
- [Oracle Pricing](../oracle_pricing.md) — Oracle adapter architecture and configuration
- [Storage Layout](../storage_layout.md) — On-chain key reference
- [Integration Guide](../integration_guide.md) — Entrypoint reference with auth requirements
- [Protocol Invariants](../protocol_invariants.md) — Accounting properties that must hold
- [Security](../security.md) — Threat model and access control
