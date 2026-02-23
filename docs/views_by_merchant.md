# Merchant Subscription Views

Query subscriptions associated with a merchant address on the `subscription_vault` contract.

---

## Functions

### `get_subscriptions_by_merchant`

Returns a paginated list of subscriptions for a given merchant.

```rust
pub fn get_subscriptions_by_merchant(
    env: Env,
    merchant: Address,
    start: u32,
    limit: u32,
) -> Vec<Subscription>
```

| Parameter  | Type      | Description                                    |
|------------|-----------|------------------------------------------------|
| `merchant` | `Address` | Merchant address to query                      |
| `start`    | `u32`     | 0-based offset into the merchant's list        |
| `limit`    | `u32`     | Maximum number of subscriptions to return       |

**Returns:** `Vec<Subscription>` — ordered chronologically (insertion order). Empty if the merchant has no subscriptions or `start` exceeds the total count.

#### Usage example (Soroban CLI)

```bash
soroban contract invoke \
  --id <CONTRACT_ID> \
  --source <IDENTITY> \
  -- get_subscriptions_by_merchant \
  --merchant <MERCHANT_ADDRESS> \
  --start 0 \
  --limit 10
```

---

### `get_merchant_subscription_count`

Returns the total number of subscriptions for a merchant. Useful for pagination metadata and dashboard summaries.

```rust
pub fn get_merchant_subscription_count(env: Env, merchant: Address) -> u32
```

| Parameter  | Type      | Description               |
|------------|-----------|---------------------------|
| `merchant` | `Address` | Merchant address to query |

**Returns:** `u32` — count of subscriptions belonging to the merchant.

---

## Pagination

Use `start` and `limit` to page through results:

```
Page 1: start=0,  limit=10  → subscriptions 0–9
Page 2: start=10, limit=10  → subscriptions 10–19
Page 3: start=20, limit=10  → subscriptions 20–29 (or fewer if end of list)
```

Combine with `get_merchant_subscription_count` to calculate total pages:

```
total_pages = ceil(count / limit)
```

---

## Performance notes

- **Index storage:** Each merchant has a `Vec<u32>` of subscription IDs stored under `DataKey::MerchantSubs(merchant)`. The index is maintained automatically when subscriptions are created.
- **Ordering:** Results are in chronological (insertion) order — oldest subscriptions first.
- **Cost:** Reading is proportional to the `limit` value, not the total number of merchant subscriptions (the ID list is loaded, but only the requested slice of subscriptions is fetched from storage).
- **Best practice:** Use small `limit` values (10–50) for UI pagination to keep transaction budgets low.

---

## Integration patterns

### Merchant dashboard

1. Call `get_merchant_subscription_count(merchant)` on page load for pagination metadata.
2. Call `get_subscriptions_by_merchant(merchant, page * pageSize, pageSize)` for each page.
3. Display subscription details, filter client-side by status if needed.

### Reporting / export

For merchants with large subscription counts, iterate through all pages:

```
offset = 0
while offset < count:
    page = get_subscriptions_by_merchant(merchant, offset, 50)
    process(page)
    offset += 50
```
