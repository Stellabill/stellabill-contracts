## Dispute / Chargeback workflow

The subscription vault implements a two-step dispute workflow (`open_dispute`, `respond_dispute`,
`resolve_dispute`) that mirrors payment-card chargeback semantics. Disputed funds are held in
escrow for a configurable window (default: [`DISPUTE_WINDOW_SECS`] = 14 days), giving the
merchant/admin time to respond.

### Entry points

| Method | Auth | Description |
|--------|------|-------------|
| `open_dispute(subscriber, subscription_id, amount, evidence_hash)` | subscriber | Moves `amount` from merchant balance to escrow; creates `Dispute` in `Open` status. |
| `respond_dispute(admin, dispute_id, evidence_hash)` | admin | Transitions dispute to `Responded` status; admin provides evidence. |
| `resolve_dispute(admin, dispute_id, resolve_to_subscriber)` | admin | Final resolution; routes escrowed funds to subscriber or merchant. |
| `get_dispute(dispute_id)` | — | Returns the `Dispute` record. |
| `get_subscription_dispute(subscription_id)` | — | Returns the active dispute ID for a subscription, if any. |

### Resolution rules

| Dispute status | Window elapsed | Allowed resolution |
|:---|:---:|:---|
| `Open` | No | Rejected — `DisputeNotResponded`. Admin must respond first. |
| `Open` | Yes | Auto-resolve to **subscriber** (default win for subscriber). |
| `Responded` | Either | Admin chooses: subscriber **or** merchant. |
| Resolved (any) | — | Rejected — `DisputeAlreadyResolved`. |

### Events

Every successful dispute mutation emits a dedicated event:

```text
topic:   ("dispute_opened", dispute_id)
payload: DisputeOpenedEvent { dispute_id, subscription_id, subscriber, merchant,
         amount, evidence_hash, timestamp, schema_version }
```

```text
topic:   ("dispute_responded", dispute_id)
payload: DisputeRespondedEvent { dispute_id, subscription_id, admin_evidence_hash,
         timestamp, schema_version }
```

```text
topic:   ("dispute_resolved", dispute_id)
payload: DisputeResolvedEvent { dispute_id, subscription_id, resolution,
         timestamp, schema_version }
```

### Security invariants

- **Double-open prevention**: `DataKey::SubscriptionDispute(u32)` tracks the active
  dispute per subscription. A second `open_dispute` for the same subscription returns
  `DisputeAlreadyOpen`.
- **Escrow accounting**: The disputed amount is subtracted from
  `MerchantBalance(merchant, token)` and held under `DisputeEscrow(dispute_id)` until
  resolution. The sum `merchant_balance + escrow` is invariant after open and before
  resolve.
- **CEI ordering**: State (escrow deduction, dispute record) is written before any
  external token transfer. For subscriber-win resolutions, the token transfer runs
  after `add_total_accounted` is decremented.
- **Admin-only gates**: `respond_dispute` and `resolve_dispute` require admin auth
  via `require_admin_auth`.
- **Evidence hashes**: Optional `BytesN<32>` hashes are stored on-chain but the
  actual evidence is assumed to be off-chain (IPFS / blob store).

### Dispute record

```rust
pub struct Dispute {
    pub id: u64,
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub amount: i128,
    pub opened_at: u64,
    pub status: DisputeStatus,  // Open | Responded | ResolvedToMerchant | ResolvedToSubscriber
    pub evidence_hash: Option<BytesN<32>>,
    pub responded_at: Option<u64>,
    pub admin_evidence_hash: Option<BytesN<32>>,
}
```

### Error codes

| Code | Variant | Meaning |
|:---:|:---|:---|
| 10001 | `DisputeNotFound` | No dispute for the given ID. |
| 10002 | `DisputeAlreadyResolved` | Dispute has already been resolved. |
| 10003 | `DisputeNotResponded` | Cannot resolve before response (window not elapsed). |
| 10004 | `DisputeWindowElapsed` | The dispute window has elapsed (advisory). |
| 10005 | `DisputeAlreadyOpen` | A dispute is already open for this subscription. |
| 10006 | `DisputeAlreadyResponded` | Dispute is not in `Open` status (already responded/resolved). |

---

## Partial refunds for mid-period downgrades and cancellations

The subscription vault supports controlled partial refunds so that merchants and operators can
return a portion of a subscriber's prepaid balance when plans are downgraded or cancelled
mid-period, without compromising balance integrity.

### Design goals

