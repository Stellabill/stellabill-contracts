# Billing Statement Reconciliation Strategy

When billing statements are pruned (compacted), the detailed history is replaced by a `BillingStatementAggregate`. To maintain financial reporting accuracy and perform reconciliation, follow this strategy.

## 1. Aggregate Structure
The `BillingStatementAggregate` stores the summary of all pruned statements:
- `pruned_count`: Total number of rows removed.
- `total_amount`: Sum of `amount` across all removed rows.
- `totals`: Per-kind breakdown (`interval`, `usage`, `one_off`).
- `oldest_period_start` / `newest_period_end`: Time range covered by pruned data.

## 2. Reconstructing Full History
To calculate the total billed amount for a subscription since its creation:
```
Total Billed = Aggregate.total_amount + Sum(LiveStatements.amount)
```

To calculate the breakdown per charge kind:
```
Total Interval = Aggregate.totals.interval + Sum(LiveStatements where kind == Interval)
Total Usage = Aggregate.totals.usage + Sum(LiveStatements where kind == Usage)
Total One-Off = Aggregate.totals.one_off + Sum(LiveStatements where kind == OneOff)
```

## 3. Verification & Integrity
- **Sequence Integrity**: The lowest `sequence` number in the live statements should be equal to `Aggregate.pruned_count`. Any gap indicates a data integrity issue.
- **Count Consistency**: `SubscriptionVault::get_total_statements` returns the count of *live* statements. The total number of statements ever created can be inferred as `Aggregate.pruned_count + LiveCount`.
- **Amount Consistency**: The `Subscription::lifetime_charged` field should always equal the sum of all billing statements (compacted + live).
    - Note: Differences may arise if refunds were processed, which are tracked separately in `MerchantEarnings`.

## 4. Reconciliation Workflow
1. Call `get_stmt_compacted_aggregate(subscription_id)` to get the summary of pruned history.
2. Call `get_sub_statements_offset` or `get_sub_statements_cursor` to fetch live detailed rows.
3. Sum the values as described above.
4. Compare against `get_subscription(subscription_id).lifetime_charged` for high-level validation.

---

# Contract-Level Reconciliation Queries

The contract provides read-only endpoints for off-chain auditors to validate the accounting equation:

```
contract_token_balance = total_prepaid + total_merchant_liabilities + recoverable
```

## API Overview

### 1. Token-Level Reconciliation: `get_token_reconciliation(token)`

Returns complete reconciliation data for a single settlement token.

**Response: `TokenLiabilities`**
- `token`: Token contract address
- `total_prepaid`: Sum of all subscriber prepaid balances
- `total_merchant_liabilities`: Sum of all merchant earnings (accruals - withdrawals - refunds)
- `recoverable_amount`: Stranded funds that can be recovered by admin
- `contract_balance`: Actual token balance held by the contract
- `computed_total`: Prepaid + merchant liabilities + recoverable
- `is_balanced`: Whether the accounting equation validates

**Usage:**
```rust
let reconciliation = client.get_token_reconciliation(&usdc_token);
assert!(reconciliation.is_balanced);
assert_eq!(
    reconciliation.contract_balance,
    reconciliation.total_prepaid
        + reconciliation.total_merchant_liabilities
        + reconciliation.recoverable_amount
);
```

### 2. Multi-Token Summary: `get_contract_reconciliation_summary(start_token_index, limit)`

Returns paginated reconciliation data for all accepted tokens.

**Parameters:**
- `start_token_index`: Index into accepted tokens list (0 for first page)
- `limit`: Maximum summaries to return (capped at 50)

**Response: `ReconciliationSummaryPage`**
- `token_summaries`: Vector of `TokenLiabilities`
- `next_token_index`: Cursor for next page, `None` when complete

**Usage:**
```rust
// Get all token reconciliations
let mut index = 0u32;
loop {
    let page = client.get_contract_reconciliation_summary(&index, &50);
    for summary in &page.token_summaries {
        println!("Token: {:?}, Balanced: {}", summary.token, summary.is_balanced);
    }
    match page.next_token_index {
        Some(next) => index = next,
        None => break,
    }
}
```

### 3. Auditable Proof Generation: `generate_reconciliation_proof(token)`

Creates an auditable snapshot with all data needed to independently validate the accounting equation.

**Response: `ReconciliationProof`**
- `timestamp`: Ledger timestamp when proof was generated
- `ledger_sequence`: Ledger sequence for temporal anchoring
- `token`: Token being audited
- `contract_balance`: Contract's token balance
- `total_prepaid`: Sum of all subscriber prepaid balances
- `total_merchant_liabilities`: Total merchant earnings liabilities
- `computed_recoverable`: Calculated recoverable amount
- `subscription_count`: Number of subscriptions scanned
- `merchant_count`: Number of merchants with earnings
- `is_valid`: Whether the accounting equation validates

