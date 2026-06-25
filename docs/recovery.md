# Admin Recovery of Stranded Funds

## Overview

The SubscriptionVault contract includes a tightly scoped administrative recovery mechanism for handling funds that become inaccessible through normal contract operations. This document outlines when recovery is appropriate, how it works, and the security controls in place.

## Purpose

Despite careful contract design, funds can become stranded in several scenarios:

- **Accidental transfers**: Users send tokens directly to the contract address by mistake
- **Deprecated flows**: Contract upgrades or bug fixes leave funds in an inaccessible state
- **Unreachable addresses**: Subscribers lose access to their keys after cancellation

The recovery mechanism provides a last-resort option to prevent permanent fund loss while maintaining strong security guarantees.

## Recovery Scenarios

### Valid Use Cases

The contract defines four specific recovery reasons, each representing a well-documented scenario:

#### 1. UserOverpayment

**When to use**: Tokens sent directly to the contract address by mistake, not associated with any subscription.

**Example**: A user copies the contract address instead of their merchant address and sends 100 USDC directly to the contract.

**Verification steps**:

- Check transaction history to confirm the transfer
- Verify no subscription exists for the sending address
- Confirm the funds are not part of any subscription balance

#### 2. FailedTransfer

**When to use**: Funds stranded due to transfer failures or stalled in an unexpected state.

**Example**: A contract upgrade changes the withdrawal flow, or an indexer missed a step, leaving some funds in an old storage pattern that's no longer accessible.

**Verification steps**:

- Document the specific bug or upgrade that caused the issue
- Identify the exact amount and location of stranded funds
- Confirm the funds cannot be recovered through normal contract operations

#### 3. ExpiredEscrow

**When to use**: Cancelled subscriptions where the subscriber has lost access to their withdrawal keys or the escrow period has expired.

**Example**: A subscriber cancels their subscription but loses their private key before withdrawing their prepaid balance.

**Verification steps**:

- Confirm the subscription is in Cancelled status
- Document evidence the subscriber has lost key access (community request, time elapsed, etc.)
- Verify the subscriber's identity through alternative means if possible

#### 4. SystemCorrection

**When to use**: System or logic corrections that require manual intervention by the admin.

**Example**: A bug caused an incorrect amount of tokens to be deducted from the total accounted balance, requiring an admin to recover the excess tokens.

**Verification steps**:

- Verify the specific system error and calculate the exact delta.
- Document the root cause and ensure a patch is deployed.

### Invalid Use Cases

Recovery should **NOT** be used for:

- ❌ Active subscriptions with accessible subscribers
- ❌ Merchant balances that can be normally withdrawn
- ❌ Disputes between subscribers and merchants
- ❌ Regular contract operations or maintenance
- ❌ "Borrowing" funds temporarily with intent to return

## Technical Implementation

### Function Signature

```rust
pub fn recover_stranded_funds(
    env: Env,
    admin: Address,
    token: Address,
    recipient: Address,
    amount: i128,
    recovery_id: String,
    reason: RecoveryReason,
) -> Result<(), Error>
```

### Security Controls

#### 1. Admin Authorization

- Only the configured admin address can invoke recovery
- Requires cryptographic signature from the admin key
- Admin key should be a multi-signature wallet or hardware wallet

```rust
admin.require_auth();
let stored_admin = env.storage().instance().get(&Symbol::new(&env, "admin"))?;
if admin != stored_admin {
    return Err(Error::Unauthorized);
}
```

#### 2. Amount Validation

- Amount must be positive (> 0)
- Prevents accidental calls with zero or negative values
- Amount must be less than or equal to the available recoverable balance
- Recoverable balance is calculated as `actual_token_balance - total_accounted_balance` to ensure user and merchant active funds are strictly protected.

```rust
if amount <= 0 {
    return Err(Error::InvalidRecoveryAmount);
}

let recoverable = contract_balance.checked_sub(accounted_balance)?;
if amount > recoverable {
    return Err(Error::InsufficientBalance);
}
```

#### 2.1 Replay Protection

- Every recovery requires a unique `recovery_id`
- Prevents duplicate execution of the same recovery operation

```rust
let recovery_key = DataKey::Recovery(recovery_id.clone());
if env.storage().persistent().has(&recovery_key) {
    return Err(Error::Replay);
}
env.storage().persistent().set(&recovery_key, &true);
```

#### 3. Audit Trail

Every recovery operation emits a `RecoveryEvent` containing:

