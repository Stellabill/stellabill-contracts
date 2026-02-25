# Gas Optimizations

This document outlines the gas optimizations implemented in the `subscription_vault` contract.

## 1. Batching Ledger Timestamp Reads
**Optimization**: In `admin::do_batch_charge` and `subscription::do_charge_subscription`, the current ledger timestamp (`env.ledger().timestamp()`) is evaluated once and passed down to `charge_one`.

**Impact**: 
- Reduces host calls and cross-VM boundary iterations.
- In `do_batch_charge`, evaluating the timestamp once instead of N times (where N is the number of subscriptions being charged in the batch) significantly lowers the cost of batched execution and helps accommodate more charges within transaction gas limits.

## 2. Reusing Instance Storage Proxies
**Optimization**: Replaced multiple repetitive calls to `env.storage().instance()` with a single local variable assignment (e.g. `let storage = env.storage().instance();`) across the contract's codebase, such as in `next_id` and `do_init` and `charge_one`.

**Impact**: 
- Helps prevent repeated proxy instantiation for storage wrappers, optimizing Wasm execution cycle costs.
- Makes the code more explicit about the amount of storage interactions.

## 3. General Storage Design
**Optimization**: Retaining efficient `Persistent` equivalent entries for unbounded subscriptions per ID and reserving `Instance` storage strictly for global vault states (Token, Admin, minimum topup, metadata). 
