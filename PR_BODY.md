Title: Add quorum-based governance over privileged actions

Summary
-------
This pull request implements a quorum-based governance layer for the subscription_vault contract. It introduces guardian-managed, weight-based voting on privileged operations (e.g., otate_admin, set_protocol_fee) and provides a deterministic, time-locked proposal lifecycle with on-chain vote tracking and execution guards.

Related issue
-------------
Closes #479

What I changed
--------------
- Added a full governance module: contracts/subscription_vault/src/governance.rs
- New/modified types in contracts/subscription_vault/src/types.rs:
  - ProposalKind, Proposal struct and event types
  - EVENT_SCHEMA_VERSION constant
  - New DataKey discriminants for guardians and proposals
- Exposed new public entrypoints in contracts/subscription_vault/src/lib.rs:
  - submit_proposal, ote_proposal, execute_proposal, cancel_proposal
  - guardian management and query helpers
- Tests: contracts/subscription_vault/src/test_governance.rs (11 tests covering happy-path and edge cases)
- Docs: docs/governance_proposals.md — architecture, usage, security notes

Files changed (high level)
-------------------------
- Added: contracts/subscription_vault/src/governance.rs
- Added: docs/governance_proposals.md
- Modified: contracts/subscription_vault/src/types.rs
- Modified: contracts/subscription_vault/src/lib.rs
- Modified: contracts/subscription_vault/src/test_governance.rs

Testing
-------
Run the governance tests locally:



I validated compilation and ran the contract test suite for the governance module locally. Please run the full test suite (cargo test --all) as part of CI.

Security considerations
-----------------------
- Quorum recalculation is performed at execution time using current guardian weights; guardian removal invalidates prior votes.
- Proposals are time-locked by an ETA and cannot execute early.
- Double-voting is prevented by per-guardian vote tracking.
- Only admin may cancel proposals.

Backward compatibility & migration
----------------------------------
- The governance system introduces new persistent keys (DataKey::Guardians, DataKey::Proposal(id)) and an instance counter (DataKey::NextProposalId). No existing keys are modified.
- Deployment instructions and transition plan are documented in docs/governance_proposals.md.

How to review
-------------
Start with:
- contracts/subscription_vault/src/types.rs — check new types and discriminants
- contracts/subscription_vault/src/governance.rs — core logic
- contracts/subscription_vault/src/lib.rs — public entrypoints
- contracts/subscription_vault/src/test_governance.rs — tests
- docs/governance_proposals.md — documentation and migration notes

Checklist
--------
- [x] Code compiles locally
- [x] Governance tests added and passing
- [x] Documentation added
- [ ] CI passing (please run)
- [ ] Reviewer approval

Deployment notes
----------------
1. Push branch and open PR linking to issue #479.
2. Merge after CI completes and reviewers approve.
3. Follow the transition plan in docs/governance_proposals.md to enable governance.

Suggested reviewers & labels
----------------------------
- Reviewers: @brodapeethar, @core-maintainer
- Labels: eature, governance, security

---

If you'd like, I can:
- push the branch and create the PR using gh once you authenticate locally, or
- produce a single gh pr create command you can run (copy/paste).
