# Stellabill Contracts

Soroban smart contracts for **Stellabill** — prepaid USDC subscription billing on the Stellar network. This repository contains the on-chain logic for recurring payments, subscriber vaults, and merchant payouts.

---

## Table of contents

- [Threat model](#threat-model)
- [What’s in this repo](#whats-in-this-repo)
- [Prerequisites](#prerequisites)
- [Local setup](#local-setup)
- [Build, test, and deploy](#build-test-and-deploy)
- [Contributing (open source)](#contributing-open-source)
- [Project layout](#project-layout)
- [License](#license)

---

## Threat model

This repository is anchored by a top-level threat model that defines the trust boundaries, adversary assumptions, and mitigations for the `subscription_vault` contract. It is intended to serve as the primary reference for future audits and security reviews.

### Actors and trust boundaries

- **Admin**: the highest-trust actor. Authorized to charge subscriptions, set configuration, rotate the admin key, enable emergency stop, and recover stranded funds. Admin compromise is a primary threat.
- **Merchant**: a semi-trusted actor. May withdraw merchant balances, pause/resume/cancel their own subscriptions, and manage merchant-scoped blocklists. Merchants cannot move subscriber funds or modify unrelated subscriptions.
- **Subscriber**: a semi-trusted actor. May create subscriptions, deposit funds, cancel/pause/resume their own subscriptions, and withdraw remaining balance after cancellation. Subscribers cannot withdraw merchant balances or alter other subscribers' subscriptions.
- **Oracle contract**: an external price feed. When enabled, the oracle provides pricing data used to resolve token amounts. It is treated as partially untrusted; bad data, stale updates, or drift must be detected and rejected.
- **Token contract**: the USDC token contract used for deposits and withdrawals. It is treated as untrusted for callback/reentrancy behavior, upgradeability risks, and non-standard transfer mechanics.
- **Soroban runtime**: trusted to enforce authentication, ledger timestamp semantics, and atomic execution of transactions.

### Assumed adversary capabilities

- Unauthorized callers may attempt to invoke admin-only or ownership-restricted entrypoints.
- A compromised admin key may attempt forced billing, configuration changes, emergency stop manipulation, or recovery operations.
- A malicious or buggy token contract may attempt recursive callbacks, transfer failures, fee-on-transfer behavior, or upgradeable semantics changes.
- An oracle may deliver stale, invalid, or manipulated prices, or drift outside configured freshness bounds.
- Attackers may attempt double-charging, replay billing requests, or exploit interval boundary edge cases.
- External observers may rely on events for indexing; the contract must emit consistent, auditable events while avoiding sensitive data leakage.

### Key mitigations by module

- **Authentication and authorization**: strict signer checks for admin-only, merchant-only, and subscriber-only flows. Shared owner checks and explicit caller validation prevent abuse of entrypoints.
- **Reentrancy protection**: the contract follows a checks-effects-interactions pattern and uses guard logic where external token calls occur, ensuring internal state is updated before any transfer.
- **Replay protection**: billing and sensitive state-changing operations use idempotency or nonce mechanisms to prevent duplicate execution and ensure once-only semantics.
- **Safe arithmetic**: all balance updates use checked arithmetic and explicit underflow/overflow handling to preserve invariants and prevent wraparound.
- **Oracle safety**: oracle pricing is gated by explicit configuration, a maximum age for price data, and rejection of invalid or stale quotes.
- **Event audit surface**: each lifecycle transition, charge attempt, withdrawal, admin rotation, oracle configuration change, and emergency stop action emits an on-chain event for auditability.
- **Data segregation**: subscription records store fixed fields per subscription, including subscriber, merchant, token address, interval, status, prepaid balance, and expiration. All calls operate on the stored token per-subscription to prevent cross-token confusion.

### Data classes and event surface

- `Subscription`: captured in storage with fields for `subscriber`, `merchant`, `amount`, `interval_seconds`, `last_payment_timestamp`, `status`, `prepaid_balance`, `expiration`, and `usage_enabled`.
- `Charge` and `deposit` flows: maintain explicit prepaid balances and status transitions such as `Active`, `Paused`, `Cancelled`, `InsufficientBalance`, and `GracePeriod`.
- Event surface includes creation, deposit, charge success/failure, withdrawal, admin rotation, oracle configuration updates, emergency stop toggles, and recovery actions.

### New attacker surface for current architecture

- **Upgradeable token contracts**: the token contract may change behavior after deployment, introducing fee-on-transfer, callback semantics, or decimals changes. The contract assumes standard token transfer behavior and defends against callbacks by updating internal state before any external call.
- **Oracle drift**: an oracle may become stale or return invalid prices. The contract enforces freshness via `max_age_seconds`, rejects invalid quotes, and falls back to safe behavior when oracle pricing is disabled or unavailable.
- **Admin key compromise**: a compromised admin is treated as a high-severity threat. Mitigations include immediate admin rotation, strong auth checks, and audit-event emission for all admin operations.

For deeper details, see [docs/security.md](docs/security.md).

## What’s in this repo

### Contract: `subscription_vault`

A single Soroban contract that implements a **prepaid subscription vault** for recurring USDC billing:

| Concept | Description |
|--------|-------------|
| **Subscriber** | User who holds a subscription; funds are held in the contract (vault) for that subscription. |
| **Merchant** | Recipient of recurring payments; can withdraw accumulated USDC. |
| **Subscription** | Agreement between subscriber and merchant: amount, billing interval, status (active/paused/cancelled), and prepaid balance. |

**Main capabilities (current / planned):**

| Method | Signature | Auth | Docs |
|--------|-----------|------|------|
| `version` | `version(env: Env) -> u32` | — | — |
| `init` | `init(env: Env, token: Address, admin: Address, min_topup: i128)` | — | [lifecycle](docs/subscription_lifecycle.md) |
| `create_subscription` | `create_subscription(env: Env, subscriber: Address, merchant: Address, amount: i128, interval_seconds: u64, expiration: Option<u64>, usage_enabled: bool) -> u32` | subscriber | [lifecycle](docs/subscription_lifecycle.md) |
| `deposit_funds` | `deposit_funds(env: Env, subscription_id: u32, subscriber: Address, amount: i128)` | subscriber | [lifecycle](docs/subscription_lifecycle.md) |
| `charge_subscription` | `charge_subscription(env: Env, subscription_id: u32)` | admin | [lifecycle](docs/subscription_lifecycle.md) |
| `batch_charge` | `batch_charge(env: Env, subscription_ids: Vec<u32>) -> BatchChargeResult` | admin | [lifecycle](docs/subscription_lifecycle.md) |
| `cancel_subscription` | `cancel_subscription(env: Env, subscription_id: u32, authorizer: Address)` | subscriber or merchant | [lifecycle](docs/subscription_lifecycle.md) |
| `pause_subscription` | `pause_subscription(env: Env, subscription_id: u32, authorizer: Address)` | subscriber or merchant | [lifecycle](docs/subscription_lifecycle.md) |
| `resume_subscription` | `resume_subscription(env: Env, subscription_id: u32, authorizer: Address)` | subscriber or merchant | [lifecycle](docs/subscription_lifecycle.md) |
| `withdraw_merchant_funds` | `withdraw_merchant_funds(env: Env, merchant: Address, amount: i128)` | merchant | [lifecycle](docs/subscription_lifecycle.md) |
| `get_subscription` | `get_subscription(env: Env, subscription_id: u32) -> Subscription` | — | — |

**Types:**

**`Subscription`**

| Field | Type | Description |
|-------|------|-------------|
| `subscriber` | `Address` | Owner of the subscription; must auth create and deposit. |
| `merchant` | `Address` | Recipient of charges. |
| `amount` | `i128` | Charge per interval (in token base units). |
| `interval_seconds` | `u64` | Minimum time between charges. |
| `last_payment_timestamp` | `u64` | Ledger timestamp of last successful charge. |
| `status` | `SubscriptionStatus` | Lifecycle state; see state machine below. |
| `prepaid_balance` | `i128` | Current balance; increased by deposit, decreased by charge. |
| `expiration` | `Option<u64>` | Optional ledger timestamp after which the subscription is treated as expired. `None` means no expiry. |
| `usage_enabled` | `bool` | Usage-billing flag; reserved for future use. |

**`SubscriptionStatus`** — `Active`, `Paused`, `Cancelled`, `InsufficientBalance`, `GracePeriod`.

See [subscription_lifecycle.md](docs/subscription_lifecycle.md) for the full state machine and transition rules.

**`Error`** (selected variants — see [docs/errors.md](docs/errors.md) for the canonical table with numeric codes):

| Variant | Category | When |
|---------|----------|------|
| `Unauthorized` | Auth | Required signer or admin mismatch. |
| `NotFound` | Not found | Subscription id or resource is missing. |
| `NotInitialized` | Not found | Contract has not been initialized. |
| `InvalidAmount` | Invalid args | Amount is zero or negative. |
| `InvalidStatusTransition` | State | Lifecycle transition not permitted from current status. |
| `NotActive` | State | Operation requires Active subscription. |
| `SubscriptionExpired` | State | Subscription has passed its `expiration` timestamp. |
| `IntervalNotElapsed` | State | Charge attempted before interval has elapsed. |
| `InsufficientBalance` | Accounting | Vault balance is insufficient. |
| `BelowMinimumTopup` | Accounting | Deposit is below the configured `min_topup` threshold. |

**Documentation:** [Subscription lifecycle and state machine](docs/subscription_lifecycle.md) — states, transitions, on-chain representation, and invariants. [Error codes](docs/errors.md) — canonical error taxonomy with numeric codes and retry guidance.

> **Implementation status:** `contracts/subscription_vault/src/lib.rs` currently exposes only the `version()` stub while the full implementation is rewritten on a future branch. The method table and type definitions above reflect the intended API as methods land. See the inline doc-comment in `lib.rs` for context.

---

## Prerequisites

- **Rust** (latest stable, e.g. 1.75+):  
  https://rustup.rs  
  `rustup default stable`
- **Soroban CLI**:  
  https://developers.stellar.org/docs/tools/soroban-cli/install  
  Used to build WASM and run tests/deploy.
- **Stellar / Soroban basics**:  
  https://developers.stellar.org/docs  
  Optional but helpful for contributing.

---

## Local setup

### 1. Clone the repository

```bash
git clone https://github.com/YOUR_ORG/stellabill-contracts.git
cd stellabill-contracts
```

(Replace `YOUR_ORG` with the actual org or user.)

### 2. Install Rust and Soroban CLI

- Install Rust: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh` then `rustup default stable`.
- Install Soroban CLI per the [official install guide](https://developers.stellar.org/docs/tools/soroban-cli/install).

### 3. Verify the environment

```bash
rustc --version
cargo --version
soroban --version
```

### 4. Build and test (no network required)

```bash
cargo build
cargo test
```

From the repo root, this builds the workspace and runs the contract unit tests (including `subscription_vault` tests in `contracts/subscription_vault/src/test.rs`).

### 5. (Optional) Build contract WASM

```bash
soroban contract build
```

This produces the WASM under `target/` for deployment to Stellar (e.g. testnet/mainnet via Soroban CLI or your CI/CD).

---

## Build, test, and deploy

| Task | Command |
|------|--------|
| Build workspace | `cargo build` |
| Run tests | `cargo test` |
| Build contract WASM | `soroban contract build` |
| Run with Soroban CLI (e.g. testnet) | See [Stellar docs](https://developers.stellar.org/docs/tools/soroban-cli) for `soroban contract deploy` and `invoke`. |

---

## Contributing (open source)

We welcome contributions from the community. Here’s how to get started and how we work.

### Before you start

- Read this README and the [Stellar / Soroban docs](https://developers.stellar.org/docs).
- Check [GitHub Issues](https://github.com/YOUR_ORG/stellabill-contracts/issues) for “good first issue” or “help wanted” labels.
- If you want to change behavior or add a feature, open an issue first so we can align on design.

### Development workflow

1. **Fork** the repo on GitHub and clone your fork.
2. **Create a branch** from `main` (or default branch):  
   `git checkout -b feature/your-feature` or `fix/your-fix`.
3. **Set up locally** as in [Local setup](#local-setup). Run `cargo test` and `cargo build` to ensure everything passes.
4. **Make changes** in small, logical commits. Keep messages clear (e.g. “Add admin check to charge_subscription”, “Fix subscription id overflow”).
5. **Run tests and build** before pushing:  
   `cargo test && cargo build` and, if you touch contract interface, `soroban contract build`.
6. **Push** to your fork and open a **Pull Request** against the upstream `main` (or default branch).

### Pull request guidelines

- **Scope**: One logical change per PR when possible (easier review and atomic history).
- **Tests**: New behavior should be covered by unit tests in the contract crate; existing tests must stay green.
- **Docs**: If you add or change a public function or type, update the README or inline docs as needed.
- **Description**: Use the PR description to explain the “why” and how to verify (e.g. steps or test commands).

### Code and design expectations

- **Rust**: Follow common Rust style (`cargo fmt`, `cargo clippy`). No `unwrap()` in contract logic without a clear justification; prefer `Result` and explicit errors.
- **Soroban**: Use `Env` for storage and auth; keep contract functions narrow and well-documented.
- **Security**: Any change that touches auth, token transfers, or admin rights will get extra review. When in doubt, open an issue first.

### Getting help

- **Questions**: Open a [GitHub Discussion](https://github.com/YOUR_ORG/stellabill-contracts/discussions) or an issue with the “question” label.
- **Bugs**: Open an issue with steps to reproduce, environment (Rust/Soroban versions), and logs if relevant.
- **Ideas**: Use Discussions or an issue with “enhancement” so we can track and discuss.

### Code of conduct

We expect all contributors and maintainers to be respectful and inclusive. By participating, you agree to uphold a constructive and professional environment. Specific CoC details (if any) will be linked in the repo (e.g. `CODE_OF_CONDUCT.md` or in the GitHub community guidelines).

---

## Project layout

```
stellabill-contracts/
├── Cargo.toml                 # Workspace root; lists contract crates
├── Cargo.lock                 # Locked dependencies (reproducible builds)
├── README.md                  # This file
├── .gitignore
├── docs/                      # Contract documentation
│   ├── subscription_lifecycle.md   # Subscription lifecycle, state machine, on-chain representation
│   ├── subscription_state_machine.md
│   ├── batch_charge.md
│   ├── billing_intervals.md
│   ├── topup_estimation.md
│   └── safe_math.md
└── contracts/
    └── subscription_vault/    # Prepaid subscription vault contract
        ├── Cargo.toml
        └── src/
            ├── lib.rs         # Contract logic and types
            └── test.rs        # Unit tests
```

---

## License

See the [LICENSE](LICENSE) file in this repository (add one if not present). Contributions are accepted under the same license.
