# Security Policy

## 1. Reporting a Vulnerability

We take the security of Stellabill contracts seriously. If you believe you have found a security vulnerability in our smart contracts or related infrastructure, please report it to us immediately.

**Please do not report security vulnerabilities via public GitHub issues or public forums.**

### How to Report
Please send an email to our security team at:
**`security@stellabill.com`**

To help us protect users and resolve the issue quickly, please include:
- A detailed description of the vulnerability, including the affected contract(s), functions, and lines of code.
- A proof-of-concept (PoC) script or transaction description demonstrating the exploit vector.
- Your assessment of the severity and potential impact (e.g., loss of funds, denial of service).
- Your public PGP key if you would like encrypted communications.

### Encrypting Your Report
For sensitive disclosures, we strongly recommend encrypting your email using our PGP key:
- **PGP Fingerprint**: `74A5 C580 9E6D F0F8 B411 A36E B177 D6C2 0F5E F145`
- **PGP Key ID**: `0F5EF145`
- **Keyserver / URI**: You can download our public key from keys.openpgp.org or contact us directly.

### Contact Email Failure Fallback
If you do not receive an acknowledgement within 24 hours, or if your email bounces due to system issues, please use the following fallback contact methods:
1. **GitHub Private Vulnerability Reporting**: Go to our repository security page at [Stellabill Contracts Security Advisories](https://github.com/Stellabill/stellabill-contracts/security/advisories) and select "Report a vulnerability" to open a confidential report.
2. **Alternative Contact**: Reach out via Signal / Keybase to the core developers (contact handles can be requested confidentially, or see our website's security metadata).

---

## 2. Automated Security Checks

All pull requests and commits are automatically scanned for security issues through CI workflows.

### cargo audit
Checks all dependencies against the [RustSec Advisory Database](https://rustsec.org/):
- **Fails** on any known vulnerability (RUSTSEC advisories)
- **Fails** on yanked crates (crates removed from crates.io)
- **Warns** on unmaintained crates

### cargo deny
Enforces dependency policies defined in `deny.toml`:
- **Licenses**: Only approved open-source licenses allowed (see `deny.toml` for the allow-list)
- **Sources**: Only crates from crates.io and explicitly approved git sources (e.g., Soroban SDK repos)
- **Bans**: Prevents wildcard dependencies; warns on duplicate dependency versions
- **Advisories**: Respects the same advisory database as cargo audit

Both tools run automatically in the `security-audit` CI job before merging any changes to main.

### Handling Security Advisories

If a new advisory causes CI to fail:

1. **Check the advisory**: Review the RustSec advisory on [rustsec.org](https://rustsec.org/)
2. **Update the affected dependency** if a fix is available
3. **If no fix exists**, add an ignore entry to `deny.toml` with a clear justification:
   ```toml
   [advisories]
   ignore = [
       { id = "RUSTSEC-XXXX-XXXX", reason = "Not applicable because..." }
   ]
   ```
4. **Open a tracking issue** to resolve the advisory once a patch is available

### Adding New Dependencies

When adding a new dependency, ensure:
1. Its license is in the `allow` list in `deny.toml`
2. It comes from crates.io (or an explicitly allowed git source in `deny.toml`)
3. `cargo audit --deny warnings` passes locally
4. `cargo deny check` passes locally

---

## 3. Response SLAs

We are committed to coordinating with security researchers to address vulnerabilities in a professional and timely manner. We adhere to the following Service Level Agreements (SLAs):
- **Initial Acknowledgement**: Within 24 hours of receiving the report.
- **Triage & Severity Assessment**: Within 72 hours of receiving the report.
- **Status Updates**: Every 3 business days during the resolution process.
- **Patch Deployment**: We aim to deploy fixes within 14 days of triage, depending on complexity, audit requirements, and coordination overhead.

---

## 3. Coordinated Disclosure & Embargo

To protect our users and the integrity of the network, we ask that you follow coordinated disclosure guidelines.
- **Embargo Period**: We request that you keep all details of the vulnerability confidential until a patch is deployed and has been active for an embargo period of **at least 30 days** (and up to 90 days for complex, multi-party integrations).
- **Coordinated Disclosure Across Chains**: Stellabill contracts may be deployed across multiple blockchains or chains (e.g., Soroban mainnet, testnet, or other instances). A vulnerability in the codebase affects all deployed instances. Therefore:
  - The embargo remains in place until **all** active production/mainnet deployments of the affected code across all chains have been successfully patched and secured.
  - The Stellabill team will coordinate patch releases across all deployed networks simultaneously to minimize the window of exploitation. We ask that researchers do not release details when only one network is patched if other networks remain vulnerable.

---

## 4. Safe Harbor Guidelines

We support and encourage security research on Stellabill. If you make a good faith effort to comply with this policy during your research, we will:
- Consider your research to be authorized.
- Work with you to understand and resolve the issue quickly.
- Commit to not initiating legal action (such as under the Computer Fraud and Abuse Act or local equivalents) against you, nor requesting law enforcement to investigate you.

### Conditions for Safe Harbor
To qualify for Safe Harbor, you must:
1. **Report Immediately**: Submit a report to us as soon as you discover a vulnerability.
2. **Do Not Exploit**: Avoid exploiting the vulnerability beyond the minimum proof of concept necessary to demonstrate its presence. Do not perform high-volume testing (DoS), drain user funds, or disrupt live contract systems.
3. **Use Sandbox Environments**: Conduct your testing on local test nets or simulated sandboxes. Do not target production mainnet deployments or real user accounts.
4. **Maintain Confidentiality**: Do not share, publish, or disclose any details of the vulnerability to third parties during the embargo period.

---

## 5. Threat Model Reference

Before conducting your security research, please review our threat model and security assumptions document:
- [Security Threat Model](docs/security.md)

This document contains deep-dives on the primary assets, trusted and untrusted actor assumptions, and our existing mitigations (e.g., reentrancy guards, replay protection, state machine boundaries, safe math).

---

## 6. Bug Bounty & Reward Tiers

We offer rewards to researchers who discover and report qualifying vulnerabilities. Rewards are determined at the sole discretion of the Stellabill team based on CVSS v3.1 severity, impact on user assets, and ease of exploitation.

| Severity | Description | Max Reward (USDC/XLM Equivalent) |
|---|---|---|
| **Critical** | Direct theft of user USDC/prepaid balances from the Vault; unauthorized control of contract; permanent loss of funds. | Up to $10,000 |
| **High** | Temporary freeze of funds; unauthorized actions on behalf of users; state machine bypasses. | Up to $5,000 |
| **Medium** | Unauthorized admin rotation bypasses; partial refund exploits; contract logic failures that don't steal assets but degrade service. | Up to $1,000 |
| **Low** | Event emission failures; minor gas/storage inefficiencies; temporary validation issues. | Up to $250 |
