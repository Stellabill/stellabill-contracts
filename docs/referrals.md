# Referral Rewards Ledger

The referral rewards ledger is an on-chain accounting system that tracks rewards
owed to referrers when new subscriptions are created with a referral address and
subsequently complete their first successful billing charge.

---

## Overview

| Concern | Answer |
|---|---|
| Who configures rewards? | Contract admin sets rate and ceiling |
| How is a referral established? | Subscriber calls `register_referral` with a referrer address |
| When is the reward credited? | On the **first** successful interval charge only |
| What triggers a reward? | `charge_subscription` or `batch_charge` success |
| Are funds separated? | Yes — referral balances are isolated from merchant earnings |
| Can a referrer double-dip? | No — the record is marked `rewarded = true` after the first credit |
| Can a subscriber self-refer? | No — `referrer == subscriber` is rejected |

---

## Storage Layout

Three new `DataKey` variants hold all referral state:

| Key | Type | Description |
|---|---|---|
| `DataKey::ReferralCfg` | `ReferralConfig` | Global program configuration |
| `DataKey::Referral(sub_id)` | `ReferralRecord` | Per-subscription referral record |
| `DataKey::ReferralBalance(address)` | `i128` | Per-referrer accumulated reward balance |

These are entirely separate from:
- Merchant earnings: `("merchant_balance", address)` keys
- Subscriber prepaid vaults: `Subscription.prepaid_balance`

---

## Reward Computation

```
raw_reward   = charge_amount × reward_bps / 10_000
final_reward = min(raw_reward, max_reward)   // if max_reward is Some(_)
```

**Example** — 5% rate, no ceiling, 10 USDC charge:
```
raw_reward = 10_000_000 × 500 / 10_000 = 500_000 (0.5 USDC)
```

**Example** — 10% rate, 0.2 USDC ceiling, 10 USDC charge:
```
raw_reward   = 10_000_000 × 1_000 / 10_000 = 1_000_000 (1 USDC)
final_reward = min(1_000_000, 200_000) = 200_000 (0.2 USDC)
```

All arithmetic uses checked operations (`checked_mul`, `checked_add`).
Overflow returns `Error::Overflow` and the charge does not proceed.

---

## Qualifying Event

The reward fires on the **first** successful `charge_subscription` (or
`batch_charge`) for a subscription that has a registered referral.

- Usage charges (`charge_usage`) do **not** trigger referral rewards.
- One-off merchant charges (`charge_one_off`) do **not** trigger referral rewards.
- Once `ReferralRecord.rewarded == true`, no subsequent charge re-evaluates.

If the program is **disabled** at the time of the first charge, the record is
still marked `rewarded = true` so that no future re-enabling of the program can
retroactively award a reward for a charge that already occurred.

---

## Entrypoints

### `configure_referral_program` (admin only)

```rust
configure_referral_program(
    env: Env,
    admin: Address,
    reward_bps: u32,          // 0–10 000 (basis points)
    max_reward: Option<i128>, // optional per-referral ceiling, in token base units
    enabled: bool,            // activate / pause the program
) -> Result<(), Error>
```

Call again at any time to update the configuration atomically. The new config
takes effect from the **next** charge event onward; already-rewarded records are
not revisited.

### `register_referral` (subscriber only)

```rust
register_referral(
    env: Env,
    subscription_id: u32,
    subscriber: Address,  // must match subscription.subscriber
    referrer: Address,    // must differ from subscriber
) -> Result<(), Error>
```

Creates a `ReferralRecord` with `rewarded = false`. Only one referrer may be
registered per subscription. Registering does not require the program to be
active — the program state is evaluated at charge time.

Blocked when emergency stop is active.

### `get_referral_record`

```rust
get_referral_record(env: Env, subscription_id: u32) -> Option<ReferralRecord>
```

Returns the current referral record, or `None` if no referral is registered.

### `get_referral_config`

```rust
get_referral_config(env: Env) -> Option<ReferralConfig>
```

Returns the active program config, or `None` if never configured.

### `get_referral_balance`

```rust
get_referral_balance(env: Env, referrer: Address) -> i128
```

Returns the accumulated unclaimed reward balance for a referrer address. Returns
`0` if the address has never received a referral reward.

### `withdraw_referral_rewards` (referrer only)

```rust
withdraw_referral_rewards(
    env: Env,
    referrer: Address,
    amount: i128,
) -> Result<(), Error>
```

Transfers `amount` token base units from the contract to `referrer`. Follows the
**Checks-Effects-Interactions (CEI)** pattern: the on-chain balance is decremented
before the token transfer is made.

Blocked when emergency stop is active.

---

## Events

| Symbol | Payload | When emitted |
|---|---|---|
| `ref_cfg_set` | `(reward_bps, max_reward, enabled)` | Config set or updated |
| `ref_registered` | `ReferralRegisteredEvent` | Referral registered for a subscription |
| `ref_rewarded` | `ReferralRewardCreditedEvent` | Reward credited to referrer balance |
| `ref_withdrawn` | `ReferralRewardWithdrawnEvent` | Referrer withdraws accumulated rewards |

---

## Errors

| Error | Code | Meaning |
|---|---|---|
| `ReferralAlreadyRegistered` | 1019 | A referral was already registered for this subscription |
| `Unauthorized` | 401 | Non-admin called `configure_referral_program` |
| `Forbidden` | 403 | Non-subscriber called `register_referral` |
| `InvalidInput` | 1015 | `reward_bps > 10_000` or self-referral attempt |
| `InvalidAmount` | 1006 | `max_reward <= 0`, or `amount <= 0` on withdrawal |
| `NotFound` | 404 | Subscription not found, or referrer has no balance |
| `InsufficientBalance` | 1003 | Withdrawal amount exceeds referrer balance |
| `EmergencyStopActive` | 1009 | Operation blocked by emergency stop |

---

## Security properties

### No double-counting

`ReferralRecord.rewarded` is set to `true` atomically with the credit write in
the same ledger transaction. A second call to `try_credit_referral_reward` for
the same subscription returns `Ok(())` immediately without any state change.

### Fund separation

Referral balances (`DataKey::ReferralBalance`) and merchant balances
(`("merchant_balance", address)`) are stored under completely different keys.
No code path reads one to write the other.

### CEI pattern on withdrawal

`do_withdraw_referral_rewards` sets the new balance in storage **before** calling
`token.transfer()`. If the token contract attempts a reentrant call back into
the vault, the stored balance already reflects the deduction.

### Self-referral prevention

`register_referral` rejects `referrer == subscriber` with `Error::InvalidInput`
before writing any state.

### Emergency stop

Both `register_referral` and `withdraw_referral_rewards` check
`require_not_emergency_stop` via their `lib.rs` wrappers and return
`Error::EmergencyStopActive` immediately if the circuit breaker is active.

---

## Example program configurations

### Conservative — 1%, no ceiling

```json
{ "reward_bps": 100, "max_reward": null, "enabled": true }
```
A 10 USDC charge yields 0.1 USDC reward.

### Growth — 5%, ceiling 1 USDC

```json
{ "reward_bps": 500, "max_reward": 1000000, "enabled": true }
```
A 10 USDC charge yields 0.5 USDC; a 100 USDC charge yields 1 USDC (capped).

### Promotional — 10%, ceiling 5 USDC

```json
{ "reward_bps": 1000, "max_reward": 5000000, "enabled": true }
```
Useful for limited-time referral campaigns.

### Paused (no rewards)

```json
{ "reward_bps": 500, "max_reward": null, "enabled": false }
```
Keeps the rate config intact while temporarily pausing credit issuance.
