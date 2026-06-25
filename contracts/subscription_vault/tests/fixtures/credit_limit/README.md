# Credit-limit invariant fixtures

Regression corpus for [`tests/credit_limit_invariant.rs`](../../credit_limit_invariant.rs).

The invariant test drives a randomized 500-step sequence of
`create` / `cancel` / `set_subscriber_credit_limit` operations against the
`subscription_vault` contract and checks, after every step, that:

1. `get_subscriber_exposure` equals an independently-tracked model value
   (the summation never wraps under realistic amounts), and
2. an accepted exposure-increasing op never leaves
   `exposure > credit_limit` (when a non-zero limit is configured).

Each sequence is fully deterministic in its `u64` seed, so a failure is
reproducible by replaying the seed.

## Format

`regression_seeds.txt` holds one decimal `u64` seed per line. Blank lines and
lines beginning with `#` are ignored. Every seed listed here is replayed on
each test run — each drives a full, independent 500-step sequence — so the
corpus only ever grows and previously-found failures stay covered.

## Capturing a new failing seed

When the fuzz test panics it prints the offending seed, e.g.:

```
credit-limit invariant violated for seed 1234567890 at step 312: ...
```

Append that seed to `regression_seeds.txt` (with a short comment describing the
bug it pins) and commit it together with the fix. The test will then replay it
forever as a deterministic regression guard.
