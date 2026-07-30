# Security Policy

<!-- SECURITY.md — Stellabill Subscription Vault -->
<!-- References: docs/threat_model.md (when published), Soroban security model -->

## Supported Versions

| Version | Supported |
|---------|-----------|
| `main` (latest) | ✅ Active security fixes |
| Tagged releases < 6 months old | ✅ Critical fixes only |
| Older tagged releases | ❌ No longer maintained |

Fixes are applied to `main` first and back-ported to supported release branches
as patch releases within the SLAs listed below.

---

## Scope

This policy covers the **Stellabill Subscription Vault** smart contract and all
code in this repository, including:

- `contracts/subscription_vault/src/` — on-chain contract logic
- Admin, operator, merchant, and subscriber entrypoints
- Oracle integration (`oracle.rs`, `oracle_adapter.rs`)
- Dispute and escrow workflow (`dispute.rs`)
- Governance and nonce/replay-protection modules
- Off-chain tooling and scripts in this repo that interact with the vault

### Out of scope

- Third-party oracle contracts not maintained here
- Dependent token contracts (SAC / Soroban Asset Contracts)
- Infrastructure outside this repository (RPC nodes, indexers, frontends)
- Known accepted trade-offs already documented in `docs/` or inline comments
- Issues in dependencies surfaced only by `cargo audit` / `cargo deny` with no
  feasible exploit path against this contract

---

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

### Preferred channel — GitHub Private Security Advisory

1. Go to **Security → Advisories → New draft security advisory** on this repo:
   `https://github.com/Stellabill/stellabill-contracts/security/advisories/new`
2. Fill in title, severity estimate, affected versions, and a description of
   the vulnerability and reproduction steps.
3. We will acknowledge receipt and invite you to the private advisory thread.

### Fallback channel — Encrypted email

If you cannot use GitHub's advisory flow, send a PGP-encrypted report to:

```
security@stellabill.io
```

Encrypt your message to the project's security key (fingerprint published in
the repository's verified commit history and on keys.openpgp.org):

```
Key ID  : TBD — publish before mainnet launch
Algorithm: Ed25519 / Curve25519
```

> **Fallback:** If PGP tooling is unavailable, send an unencrypted summary to
> `security@stellabill.io` to initiate contact; the team will establish a
> secure channel before you share full details.

### What to include

- Affected contract entrypoint(s) or module(s)
- Severity assessment (Critical / High / Medium / Low) with rationale
- Step-by-step reproduction (ideally a failing test case or PoC)
- Potential impact: fund loss, DoS, privilege escalation, data integrity
- Suggested fix or mitigation (optional but appreciated)

---

## Response SLAs

| Severity | Acknowledgement | Status update | Target resolution |
|----------|----------------|---------------|-------------------|
| **Critical** — direct fund loss or admin key compromise | 24 h | 48 h | 7 days |
| **High** — significant fund risk or privilege escalation | 48 h | 5 days | 14 days |
| **Medium** — limited impact, requires unusual conditions | 72 h | 7 days | 30 days |
| **Low** — hardening or defence-in-depth improvement | 7 days | 14 days | 90 days |

All times are business days (UTC). If we cannot meet a deadline we will notify
you with a revised estimate before the deadline passes.

---

## Coordinated Disclosure & Embargo

We follow **coordinated disclosure**:

1. Reporter shares full details privately.
2. We reproduce, assess, and develop a fix in a private advisory branch.
3. We notify any downstream integrators or dependent protocols under NDA at
   least **7 days** before public release (longer for Critical findings when
   coordinating across multiple chains or indexers).
4. Fix is merged to `main` and released simultaneously with the public advisory.
5. A CVE (or equivalent Soroban/Stellar ecosystem identifier) is requested at
   time of release if the issue warrants one.

**Default embargo:** 90 days from initial acknowledgement, or sooner when the
fix is live and all known integrators have had adequate notice. We will request
an extension if coordinating across multiple chains or awaiting an upstream
Soroban SDK patch, and will inform the reporter promptly.

**Contact-email failure fallback:** If the reporter does not receive an
acknowledgement within 24 h (Critical) or 72 h (other), please ping
`@stellabill-security` on the public Discord or Telegram and reference your
report. We treat unanswered Critical reports as a page-level incident.

