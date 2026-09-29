#![cfg(test)]

//! Adversarial coverage for `oracle::get_oracle_price_history`.
//!
//! The function is the only read path for the oracle price history, and it does
//! **not** read the ring buffer the oracle module writes. It reconstructs a list
//! from a different storage kind and a different key shape:
//!
//! | writer (`oracle::record_price`)          | reader (`get_oracle_price_history`)            |
//! | ---------------------------------------- | --------------------------------------------- |
//! | `DataKey::OraclePriceHistoryMeta(token)` | `(token, Symbol("oracle_price_history_meta"))` |
//! | `DataKey::OraclePriceHistoryEntry(t, i)` | `(token, Symbol("oph_{i}"))`                   |
//! | instance storage                         | persistent storage                             |
//!
//! So the properties worth pinning are the ones a caller can hit in production:
//! the reader returns an empty list unless the *reader's own* keys exist, it
//! silently skips holes instead of erroring, it truncates rather than wrapping
//! like a ring buffer, and it never validates the values it returns.
//!
//! The function reads the executing contract's storage, so every read and write
//! runs inside `Env::as_contract` via the `Ctx` helper below.

use crate::oracle::get_oracle_price_history;
use crate::types::{DataKey, OraclePriceHistoryMeta};
use crate::SubscriptionVault;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, Symbol, Vec};

struct Ctx {
    env: Env,
    cid: Address,
}

impl Ctx {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        let cid = env.register(SubscriptionVault, ());
        Self { env, cid }
    }

    fn token(&self) -> Address {
        Address::generate(&self.env)
    }

    fn meta_key(&self, token: &Address) -> (Address, Symbol) {
        (
            token.clone(),
            Symbol::new(&self.env, "oracle_price_history_meta"),
        )
    }

    fn entry_key(&self, token: &Address, index: u32) -> (Address, Symbol) {
        (
            token.clone(),
            Symbol::new(&self.env, &format!("oph_{index}")),
        )
    }

    /// Write the metadata key the reader looks for in persistent storage.
    fn set_meta(&self, token: &Address, count: u32, cursor: u32) {
        let key = self.meta_key(token);
        let value = OraclePriceHistoryMeta { count, cursor };
        self.env.as_contract(&self.cid, || {
            self.env.storage().persistent().set(&key, &value);
        });
    }

    fn set_meta_raw(&self, key: &(Address, Symbol), value: &OraclePriceHistoryMeta) {
        self.env.as_contract(&self.cid, || {
            self.env.storage().persistent().set(key, value);
        });
    }

    fn set_entry(&self, token: &Address, index: u32, price: i128) {
        let key = self.entry_key(token, index);
        self.env.as_contract(&self.cid, || {
            self.env.storage().persistent().set(&key, &price);
        });
    }

    fn set_entry_raw(&self, key: &(Address, Symbol), price: i128) {
        self.env.as_contract(&self.cid, || {
            self.env.storage().persistent().set(key, &price);
        });
    }

    /// Write a sample the way `oracle::record_price` does: instance storage
    /// under an enum `DataKey`.
    fn set_ring_buffer_entry(&self, token: &Address, slot: u32, price: i128) {
        let key = DataKey::OraclePriceHistoryEntry(token.clone(), slot);
        self.env.as_contract(&self.cid, || {
            self.env.storage().instance().set(&key, &price);
        });
    }

    fn set_ring_buffer_meta(&self, token: &Address, count: u32, cursor: u32) {
        let key = DataKey::OraclePriceHistoryMeta(token.clone());
        let value = OraclePriceHistoryMeta { count, cursor };
        self.env.as_contract(&self.cid, || {
            self.env.storage().instance().set(&key, &value);
        });
    }

    fn has_instance_meta(&self, token: &Address) -> bool {
        let key = DataKey::OraclePriceHistoryMeta(token.clone());
        let mut found = false;
        self.env.as_contract(&self.cid, || {
            found = self.env.storage().instance().has(&key);
        });
        found
    }

    fn read(&self, token: &Address) -> Vec<i128> {
        let mut out = Vec::new(&self.env);
        self.env.as_contract(&self.cid, || {
            let prices = get_oracle_price_history(&self.env, token);
            for i in 0..prices.len() {
                out.push_back(prices.get(i).unwrap());
            }
        });
        out
    }

    /// Seed the keys the reader actually looks for.
    fn seed(&self, token: &Address, count: u32, prices: &[i128]) {
        self.set_meta(token, count, 0);
        for (index, price) in prices.iter().enumerate() {
            self.set_entry(token, index as u32, *price);
        }
    }

    fn get_meta(&self, token: &Address) -> OraclePriceHistoryMeta {
        let key = self.meta_key(token);
        let mut found: Option<OraclePriceHistoryMeta> = None;
        self.env.as_contract(&self.cid, || {
            found = self.env.storage().persistent().get(&key);
        });
        found.expect("metadata must be present")
    }
}

