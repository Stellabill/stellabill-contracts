# Oracle Testnet Deployment Runbook

## Overview

This runbook covers deploying and operating the optional oracle pricing adapter on
Stellar **testnet**. The adapter enables cross-currency charge pricing by resolving
a subscription's quote-denominated amount to a token-denominated amount using an
external oracle contract.

When the oracle is properly configured and healthy, the vault converts charges via:

```
token_amount = ceil(quote_amount * 10^token_decimals / price)
```

See [`docs/oracle_pricing.md`](../oracle_pricing.md) for the full specification.

---

## Prerequisites

- Stellar CLI (`stellar`) pointed at **testnet**.
- Admin private key (or multisig authorization).
- Oracle contract already deployed and accessible.
- Network passphrase: `Test SDF Network ; September 2015`.
- Testnet RPC URL (e.g., `https://soroban-testnet.stellar.org`).

---

## Step 1: Register the Admin (One-Time)

The admin account must exist on-chain before any oracle configuration call.

```bash
stellar account info \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  ADMIN_PUBLIC_KEY
```

If the account does not exist, fund it via Friendbot:

```bash
curl -X POST "https://friendbot.stellar.org?addr=ADMIN_PUBLIC_KEY"
```

---

## Step 2: Deploy the Oracle Contract (Reference)

The oracle contract must expose the following interface:

- `latest_price() -> OraclePrice` — returns `{ price: i128, timestamp: u64 }`.
- `get_observations(since: u64) -> Vec<OraclePrice>` — returns price history for TWAP.

Deploy the oracle to testnet:

```bash
stellar contract deploy \
  --wasm oracle_contract.wasm \
  --source ADMIN_SECRET \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015"
```

Record the returned contract ID as `ORACLE_CONTRACT_ID`.

---

## Step 3: Initialise Oracle in the Vault

Configure the vault to use the oracle with a **Spot** adapter (default). This is the
simplest setup — one latest-price read per charge.

```bash
stellar contract invoke \
  --id VAULT_CONTRACT_ID \
  --source ADMIN_SECRET \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  -- \
  set_oracle_config \
  --admin ADMIN_PUBLIC_KEY \
  --enabled true \
  --oracle 'Some("ORACLE_CONTRACT_ID")' \
  --max_age_seconds 300 \
  --kind Spot \
  --window_secs 0 \
  --fixed_numerator 0 \
  --fixed_denominator 1
```

**Parameter guide:**

| Parameter            | Value                   | Notes                                       |
| -------------------- | ----------------------- | ------------------------------------------- |
| `enabled`            | `true`                  | Activates oracle pricing on all charges     |
| `oracle`             | `Some("...")`           | Oracle contract address                     |
| `max_age_seconds`    | `300`                   | 5-minute staleness threshold                |
| `kind`               | `Spot`                  | Use `Twap` or `FixedRate` for other modes   |
| `window_secs`        | `0` (ignored for Spot)  | TWAP window; min 60 for Twap                |
| `fixed_numerator`    | `0` (ignored for Spot)  | Fixed-rate numerator                        |
| `fixed_denominator`  | `1` (ignored for Spot)  | Fixed-rate denominator (must be non-zero)   |

**Events emitted:** `oracle_config_updated`.

### Verification

```bash
stellar contract invoke \
  --id VAULT_CONTRACT_ID \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  --source ADMIN_SECRET \
  -- \
  get_oracle_config
```

Expected output (trimmed):

```json
{
  "enabled": true,
  "oracle": "ORACLE_CONTRACT_ID",
  "max_age_seconds": 300,
  "kind": "Spot",
  "window_secs": 0,
  "fixed_numerator": 0,
  "fixed_denominator": 1
}
```

---

## Step 4: Submit Initial Price

Ensure the oracle has at least one valid price observation before the first
charge. A missing price will cause every charge to fail with
`OraclePriceUnavailable`.

### 4a. Verify oracle is producing prices

```bash
stellar contract invoke \
  --id ORACLE_CONTRACT_ID \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  --source ADMIN_SECRET \
  -- \
  latest_price
```

Expected: A `{ price: i128, timestamp: u64 }` tuple with a **positive** price and
a recent (within `max_age_seconds`) timestamp.

### 4b. If no price exists, submit one

The exact submission command depends on the oracle contract's ABI. A typical
`submit_price` entrypoint:

```bash
stellar contract invoke \
  --id ORACLE_CONTRACT_ID \
  --source ADMIN_SECRET \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  -- \
  submit_price \
  --base TOKEN_A_ADDRESS \
  --quote TOKEN_B_ADDRESS \
  --price 10000000 \
  --timestamp $(date +%s)
```

Scale: the price is expressed with 7 decimal places — `10_000_000` = 1.0.

---

## Step 5: Validate Freshness

