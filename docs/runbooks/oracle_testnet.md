# Oracle Adapter Testnet Deployment Runbook

> **Issue #853** — Operator runbook for deploying `oracle_adapter` to testnet  
> **Audience:** On-call engineers responding to testnet incidents  
> **Last updated:** 2026-07-30  
> **Stellar testnet passphrase:** `Test SDF Network ; September 2015`

---

## Table of Contents

1. [Overview](#overview)
2. [Prerequisites](#prerequisites)
3. [Pre-Deployment Checklist](#pre-deployment-checklist)
4. [Step 1: Oracle Admin Registration](#step-1-oracle-admin-registration)
5. [Step 2: Initial Price Submission](#step-2-initial-price-submission)
6. [Step 3: Freshness Validation](#step-3-freshness-validation)
7. [Step 4: Vault Oracle Configuration](#step-4-vault-oracle-configuration)
8. [Step 5: End-to-End Charge Verification](#step-5-end-to-end-charge-verification)
9. [Monitoring & Alerting](#monitoring--alerting)
10. [Failure Recovery](#failure-recovery)
11. [Rollback Procedures](#rollback-procedures)
12. [Edge Case Procedures](#edge-case-procedures)
13. [Security Assumptions](#security-assumptions)
14. [Contact Points & Paging](#contact-points--paging)
15. [Appendix: Error Reference](#appendix-error-reference)
16. [Appendix: Event Reference](#appendix-event-reference)

---

## Overview

This runbook covers the complete deployment and operational validation of the `oracle_adapter` pricing module on **Stellar testnet** for the Subscription Vault contract. The oracle adapter enables cross-currency subscription billing by resolving quote-denominated amounts into token base units at charge time.

### Supported Pricing Strategies

| Strategy | Description | Oracle Reads | Use Case |
|----------|-------------|--------------|----------|
| **Spot** | Latest single price sample | Yes | Default; low-latency billing |
| **TWAP** | Median across sliding window | Yes | Manipulation-resistant pricing |
| **FixedRate** | Deterministic ratio (no oracle) | No | Pegged pairs; test environments |

### Deployment Flow

```
┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
│ 1. Register     │────▶│ 2. Submit Initial │────▶│ 3. Validate     │
│    Oracle Admin │     │    Price          │     │    Freshness    │
└─────────────────┘     └──────────────────┘     └─────────────────┘
         │                                               │
         ▼                                               ▼
┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
│ 6. Monitor &    │◀────│ 5. Verify E2E    │◀────│ 4. Configure    │
│    Alert        │     │    Charge          │     │    Vault Oracle │
└─────────────────┘     └──────────────────┘     └─────────────────┘
```

---

## Prerequisites

### Required Tools

- **Stellar CLI** (`stellar`) — latest stable release
- **Soroban RPC endpoint** — testnet access
- **Admin private key** — stored in secure key management (1Password / HashiCorp Vault)
- **Oracle contract Wasm** — pre-compiled and tested

### Environment Variables

Set these in your shell before proceeding:

```bash
export NETWORK_PASSPHRASE="Test SDF Network ; September 2015"
export RPC_URL="https://soroban-rpc.testnet.stellar.org"
export ADMIN_SECRET="<your-admin-secret-key>"
export CONTRACT_ID="<subscription-vault-contract-id>"
```

### Helper: Derive Admin Address

```bash
export ADMIN_ADDRESS=$(stellar address from-secret "$ADMIN_SECRET")
echo "Admin address: $ADMIN_ADDRESS"
```

> **Security note:** Never commit `ADMIN_SECRET` to version control. Use environment-specific secret injection.

---

## Pre-Deployment Checklist

Run these checks before any state-mutating operation:

### 1. Verify Contract State

```bash
stellar contract invoke   --id "$CONTRACT_ID"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --source "$ADMIN_SECRET"   --   get_admin
```

**Expected:** Returns the admin address matching `$ADMIN_ADDRESS`.

### 2. Verify No Existing Oracle Config

```bash
stellar contract invoke   --id "$CONTRACT_ID"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --source "$ADMIN_SECRET"   --   get_oracle_config
```

**Expected:**
```json
{
  "enabled": false,
  "oracle": null,
  "max_age_seconds": 0,
  "kind": "Spot",
  "window_secs": 0,
  "fixed_numerator": 0,
  "fixed_denominator": 1
}
```

> If `enabled: true`, consult [Rollback: Disable Existing Oracle](#rollback-disable-existing-oracle) before proceeding.

### 3. Verify Contract Not in Emergency Stop

```bash
stellar contract invoke   --id "$CONTRACT_ID"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --source "$ADMIN_SECRET"   --   get_emergency_stop_status
```

**Expected:** `false`. If `true`, resolve the emergency before proceeding.

### 4. Verify Admin Nonce (for replay protection)

```bash
stellar contract invoke   --id "$CONTRACT_ID"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --source "$ADMIN_SECRET"   --   get_admin_nonce   --signer "$ADMIN_ADDRESS"   --domain 0
```

**Record the returned nonce** — you will need it for `set_oracle_config` (domain 0 = general admin ops).

---

## Step 1: Oracle Admin Registration

### 1.1 Deploy the Oracle Contract

If the oracle contract is not yet deployed:

```bash
# Build the oracle contract (from oracle repo root)
cd /path/to/oracle-contract
cargo build --target wasm32-unknown-unknown --release

# Deploy to testnet
stellar contract deploy   --wasm target/wasm32-unknown-unknown/release/oracle.wasm   --source "$ADMIN_SECRET"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"
```

**Record the returned contract ID** as `ORACLE_ID`.

### 1.2 Initialize the Oracle Contract

```bash
stellar contract invoke   --id "$ORACLE_ID"   --source "$ADMIN_SECRET"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --   init   --admin "$ADMIN_ADDRESS"
```

**Verification:**

```bash
stellar contract invoke   --id "$ORACLE_ID"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --source "$ADMIN_SECRET"   --   get_admin
```

**Expected:** Returns `$ADMIN_ADDRESS`.

### 1.3 Register Accepted Tokens with the Oracle

The oracle must know which token pairs it will price. For each token:

```bash
export TOKEN_ADDRESS="<token-contract-address>"
export TOKEN_DECIMALS=7  # e.g., USDC = 6, XLM = 7

stellar contract invoke   --id "$ORACLE_ID"   --source "$ADMIN_SECRET"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --   add_asset   --asset "$TOKEN_ADDRESS"   --decimals "$TOKEN_DECIMALS"
```

**Verification:**

```bash
stellar contract invoke   --id "$ORACLE_ID"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --source "$ADMIN_SECRET"   --   get_asset   --asset "$TOKEN_ADDRESS"
```

**Expected:** Returns `{ "address": "...", "decimals": 7 }`.

---

## Step 2: Initial Price Submission

### 2.1 Submit the First Price Observation

The oracle contract must have at least one price sample before the vault can charge subscriptions.

```bash
export BASE_TOKEN="<base-token-address>"    # e.g., USDC
export QUOTE_TOKEN="<quote-token-address>"  # e.g., XLM
export PRICE=15000000                         # 1.5 in PRICE_SCALE (10^7)
                                                # e.g., 1 USDC = 1.5 XLM

stellar contract invoke   --id "$ORACLE_ID"   --source "$ADMIN_SECRET"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --   set_price   --base "$BASE_TOKEN"   --quote "$QUOTE_TOKEN"   --price "$PRICE"
```

> **Price scale:** All oracle prices use `PRICE_SCALE = 10^7`. A price of `15,000,000` means `1.5` quote units per 1 base unit.

### 2.2 Verify Price Retrieval

```bash
stellar contract invoke   --id "$ORACLE_ID"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --source "$ADMIN_SECRET"   --   latest_price   --base "$BASE_TOKEN"   --quote "$QUOTE_TOKEN"
```

**Expected:**
```json
{
  "price": 15000000,
  "timestamp": <current-ledger-timestamp>
}
```

### 2.3 Submit Multiple Observations (for TWAP strategy)

If using **Twap** strategy, seed the oracle with multiple observations across the intended window:

```bash
# Submit 5 observations, 30 seconds apart (simulated)
for i in $(seq 1 5); do
  stellar contract invoke     --id "$ORACLE_ID"     --source "$ADMIN_SECRET"     --rpc-url "$RPC_URL"     --network-passphrase "$NETWORK_PASSPHRASE"     --     set_price     --base "$BASE_TOKEN"     --quote "$QUOTE_TOKEN"     --price "$PRICE"
  sleep 30
done
```

**Verification:**

```bash
stellar contract invoke   --id "$ORACLE_ID"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --source "$ADMIN_SECRET"   --   get_observations   --base "$BASE_TOKEN"   --quote "$QUOTE_TOKEN"   --since 0
```

**Expected:** Returns a vector of `OraclePrice` structs with monotonically increasing timestamps.

---

## Step 3: Freshness Validation

### 3.1 Configure Vault Oracle with Conservative Freshness

Set `max_age_seconds` to a value that gives operators time to respond before charges fail. For testnet, **300 seconds (5 minutes)** is recommended.

```bash
export ORACLE_KIND="Spot"        # Options: Spot, Twap, FixedRate
export MAX_AGE_SECONDS=300
export WINDOW_SECS=0             # Ignored for Spot/FixedRate
export FIXED_NUMERATOR=0         # Ignored for Spot/Twap
export FIXED_DENOMINATOR=1       # Ignored for Spot/Twap

stellar contract invoke   --id "$CONTRACT_ID"   --source "$ADMIN_SECRET"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --   set_oracle_config   --admin "$ADMIN_ADDRESS"   --enabled true   --oracle "$ORACLE_ID"   --max_age_seconds "$MAX_AGE_SECONDS"   --kind "$ORACLE_KIND"   --window_secs "$WINDOW_SECS"   --fixed_numerator "$FIXED_NUMERATOR"   --fixed_denominator "$FIXED_DENOMINATOR"
```

**Expected:** Transaction succeeds. Event `oracle_config_updated` is emitted.

### 3.2 Verify Configuration Persistence

```bash
stellar contract invoke   --id "$CONTRACT_ID"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --source "$ADMIN_SECRET"   --   get_oracle_config
```

**Expected:**
```json
{
  "enabled": true,
  "oracle": "<ORACLE_ID>",
  "max_age_seconds": 300,
  "kind": "Spot",
  "window_secs": 0,
  "fixed_numerator": 0,
  "fixed_denominator": 1
}
```

### 3.3 Run Liveness Check

```bash
stellar contract invoke   --id "$CONTRACT_ID"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --source "$ADMIN_SECRET"   --   emit_oracle_liveness
```

**Expected:**
```json
{
  "last_sample_ts": <timestamp>,
  "age": <seconds-since-last-sample>,
  "healthy": true,
  "timestamp": <current-ledger-timestamp>,
  "schema_version": 2
}
```

> **Healthy threshold:** `age <= max_age_seconds / 2` (150 seconds for 300s max_age).  
> If `healthy: false`, the oracle is approaching staleness — investigate immediately.

### 3.4 Set Deviation Circuit Breaker (Optional but Recommended)

```bash
export DEVIATION_BPS=500  # 5% maximum deviation from median

stellar contract invoke   --id "$CONTRACT_ID"   --source "$ADMIN_SECRET"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --   set_oracle_deviation_bps   --admin "$ADMIN_ADDRESS"   --bps "$DEVIATION_BPS"
```

> **Behavior:** If the latest price deviates more than 5% from the median of the last 10 samples, charges are rejected with `Error::OracleDeviationTooHigh` (code 3009).

---

## Step 4: Vault Oracle Configuration

### 4.1 Add the Token to Accepted Tokens List

The settlement token must be in the vault's accepted tokens list:

```bash
stellar contract invoke   --id "$CONTRACT_ID"   --source "$ADMIN_SECRET"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --   add_accepted_token   --admin "$ADMIN_ADDRESS"   --token "$TOKEN_ADDRESS"   --decimals "$TOKEN_DECIMALS"
```

### 4.2 Verify Token Acceptance

```bash
stellar contract invoke   --id "$CONTRACT_ID"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --source "$ADMIN_SECRET"   --   list_accepted_tokens
```

**Expected:** The token appears in the returned list with correct decimals.

---

## Step 5: End-to-End Charge Verification

### 5.1 Create a Test Subscription with Oracle Pricing

```bash
export SUBSCRIBER_SECRET="<subscriber-secret>"
export SUBSCRIBER_ADDRESS=$(stellar address from-secret "$SUBSCRIBER_SECRET")
export MERCHANT_ADDRESS="<merchant-address>"
export AMOUNT=10000000           # 10.0 in quote units (e.g., USD cents)
export INTERVAL_SECONDS=86400    # 1 day

stellar contract invoke   --id "$CONTRACT_ID"   --source "$SUBSCRIBER_SECRET"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --   create_subscription_with_token   --subscriber "$SUBSCRIBER_ADDRESS"   --merchant "$MERCHANT_ADDRESS"   --token "$TOKEN_ADDRESS"   --amount "$AMOUNT"   --interval_seconds "$INTERVAL_SECONDS"   --usage_enabled false   --lifetime_cap null   --expires_at null   --expires_at_ledger null   --sub_account_label null
```

**Record the returned subscription ID** as `SUBSCRIPTION_ID`.

### 5.2 Deposit Funds

```bash
stellar contract invoke   --id "$CONTRACT_ID"   --source "$SUBSCRIBER_SECRET"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --   deposit_funds   --subscription_id "$SUBSCRIPTION_ID"   --subscriber "$SUBSCRIBER_ADDRESS"   --amount 500000000
```

### 5.3 Trigger a Charge

```bash
stellar contract invoke   --id "$CONTRACT_ID"   --source "$ADMIN_SECRET"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --   charge_subscription   --subscription_id "$SUBSCRIPTION_ID"
```

**Expected:** Transaction succeeds. The charge amount is converted from quote units to token base units using the oracle price.

### 5.4 Verify Charge Event

Query the contract events for `oracle_charge_resolved`:

```bash
# Using stellar-cli event query (or your indexer)
stellar events   --id "$CONTRACT_ID"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --topic "oracle_charge_resolved"
```

**Expected event payload:**
```json
{
  "subscription_id": <SUBSCRIPTION_ID>,
  "quote_amount": 10000000,
  "token_amount": <converted-amount>,
  "price": 15000000,
  "price_timestamp": <oracle-timestamp>,
  "timestamp": <charge-timestamp>,
  "schema_version": 2
}
```

> **Conversion formula:** `token_amount = ceil(quote_amount * 10^token_decimals / price)`  
> Example: `ceil(10,000,000 * 10^7 / 15,000,000) = ceil(6,666,666.67) = 6,666,667` token base units

### 5.5 Verify Subscription State

```bash
stellar contract invoke   --id "$CONTRACT_ID"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --source "$ADMIN_SECRET"   --   get_subscription   --subscription_id "$SUBSCRIPTION_ID"
```

**Expected:** `lifetime_charged` increased by the token amount, `last_payment_timestamp` updated.

---

## Monitoring & Alerting

### Automated Liveness Checks

Schedule this check every 60 seconds via your monitoring system (Datadog, Grafana, PagerDuty):

```bash
#!/bin/bash
# oracle_health_check.sh

RESULT=$(stellar contract invoke   --id "$CONTRACT_ID"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --source "$ADMIN_SECRET"   --   emit_oracle_liveness 2>&1)

if echo "$RESULT" | grep -q '"healthy": false'; then
  echo "CRITICAL: Oracle unhealthy — age approaching staleness threshold"
  # Trigger PagerDuty / Slack alert
  curl -X POST "$PAGERDUTY_INTEGRATION_KEY"     -H "Content-Type: application/json"     -d '{
      "routing_key": "'"$PAGERDUTY_ROUTING_KEY"'",
      "event_action": "trigger",
      "payload": {
        "summary": "Stellabill testnet oracle approaching staleness",
        "severity": "critical",
        "source": "oracle_health_check",
        "custom_details": { "result": "'"$RESULT"'" }
      }
    }'
  exit 1
elif echo "$RESULT" | grep -q '"healthy": true'; then
  echo "OK: Oracle healthy"
  exit 0
else
  echo "UNKNOWN: Oracle liveness check failed — $RESULT"
  exit 2
fi
```

### Paging Thresholds

| Condition | Severity | Auto-page | Response SLA |
|-----------|----------|-----------|--------------|
| `healthy: false` (age > max_age/2) | **Warning** | No | 15 min |
| `OraclePriceStale` (code 5008) on charge | **Critical** | Yes | 5 min |
| `OraclePriceUnavailable` (code 5007) | **Critical** | Yes | 5 min |
| `OracleDeviationTooHigh` (code 3009) | **Critical** | Yes | 5 min |
| `OracleNotConfigured` (code 5006) | **Critical** | Yes | 5 min |
| Oracle contract not responding | **Critical** | Yes | 5 min |

### Dashboard Metrics

Track these metrics in your observability platform:

- `oracle_liveness_age` — Current oracle sample age in seconds
- `oracle_liveness_healthy` — Boolean (1 = healthy, 0 = unhealthy)
- `oracle_charge_resolved_count` — Successful oracle-based charges per hour
- `oracle_charge_failure_count` — Failed charges by error code per hour
- `oracle_price_deviation_bps` — Current deviation from median (if circuit breaker enabled)

---

## Failure Recovery

### Scenario A: Oracle Price is Stale (`Error::OraclePriceStale`, code 5008)

**Symptoms:** Charges fail. Liveness check shows `healthy: false` or age > `max_age_seconds`.

**Root Causes:**
- Oracle price feed stopped updating
- Network congestion delayed oracle transactions
- Oracle operator key compromised or lost

**Recovery Steps:**

1. **Check oracle contract directly:**
   ```bash
   stellar contract invoke      --id "$ORACLE_ID"      --rpc-url "$RPC_URL"      --network-passphrase "$NETWORK_PASSPHRASE"      --source "$ADMIN_SECRET"      --      latest_price      --base "$BASE_TOKEN"      --quote "$QUOTE_TOKEN"
   ```

2. **If the oracle has no recent price, submit a fresh price:**
   ```bash
   stellar contract invoke      --id "$ORACLE_ID"      --source "$ADMIN_SECRET"      --rpc-url "$RPC_URL"      --network-passphrase "$NETWORK_PASSPHRASE"      --      set_price      --base "$BASE_TOKEN"      --quote "$QUOTE_TOKEN"      --price "$PRICE"
   ```

3. **Verify freshness:**
   ```bash
   stellar contract invoke      --id "$CONTRACT_ID"      --rpc-url "$RPC_URL"      --network-passphrase "$NETWORK_PASSPHRASE"      --source "$ADMIN_SECRET"      --      emit_oracle_liveness
   ```

4. **Retry the failed charge:**
   ```bash
   stellar contract invoke      --id "$CONTRACT_ID"      --source "$ADMIN_SECRET"      --rpc-url "$RPC_URL"      --network-passphrase "$NETWORK_PASSPHRASE"      --      charge_subscription      --subscription_id "$SUBSCRIPTION_ID"
   ```

> **Note:** `OraclePriceStale` is safe to retry after the oracle recovers. Do not auto-retry blindly — verify the oracle first.

---

### Scenario B: Oracle Price is Unavailable (`Error::OraclePriceUnavailable`, code 5007)

**Symptoms:** Charges fail. `latest_price` returns empty or malformed payload.

**Root Causes:**
- Oracle contract not initialized
- Token pair not registered with oracle
- Oracle contract wasm corrupted or upgraded incorrectly

**Recovery Steps:**

1. **Verify oracle initialization:**
   ```bash
   stellar contract invoke      --id "$ORACLE_ID"      --rpc-url "$RPC_URL"      --network-passphrase "$NETWORK_PASSPHRASE"      --source "$ADMIN_SECRET"      --      get_admin
   ```
   If this fails, the oracle is not initialized. Re-deploy and re-initialize.

2. **Verify token pair registration:**
   ```bash
   stellar contract invoke      --id "$ORACLE_ID"      --rpc-url "$RPC_URL"      --network-passphrase "$NETWORK_PASSPHRASE"      --source "$ADMIN_SECRET"      --      get_asset      --asset "$TOKEN_ADDRESS"
   ```
   If missing, re-register the token (see [Step 1.3](#13-register-accepted-tokens-with-the-oracle)).

3. **Re-submit initial price** (see [Step 2](#step-2-initial-price-submission)).

---

### Scenario C: Oracle Deviation Too High (`Error::OracleDeviationTooHigh`, code 3009)

**Symptoms:** Charges fail. Event `oracle_deviation_breaker` emitted.

**Root Causes:**
- Legitimate market volatility exceeding threshold
- Oracle price feed manipulation or compromise
- Stale median due to infrequent updates

**Recovery Steps:**

1. **Check the deviation event:**
   ```bash
   stellar events      --id "$CONTRACT_ID"      --rpc-url "$RPC_URL"      --network-passphrase "$NETWORK_PASSPHRASE"      --topic "oracle_deviation_breaker"
   ```

2. **Verify off-chain price sources:**
   - Check CoinGecko, CoinMarketCap, or your primary exchange for the actual market price
   - Compare with the oracle's `latest_price`

3. **If the oracle price is wrong (compromised feed):**
   - **Do not** disable the circuit breaker
   - Investigate the oracle source
   - If using a multi-source oracle, check which source is anomalous
   - Consider switching to `FixedRate` temporarily (see [Rollback: Switch to FixedRate](#rollback-switch-to-fixedrate))

4. **If the market is genuinely volatile:**
   - Temporarily raise the deviation threshold (admin only):
     ```bash
     stellar contract invoke        --id "$CONTRACT_ID"        --source "$ADMIN_SECRET"        --rpc-url "$RPC_URL"        --network-passphrase "$NETWORK_PASSPHRASE"        --        set_oracle_deviation_bps        --admin "$ADMIN_ADDRESS"        --bps 1000  # 10% temporarily
     ```
   - Monitor closely. Lower the threshold once volatility subsides.

---

### Scenario D: Oracle Contract Not Responding

**Symptoms:** All `stellar contract invoke` calls to `$ORACLE_ID` timeout or return host errors.

**Root Causes:**
- Oracle contract TTL expired (testnet has short TTLs)
- Oracle contract was accidentally deleted
- Soroban RPC endpoint issues

**Recovery Steps:**

1. **Check contract existence:**
   ```bash
   stellar contract info      --id "$ORACLE_ID"      --rpc-url "$RPC_URL"      --network-passphrase "$NETWORK_PASSPHRASE"
   ```

2. **If contract TTL expired, re-deploy:**
   ```bash
   # Re-deploy the oracle contract
   stellar contract deploy      --wasm target/wasm32-unknown-unknown/release/oracle.wasm      --source "$ADMIN_SECRET"      --rpc-url "$RPC_URL"      --network-passphrase "$NETWORK_PASSPHRASE"
   ```
   Update `$ORACLE_ID` and re-run [Step 1](#step-1-oracle-admin-registration) through [Step 3](#step-3-freshness-validation).

3. **Update vault oracle config with new oracle address:**
   ```bash
   stellar contract invoke      --id "$CONTRACT_ID"      --source "$ADMIN_SECRET"      --rpc-url "$RPC_URL"      --network-passphrase "$NETWORK_PASSPHRASE"      --      set_oracle_config      --admin "$ADMIN_ADDRESS"      --enabled true      --oracle "$NEW_ORACLE_ID"      --max_age_seconds "$MAX_AGE_SECONDS"      --kind "$ORACLE_KIND"      --window_secs "$WINDOW_SECS"      --fixed_numerator "$FIXED_NUMERATOR"      --fixed_denominator "$FIXED_DENOMINATOR"
   ```

---

## Rollback Procedures

### Rollback: Disable Existing Oracle

If the oracle deployment is causing widespread charge failures, disable oracle pricing immediately:

```bash
stellar contract invoke   --id "$CONTRACT_ID"   --source "$ADMIN_SECRET"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --   set_oracle_config   --admin "$ADMIN_ADDRESS"   --enabled false   --oracle null   --max_age_seconds 0   --kind "Spot"   --window_secs 0   --fixed_numerator 0   --fixed_denominator 1
```

**Effect:** All subscriptions revert to token-denominated amounts (`subscription.amount` is used directly). No oracle reads occur during charging.

**Post-rollback verification:**
```bash
stellar contract invoke   --id "$CONTRACT_ID"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --source "$ADMIN_SECRET"   --   get_oracle_config
```

**Expected:** `enabled: false`.

---

### Rollback: Switch to FixedRate

If the oracle feed is compromised but you need to maintain cross-currency billing:

```bash
export FIXED_RATE=15000000  # 1.5 in PRICE_SCALE (10^7)
# fixed_numerator * PRICE_SCALE / fixed_denominator = FIXED_RATE
# e.g., fixed_numerator=15, fixed_denominator=10 -> 15 * 10^7 / 10 = 15,000,000

stellar contract invoke   --id "$CONTRACT_ID"   --source "$ADMIN_SECRET"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --   set_oracle_config   --admin "$ADMIN_ADDRESS"   --enabled true   --oracle null   --max_age_seconds 0   --kind "FixedRate"   --window_secs 0   --fixed_numerator 15   --fixed_denominator 10
```

**Effect:** Charges use the deterministic fixed rate. No oracle contract reads. Staleness checks are skipped.

**Security note:** The fixed rate can only be changed by admin auth. Monitor for unauthorized config changes.

---

### Rollback: Revert to Previous Oracle Address

If a new oracle address is misconfigured, revert to the previous known-good address:

```bash
export PREVIOUS_ORACLE_ID="<previous-oracle-id>"

stellar contract invoke   --id "$CONTRACT_ID"   --source "$ADMIN_SECRET"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --   set_oracle_config   --admin "$ADMIN_ADDRESS"   --enabled true   --oracle "$PREVIOUS_ORACLE_ID"   --max_age_seconds "$MAX_AGE_SECONDS"   --kind "$ORACLE_KIND"   --window_secs "$WINDOW_SECS"   --fixed_numerator "$FIXED_NUMERATOR"   --fixed_denominator "$FIXED_DENOMINATOR"
```

---

## Edge Case Procedures

### Edge Case 1: Freshness Threshold Change During Incident

**Scenario:** You need to raise `max_age_seconds` because the oracle feed is experiencing delays.

**Procedure:**

1. **Current config cooldown check:** The `set_oracle_config` call enforces a 6-hour config cooldown (`CONFIG_COOLDOWN_SECS = 21,600`). If you recently changed oracle config, the call will fail with `Error::CooldownActive` (code 12001).

2. **If cooldown is active, use emergency governance (if available):**
   - Submit a governance proposal to override the cooldown
   - Requires guardian quorum
   - See `docs/governance/authoring.md` for proposal format

3. **If no governance is configured, wait for cooldown to elapse** or use the [Rollback: Switch to FixedRate](#rollback-switch-to-fixedrate) as a temporary measure.

4. **Apply the threshold change:**
   ```bash
   stellar contract invoke      --id "$CONTRACT_ID"      --source "$ADMIN_SECRET"      --rpc-url "$RPC_URL"      --network-passphrase "$NETWORK_PASSPHRASE"      --      set_oracle_config      --admin "$ADMIN_ADDRESS"      --enabled true      --oracle "$ORACLE_ID"      --max_age_seconds 600      --kind "$ORACLE_KIND"      --window_secs "$WINDOW_SECS"      --fixed_numerator "$FIXED_NUMERATOR"      --fixed_denominator "$FIXED_DENOMINATOR"
   ```

> **Warning:** Raising `max_age_seconds` increases the window for stale prices to be accepted. Lower it back to the original value (300s) once the incident is resolved.

---

### Edge Case 2: Oracle Upgrade During Incident

**Scenario:** The oracle contract needs an urgent upgrade (e.g., security patch) while charges are failing.

**Procedure:**

1. **Disable oracle pricing first** (prevents charges from failing during the upgrade window):
   ```bash
   # See Rollback: Disable Existing Oracle
   ```

2. **Deploy the upgraded oracle contract:**
   ```bash
   stellar contract deploy      --wasm target/wasm32-unknown-unknown/release/oracle_v2.wasm      --source "$ADMIN_SECRET"      --rpc-url "$RPC_URL"      --network-passphrase "$NETWORK_PASSPHRASE"
   ```

3. **Initialize the new oracle and migrate state** (follow the oracle contract's migration guide).

4. **Re-configure the vault with the new oracle address:**
   ```bash
   # Use the new oracle ID
   ```

5. **Run full validation** ([Step 3](#step-3-freshness-validation) through [Step 5](#step-5-end-to-end-charge-verification)).

6. **Re-enable oracle pricing** only after validation passes.

---

### Edge Case 3: Price Rollback

**Scenario:** An incorrect price was submitted to the oracle and consumed by charges before the deviation circuit breaker caught it.

**Procedure:**

1. **Identify affected subscriptions:** Query `oracle_charge_resolved` events for the incorrect price timestamp:
   ```bash
   stellar events      --id "$CONTRACT_ID"      --rpc-url "$RPC_URL"      --network-passphrase "$NETWORK_PASSPHRASE"      --topic "oracle_charge_resolved"      --start-ledger <incident-start-ledger>      --end-ledger <incident-end-ledger>
   ```

2. **Calculate the overcharge/undercharge amount:**
   - Correct price: `P_correct`
   - Incorrect price: `P_incorrect`
   - Quote amount: `Q`
   - Token decimals: `D`
   - Correct token amount: `ceil(Q * 10^D / P_correct)`
   - Incorrect token amount: `ceil(Q * 10^D / P_incorrect)`
   - Delta: `incorrect - correct` (positive = overcharge, negative = undercharge)

3. **For overcharged subscriptions:**
   - Use `partial_refund` to return the delta to the subscriber:
     ```bash
     stellar contract invoke        --id "$CONTRACT_ID"        --source "$ADMIN_SECRET"        --rpc-url "$RPC_URL"        --network-passphrase "$NETWORK_PASSPHRASE"        --        partial_refund        --admin "$ADMIN_ADDRESS"        --subscription_id "$SUBSCRIPTION_ID"        --subscriber "$SUBSCRIBER_ADDRESS"        --amount "$DELTA"
     ```

4. **For undercharged subscriptions:**
   - The merchant was underpaid. Use `merchant_refund` to recover from the merchant, then deposit to the subscription:
     ```bash
     # This is a business decision — consult finance team
     ```

5. **Submit the correct price to the oracle:**
   ```bash
   stellar contract invoke      --id "$ORACLE_ID"      --source "$ADMIN_SECRET"      --rpc-url "$RPC_URL"      --network-passphrase "$NETWORK_PASSPHRASE"      --      set_price      --base "$BASE_TOKEN"      --quote "$QUOTE_TOKEN"      --price "$P_CORRECT"
   ```

6. **Document the incident** in the post-mortem tracker with:
   - Incorrect price and timestamp
   - Affected subscription IDs
   - Refund amounts
   - Root cause (oracle source error, operator mistake, etc.)

---

### Edge Case 4: TWAP Window Too Short

**Scenario:** Using TWAP strategy but `window_secs` is set below the minimum (`MIN_TWAP_WINDOW_SECS = 60`).

**Symptoms:** `set_oracle_config` fails with `Error::InvalidInput` (code 3002).

**Recovery:**
```bash
# Set window to at least 60 seconds (recommended: 300 for testnet)
export WINDOW_SECS=300

stellar contract invoke   --id "$CONTRACT_ID"   --source "$ADMIN_SECRET"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --   set_oracle_config   --admin "$ADMIN_ADDRESS"   --enabled true   --oracle "$ORACLE_ID"   --max_age_seconds "$MAX_AGE_SECONDS"   --kind "Twap"   --window_secs "$WINDOW_SECS"   --fixed_numerator 0   --fixed_denominator 1
```

---

### Edge Case 5: FixedRate Denominator is Zero

**Scenario:** Switching to FixedRate but `fixed_denominator` is accidentally set to 0.

**Symptoms:** `set_oracle_config` fails with `Error::InvalidInput` (code 3002).

**Recovery:**
```bash
# Always set denominator > 0
stellar contract invoke   --id "$CONTRACT_ID"   --source "$ADMIN_SECRET"   --rpc-url "$RPC_URL"   --network-passphrase "$NETWORK_PASSPHRASE"   --   set_oracle_config   --admin "$ADMIN_ADDRESS"   --enabled true   --oracle null   --max_age_seconds 0   --kind "FixedRate"   --window_secs 0   --fixed_numerator 15   --fixed_denominator 10  # Must be > 0
```

---

## Security Assumptions

### Invariants

| # | Assumption | Violation Impact | Detection |
|---|-----------|------------------|-----------|
| 1 | Admin private key is secure | Attacker can reconfigure oracle, steal funds | Monitor `set_oracle_config` events; unexpected config changes page on-call |
| 2 | Oracle contract is not self-owned | Permanent admin lock | Verify `get_admin` != contract address after init |
| 3 | `max_age_seconds > 0` when enabled | Zero disables staleness guard (rejected by contract) | Contract enforces at config time |
| 4 | Deviation threshold is reasonable | Too high = no protection; too low = false positives | Start with 500 bps (5%), tune based on asset volatility |
| 5 | TWAP window >= 60s | Shorter windows are vulnerable to flash-loan manipulation | Contract enforces `MIN_TWAP_WINDOW_SECS` |
| 6 | FixedRate denominator != 0 | Division by zero panic | Contract validates at config time |
| 7 | Oracle price > 0 | Non-positive prices are rejected | `validate_price` rejects ≤ 0 |
| 8 | Price history ring buffer (10 samples) | Oldest samples are overwritten; median uses last 10 | Monitor `oracle_deviation_breaker` events for frequent triggers |

### Threat Model

| Threat | Likelihood | Impact | Mitigation |
|--------|-----------|--------|------------|
| Oracle feed compromise (single source) | Medium | High — incorrect charges | TWAP median; deviation circuit breaker; multi-source oracle |
| Admin key compromise | Low | Critical — full control | 2-of-3 multisig admin; governance proposals for config changes |
| Stale oracle (operator outage) | Medium | Medium — charges fail | Automated liveness monitoring; paging at 50% threshold |
| Front-running price updates | Low | Low — minor arbitrage | TWAP window dilutes single-block manipulation |
| Replay attack on config change | Low | Medium — config reverted | Nonce-based replay protection on admin ops |

---

## Contact Points & Paging

### On-Call Escalation

| Level | Role | Contact | Response Time |
|-------|------|---------|---------------|
| L1 | On-call Engineer | PagerDuty rotation | 5 min (critical) |
| L2 | Protocol Lead | Slack: `#protocol-alerts` | 15 min |
| L3 | Security Team | security@stellabill.io | 30 min |
| L4 | Stellar Foundation | Testnet support channel | 2 hours |

### Alert Routing

```
┌─────────────────┐
│  Monitoring     │
│  (Datadog)      │
└────────┬────────┘
         │
    ┌────┴────┐
    ▼         ▼
┌────────┐ ┌──────────┐
│Warning │ │ Critical │
│(Slack) │ │(PagerDuty)│
└────────┘ └────┬─────┘
                │
         ┌──────┴──────┐
         ▼             ▼
    ┌─────────┐  ┌──────────┐
    │ L1 Eng  │─▶│ L2 Lead  │
    │ 5 min   │  │ 15 min   │
    └─────────┘  └──────────┘
```

### Runbook Feedback

If this runbook is unclear, incorrect, or missing a scenario:
- Open an issue: `https://github.com/Stellabill/stellabill-contracts/issues`
- Tag: `runbook`, `oracle`, `on-call`
- Assign: `@protocol-team`

---

## Appendix: Error Reference

### Oracle-Specific Error Codes

| Code | Variant | Meaning | Recovery |
|------|---------|---------|----------|
| 3007 | `OraclePriceInvalid` | Oracle returned non-positive price | Investigate oracle data feed; do not retry blindly |
| 3009 | `OracleDeviationTooHigh` | Price deviation exceeds threshold | Check market volatility; raise threshold temporarily if legitimate |
| 5006 | `OracleNotConfigured` | Oracle enabled but no address set, or disabled | Admin must call `set_oracle_config` with valid oracle |
| 5007 | `OraclePriceUnavailable` | Oracle payload missing or malformed | Retry after oracle data recovers; check oracle contract health |
| 5008 | `OraclePriceStale` | Oracle quote older than `max_age_seconds` | Retry after fresh quote is published; check oracle update cadence |
| 12001 | `CooldownActive` | Config mutation attempted within 6-hour cooldown | Wait for cooldown or use governance override |

### Retry Guidance

- **Safe to retry after recovery:** `OraclePriceStale`, `OraclePriceUnavailable`
- **Fix input before retry:** `OracleNotConfigured`, `InvalidInput` (bad config)
- **Investigate before retry:** `OraclePriceInvalid`, `OracleDeviationTooHigh`
- **Never auto-retry:** `CooldownActive` (wait or use governance)

---

## Appendix: Event Reference

### Oracle Events

| Event | Topic | Emitted By | Payload |
|-------|-------|------------|---------|
| `oracle_config_updated` | `["oracle_config_updated"]` | `set_oracle_config` | `OracleConfigUpdatedEvent` — full config state |
| `oracle_charge_resolved` | `["oracle_charge_resolved"]` | Charge path | `OracleChargeResolvedEvent` — quote/token amounts, price used |
| `oracle_liveness` | `["oracle_liveness"]` | `emit_oracle_liveness` | `OracleLivenessEvent` — age, healthy flag |
| `oracle_deviation_breaker` | `["oracle_deviation_breaker"]` | Charge path (rejection) | `OracleDeviationBreakerEvent` — token, latest, median, deviation |
| `admin_config_changed` | `["admin_config_changed"]` | Any admin config mutation | `AdminConfigChangedEvent` — key label, prev timestamp |

### Event Field Reference

**OracleConfigUpdatedEvent:**
```json
{
  "enabled": bool,
  "oracle": Option<Address>,
  "max_age_seconds": u64,
  "kind": "Spot" | "Twap" | "FixedRate",
  "window_secs": u64,
  "fixed_numerator": u128,
  "fixed_denominator": u128,
  "timestamp": u64,
  "schema_version": u32
}
```

**OracleChargeResolvedEvent:**
```json
{
  "subscription_id": u32,
  "quote_amount": i128,
  "token_amount": i128,
  "price": i128,
  "price_timestamp": u64,
  "timestamp": u64,
  "schema_version": u32
}
```

**OracleLivenessEvent:**
```json
{
  "last_sample_ts": u64,
  "age": u64,
  "healthy": bool,
  "timestamp": u64,
  "schema_version": u32
}
```

**OracleDeviationBreakerEvent:**
```json
{
  "token": Address,
  "latest_price": i128,
  "median_price": i128,
  "deviation_bps": u64,
  "threshold_bps": u32,
  "timestamp": u64
}
```

---

## Test Output Reference

### Validation Commands

After completing this runbook, run these commands to verify the deployment:

```bash
# 1. Full test suite
cargo test --all --features oracle-pricing

# 2. Oracle-specific tests
cargo test --package subscription_vault test_oracle

# 3. Staleness boundary tests
cargo test --package subscription_vault test_oracle_staleness_boundary

# 4. Liveness tests
cargo test --package subscription_vault test_oracle_liveness
```

### Expected Test Output

```
running 15 tests
test test_oracle_liveness::test_emit_oracle_liveness_succeeds_when_configured ... ok
test test_oracle_liveness::test_emit_oracle_liveness_fails_when_not_configured ... ok
test test_oracle_liveness::test_emit_oracle_liveness_fails_when_disabled ... ok
test test_oracle_liveness::test_emit_oracle_liveness_fails_when_no_oracle_address ... ok
test test_oracle_liveness::test_emit_oracle_liveness_fails_when_max_age_zero ... ok
test test_oracle_liveness::test_oracle_liveness_healthy_threshold ... ok
test test_oracle_liveness::test_oracle_liveness_event_emitted ... ok
test test_oracle_liveness::test_oracle_liveness_no_auth_required ... ok
test test_oracle_liveness::test_oracle_liveness_repeated_calls ... ok
test test_oracle_liveness::test_oracle_liveness_with_different_max_ages ... ok
test test_oracle_liveness::test_oracle_liveness_event_fields ... ok
test test_oracle_liveness::test_oracle_liveness_edge_case_exact_threshold ... ok
test test_oracle_liveness::test_oracle_liveness_config_persistence ... ok
test test_oracle_staleness_boundary::stale_at_exact_threshold_accepted ... ok
test test_oracle_staleness_boundary::stale_at_threshold_plus_one_rejected ... ok

test result: ok. 15 passed; 0 failed; 0 ignored
```

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-07-30 | Protocol Team | Initial runbook for Issue #853 |

---

> **Document ID:** `docs/runbooks/oracle_testnet.md`  
> **Related:** `docs/oracle_pricing.md`, `docs/errors.md`, `contracts/subscription_vault/src/oracle.rs`, `contracts/subscription_vault/src/oracle_adapter.rs`  
> **Closes:** #853