// ── Empty and unknown-token behaviour ────────────────────────────────────

#[test]
fn test_unknown_token_returns_an_empty_vec() {
    let ctx = Ctx::new();
    let token = ctx.token();

    assert!(ctx.read(&token).is_empty());
}

#[test]
fn test_empty_env_returns_an_empty_vec_for_every_token() {
    let ctx = Ctx::new();

    for _ in 0..3 {
        let token = ctx.token();
        assert!(ctx.read(&token).is_empty());
    }
}

#[test]
fn test_a_token_with_only_entries_and_no_meta_reads_as_empty() {
    let ctx = Ctx::new();
    let token = ctx.token();

    // Entries without the metadata are unreachable: the loop is bounded by
    // `meta.count`, and a missing meta short-circuits to an empty Vec.
    ctx.set_entry(&token, 0, 42);
    ctx.set_entry(&token, 1, 43);

    assert!(ctx.read(&token).is_empty());
}

#[test]
fn test_count_of_zero_hides_existing_entries() {
    let ctx = Ctx::new();
    let token = ctx.token();

    ctx.set_meta(&token, 0, 3);
    ctx.set_entry(&token, 0, 42);

    assert!(ctx.read(&token).is_empty());
}

// ── The reader's own key shape ──────────────────────────────────────────

#[test]
fn test_returns_prices_in_index_order() {
    let ctx = Ctx::new();
    let token = ctx.token();
    ctx.seed(&token, 3, &[10, 20, 30]);

    assert_eq!(ctx.read(&token), Vec::from_array(&ctx.env, [10, 20, 30]));
}

#[test]
fn test_the_meta_symbol_must_match_exactly() {
    let ctx = Ctx::new();
    let token = ctx.token();
    ctx.seed(&token, 1, &[10]);

    // A subtly different meta key is invisible to the reader...
    let wrong_key = (token.clone(), Symbol::new(&ctx.env, "oracle_price_history"));
    ctx.set_meta_raw(
        &wrong_key,
        &OraclePriceHistoryMeta {
            count: 99,
            cursor: 0,
        },
    );

    // ...but the exact symbol is what makes it work, so the value is unchanged.
    assert_eq!(ctx.read(&token), Vec::from_array(&ctx.env, [10]));
}

#[test]
fn test_entry_keys_are_not_zero_padded() {
    let ctx = Ctx::new();
    let token = ctx.token();

    // count = 1 looks for "oph_0"; "oph_00" is never read.
    ctx.set_meta(&token, 1, 0);
    ctx.set_entry_raw(&(token.clone(), Symbol::new(&ctx.env, "oph_00")), 7);

    assert!(ctx.read(&token).is_empty());
}

#[test]
fn test_the_ring_buffer_keys_are_invisible_to_the_reader() {
    let ctx = Ctx::new();
    let token = ctx.token();

    // This is exactly what `oracle::record_price` writes.
    ctx.set_ring_buffer_entry(&token, 0, 99);
    ctx.set_ring_buffer_meta(&token, 1, 1);

    // The reader consults neither instance storage nor the `DataKey` enum, so a
    // fully populated ring buffer is reported as an empty history.
    assert!(ctx.read(&token).is_empty());
}

#[test]
fn test_persistent_entries_without_any_ring_buffer_state_still_read() {
    let ctx = Ctx::new();
    let token = ctx.token();
    ctx.seed(&token, 2, &[5, 6]);

    assert!(!ctx.has_instance_meta(&token));
    assert_eq!(ctx.read(&token), Vec::from_array(&ctx.env, [5, 6]));
}

// ── Holes, over- and under-claimed counts ───────────────────────────────

