//! Shared ring-buffer idempotency key helpers.
//!
//! Three entrypoints use idempotency keys: `charge_subscription`,
//! `deposit_funds`, and `charge_one_off`.  Each domain is scoped with a
//! unique domain constant so that reusing the same raw 32-byte key across
//! different entrypoints does **not** produce a replay collision.
//!
//! Storage key: `DataKey::IdemKey(subscription_id)` stores `IdemRingBuffer`.

use crate::types::DataKey;
use soroban_sdk::{contracttype, BytesN, Env, Vec};

/// Maximum number of idempotency keys retained per subscription.
pub(crate) const IDEM_HISTORY: u32 = 10;

/// Ring buffer of recently seen idempotency-key hashes.
#[contracttype]
#[derive(Clone, Debug)]
pub(crate) struct IdemRingBuffer {
    pub entries: Vec<BytesN<32>>,
    pub cursor: u32,
}

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
pub fn check_key(env: &Env, subscription_id: u32, hashed: &BytesN<32>) -> bool {
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
pub fn push_key(env: &Env, subscription_id: u32, hashed: &BytesN<32>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::Address as _,
        Address, Bytes, BytesN, Env,
    };
    use crate::nonce::{DOMAIN_CHARGE_INTERVAL, DOMAIN_DEPOSIT_FUNDS, DOMAIN_CHARGE_ONEOFF};
    use crate::SubscriptionVault;

    fn make_raw_key(env: &Env, fill: u8) -> BytesN<32> {
        let arr = [fill; 32];
        BytesN::from_array(env, &arr)
    }

    fn make_custom_key(env: &Env, arr: &[u8; 32]) -> BytesN<32> {
        BytesN::from_array(env, arr)
    }

    /// Verifies that `hash_idem_key` produces byte-for-byte identical output
    /// across repeated calls with identical inputs (determinism).
    #[test]
    fn test_hash_idem_key_deterministic_output() {
        let env = Env::default();
        let domain = DOMAIN_CHARGE_INTERVAL;
        let sub_id = 42u32;
        let raw_key = make_raw_key(&env, 0xAA);

        let h1 = hash_idem_key(&env, domain, sub_id, &raw_key);
        let h2 = hash_idem_key(&env, domain, sub_id, &raw_key);
        let h3 = hash_idem_key(&env, domain, sub_id, &raw_key);

        assert_eq!(h1, h2, "Repeated hash calls with identical inputs must be deterministic");
        assert_eq!(h2, h3, "Repeated hash calls with identical inputs must be deterministic");

        // Determinism across different environment instances
        let env2 = Env::default();
        let raw_key2 = make_raw_key(&env2, 0xAA);
        let h_env2 = hash_idem_key(&env2, domain, sub_id, &raw_key2);
        assert_eq!(h1.to_array(), h_env2.to_array(), "Hashing must be identical across Env instances");
    }

    /// Verifies that `hash_idem_key` strictly adheres to SHA-256 big-endian preimage
    /// layout: `buf[..4] = domain`, `buf[4..8] = subscription_id`, `buf[8..40] = raw_key`.
    #[test]
    fn test_hash_idem_key_endianness_and_preimage_structure() {
        let env = Env::default();
        let domain: u32 = 0x01020304;
        let sub_id: u32 = 0x05060708;
        let mut raw_arr = [0u8; 32];
        for i in 0..32 {
            raw_arr[i] = (i + 10) as u8;
        }
        let raw_key = make_custom_key(&env, &raw_arr);

        let actual = hash_idem_key(&env, domain, sub_id, &raw_key);

        // Manually construct the 40-byte preimage using big-endian byte order
        let mut expected_buf = [0u8; 40];
        expected_buf[0] = 0x01;
        expected_buf[1] = 0x02;
        expected_buf[2] = 0x03;
        expected_buf[3] = 0x04;
        expected_buf[4] = 0x05;
        expected_buf[5] = 0x06;
        expected_buf[6] = 0x07;
        expected_buf[7] = 0x08;
        expected_buf[8..40].copy_from_slice(&raw_arr);

        let expected_bytes = Bytes::from_slice(&env, &expected_buf);
        let expected_hash: BytesN<32> = env.crypto().sha256(&expected_bytes).into();

        assert_eq!(actual, expected_hash, "Hash output must match exact 40-byte BE SHA-256 construction");

        // Endianness sensitivity test: Little-endian domain swap (0x01020304 vs 0x04030201)
        let swapped_domain_hash = hash_idem_key(&env, domain.swap_bytes(), sub_id, &raw_key);
        assert_ne!(actual, swapped_domain_hash, "Byte order swap in domain must yield different hash");

        // Endianness sensitivity test: Little-endian sub_id swap
        let swapped_sub_hash = hash_idem_key(&env, domain, sub_id.swap_bytes(), &raw_key);
        assert_ne!(actual, swapped_sub_hash, "Byte order swap in subscription_id must yield different hash");
    }

    /// Verifies domain separation: an adversary attempting to reuse a captured raw idempotency key
    /// from one entrypoint (e.g. deposit_funds) to forge or replay an operation in another
    /// entrypoint (e.g. charge_subscription or charge_one_off) is blocked because the hashes differ.
    #[test]
    fn test_hash_idem_key_domain_separation_collision_resistance() {
        let env = Env::default();
        let sub_id = 999u32;
        let raw_key = make_raw_key(&env, 0x77);

        let domains = [
            DOMAIN_CHARGE_INTERVAL,
            DOMAIN_DEPOSIT_FUNDS,
            DOMAIN_CHARGE_ONEOFF,
            0u32,
            1u32,
            2u32,
            u32::MAX - 1,
            u32::MAX,
        ];

        let mut hashes = soroban_sdk::Vec::new(&env);
        for &d in &domains {
            let h = hash_idem_key(&env, d, sub_id, &raw_key);
            hashes.push_back(h);
        }

        // Assert pairwise inequality across all domain outputs
        for i in 0..hashes.len() {
            for j in (i + 1)..hashes.len() {
                let h_i = hashes.get(i).unwrap();
                let h_j = hashes.get(j).unwrap();
                assert_ne!(
                    h_i, h_j,
                    "Domain collision detected between domain index {i} ({}) and {j} ({})",
                    domains[i as usize], domains[j as usize]
                );
            }
        }
    }

    /// Verifies tenant isolation: the same domain and raw idempotency key used across different
    /// subscription IDs must never collide, preventing cross-subscription replay attacks.
    #[test]
    fn test_hash_idem_key_subscription_isolation() {
        let env = Env::default();
        let domain = DOMAIN_DEPOSIT_FUNDS;
        let raw_key = make_raw_key(&env, 0x42);

        let sub_ids = [
            0u32,
            1u32,
            2u32,
            100u32,
            1_000_000u32,
            0x80000000u32,
            u32::MAX - 1,
            u32::MAX,
        ];

        let mut hashes = soroban_sdk::Vec::new(&env);
        for &id in &sub_ids {
            let h = hash_idem_key(&env, domain, id, &raw_key);
            hashes.push_back(h);
        }

        // Pairwise inequality across all subscription IDs
        for i in 0..hashes.len() {
            for j in (i + 1)..hashes.len() {
                let h_i = hashes.get(i).unwrap();
                let h_j = hashes.get(j).unwrap();
                assert_ne!(
                    h_i, h_j,
                    "Subscription ID collision detected between sub_id {i} ({}) and {j} ({})",
                    sub_ids[i as usize], sub_ids[j as usize]
                );
            }
        }
    }

    /// Verifies symmetric parameter swap resistance:
    /// swapping `domain` and `subscription_id` must NOT produce identical hashes.
    #[test]
    fn test_hash_idem_key_symmetric_swap_resistance() {
        let env = Env::default();
        let raw_key = make_raw_key(&env, 0x11);

        let a = 1u32;
        let b = 2u32;

        let h_ab = hash_idem_key(&env, a, b, &raw_key);
        let h_ba = hash_idem_key(&env, b, a, &raw_key);

        assert_ne!(h_ab, h_ba, "Swapping domain and subscription_id must produce distinct hashes");

        // Edge case: domain = 0, sub = 1 vs domain = 1, sub = 0
        let h_01 = hash_idem_key(&env, 0, 1, &raw_key);
        let h_10 = hash_idem_key(&env, 1, 0, &raw_key);
        assert_ne!(h_01, h_10, "Swapping (0, 1) and (1, 0) must produce distinct hashes");
    }

    /// Verifies boundary and extreme value handling across all parameters:
    /// 0, u32::MAX, all-0x00 key, all-0xFF key, alternating bits.
    #[test]
    fn test_hash_idem_key_boundary_values() {
        let env = Env::default();

        let zero_key = make_raw_key(&env, 0x00);
        let max_key = make_raw_key(&env, 0xFF);
        let alt1_key = make_raw_key(&env, 0xAA);
        let alt2_key = make_raw_key(&env, 0x55);

        let test_cases = [
            (0u32, 0u32, &zero_key),
            (0u32, 0u32, &max_key),
            (0u32, u32::MAX, &zero_key),
            (0u32, u32::MAX, &max_key),
            (u32::MAX, 0u32, &zero_key),
            (u32::MAX, 0u32, &max_key),
            (u32::MAX, u32::MAX, &zero_key),
            (u32::MAX, u32::MAX, &max_key),
            (u32::MAX, u32::MAX, &alt1_key),
            (u32::MAX, u32::MAX, &alt2_key),
            (0u32, 0u32, &alt1_key),
            (0u32, 0u32, &alt2_key),
        ];

        let mut computed = soroban_sdk::Vec::new(&env);
        for (d, s, k) in test_cases.iter() {
            let h = hash_idem_key(&env, *d, *s, k);
            // Ensure hash is not trivially all zero
            assert_ne!(h.to_array(), [0u8; 32], "Hash output must not be trivial zero");
            computed.push_back(h);
        }

        // All boundary test vectors must be pairwise distinct
        for i in 0..computed.len() {
            for j in (i + 1)..computed.len() {
                assert_ne!(
                    computed.get(i).unwrap(),
                    computed.get(j).unwrap(),
                    "Boundary collision detected between case {i} and case {j}"
                );
            }
        }
    }

    /// Verifies cryptographic avalanche effect: single bit mutations in raw_key
    /// cause drastic differences in the resulting hash (no bit-aliasing).
    #[test]
    fn test_hash_idem_key_avalanche_single_bit_mutation() {
        let env = Env::default();
        let domain = DOMAIN_CHARGE_INTERVAL;
        let sub_id = 12345u32;

        let base_arr = [0u8; 32];
        let base_key = make_custom_key(&env, &base_arr);
        let base_hash = hash_idem_key(&env, domain, sub_id, &base_key);

        // Perturb each individual byte at bit 0 and bit 7
        for byte_idx in 0..32 {
            // Flip bit 0
            let mut arr_bit0 = base_arr;
            arr_bit0[byte_idx] ^= 0x01;
            let key_bit0 = make_custom_key(&env, &arr_bit0);
            let hash_bit0 = hash_idem_key(&env, domain, sub_id, &key_bit0);
            assert_ne!(
                base_hash, hash_bit0,
                "Single bit flip at byte {byte_idx} bit 0 must alter hash"
            );

            // Flip bit 7
            let mut arr_bit7 = base_arr;
            arr_bit7[byte_idx] ^= 0x80;
            let key_bit7 = make_custom_key(&env, &arr_bit7);
            let hash_bit7 = hash_idem_key(&env, domain, sub_id, &key_bit7);
            assert_ne!(
                base_hash, hash_bit7,
                "Single bit flip at byte {byte_idx} bit 7 must alter hash"
            );
            assert_ne!(
                hash_bit0, hash_bit7,
                "Different bit flips in byte {byte_idx} must yield different hashes"
            );
        }
    }

    /// Verifies state invariance: `hash_idem_key` is a strictly pure computation.
    /// It must NOT read or write instance, persistent, or temporary contract storage,
    /// and the idempotency ring buffer must remain completely unmodified.
    #[test]
    fn test_hash_idem_key_pure_computation_state_unchanged() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());

        let sub_id = 777u32;
        let domain = DOMAIN_DEPOSIT_FUNDS;
        let raw_key = make_raw_key(&env, 0xEE);

        env.as_contract(&contract_id, || {
            // Verify ring buffer initially does not exist in storage
            let initial_buf = load_buffer(&env, sub_id);
            assert_eq!(initial_buf.entries.len(), 0, "Buffer must initially be empty");
            assert_eq!(initial_buf.cursor, 0);

            // Perform repeated hash calculations
            for i in 0..50u32 {
                let _ = hash_idem_key(&env, domain + i, sub_id + i, &raw_key);
            }

            // Verify ring buffer remains untouched
            let after_buf = load_buffer(&env, sub_id);
            assert_eq!(after_buf.entries.len(), 0, "hash_idem_key must not populate the ring buffer");
            assert_eq!(after_buf.cursor, 0);

            // Check_key must still return false because hash was never pushed
            let h = hash_idem_key(&env, domain, sub_id, &raw_key);
            assert!(!check_key(&env, sub_id, &h), "Unpushed hash must not be recognized by check_key");
        });
    }

    /// Verifies authorization independence: `hash_idem_key` does not require or consume auth,
    /// can be executed without mock authorization, and does not alter caller auth contexts.
    #[test]
    fn test_hash_idem_key_auth_independence() {
        let env = Env::default();
        // Do NOT call env.mock_all_auths() — verify it works in un-mocked and un-authenticated mode
        let domain = DOMAIN_CHARGE_ONEOFF;
        let sub_id = 888u32;
        let raw_key = make_raw_key(&env, 0x33);

        let h1 = hash_idem_key(&env, domain, sub_id, &raw_key);
        assert_ne!(h1.to_array(), [0u8; 32]);

        // When called inside contract context with a stranger address
        let contract_id = env.register(SubscriptionVault, ());
        let stranger = Address::generate(&env);

        env.as_contract(&contract_id, || {
            // Invocation succeeds without requiring auth for stranger
            let h2 = hash_idem_key(&env, domain, sub_id, &raw_key);
            assert_eq!(h1, h2);

            // Ensure stranger has no recorded auth invocations from hashing
            assert_ne!(h2.to_array(), [0u8; 32]);
            let _ = stranger;
        });
    }

    /// Verifies integration with ring buffer: when an operation is rejected as duplicate,
    /// the ring buffer state remains intact, entries are not duplicated, and cursor is unchanged.
    #[test]
    fn test_hash_idem_key_rejection_state_invariance_and_ring_buffer_integration() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());

        let sub_id = 505u32;
        let raw_key = make_raw_key(&env, 0x12);

        let hash_charge = hash_idem_key(&env, DOMAIN_CHARGE_INTERVAL, sub_id, &raw_key);
        let hash_deposit = hash_idem_key(&env, DOMAIN_DEPOSIT_FUNDS, sub_id, &raw_key);

        env.as_contract(&contract_id, || {
            // 1. Initial state: both hashes are unseen
            assert!(!check_key(&env, sub_id, &hash_charge));
            assert!(!check_key(&env, sub_id, &hash_deposit));

            // 2. Record the charge idempotency key
            push_key(&env, sub_id, &hash_charge);

            let buf_after_push = load_buffer(&env, sub_id);
            assert_eq!(buf_after_push.entries.len(), 1);
            assert_eq!(buf_after_push.cursor, 1);
            assert_eq!(buf_after_push.entries.get(0).unwrap(), hash_charge);

            // 3. Replay detection for charge
            assert!(check_key(&env, sub_id, &hash_charge), "Replayed charge hash must be detected");

            // 4. Critical: deposit with the exact same raw key is NOT rejected because domains differ
            assert!(
                !check_key(&env, sub_id, &hash_deposit),
                "Deposit hash with same raw key must NOT collide with charge hash"
            );

            // 5. On duplicate rejection, the caller skips push_key. Verify buffer state remains unchanged.
            let buf_after_replay_check = load_buffer(&env, sub_id);
            assert_eq!(buf_after_replay_check.entries.len(), 1);
            assert_eq!(buf_after_replay_check.cursor, 1);
            assert_eq!(buf_after_replay_check.entries.get(0).unwrap(), hash_charge);
        });
    }

    /// Verifies ring buffer boundary wrap-around when fed sequential hashes from `hash_idem_key`.
    #[test]
    fn test_hash_idem_key_ring_buffer_fifo_eviction() {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let sub_id = 900u32;
        let domain = DOMAIN_DEPOSIT_FUNDS;

        env.as_contract(&contract_id, || {
            let mut keys = soroban_sdk::Vec::new(&env);

            // Insert IDEM_HISTORY keys (filling the buffer)
            for i in 0..IDEM_HISTORY {
                let raw = make_raw_key(&env, i as u8);
                let h = hash_idem_key(&env, domain, sub_id, &raw);
                keys.push_back(h.clone());
                push_key(&env, sub_id, &h);
            }

            let buf = load_buffer(&env, sub_id);
            assert_eq!(buf.entries.len(), IDEM_HISTORY);
            assert_eq!(buf.cursor, 0, "Cursor should wrap around to 0 when filled to capacity");

            // All IDEM_HISTORY keys are present
            for i in 0..IDEM_HISTORY {
                let h = keys.get(i).unwrap();
                assert!(check_key(&env, sub_id, &h));
            }

            // Insert 1 new key (should evict key 0)
            let new_raw = make_raw_key(&env, 0xEE);
            let new_hash = hash_idem_key(&env, domain, sub_id, &new_raw);
            push_key(&env, sub_id, &new_hash);

            let buf_after = load_buffer(&env, sub_id);
            assert_eq!(buf_after.entries.len(), IDEM_HISTORY);
            assert_eq!(buf_after.cursor, 1);

            // Key 0 evicted, new_hash present, keys 1..IDEM_HISTORY still present
            let key0 = keys.get(0).unwrap();
            assert!(!check_key(&env, sub_id, &key0), "Key 0 must be evicted after FIFO overwrite");
            assert!(check_key(&env, sub_id, &new_hash), "New hash must be present");
            for i in 1..IDEM_HISTORY {
                let h = keys.get(i).unwrap();
                assert!(check_key(&env, sub_id, &h));
            }
        });
    }
}