### 5a. Liveness check

```bash
stellar contract invoke \
  --id VAULT_CONTRACT_ID \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  --source ADMIN_SECRET \
  -- \
  emit_oracle_liveness
```

Interpretation:

| `healthy` | Meaning                                                |
| --------- | ------------------------------------------------------ |
| `true`    | Oracle price age ≤ `max_age_seconds / 2` — healthy      |
| `false`   | Oracle price age > `max_age_seconds / 2` — approaching staleness |
| Error     | Oracle not configured or unavailable                   |

### 5b. End-to-end charge test

Create a test subscription and attempt a charge to verify the full oracle
pipeline:

```bash
# 1. Create subscription with quote-denominated amount
stellar contract invoke \
  --id VAULT_CONTRACT_ID \
  --source SUBSCRIBER_SECRET \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  -- \
  create_subscription \
  --subscriber SUBSCRIBER_PUBLIC_KEY \
  --merchant MERCHANT_PUBLIC_KEY \
  --amount 100000000 \
  --interval_seconds 86400 \
  --usage_enabled false \
  --lifetime_cap 'None' \
  --expires_at 'None'

# 2. Deposit tokens
stellar contract invoke \
  --id VAULT_CONTRACT_ID \
  --source SUBSCRIBER_SECRET \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  -- \
  deposit_funds \
  --subscription_id SUB_ID \
  --amount 500000000

# 3. Advance time past interval
# (wait 86400+ seconds on testnet, or simulate by advancing ledger)

# 4. Charge subscription
stellar contract invoke \
  --id VAULT_CONTRACT_ID \
  --source ADMIN_SECRET \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  -- \
  charge_subscription \
  --subscription_id SUB_ID
```

If the charge succeeds, the `oracle_charge_resolved` event is emitted and the
subscription's prepaid balance is debited by the oracle-resolved token amount.

---

## Step 6: TWAP Configuration (Optional)

For a manipulation-resistant TWAP adapter:

```bash
stellar contract invoke \
  --id VAULT_CONTRACT_ID \
  --source ADMIN_SECRET \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  -- \
  set_oracle_config \
  --admin ADMIN_PUBLIC_KEY \
  --enabled true \
  --oracle 'Some("ORACLE_CONTRACT_ID")' \
  --max_age_seconds 300 \
  --kind Twap \
  --window_secs 60 \
  --fixed_numerator 0 \
  --fixed_denominator 1
```

**Minimum `window_secs` is 60** — the contract rejects shorter windows with
`Error::InvalidInput`. For production-like testnet environments, use 300 seconds
(5 minutes).

---

## Step 7: FixedRate Adapter (No Oracle Reads)

For pegged pairs or deterministic pricing without oracle reads:

```bash
stellar contract invoke \
  --id VAULT_CONTRACT_ID \
  --source ADMIN_SECRET \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  -- \
  set_oracle_config \
  --admin ADMIN_PUBLIC_KEY \
  --enabled true \
  --oracle 'None' \
  --max_age_seconds 0 \
  --kind FixedRate \
  --window_secs 0 \
  --fixed_numerator 10000000 \
  --fixed_denominator 1
```

This returns `price = (10_000_000 * 10^7) / 1 = 100_000_000_000_000` — a 1:1
peg after applying the scale factor. Adjust `fixed_numerator` / `fixed_denominator`
for other ratios.

---

## Failure Recovery

### Oracle price becomes stale

**Symptom:** Charges fail with `OraclePriceStale` or `OraclePriceUnavailable`.

**Recovery:** Ensure the oracle contract is receiving fresh price observations.
If it cannot be restored promptly, disable the oracle temporarily:

```bash
stellar contract invoke \
  --id VAULT_CONTRACT_ID \
  --source ADMIN_SECRET \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  -- \
  set_oracle_config \
  --admin ADMIN_PUBLIC_KEY \
  --enabled false \
  --oracle 'None' \
  --max_age_seconds 0 \
  --kind Spot \
  --window_secs 0 \
  --fixed_numerator 0 \
  --fixed_denominator 1
```

With the oracle disabled, charges fall back to using `subscription.amount`
directly (token-denominated). Re-enable once the oracle feed stabilises.

### Oracle contract upgraded

1. Disable the oracle in the vault (command above).
2. Deploy and warm up the new oracle contract.
3. Update the vault oracle address:

```bash
stellar contract invoke \
  --id VAULT_CONTRACT_ID \
  --source ADMIN_SECRET \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  -- \
  set_oracle_config \
  --admin ADMIN_PUBLIC_KEY \
  --enabled true \
  --oracle 'Some("NEW_ORACLE_CONTRACT_ID")' \
  --max_age_seconds 300 \
  --kind Spot \
  --window_secs 0 \
  --fixed_numerator 0 \
  --fixed_denominator 1
```