- Admin address (who authorized)
- Token address
- Recipient address (where funds went)
- Amount recovered
- Recovery reason (why it was done)
- Timestamp (when it occurred)

**Backend Indexer Guidance**:
Indexers should listen for the `recovery` topic. The event payload is a `RecoveryEvent` struct. Indexers should log the `recovery_id` (from transaction parameters) alongside the event to ensure a complete audit trail and match it with governance proposals.

```rust
let recovery_event = RecoveryEvent {
    admin: admin.clone(),
    recipient: recipient.clone(),
    token: token.clone(),
    amount,
    reason: reason.clone(),
    timestamp: env.ledger().timestamp(),
};

env.events().publish((Symbol::new(&env, "recovery"), admin.clone()), recovery_event);
```

#### 4. State Protection

- Recovery does not modify subscription state
- Active subscriptions remain unaffected
- Merchant balances remain intact

## Governance Process

### Before Recovery

1. **Documentation**: Create detailed documentation of the stranded fund situation
   - How the funds became stranded
   - Amount and location
   - Evidence supporting the recovery reason

2. **Community Review**: Submit the recovery proposal for community review
   - Public forum post or governance proposal
   - Minimum review period (e.g., 7 days)
   - Allow for objections or alternative solutions

3. **Verification**: Multiple parties verify the claim
   - Technical team confirms funds are truly stranded
   - Community members validate the evidence
   - Legal review if necessary

4. **Authorization**: Admin multi-sig approves the recovery
   - Sufficient signatures from authorized parties
   - Recorded vote or decision

### During Recovery

1. **Execution**: Admin invokes `recover_stranded_funds`
   - Provide all required parameters
   - Ensure recipient address is correct
   - Execute transaction

2. **Event Monitoring**: Verify the recovery event is emitted
   - Check on-chain events
   - Confirm details match the proposal

### After Recovery

1. **Reporting**: Publish post-recovery report
   - Confirm successful execution
   - Link to transaction hash
   - Update community on outcome

2. **Reconciliation**: Verify recipient received funds
   - Check recipient balance
   - Confirm amount matches

3. **Documentation**: Update records
   - Add to historical recovery log
   - Note lessons learned
   - Update procedures if needed

## Security Considerations

### Threat Model

#### Compromised Admin Key

**Risk**: If the admin key is compromised, an attacker could recover legitimate funds.

**Mitigations**:

- Use multi-signature wallet for admin (requires multiple keys)
- Implement time-locks (recovery requires waiting period)
- Monitor recovery events in real-time
- Have emergency pause mechanism

#### Collusion

**Risk**: Admin and recipient collude to steal funds claiming they're stranded.

**Mitigations**:

- Transparent governance process
- Community oversight and review
- On-chain audit trail
- Social accountability

#### Human Error

**Risk**: Admin accidentally recovers wrong amount or to wrong address.

**Mitigations**:

- Multiple verification steps
- Dry-run simulations before execution
- Clear documentation and checklists
- Peer review of parameters

### Residual Risks

Even with controls, some risks remain:

1. **Admin Trust**: System relies on admin integrity and competence
2. **Governance Quality**: Decisions depend on community engagement and diligence
3. **Technical Mistakes**: Complex scenarios may be misjudged
4. **Time Pressure**: Emergency situations may bypass normal processes

These risks are accepted trade-offs for the ability to recover genuinely stranded funds.

## Monitoring and Auditing

### Real-time Monitoring

Set up alerts for recovery events:

```javascript
// Pseudocode for monitoring
contract.events.on("recovery", (event) => {
  alert({
    admin: event.admin,
    recipient: event.recipient,
    amount: event.amount,
    reason: event.reason,
    timestamp: event.timestamp,
  });
});
```

### Historical Audit

Maintain a public log of all recoveries:

| Date       | Admin     | Token    | Recipient | Amount   | Reason             | Tx Hash  |
| ---------- | --------- | -------- | --------- | -------- | ------------------ | -------- |
| 2026-03-28 | admin.xlm | USDC     | user.xlm  | 100 USDC | UserOverpayment    | 0xabc... |

### Regular Review

- Quarterly review of all recovery operations
- Annual security audit including recovery functionality
- Continuous improvement of governance processes

## Testing

The recovery feature includes comprehensive test coverage:

