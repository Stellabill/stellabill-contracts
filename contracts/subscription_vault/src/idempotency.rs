//! Shared ring-buffer idempotency key helpers.
//!
//! Three entrypoints use idempotency keys: `charge_subscription`,
//! `deposit_funds`, and `charge_one_off`.  Each domain is scoped with a
//! unique domain constant so that reusing the same raw 32-byte key across
//! different entrypoints does **not** produce a replay collision.
//!
//! Storage key: `DataKey::IdemKey(subscription_id)` stores `IdemRingBuffer`.

use crate::types::{DataKey, IdemRingBuffer, IDEM_HISTORY};
use soroban_sdk::{BytesN, Env, Vec};

/// Return the raw byte representation of a 32-byte idempotency key.
fn key_bytes(key: &BytesN<32>) -> [u8; 32] {
    let mut out = [0u8; 32];
    let arr = key.to_array();
    out.copy_from_slice(&arr);
    out
}

/// Hash (domain, subscription_id, raw_key) into a 32-byte fingerprint.
///
/// The caller **must** supply the correct `domain` constant for their
/// entrypoint so that two different operations receiving the same raw key
/// produce different fingerprints.
pub fn hash_idem_key(
    env: &Env,
    domain: u32,
    subscription_id: u32,
    raw_key: &BytesN<32>,
) -> BytesN<32> {
    let raw = key_bytes(raw_key);
    let mut buf = [0u8; 40];
    buf[..4].copy_from_slice(&domain.to_be_bytes());
    buf[4..8].copy_from_slice(&subscription_id.to_be_bytes());
    buf[8..40].copy_from_slice(&raw);
    let input = soroban_sdk::Bytes::from_slice(env, &buf);
    env.crypto().sha256(&input).into()
}

/// Load the ring buffer for `subscription_id`.
///
/// Returns an empty buffer when no idempotency key has ever been stored.
fn load_buffer(env: &Env, subscription_id: u32) -> IdemRingBuffer {
    env.storage()
        .instance()
        .get(&DataKey::IdemKey(subscription_id))
        .unwrap_or(IdemRingBuffer {
            entries: Vec::new(env),
            cursor: 0,
        })
}

/// Persist the ring buffer for `subscription_id`.
fn save_buffer(env: &Env, subscription_id: u32, buf: &IdemRingBuffer) {
    env.storage()
        .instance()
        .set(&DataKey::IdemKey(subscription_id), buf);
}

/// Check whether `hashed` already exists in the ring buffer.
///
/// Returns `true` when the key is a duplicate (replay).
pub fn check_key(
    env: &Env,
    subscription_id: u32,
    hashed: &BytesN<32>,
) -> bool {
    let buf = load_buffer(env, subscription_id);
    for entry in buf.entries.iter() {
        if entry == *hashed {
            return true;
        }
    }
    false
}

/// Insert a new idempotency key hash into the ring buffer.
///
/// When the buffer is full the oldest entry (at `cursor`) is silently
/// overwritten.
pub fn push_key(
    env: &Env,
    subscription_id: u32,
    hashed: &BytesN<32>,
) {
    let mut buf = load_buffer(env, subscription_id);
    if buf.entries.len() < IDEM_HISTORY {
        buf.entries.push_back(hashed.clone());
    } else {
        let idx = buf.cursor as usize % IDEM_HISTORY as usize;
        if idx < buf.entries.len() as usize {
            buf.entries.set(idx as u32, hashed.clone());
        } else {
            buf.entries.push_back(hashed.clone());
        }
    }
    buf.cursor = buf.cursor.wrapping_add(1) % IDEM_HISTORY;
    save_buffer(env, subscription_id, &buf);
}
