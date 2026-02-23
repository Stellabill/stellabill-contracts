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
| `start_from_id` | `u32`     | ID to start from (inclusive). Use `0` to start from the beginning, or the last ID + 1 from the previous page |
| `limit`         | `u32`     | Maximum number of subscription IDs to return per page. Must be greater than 0                                |

## Returns

Returns a `SubscriptionsPage` struct containing:

```rust
pub struct SubscriptionsPage {
    pub subscription_ids: Vec<u32>,
    pub has_next: bool,
}
```

- **`subscription_ids`**: Vector of subscription IDs matching the query parameters
- **`has_next`**: Boolean indicating if there are more results available beyond this page

## Performance Characteristics

- **Time Complexity**: O(n) where n is the number of subscriptions to scan
- **Space Complexity**: O(limit) for storing the result page
- **Storage Accesses**: O(n) read operations across the subscription ID range

## Errors

- **`Error::NotFound`**: Returned if `limit` is 0

## Usage Examples

### Example 1: Fetch First Page

```rust
let page = client.list_subscriptions_by_subscriber(
    &subscriber_address,
    &0u32,      // Start from the beginning
    &20u32,     // Limit to 20 results
);

if !page.subscription_ids.is_empty() {
    println!("Found {} subscriptions", page.subscription_ids.len());
    for sub_id in page.subscription_ids {
        let sub = client.get_subscription(&sub_id);
        println!("Subscription {}: {} per {} seconds",
                 sub_id, sub.amount, sub.interval_seconds);
    }
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

    if !page.has_next {
        break;
    }

    // Move to next page (start after the last ID from this page)
    if let Some(&last_id) = page.subscription_ids.last() {
        start_id = last_id + 1;
    } else {
        break;  // No results on this page
    }
}

println!("Total subscriptions: {}", all_subscriptions.len());
```

### Example 3: Resume From a Specific ID

```rust
let last_seen_id = 42u32;  // Previously retrieved from pagination
let resume_from = last_seen_id + 1;

let page = client.list_subscriptions_by_subscriber(
    &subscriber_address,
    &resume_from,
    &20u32,
);
```

### Example 4: Check if Subscription Exists

```rust
let subscription_id = 100u32;

let page = client.list_subscriptions_by_subscriber(
    &subscriber_address,
    &subscription_id,
    &1u32,
);

let exists = page.subscription_ids.get(0)
    .map(|&id| id == subscription_id)
    .unwrap_or(false);

if exists {
    println!("Subscription {} exists", subscription_id);
}
```

## Pagination Strategy

The function uses **cursor-based pagination** with inclusive lower bounds:

1. **Start ID**: The `start_from_id` parameter is inclusive, meaning the result set can include that ID if it belongs to the subscriber
2. **Pagination Cursor**: Use `last_id + 1` from the current page as `start_from_id` for the next page to avoid gaps
3. **Predictable Ordering**: Results are always ordered by subscription ID in ascending order (0, 1, 2, ...)
4. **Has Next Detection**: The function checks if there are subscriptions beyond the current page limit to populate `has_next`

## Edge Cases

### Zero Subscriptions

If a subscriber has no subscriptions, the response contains:

- Empty `subscription_ids` vector
- `has_next = false`

### Exact Multiple of Limit

If subscriptions divide evenly into pages:

- Last page returns exactly `limit` subscriptions
- `has_next = false` (no more subscriptions after this)

### Start ID Beyond Range

If `start_from_id` is greater than the highest subscription ID:

- Returns empty `subscription_ids` vector
- `has_next = false`

### Single Subscription Page

When `limit=1`, you can iterate one subscription at a time:

```rust
let mut start_id = 0u32;
while let Ok(page) = client.list_subscriptions_by_subscriber(&subscriber, &start_id, &1u32) {
    if page.subscription_ids.is_empty() { break; }
    let id = page.subscription_ids.get(0).unwrap();
    start_id = id + 1;
    // Process subscription
}
```

## Off-Chain Usage (Indexers & UI)

This function is optimized for off-chain indexing and UI consumption:

### For Indexers

1. Store the last retrieved `start_from_id` value
2. Use `start_from_id + 1` on next sync to avoid duplicate processing
3. Batch multiple pagination requests for efficiency
4. Cache results with TTL matching subscription state change frequency

### For UI Applications

1. Display first page with reasonable `limit` (10-50)
2. Load next page on demand as user scrolls
3. Show "has_next" indicator to inform user of more data
4. Optionally cache pages locally with appropriate invalidation strategy

### For Analytics

1. Full pagination scan is feasible for reasonable subscriber counts
2. Use small limit (e.g., 5) in batched requests if scanning must avoid blocking
3. ID-based ordering enables reliable incremental updates

## Testing

The feature includes comprehensive test coverage for:

- Zero subscriptions per subscriber
- Single subscription queries
- Multiple subscriptions
- Pagination (first page, second page)
- Subscriber filtering (multi-subscriber isolation)
- Small limit values (limit=1)
- Error on limit=0
- Correct start_from_id cursor behavior
- Stable ordering across multiple queries
- Multiple merchants per subscriber

All tests verify:

- Correct result counts
- Accurate ID matching
- has_next flag accuracy
- Subscriber filtering isolation

## Related Functions

- **`get_subscription(id)`**: Retrieve full details of a specific subscription by ID
- **`get_subscriptions_by_merchant(merchant, start, limit)`**: List subscriptions for a specific merchant
- **`get_next_charge_info(id)`**: Get billing information for a subscription