**Security Properties:**
- Read-only: Cannot modify contract state
- Temporally anchored: Includes ledger sequence
- Self-contained: All validation data in one struct

### 4. Paginated Prepaid Query: `query_prepaid_balances_paginated(request)`

Bounded-compute query for aggregating prepaid balances across subscriptions.

**Request: `PrepaidQueryRequest`**
- `token`: Token to filter by (required)
- `start_subscription_id`: Starting subscription ID (inclusive)
- `scan_limit`: Maximum subscriptions to scan (capped at 500)

**Response: `PrepaidQueryResult`**
- `token`: Token queried
- `partial_total`: Sum of prepaid balances in scan window
- `subscriptions_count`: Number of subscriptions with non-zero prepaid
- `next_start_id`: Next ID to scan, `None` if complete
- `has_more`: Whether more subscriptions exist beyond window

**Off-Chain Aggregation Example:**
```rust
let mut total_prepaid = 0i128;
let mut start_id = 0u32;

loop {
    let result = client.query_prepaid_balances_paginated(&PrepaidQueryRequest {
        token: usdc_token.clone(),
        start_subscription_id: start_id,
        scan_limit: 500,
    });

    total_prepaid += result.partial_total;

    if !result.has_more {
        break;
    }
    start_id = result.next_start_id.unwrap();
}
```

## Reconciliation Workflow for Auditors

### Quick Validation (Single Token)
```rust
// 1. Get reconciliation data
let recon = client.get_token_reconciliation(&token);

// 2. Verify accounting equation
assert!(recon.is_balanced, "Accounting equation does not balance!");

// 3. Verify specific amounts
assert_eq!(
    recon.contract_balance,
    recon.total_prepaid + recon.total_merchant_liabilities + recon.recoverable_amount
);
```

### Full Audit with Proof Generation
```rust
// Generate proof for record keeping
let proof = client.generate_reconciliation_proof(&token);

// Store proof off-chain with ledger sequence for temporal reference
store_audit_record(proof.ledger_sequence, proof);

// Verify at a later date
let current = client.get_token_reconciliation(&token);
assert_eq!(current.contract_balance, proof.contract_balance); // Or investigate changes
```

### Multi-Token Portfolio Reconciliation
```rust
let mut all_balanced = true;
let mut start_index = 0u32;

loop {
    let page = client.get_contract_reconciliation_summary(&start_index, &50);

    for summary in &page.token_summaries {
        if !summary.is_balanced {
            all_balanced = false;
            log_imbalance(&summary.token, summary);
        }
    }

    match page.next_token_index {
        Some(next) => start_index = next,
        None => break,
    }
}

assert!(all_balanced, "Some tokens have accounting imbalances!");
```

## Performance & Security Considerations

### Bounded Compute
- `MAX_PREPAID_SCAN_DEPTH = 500`: Limits subscription scans per call
- `MAX_TOKEN_SUMMARIES_PER_PAGE = 50`: Limits token summaries per call
- Indexers should chain paginated calls to build complete totals

### Gas Efficiency
- `get_token_reconciliation`: O(subscriptions + merchants) — use for spot checks
- `generate_reconciliation_proof`: Same complexity but returns compact proof
- `query_prepaid_balances_paginated`: O(scan_limit) — bounded and predictable

### Read-Only Safety
All reconciliation endpoints are read-only and cannot modify contract state. They:
- Require no authentication
- Emit no events
- Have no side effects
- Are safe to call at any time

## Indexer Integration

Indexers computing off-chain proofs should:

1. **Use paginated queries** for large datasets
2. **Validate proofs** against on-chain data periodically
3. **Store ledger sequences** with proof records for temporal validation
4. **Monitor `is_balanced`** for anomaly detection
5. **Aggregate across pages** to verify total contract liabilities

Example indexer proof computation:
```rust
// 1. Collect paginated prepaid data
let prepaid_total = aggregate_paginated_prepaid(&client, &token);

// 2. Get merchant liabilities (from indexed data or contract)
let merchant_total = get_indexed_merchant_liabilities(&token);

// 3. Get contract balance from token contract
let contract_balance = token_client.balance(&vault_address);

// 4. Compute and verify
let recoverable = contract_balance - prepaid_total - merchant_total;
assert!(recoverable >= 0, "Negative recoverable indicates data inconsistency");
```