- ✅ Successful recovery with all reason types
- ✅ Unauthorized caller rejection
- ✅ Zero and negative amount rejection
- ✅ Event emission verification
- ✅ Large and small amount handling
- ✅ Multiple sequential recoveries
- ✅ Different recipient types
- ✅ Interaction with active subscriptions
- ✅ Interaction with cancelled subscriptions
- ✅ Edge cases (max values, idempotency)

Total: 17 dedicated test cases achieving >95% coverage of recovery logic.

## Best Practices

### For Administrators

1. **Always verify** fund status before recovery
2. **Document everything** - assumptions, evidence, decisions
3. **Follow the process** - no shortcuts, even for "obvious" cases
4. **Communicate clearly** - keep community informed
5. **Learn from incidents** - improve procedures after each recovery

### For Community Members

1. **Stay engaged** - review recovery proposals
2. **Ask questions** - challenge assumptions respectfully
3. **Provide evidence** - help verify claims
4. **Report issues** - flag potential misuse early
5. **Participate in governance** - your voice matters

### For Integrators

1. **Monitor events** - watch for unexpected recoveries
2. **Validate assumptions** - don't assume admin is always right
3. **Build safeguards** - add your own monitoring layer
4. **Report anomalies** - help identify suspicious activity
5. **Have contingencies** - plan for admin key compromise

## Examples

### Example 1: Accidental Transfer Recovery

**Scenario**: User accidentally sends 50 USDC to contract address.

**Process**:

1. User reports the mistake via community forum
2. Admin verifies the transaction on-chain
3. Confirms no subscription exists for that address
4. Creates recovery proposal with evidence
5. Community reviews for 7 days
6. Admin executes recovery back to user's correct address

**Code**:

```rust
client.recover_stranded_funds(
    &admin,
    &usdc_token_address,
    &user_correct_address,
    &50_000000, // 50 USDC with 6 decimals
    &String::from_str(&env, "rec_2026_03_28_01"),
    &RecoveryReason::UserOverpayment
);
```

## Snapshot export and restore (forensic recovery)

The contract exposes paginated snapshot endpoints to capture and restore full state pages
for incident response. These are admin-only operations.

- `export_full_snapshot_page(admin, start_id, size) -> FullSnapshotPage` — returns a page
   of `SubscriptionSummary` entries and `MerchantBalanceEntry` records. Use `next_start_id`
   to iterate pages until none remain.

- `restore_snapshot_page(admin, start_id, subscriptions, balances, next_start_id)` — restores
   the provided subscription and balance records into the target contract instance. This
   function requires the emergency stop to be active to prevent interleaving with live traffic.

Usage notes:

- Limit `size` to a reasonable value (<= 100) to avoid long-running calls.
- Always enable emergency stop on the target deployment before running `restore_snapshot_page`.
- Verify exported pages (checksums, counts) off-chain before restore.
- Each export and restore emits `snapshot_exported` and `snapshot_restored` events respectively.


### Example 2: Expired Escrow Recovery

**Scenario**: Subscriber cancels but loses keys before withdrawal.

**Process**:

1. Subscriber contacts support via alternative channel
2. Provides identity verification
3. Admin verifies subscription is cancelled with balance
4. Confirms reasonable time elapsed (e.g., 90 days)
5. Creates proposal documenting the situation
6. After community review, recovers to subscriber's new address

**Code**:

```rust
client.recover_stranded_funds(
    &admin,
    &token_address,
    &subscriber_new_address,
    &remaining_balance,
    &String::from_str(&env, "rec_2026_03_28_02"),
    &RecoveryReason::ExpiredEscrow
);
```

## Conclusion

The admin recovery mechanism is a safety net for exceptional circumstances. It's designed with multiple layers of security and transparency, but ultimately relies on:

- **Technical safeguards**: Authorization, validation, audit trails
- **Social safeguards**: Community oversight, governance processes
- **Procedural safeguards**: Documentation, verification, review periods

Used responsibly with strong governance, recovery can save genuinely stranded funds. Misused or poorly governed, it could undermine trust in the system. The contract provides the tools; the community provides the judgment.

## References

- Contract source: `contracts/subscription_vault/src/lib.rs`
- Test suite: `contracts/subscription_vault/src/test.rs`
- State machine documentation: `docs/subscription_state_machine.md`
- Admin recovery function: `recover_stranded_funds()`
- Recovery event type: `RecoveryEvent`
- Recovery reasons: `RecoveryReason` enum

## Changelog

- 2026-02-21: Initial documentation for admin recovery feature