### Oracle deviation circuit breaker

When the optional deviation check is enabled (`set_oracle_deviation_bps`), a
sudden price spike exceeding the threshold causes charges to fail with
`OracleDeviationTooHigh`. Recovery is automatic once prices return to within the
allowed band. To temporarily relax the check:

```bash
# Set a 500 bps (5%) deviation tolerance
# (This requires the set_oracle_deviation_bps entrypoint to be implemented)
# Adjust as needed for the integration layer.
```

---

## Rollback Plan

### Full rollback: disable oracle

```bash
stellar contract invoke \
  --id VAULT_CONTRACT_ID \
  --source ADMIN_SECRET \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  -- \
  set_oracle_config \
  --admin ADMIN_PUBLIC_KEY \
  --enabled false \
  --oracle 'None' \
  --max_age_seconds 0 \
  --kind Spot \
  --window_secs 0 \
  --fixed_numerator 0 \
  --fixed_denominator 1
```

All subsequent charges treat `subscription.amount` as token-denominated. No
existing state is affected — only future charges change behaviour.

### Config cooldown

All `set_oracle_config` calls are subject to the contract's
[admin config cooldown](admin_rotation.md) (`CONFIG_COOLDOWN_SECS`). Sequential
changes within the cooldown window are rejected — wait and retry.

---

## Monitoring & Alerting

### Recommended monitoring schedule

| Check                               | Frequency | Alert if                                 |
| ----------------------------------- | --------- | ---------------------------------------- |
| `emit_oracle_liveness`              | Every 60s | `healthy == false` or error              |
| Oracle `latest_price` age           | Every 60s | Age > `max_age_seconds / 2`              |
| Charge failure rate                 | Per block | Any `charge_failed` with oracle error    |
| Oracle contract health              | Every 120s | `latest_price` returns error or zero     |

### Paging thresholds

- **SEV-2:** `emit_oracle_liveness` returns `healthy == false` — oracle feed is
  approaching staleness but charges still succeed.
- **SEV-1:** `OraclePriceStale` error observed on any charge — pricing is
  impacted. Escalate immediately to the oracle provider.
- **SEV-1:** `set_oracle_config` cooldown bypassed in error — contact protocol
  security.

### Contact points

| Team                    | Channel           | Response SLA |
| ----------------------- | ----------------- | ------------ |
| Oracle provider         | `#oracle-alerts`  | 15 minutes   |
| Protocol DevOps         | `#infra-alerts`   | 30 minutes   |
| Protocol Security       | `#security`       | 5 minutes    |

---

## Security Notes

| Concern                                          | Mitigation                                                                 |
| ------------------------------------------------ | -------------------------------------------------------------------------- |
| Oracle address set to malicious contract         | Admin-only config; cooldown gives time to detect                           |
| Flash-loan price manipulation (Spot)             | Use TWAP with window ≥ 60s for production-like environments                |
| Oracle feed stopped                              | `emit_oracle_liveness` alerts; monitor stale price before charges fail     |
| FixedRate denominator set to 0                   | Rejected at configuration time with `InvalidInput`                         |
| max_age_seconds = 0 with oracle enabled          | Rejected at configuration time                                             |
| Config cooldown prevents rapid disabling         | Factor cooldown into incident response timeline                            |
| Same admin controls oracle AND fee params        | Consider a separate multisig for oracle config in production               |

---

## Events Reference

| Event                      | Topics                              | Payload                                                             |
| -------------------------- | ----------------------------------- | ------------------------------------------------------------------- |
| `oracle_config_updated`    | `["oracle_config_updated"]`         | `OracleConfigUpdatedEvent { enabled, oracle, max_age_seconds, kind, ... }` |
| `oracle_charge_resolved`   | `["oracle_charge_resolved"]`        | `{ quote_amount, token_amount, price, price_timestamp, timestamp }` |
| `oracle_liveness`          | `["oracle_liveness"]`               | `OracleLivenessEvent { last_sample_ts, age, healthy, timestamp }`   |
| `fee_converted`            | `["fee_converted", subscription_id]`| `FeeConvertedEvent { source, target, original, converted, rate }`   |

---

## References

- [`docs/oracle_pricing.md`](../oracle_pricing.md) — Full oracle specification
- [`docs/runbooks/admin_rotation.md`](admin_rotation.md) — Admin rotation procedures
- [`docs/fee_routing.md`](../fee_routing.md) — Fee routing and rounding rules
- [`contracts/subscription_vault/src/oracle_adapter.rs`](../../contracts/subscription_vault/src/oracle_adapter.rs) — Oracle adapter implementations
- [`contracts/subscription_vault/src/oracle.rs`](../../contracts/subscription_vault/src/oracle.rs) — Oracle configuration module (inline in lib.rs)
