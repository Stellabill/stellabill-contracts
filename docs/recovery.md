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

The contract defines three specific recovery reasons, each representing a well-documented scenario:

#### 1. AccidentalTransfer

**When to use**: Tokens sent directly to the contract address by mistake, not associated with any subscription.

**Example**: A user copies the contract address instead of their merchant address and sends 100 USDC directly to the contract.

**Verification steps**:

- Check transaction history to confirm the transfer
- Verify no subscription exists for the sending address
- Confirm the funds are not part of any subscription balance

#### 2. DeprecatedFlow

**When to use**: Funds stranded due to contract upgrades, logic errors, or deprecated functionality.

**Example**: A contract upgrade changes the withdrawal flow, leaving some funds in the old storage pattern that's no longer accessible.

**Verification steps**:

- Document the specific bug or upgrade that caused the issue
- Identify the exact amount and location of stranded funds
- Confirm the funds cannot be recovered through normal contract operations

#### 3. UnreachableSubscriber

**When to use**: Cancelled subscriptions where the subscriber has lost access to their withdrawal keys.

**Example**: A subscriber cancels their subscription but loses their private key before withdrawing their prepaid balance.

**Verification steps**:

- Confirm the subscription is in Cancelled status
- Document evidence the subscriber has lost key access (community request, time elapsed, etc.)
- Verify the subscriber's identity through alternative means if possible

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
    recipient: Address,
    amount: i128,
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

```rust
if amount <= 0 {
    return Err(Error::InvalidRecoveryAmount);
}
```

#### 3. Audit Trail

Every recovery operation emits a `RecoveryEvent` containing:

- Admin address (who authorized)
- Recipient address (where funds went)
- Amount recovered
- Recovery reason (why it was done)
- Timestamp (when it occurred)

```rust
let recovery_event = RecoveryEvent {
    admin: admin.clone(),
    recipient: recipient.clone(),
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

| Date       | Admin     | Recipient | Amount   | Reason             | Tx Hash  |
| ---------- | --------- | --------- | -------- | ------------------ | -------- |
| 2026-02-21 | admin.xlm | user.xlm  | 100 USDC | AccidentalTransfer | 0xabc... |

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
    &user_correct_address,
    &50_000000, // 50 USDC with 6 decimals
    &RecoveryReason::AccidentalTransfer
);
```

### Example 2: Unreachable Subscriber Recovery

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
    &subscriber_new_address,
    &remaining_balance,
    &RecoveryReason::UnreachableSubscriber
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
