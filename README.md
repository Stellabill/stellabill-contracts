# Stellabill Contracts

Soroban smart contracts for **Stellabill** — prepaid USDC subscription billing on the Stellar network. This repository contains the on-chain logic for recurring payments, subscriber vaults, and merchant payouts.

---

## Table of contents

- [What’s in this repo](#whats-in-this-repo)
- [Prerequisites](#prerequisites)
- [Local setup](#local-setup)
- [Build, test, and deploy](#build-test-and-deploy)
- [Contributing (open source)](#contributing-open-source)
- [Project layout](#project-layout)
- [License](#license)

---

## What’s in this repo

### Contract: `subscription_vault`

A single Soroban contract that implements a **prepaid subscription vault** for recurring USDC billing:

| Concept | Description |
|--------|-------------|
| **Subscriber** | User who holds a subscription; funds are held in the contract (vault) for that subscription. |
| **Merchant** | Recipient of recurring payments; can withdraw accumulated USDC. |
| **Subscription** | Agreement between subscriber and merchant: amount, billing interval, status (active/paused/cancelled), and prepaid balance. |

**Main capabilities (current / planned):**

- **`init`** — Set the USDC token address and admin (e.g. billing backend).
- **`create_subscription`** — Subscriber creates a subscription (subscriber, merchant, amount, interval, usage flag). Auth: subscriber.
- **`deposit_funds`** — Subscriber tops up prepaid balance for a subscription. Auth: subscriber.
- **`charge_subscription`** — Billing engine charges one interval: deduct from vault, pay merchant, update last payment time. (Intended to be restricted to admin/authorized caller.)
- **`cancel_subscription`** — Subscriber or merchant cancels; remaining balance can be withdrawn by subscriber. Auth: subscriber or merchant.
- **`pause_subscription`** — Pause so no charges occur until resumed. Auth: subscriber or merchant.
- **`withdraw_merchant_funds`** — Merchant withdraws accumulated USDC. Auth: merchant.
- **`get_subscription`** — Read subscription by id (for indexers and UIs).

**Types:**

- **`Subscription`** — `subscriber`, `merchant`, `amount`, `interval_seconds`, `last_payment_timestamp`, `status`, `prepaid_balance`, `usage_enabled`.
- **`SubscriptionStatus`** — `Active`, `Paused`, `Cancelled`, `InsufficientBalance`.
- **`Error`** — `NotFound`, `Unauthorized`.

**Documentation:** [Subscription lifecycle and state machine](docs/subscription_lifecycle.md) — states, transitions, on-chain representation, and invariants.

The contract is in early development; several functions still have `TODO` placeholders (e.g. token transfers, admin checks, full charge/withdraw logic). See the source in `contracts/subscription_vault/src/lib.rs` for details.

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