- **Safety first** – No fund creation or loss; all refunds are debits from existing balances.
- **Explicit authorization** – Only the contract admin can authorize partial refunds.
- **Predictable semantics** – Refunds operate on remaining prepaid balances and do not
  retroactively alter past charges.
- **Clear observability** – Each refund emits a dedicated event for off-chain reconciliation.

### Entry point

`partial_refund(admin, subscription_id, subscriber, amount) -> Result<(), Error>`

#### Authorization model

- `admin` must be the stored contract admin. The call is gated via `require_admin_auth`,
  which calls `admin.require_auth()` and verifies the address matches the stored admin.
- `subscriber` is a **validation target only** — it is checked against the subscription's
  `subscriber` field to prevent misdirected refunds, but the subscriber does **not** need
  to co-sign the transaction. The admin acts on the subscriber's behalf.

This design allows a backend operations service running as the contract admin to issue
refunds without requiring the subscriber to be online or to sign.

#### Preconditions

| Condition | Error on failure |
|-----------|-----------------|
| `admin` is the stored contract admin | `Unauthorized` |
| `amount > 0` | `InvalidAmount` |
| `subscriber` matches `subscription.subscriber` | `Unauthorized` |
| `amount <= subscription.prepaid_balance` | `InsufficientBalance` |

#### Effects (CEI pattern)

1. **Checks** — all preconditions validated before any state change.
2. **Effects** — `subscription.prepaid_balance` decremented by `amount` and persisted.
3. **Interactions** — token transfer from vault to subscriber executed after state update.

This ordering prevents reentrancy: if the token transfer re-enters the contract, the
balance has already been debited so a second refund of the same amount will fail the
`InsufficientBalance` check.

#### Event

Every successful partial refund emits:

```
topic:   ("partial_refund", subscription_id)
payload: PartialRefundEvent {
    subscription_id: u32,
    subscriber:      Address,
    amount:          i128,
    timestamp:       u64,
}
```

### Refund semantics

Partial refunds work against the **remaining prepaid balance**:

- Funds that have not yet been charged (unused balance) can be partially refunded.
- Previously processed charges that already credited merchant balances are not
  modified by this API; they remain part of the settlement history.
- Multiple successive partial refunds are allowed as long as each individual
  `amount <= current prepaid_balance` at the time of the call.
- A refund equal to the full remaining balance is valid ("full-balance-as-partial").
- Partial refunds are permitted on subscriptions in **any status**, including
  `Cancelled`. This supports the common pattern of cancelling first, then issuing
  a prorated refund of the remaining balance before the subscriber withdraws.

### Common flows

#### Cancellation with prorated refund

```
1. cancel_subscription(subscription_id, subscriber)
2. partial_refund(admin, subscription_id, subscriber, prorated_amount)
3. withdraw_subscriber_funds(subscription_id, subscriber)   // withdraw remainder
```

#### Mid-period downgrade

```
1. partial_refund(admin, subscription_id, subscriber, agreed_amount)
   // Future charges will use the new (lower) plan amount
```

### Security notes

- **Over-refund protection**: `amount > prepaid_balance` is rejected with
  `InsufficientBalance`. The contract cannot create tokens; it can only transfer
  what it holds.
- **Subscriber ownership check**: passing a `subscriber` address that does not match
  the subscription record returns `Unauthorized`, preventing refunds to wrong addresses.
- **Admin-only gate**: non-admin callers receive `Unauthorized` regardless of other
  parameters.
- **CEI ordering**: state is written before the token transfer, eliminating the
  reentrancy window present in naive implementations.

### Test coverage

| Scenario | Test |
|----------|------|
| Basic debit + token transfer | `test_partial_refund_debits_prepaid_and_transfers_tokens` |
| Zero amount rejected | `test_partial_refund_rejects_invalid_amounts_and_auth` |
| Negative amount rejected | `test_partial_refund_rejects_invalid_amounts_and_auth` |
| Over-refund rejected | `test_partial_refund_rejects_invalid_amounts_and_auth` |
| Non-admin rejected | `test_partial_refund_rejects_invalid_amounts_and_auth` |
| Wrong subscriber rejected | `test_partial_refund_rejects_invalid_amounts_and_auth` |
| Repeated refunds are cumulative | `test_partial_refund_repeated_debits_are_cumulative` |
| Cumulative drain then over-refund fails | `test_partial_refund_cumulative_exact_drain_then_over_refund_fails` |
| Full balance as partial | `test_partial_refund_full_balance_as_partial_succeeds` |
| Refund after cancellation | `test_partial_refund_after_cancellation_succeeds` |
| Event emission | `test_partial_refund_emits_event` |
