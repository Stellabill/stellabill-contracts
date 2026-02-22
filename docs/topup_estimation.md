# Top-up estimation

Read-only helper to compute how much additional prepaid balance is required to cover a specified number of future billing intervals.

## Function

`estimate_topup_for_intervals(env, subscription_id, num_intervals) -> Result<i128, Error>`

- **subscription_id**: The subscription to evaluate.
- **num_intervals**: Number of future intervals to cover (e.g. 3 for “next 3 charges”).
- **Returns**: Additional amount (in token base units) the subscriber should top up. Zero if current balance already covers `num_intervals` or more.

## Behavior

- Uses **safe math** (`checked_mul`, `checked_sub`); returns `Error::Overflow` if `amount * num_intervals` would overflow.
- **Zero intervals:** returns `Ok(0)` (no top-up needed).
- **Insufficient balance:** returns the shortfall (positive amount to add).
- **Balance already sufficient:** returns `0`.
- **Subscription not found:** returns `Err(Error::NotFound)`.

## Usage (UI)

- Call the helper with the subscription ID and desired number of intervals (e.g. 3).
- If result is `0`, show “Your balance covers the next N payments.”
- If result is positive, show “Add X USDC to cover the next N payments” and optionally pre-fill the deposit amount.

## Limitations

- Does not account for future charges that might occur before the user tops up; it is a snapshot.
- Assumes `amount` and `prepaid_balance` are in the same token base units (e.g. 6 decimals for USDC).