#[test]
fn test_a_missing_slot_is_skipped_silently() {
    let ctx = Ctx::new();
    let token = ctx.token();

    ctx.set_meta(&token, 4, 0);
    ctx.set_entry(&token, 0, 10);
    // slot 1 missing
    ctx.set_entry(&token, 2, 30);
    // slot 3 missing

    // No error and no placeholder: the hole simply shrinks the result.
    assert_eq!(ctx.read(&token), Vec::from_array(&ctx.env, [10, 30]));
}

#[test]
fn test_an_over_claimed_count_returns_a_shorter_vec_without_panicking() {
    let ctx = Ctx::new();
    let token = ctx.token();
    ctx.seed(&token, 10, &[1, 2]);

    assert_eq!(ctx.read(&token), Vec::from_array(&ctx.env, [1, 2]));
}

#[test]
fn test_an_under_claimed_count_truncates_instead_of_wrapping() {
    let ctx = Ctx::new();
    let token = ctx.token();
    ctx.seed(&token, 5, &[10, 20, 30, 40, 50]);

    // Only `count` slots are ever visited, so the older samples stay hidden:
    // this is a truncated list, not a ring-buffer window.
    ctx.set_meta(&token, 2, 5);

    assert_eq!(ctx.read(&token), Vec::from_array(&ctx.env, [10, 20]));
}

#[test]
fn test_cursor_is_not_used_to_reorder_the_result() {
    let ctx = Ctx::new();
    let token = ctx.token();

    // A ring buffer at cursor 2 would put slot 2 first. The reader ignores
    // `cursor` entirely and always starts at slot 0.
    ctx.set_meta(&token, 3, 2);
    ctx.set_entry(&token, 0, 10);
    ctx.set_entry(&token, 1, 20);
    ctx.set_entry(&token, 2, 30);

    assert_eq!(ctx.read(&token), Vec::from_array(&ctx.env, [10, 20, 30]));
}

#[test]
fn test_a_slot_overwritten_later_still_reads_in_slot_order() {
    let ctx = Ctx::new();
    let token = ctx.token();

    ctx.set_meta(&token, 2, 0);
    ctx.set_entry(&token, 1, 20);
    // Written "later" but into the lower slot.
    ctx.set_entry(&token, 0, 10);

    assert_eq!(ctx.read(&token), Vec::from_array(&ctx.env, [10, 20]));
}

// ── Value fidelity and token isolation ─────────────────────────────────

#[test]
fn test_extreme_and_sign_edge_values_are_returned_verbatim() {
    let ctx = Ctx::new();
    let token = ctx.token();
    let prices = [0i128, -1, i128::MIN, i128::MAX];
    ctx.seed(&token, prices.len() as u32, &prices);

    // No validation: zero, negative and saturated values all pass through.
    assert_eq!(ctx.read(&token), Vec::from_array(&ctx.env, prices));
}

#[test]
fn test_histories_are_isolated_per_token() {
    let ctx = Ctx::new();
    let first = ctx.token();
    let second = ctx.token();

    ctx.seed(&first, 2, &[1, 2]);
    ctx.seed(&second, 1, &[900]);

    assert_eq!(ctx.read(&first), Vec::from_array(&ctx.env, [1, 2]));
    assert_eq!(ctx.read(&second), Vec::from_array(&ctx.env, [900]));
}

#[test]
fn test_another_tokens_entries_at_the_same_index_do_not_leak() {
    let ctx = Ctx::new();
    let first = ctx.token();
    let second = ctx.token();

    ctx.set_meta(&first, 1, 0);
    // Same symbol, different token: must not be picked up.
    ctx.set_entry(&second, 0, 777);

    assert!(ctx.read(&first).is_empty());
}

// ── Purity ──────────────────────────────────────────────────────────────

#[test]
fn test_repeated_reads_are_stable_and_write_nothing() {
    let ctx = Ctx::new();
    let token = ctx.token();
    ctx.seed(&token, 2, &[4, 5]);

    let first = ctx.read(&token);
    let second = ctx.read(&token);

    assert_eq!(first, second);
    // The metadata is only ever read, never rewritten.
    let before = ctx.get_meta(&token);
    ctx.read(&token);
    let after = ctx.get_meta(&token);
    assert_eq!(
        (before.count, before.cursor),
        (after.count, after.cursor)
    );
}