---

## Threat Model Summary

The Subscription Vault holds subscriber funds in escrow and exposes privileged
entrypoints to an admin key, an operator role, and per-merchant addresses. Key
risk areas are:

| Area | Module | Key controls |
|------|--------|-------------|
| Admin key compromise | `admin.rs` | Two-step rotation (`propose_admin` / `claim_admin_role`), 6 h config cooldown, nonce replay protection |
| Fund theft / unauthorised withdrawal | `merchant.rs`, `accounting.rs` | Per-token accounting invariant; `recoverable = contract_balance − total_accounted`; recovery requires replay-ID |
| Batch charge replay / front-running | `admin.rs`, `charge_core.rs` | Per-admin nonce per domain (`DOMAIN_BATCH_CHARGE`), per-subscription charge-salt (SHA-256 of `id ∥ last_ts ∥ ledger_seq`) |
| Reentrancy | `reentrancy.rs` | RAII `ReentrancyGuard` on all fund-moving entrypoints; CEI ordering enforced throughout |
| Oracle manipulation | `oracle.rs` | Deviation circuit breaker (ring-buffer median, configurable bps), staleness TTL, `max_age_seconds` guard |
| Treasury / fee-rate change | `admin.rs` | 48 h timelock queue (`PendingTreasuryChange`) before fee changes take effect |
| Governance takeover | `governance.rs` | Supermajority required; proposals expire; cooldown bypass gated on on-chain quorum |
| Dispute escrow over-pay | `dispute.rs` | Cumulative `DisputeEscrowLedger` invariant; escrow removed only when fully disbursed |
| Blocklist bypass | `blocklist.rs`, `charge_core.rs` | Subscriber, merchant, and all split-payees are checked against the blocklist before every charge |

A full threat model document (with trust boundaries, data-flow diagram, and
attacker assumptions) will be published at `docs/threat_model.md` before
mainnet launch.

---

## Safe Harbor

Stellabill recognises and supports responsible security research. We commit to:

- **Not pursue civil or criminal action** against researchers who comply with
  this policy and act in good faith.
- **Not file abuse complaints** with a researcher's ISP or hosting provider for
  compliant security testing.
- Treat compliant research as **authorised access** for purposes of the
  Computer Fraud and Abuse Act (CFAA), the EU NIS2 Directive, and analogous
  legislation.

### Conditions for safe harbor

1. Research is conducted against **testnet deployments only**, or against a
   local devnet using a fork of this repository. Do **not** test against
   Stellar Mainnet contracts holding real funds.
2. You do not exfiltrate, modify, or destroy data beyond what is necessary to
   demonstrate the vulnerability.
3. You do not perform denial-of-service attacks or degrade service for other
   users.
4. You report findings to us before any public disclosure.
5. You give us a reasonable period to remediate before disclosing publicly.

If you inadvertently access mainnet funds or production state, stop immediately,
preserve evidence, and notify us so we can assess and mitigate impact.

---

## Reward Tiers (Bug Bounty)

> A formal on-chain bug bounty program is planned for mainnet launch. Until
> then, we offer **discretionary rewards** based on severity and quality of
> report, paid in USDC on Stellar.

| Severity | Indicative reward |
|----------|-----------------|
| **Critical** — direct loss of subscriber or merchant funds | Up to $20 000 |
| **High** — privilege escalation, replay enabling fund drain | Up to $5 000 |
| **Medium** — limited-scope fund risk or DoS | Up to $1 000 |
| **Low** — hardening, defence-in-depth | Up to $250 |

Rewards are **at our sole discretion** until the formal program launches.
Duplicate reports receive a reduced reward proportional to delta value added.
Reporter must not have been the introducer of the vulnerability.

---

## Acknowledgements

We gratefully acknowledge security researchers who have helped improve the
Stellabill Subscription Vault. Researchers who wish to be credited (by name,
handle, or anonymously) will be listed here after disclosure.

---

## Version History

| Date | Change |
|------|--------|
| 2026-07-30 | Initial policy — issue #856 |
