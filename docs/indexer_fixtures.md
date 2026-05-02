# Event Indexer Compatibility Fixtures

This repository includes a dedicated fixture pack to ensure all smart contract events maintain stable, predictable payloads for downstream indexers and analytics pipelines.

## Fixture Catalog

The following fixtures are located in `contracts/subscription_vault/src/test_indexer_fixtures.rs`. Each test is prefixed with `fixture_` and exercises a specific lifecycle or event scenario.

| Test Name | Description | Key Events |
|-----------|-------------|------------|
| `fixture_successful_charge_and_deposit` | Standard deposit followed by a successful recurring charge. | `funds_deposited`, `subscription_charged` |
| `fixture_insufficient_balance_grace_period` | Charge fails due to lack of funds, entering the grace window. | `subscription_charge_failed` (GracePeriod) |
| `fixture_insufficient_balance_terminal` | Charge fails after grace period expires, entering terminal failed state. | `subscription_charge_failed` (InsufficientBalance) |
| `fixture_unauthorized_charge_rejected` | Attempt to perform an action (e.g., pause) by an unauthorized address. | (None, returns `Error::Unauthorized`) |
| `fixture_refund_on_cancel_with_prepaid` | Cancellation of a subscription that still has a prepaid balance. | `subscription_cancelled` (refund_amount > 0) |
| `fixture_subscriber_withdrawal_after_cancel` | Subscriber withdraws their remaining balance from a cancelled vault. | `subscriber_withdrawal` |
| `fixture_merchant_withdrawal` | Merchant withdraws their earned fees from the contract. | `merchant_withdrawal` |
| `fixture_batch_charge_all_success` | Batch operation where all charges succeed. | Multiple `subscription_charged` |
| `fixture_batch_charge_partial_failure` | Batch operation where some charges succeed and some fail. | Mixed success/failure events |
| `fixture_batch_charge_all_fail` | Batch operation where all charges fail. | Multiple `subscription_charge_failed` |
| `fixture_lifetime_cap_reached` | Subscription reaches its lifetime charge cap and auto-cancels. | `lifetime_cap_reached`, `subscription_cancelled` |
| `fixture_grace_period_to_active_recovery` | Recovery flow from GracePeriod back to Active after a deposit. | `funds_deposited`, `subscription_resumed` |
| `fixture_pause_resume_charge_sequence` | Manual pausing and resuming of a subscription by the authorizer. | `subscription_paused`, `subscription_resumed` |
| `fixture_replay_protection` | Attempt to charge the same subscription twice in the same billing period. | (None, returns `Error::Replay`) |
| `fixture_emergency_stop_blocks_charge` | Admin enables emergency stop, blocking all charge operations. | `emergency_stop_enabled` |
| `fixture_one_off_charge` | Merchant applies a manual one-off charge to a subscription. | `one_off_charged` |

## Running the Fixtures

To validate the current canonical event sequences or regenerate snapshots:

```bash
cargo test fixture_ -- --nocapture
```

Snapshots are stored under:
`contracts/subscription_vault/test_snapshots/test/`

## Event Schemas & Indexing

All events follow the `soroban-sdk` event pattern. Indexers should listen for the following topics:

- `funds_deposited`: `(Symbol::new(env, "funds_deposited"), subscription_id)`
- `subscription_charged`: `(Symbol::new(env, "subscription_charged"), subscription_id)`
- `subscription_charge_failed`: `(Symbol::new(env, "subscription_charge_failed"), subscription_id)`
- `subscription_cancelled`: `(Symbol::new(env, "subscription_cancelled"), subscription_id)`
- `merchant_withdrawal`: `(Symbol::new(env, "merchant_withdrawal"), merchant_address)`
- `one_off_charged`: `(Symbol::new(env, "one_off_charged"), subscription_id)`

## Security Assumptions

1. **Deterministic Behavior**: All fixtures use controlled environment timestamps (`test_env.jump`) to ensure stable event ordering.
2. **Privacy**: Events include `subscription_id` and `amount` but avoid leaking sensitive metadata keys unless explicitly indexed.
3. **Negative Cases**: Fixtures like `fixture_unauthorized_charge_rejected` ensure that no state is mutated and no valid events are emitted for failed authorization.