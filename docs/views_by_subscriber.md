# List Subscriptions by Subscriber

## Overview

`list_subscriptions_by_subscriber` is a read-only view function that retrieves all subscriptions owned by a given subscriber address with support for efficient pagination.

## Function Signature

```rust
pub fn list_subscriptions_by_subscriber(
    env: Env,
    subscriber: Address,
    start_from_id: u32,
    limit: u32,
) -> Result<SubscriptionsPage, Error>
```

## Parameters

| Parameter       | Type      | Description                                                                                                  |
| --------------- | --------- | ------------------------------------------------------------------------------------------------------------ |
| `env`           | `Env`     | Contract environment reference                                                                               |
| `subscriber`    | `Address` | The Stellar address of the subscriber to query                                                               |
| `start_from_id` | `u32`     | ID to start scanning from (inclusive). Use `0` for the first page, or `next_start_id` from the previous page |
| `limit`         | `u32`     | Maximum number of subscription IDs to return per page. Must be in `1..=100`                                  |

## Returns

Returns a `SubscriptionsPage` struct:

```rust
pub struct SubscriptionsPage {
    pub subscription_ids: Vec<u32>,
    pub next_start_id: Option<u32>,
}
```

- **`subscription_ids`**: Vector of subscription IDs belonging to this subscriber, in ascending ID order.
- **`next_start_id`**: When `Some(id)`, pass it as `start_from_id` on the next call to resume pagination. `None` means there are no more IDs to scan in the current budget window.

## Performance Characteristics

- **Time Complexity**: O(min(`MAX_SCAN_DEPTH`, `next_id − start_from_id`)) storage reads per call.
- **Space Complexity**: O(limit) for the result page.
- **Scan cap**: At most `MAX_SCAN_DEPTH` (1 000) IDs are inspected per call. If the budget is exhausted before `limit` matches are found, `next_start_id` is set to the first unscanned position so the caller can chain another call.

## Errors

- **`Error::InvalidInput`**: Returned if `limit` is `0` or greater than `100`.

## Usage Examples

### Example 1: Fetch First Page

```rust
let page = client.list_subscriptions_by_subscriber(
    &subscriber_address,
    &0u32,      // Start from the beginning
    &20u32,     // Limit to 20 results
);

for sub_id in page.subscription_ids {
    let sub = client.get_subscription(&sub_id);
    println!("Subscription {}: {} per {} seconds",
             sub_id, sub.amount, sub.interval_seconds);
}
```

### Example 2: Paginate Through All Subscriptions

```rust
let mut start_id = 0u32;
let mut all_subscriptions = Vec::new();

loop {
    let page = client.list_subscriptions_by_subscriber(
        &subscriber_address,
        &start_id,
        &20u32,
    );

    all_subscriptions.extend(page.subscription_ids.iter());

    match page.next_start_id {
        None => break,
        Some(next) => start_id = next,
    }
}

println!("Total subscriptions: {}", all_subscriptions.len());
```

### Example 3: Resume From a Specific ID

```rust
let resume_from = 43u32; // Previously stored from next_start_id

let page = client.list_subscriptions_by_subscriber(
    &subscriber_address,
    &resume_from,
    &20u32,
);
```

### Example 4: Check if a Specific Subscription Exists

```rust
let subscription_id = 100u32;

let page = client.list_subscriptions_by_subscriber(
    &subscriber_address,
    &subscription_id,
    &1u32,
);

let exists = page.subscription_ids.get(0)
    .map(|id| id == subscription_id)
    .unwrap_or(false);
```

## Pagination Strategy

The function uses **cursor-based pagination** with inclusive lower bounds:

1. **Start ID**: The `start_from_id` parameter is inclusive — the result set can include that ID if it belongs to the subscriber.
2. **Pagination cursor**: Pass `next_start_id` verbatim as `start_from_id` on the next call. Do not add 1; the contract already advances past the last scanned position.
3. **Predictable ordering**: Results are always ordered by subscription ID in ascending order.
4. **Scan budget**: Each call scans at most `MAX_SCAN_DEPTH` (1 000) IDs. Sparse distributions may require more round-trips; `next_start_id` is always set correctly for resumption.

## Edge Cases

### Zero Subscriptions

- Empty `subscription_ids` vector and `next_start_id = None`.

### Exact Multiple of Limit

When the subscriber has exactly `n × limit` subscriptions spread densely:

- All pages except the last will have `next_start_id = Some(...)`.
- The final page returns `limit` results and `next_start_id = None` once the scan window is exhausted.

### Start ID Beyond Range

If `start_from_id` is greater than the highest allocated subscription ID:

- Returns empty `subscription_ids` and `next_start_id = None`.

### Sparse ID Ranges

If many IDs belong to other subscribers, the scan budget may be exhausted before `limit` matches are found. `next_start_id` will point past the scanned window; chain calls until `next_start_id` is `None`.

## Off-Chain Usage (Indexers & UI)

### For Indexers

1. Store `next_start_id` between syncs.
2. On next sync, pass the stored value as `start_from_id` — no ± 1 adjustment needed.
3. Chain calls until `next_start_id` is `None`.

### For UI Applications

1. Display first page with a reasonable `limit` (10–50).
2. Load next page on demand using `next_start_id`.
3. Show a "load more" control when `next_start_id` is `Some`.

### For Analytics

Full enumeration is feasible for typical subscriber counts. Use a small limit (e.g., 10) in batched requests when scanning must avoid blocking the host.

## Performance Constraints

| Constant                  | Value | Purpose                                     |
|---------------------------|-------|---------------------------------------------|
| `MAX_SUBSCRIPTION_LIST_PAGE` | 100   | Hard upper bound on `limit` per call        |
| `MAX_SCAN_DEPTH`          | 1 000 | Max IDs inspected per call (read-path guard) |

## Testing

The feature includes comprehensive test coverage in `test_query_performance.rs`:

- Zero subscriptions
- Basic pagination (first / middle / last page)
- Scan depth boundary enforcement
- Sparse ID ranges
- Limit of 1 and limit of 100
- Invalid limit (0, > 100) → `Error::InvalidInput`
- Multi-subscriber isolation (no cross-contamination)
- Exact multiple of limit
- Start ID beyond range
- Deterministic ascending ordering

## Related Functions

- **`get_subscription(id)`**: Retrieve full details of a specific subscription by ID.
- **`get_subscriptions_by_merchant(merchant, start, limit)`**: Offset-based list for a specific merchant.
- **`get_next_charge_info(id)`**: Next billing timestamp for a subscription.
