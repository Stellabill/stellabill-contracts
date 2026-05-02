#![cfg(test)]
extern crate std;

// Migration fixture test suite.
//
// Verifies that the migration export hooks correctly snapshot all contract
// state — balances, roles, statuses, lifetime accounting — without mutating
// anything, inflating balances, or escalating roles.
//
// Coverage targets:
// - export_contract_snapshot
// - export_subscription_summary
// - export_subscription_summaries (pagination, sparse IDs, partial batches)
// - Security invariants (read-only, admin-only, no balance mutation)
// - All subscription statuses are preserved faithfully in exports
// - Lifetime cap accounting is preserved
// - Expiring subscriptions are exported with correct `expires_at`

use super::*;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{token, Address, Env};

const T0: u64 = 1_000_000;
const INTERVAL: u64 = 60;
const AMOUNT: i128 = 5_000_000; // 5 USDC — above min_topup (1 USDC)
const MIN_TOPUP: i128 = 1_000_000;
const GRACE_PERIOD: u64 = 7 * 24 * 60 * 60;

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn patch_status_migration(te: &MigrationTestEnv, id: u32, status: SubscriptionStatus) {
    let mut sub = te.client.get_subscription(&id);
    sub.status = status;
    te.env.as_contract(&te.client.address, || {
        te.env.storage().instance().set(&id, &sub);
    });
}

// ─── Setup helpers ────────────────────────────────────────────────────────────

struct MigrationTestEnv {
    env: Env,
    client: SubscriptionVaultClient<'static>,
    token: token::Client<'static>,
    token_admin: token::StellarAssetClient<'static>,
    admin: Address,
}

impl MigrationTestEnv {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|l| l.timestamp = T0);

        let admin = Address::generate(&env);
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let token_admin_addr = Address::generate(&env);
        let token_id = env.register_stellar_asset_contract_v2(token_admin_addr.clone());
        let token = token::Client::new(&env, &token_id.address());
        let token_admin = token::StellarAssetClient::new(&env, &token_id.address());

        client.init(&token_id.address(), &6, &admin, &MIN_TOPUP, &GRACE_PERIOD);

        Self { env, client, token, token_admin, admin }
    }

    fn token_address(&self) -> Address {
        self.token.address.clone()
    }

    /// Mint `amount` to `address` from token admin.
    fn mint(&self, address: &Address, amount: i128) {
        self.token_admin.mint(address, &amount);
    }

    /// Create a subscription, deposit funds, and return its ID.
    fn make_subscription(
        &self,
        subscriber: &Address,
        merchant: &Address,
        deposit: i128,
    ) -> u32 {
        let id = self.client.create_subscription(
            subscriber,
            merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
        );
        self.mint(subscriber, deposit);
        self.client.deposit_funds(&id, subscriber, &deposit);
        id
    }

    /// Create a subscription with a lifetime cap.
    fn make_subscription_with_cap(
        &self,
        subscriber: &Address,
        merchant: &Address,
        cap: i128,
        deposit: i128,
    ) -> u32 {
        let id = self.client.create_subscription(
            subscriber,
            merchant,
            &AMOUNT,
            &INTERVAL,
            &false,
            &Some(cap),
            &None::<u64>,
        );
        self.mint(subscriber, deposit);
        self.client.deposit_funds(&id, subscriber, &deposit);
        id
    }

    /// Create a subscription that expires at `expires_at`.
    fn make_expiring_subscription(
        &self,
        subscriber: &Address,
        merchant: &Address,
        expires_at: u64,
        deposit: i128,
    ) -> u32 {
        let id = self.client.create_subscription_with_token(
            subscriber,
            merchant,
            &self.token_address(),
            &AMOUNT,
            &INTERVAL,
            &false,
            &None::<i128>,
            &Some(expires_at),
        );
        self.mint(subscriber, deposit);
        self.client.deposit_funds(&id, subscriber, &deposit);
        id
    }

    fn set_timestamp(&self, ts: u64) {
        self.env.ledger().with_mut(|l| l.timestamp = ts);
    }

    fn jump(&self, delta: u64) {
        let ts = self.env.ledger().timestamp();
        self.set_timestamp(ts + delta);
    }
}

// ─── Contract snapshot ────────────────────────────────────────────────────────

#[test]
fn test_migration_snapshot_captures_all_config_fields() {
    let te = MigrationTestEnv::new();
    let snapshot = te.client.export_contract_snapshot(&te.admin);

    assert_eq!(snapshot.admin, te.admin, "admin address preserved");
    assert_eq!(snapshot.token, te.token_address(), "token address preserved");
    assert_eq!(snapshot.min_topup, MIN_TOPUP, "min_topup preserved");
    assert_eq!(snapshot.next_id, 0, "no subscriptions yet");
    assert_eq!(snapshot.storage_version, 2, "storage version is 2");
    assert_eq!(snapshot.timestamp, T0, "timestamp matches ledger");
}

#[test]
fn test_migration_snapshot_next_id_increments_with_subscriptions() {
    let te = MigrationTestEnv::new();
    let sub1 = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    te.make_subscription(&sub1, &merchant, 5_000_000);
    te.make_subscription(&Address::generate(&te.env), &merchant, 5_000_000);

    let snapshot = te.client.export_contract_snapshot(&te.admin);
    assert_eq!(snapshot.next_id, 2, "next_id reflects two created subscriptions");
}

#[test]
fn test_migration_snapshot_does_not_mutate_state() {
    let te = MigrationTestEnv::new();
    let sub = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let id = te.make_subscription(&sub, &merchant, 5_000_000);

    let before = te.client.get_subscription(&id);

    // Export snapshot twice — state must remain identical
    let _ = te.client.export_contract_snapshot(&te.admin);
    let _ = te.client.export_contract_snapshot(&te.admin);

    let after = te.client.get_subscription(&id);
    assert_eq!(before.prepaid_balance, after.prepaid_balance, "balance unchanged");
    assert_eq!(before.lifetime_charged, after.lifetime_charged, "charged unchanged");
    assert_eq!(before.status, after.status, "status unchanged");
}

#[test]
fn test_migration_snapshot_requires_admin() {
    let te = MigrationTestEnv::new();
    let stranger = Address::generate(&te.env);
    let result = te.client.try_export_contract_snapshot(&stranger);
    assert!(result.is_err(), "stranger must not export snapshot");
}

// ─── Single subscription summary ─────────────────────────────────────────────

#[test]
fn test_migration_single_summary_preserves_all_fields() {
    let te = MigrationTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let deposit = 8_000_000i128;
    let id = te.make_subscription(&subscriber, &merchant, deposit);

    let summary = te.client.export_subscription_summary(&te.admin, &id);

    assert_eq!(summary.subscription_id, id);
    assert_eq!(summary.subscriber, subscriber, "subscriber preserved");
    assert_eq!(summary.merchant, merchant, "merchant preserved");
    assert_eq!(summary.token, te.token_address(), "token preserved");
    assert_eq!(summary.amount, AMOUNT, "amount preserved");
    assert_eq!(summary.interval_seconds, INTERVAL, "interval preserved");
    assert_eq!(summary.prepaid_balance, deposit, "full deposit in balance");
    assert_eq!(summary.lifetime_charged, 0, "no charges yet");
    assert_eq!(summary.usage_enabled, false);
    assert_eq!(summary.lifetime_cap, None);
    assert_eq!(summary.expires_at, None);
    assert_eq!(summary.status, SubscriptionStatus::Active);
}

#[test]
fn test_migration_single_summary_preserves_lifetime_cap_and_charged() {
    let te = MigrationTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let cap = 20_000_000i128;
    let deposit = 15_000_000i128;
    let id = te.make_subscription_with_cap(&subscriber, &merchant, cap, deposit);

    // Trigger one charge so lifetime_charged > 0
    // The deposit already transferred funds to the contract; no extra mint needed
    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&id);

    let summary = te.client.export_subscription_summary(&te.admin, &id);

    assert_eq!(summary.lifetime_cap, Some(cap), "lifetime_cap preserved");
    assert_eq!(summary.lifetime_charged, AMOUNT, "one charge recorded");
    // Balance decremented by one charge
    assert_eq!(
        summary.prepaid_balance,
        deposit - AMOUNT,
        "balance reduced by one charge"
    );
}

#[test]
fn test_migration_single_summary_not_found_returns_error() {
    let te = MigrationTestEnv::new();
    let result = te.client.try_export_subscription_summary(&te.admin, &9999);
    assert!(
        matches!(result, Err(Ok(Error::NotFound))),
        "missing subscription returns NotFound"
    );
}

#[test]
fn test_migration_single_summary_requires_admin() {
    let te = MigrationTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let id = te.make_subscription(&subscriber, &merchant, 5_000_000);
    let stranger = Address::generate(&te.env);

    let result = te.client.try_export_subscription_summary(&stranger, &id);
    assert!(result.is_err(), "stranger must not export summary");
}

// ─── Paginated export — basic correctness ─────────────────────────────────────

#[test]
fn test_migration_paginated_export_all_subscriptions() {
    let te = MigrationTestEnv::new();
    let merchant = Address::generate(&te.env);
    let id0 = te.make_subscription(&Address::generate(&te.env), &merchant, 5_000_000);
    let id1 = te.make_subscription(&Address::generate(&te.env), &merchant, 5_000_000);
    let id2 = te.make_subscription(&Address::generate(&te.env), &merchant, 5_000_000);

    let summaries = te.client.export_subscription_summaries(&te.admin, &0, &10);
    assert_eq!(summaries.len(), 3, "all three subscriptions exported");
    assert_eq!(summaries.get(0).unwrap().subscription_id, id0);
    assert_eq!(summaries.get(1).unwrap().subscription_id, id1);
    assert_eq!(summaries.get(2).unwrap().subscription_id, id2);
}

#[test]
fn test_migration_paginated_export_respects_limit() {
    let te = MigrationTestEnv::new();
    let merchant = Address::generate(&te.env);
    for _ in 0..5 {
        te.make_subscription(&Address::generate(&te.env), &merchant, 5_000_000);
    }

    let page = te.client.export_subscription_summaries(&te.admin, &0, &3);
    assert_eq!(page.len(), 3, "limit=3 returns exactly 3 records");
    assert_eq!(page.get(0).unwrap().subscription_id, 0);
    assert_eq!(page.get(2).unwrap().subscription_id, 2);
}

#[test]
fn test_migration_paginated_export_cursor_resumes_correctly() {
    let te = MigrationTestEnv::new();
    let merchant = Address::generate(&te.env);
    for _ in 0..6 {
        te.make_subscription(&Address::generate(&te.env), &merchant, 5_000_000);
    }

    let page1 = te.client.export_subscription_summaries(&te.admin, &0, &3);
    let page2 = te.client.export_subscription_summaries(&te.admin, &3, &3);
    assert_eq!(page1.len(), 3);
    assert_eq!(page2.len(), 3);
    // Pages are disjoint
    assert_ne!(
        page1.get(0).unwrap().subscription_id,
        page2.get(0).unwrap().subscription_id
    );
    // Together they cover all 6
    let last_of_p1 = page1.get(2).unwrap().subscription_id;
    let first_of_p2 = page2.get(0).unwrap().subscription_id;
    assert_eq!(last_of_p1 + 1, first_of_p2, "pages are contiguous");
}

#[test]
fn test_migration_paginated_export_empty_when_no_subscriptions() {
    let te = MigrationTestEnv::new();
    let summaries = te.client.export_subscription_summaries(&te.admin, &0, &10);
    assert_eq!(summaries.len(), 0, "empty vault returns empty export");
}

#[test]
fn test_migration_paginated_export_start_beyond_range_returns_empty() {
    let te = MigrationTestEnv::new();
    let merchant = Address::generate(&te.env);
    te.make_subscription(&Address::generate(&te.env), &merchant, 5_000_000);

    let summaries = te.client.export_subscription_summaries(&te.admin, &100, &10);
    assert_eq!(summaries.len(), 0, "start beyond next_id returns empty");
}

#[test]
fn test_migration_paginated_export_limit_zero_returns_empty() {
    let te = MigrationTestEnv::new();
    let merchant = Address::generate(&te.env);
    te.make_subscription(&Address::generate(&te.env), &merchant, 5_000_000);

    let summaries = te.client.export_subscription_summaries(&te.admin, &0, &0);
    assert_eq!(summaries.len(), 0, "limit=0 returns empty");
}

#[test]
fn test_migration_paginated_export_limit_exceeds_max_returns_error() {
    let te = MigrationTestEnv::new();
    let result = te.client.try_export_subscription_summaries(&te.admin, &0, &101);
    assert_eq!(
        result,
        Err(Ok(Error::InvalidExportLimit)),
        "limit > 100 returns InvalidExportLimit"
    );
}

#[test]
fn test_migration_paginated_export_requires_admin() {
    let te = MigrationTestEnv::new();
    let stranger = Address::generate(&te.env);
    let result = te.client.try_export_subscription_summaries(&stranger, &0, &10);
    assert!(result.is_err(), "stranger must not export summaries");
}

// ─── Status preservation ──────────────────────────────────────────────────────

#[test]
fn test_migration_preserves_active_status() {
    let te = MigrationTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let id = te.make_subscription(&subscriber, &merchant, 5_000_000);

    let summary = te.client.export_subscription_summary(&te.admin, &id);
    assert_eq!(summary.status, SubscriptionStatus::Active);
}

#[test]
fn test_migration_preserves_paused_status() {
    let te = MigrationTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let id = te.make_subscription(&subscriber, &merchant, 5_000_000);
    te.client.pause_subscription(&id, &subscriber);

    let summary = te.client.export_subscription_summary(&te.admin, &id);
    assert_eq!(summary.status, SubscriptionStatus::Paused);
}

#[test]
fn test_migration_preserves_cancelled_status() {
    let te = MigrationTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let id = te.make_subscription(&subscriber, &merchant, 5_000_000);
    te.client.cancel_subscription(&id, &subscriber);

    let summary = te.client.export_subscription_summary(&te.admin, &id);
    assert_eq!(summary.status, SubscriptionStatus::Cancelled);
}

#[test]
fn test_migration_preserves_insufficient_balance_status() {
    let te = MigrationTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    let id = te.client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );

    // Directly patch to InsufficientBalance (error-returning charge calls roll back in test env)
    patch_status_migration(&te, id, SubscriptionStatus::InsufficientBalance);

    let summary = te.client.export_subscription_summary(&te.admin, &id);
    assert_eq!(summary.status, SubscriptionStatus::InsufficientBalance);
}

#[test]
fn test_migration_preserves_expired_status() {
    let te = MigrationTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let expires_at = T0 + 2 * INTERVAL;
    let id = te.make_expiring_subscription(&subscriber, &merchant, expires_at, 5_000_000);

    // Directly patch to Expired (error-returning calls roll back state in the test env)
    patch_status_migration(&te, id, SubscriptionStatus::Expired);

    let summary = te.client.export_subscription_summary(&te.admin, &id);
    assert_eq!(summary.status, SubscriptionStatus::Expired);
    assert_eq!(summary.expires_at, Some(expires_at), "expires_at preserved");
}

// ─── Mixed-status batch export ─────────────────────────────────────────────

#[test]
fn test_migration_batch_export_preserves_mixed_statuses() {
    let te = MigrationTestEnv::new();
    let merchant = Address::generate(&te.env);

    let sub_active = Address::generate(&te.env);
    let sub_paused = Address::generate(&te.env);
    let sub_cancelled = Address::generate(&te.env);

    let id_active = te.make_subscription(&sub_active, &merchant, 5_000_000);
    let id_paused = te.make_subscription(&sub_paused, &merchant, 5_000_000);
    let id_cancelled = te.make_subscription(&sub_cancelled, &merchant, 5_000_000);

    te.client.pause_subscription(&id_paused, &sub_paused);
    te.client.cancel_subscription(&id_cancelled, &sub_cancelled);

    let summaries = te.client.export_subscription_summaries(&te.admin, &0, &10);
    assert_eq!(summaries.len(), 3);

    let find = |target_id: u32| -> SubscriptionStatus {
        for i in 0..summaries.len() {
            let s = summaries.get(i).unwrap();
            if s.subscription_id == target_id {
                return s.status;
            }
        }
        panic!("subscription {} not found in export", target_id);
    };

    assert_eq!(find(id_active), SubscriptionStatus::Active);
    assert_eq!(find(id_paused), SubscriptionStatus::Paused);
    assert_eq!(find(id_cancelled), SubscriptionStatus::Cancelled);
}

// ─── Balance accounting invariants ────────────────────────────────────────────

#[test]
fn test_migration_export_does_not_inflate_balances() {
    let te = MigrationTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let deposit = 10_000_000i128;
    let id = te.make_subscription(&subscriber, &merchant, deposit);

    // Run a charge so lifetime_charged > 0
    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&id);

    let sub_before = te.client.get_subscription(&id);

    // Export multiple times
    for _ in 0..3 {
        let _ = te.client.export_subscription_summary(&te.admin, &id);
        let _ = te.client.export_subscription_summaries(&te.admin, &0, &10);
    }

    let sub_after = te.client.get_subscription(&id);
    assert_eq!(sub_before.prepaid_balance, sub_after.prepaid_balance, "balance not mutated");
    assert_eq!(sub_before.lifetime_charged, sub_after.lifetime_charged, "charged not mutated");
}

#[test]
fn test_migration_summary_balance_matches_subscription_record() {
    let te = MigrationTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let deposit = 12_000_000i128;
    let id = te.make_subscription(&subscriber, &merchant, deposit);

    // Charge twice
    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&id);
    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&id);

    let sub = te.client.get_subscription(&id);
    let summary = te.client.export_subscription_summary(&te.admin, &id);

    assert_eq!(summary.prepaid_balance, sub.prepaid_balance, "balance fields match");
    assert_eq!(summary.lifetime_charged, sub.lifetime_charged, "charged fields match");
    assert_eq!(summary.last_payment_timestamp, sub.last_payment_timestamp, "timestamp fields match");
}

// ─── Security: no role escalation via export ──────────────────────────────────

#[test]
fn test_migration_export_does_not_change_admin() {
    let te = MigrationTestEnv::new();
    let stored_admin_before = te.client.export_contract_snapshot(&te.admin).admin;

    let _ = te.client.export_contract_snapshot(&te.admin);
    let _ = te.client.export_subscription_summaries(&te.admin, &0, &10);

    let stored_admin_after = te.client.export_contract_snapshot(&te.admin).admin;
    assert_eq!(stored_admin_before, stored_admin_after, "admin is immutable after exports");
}

// ─── Lifetime cap accounting ──────────────────────────────────────────────────

#[test]
fn test_migration_lifetime_cap_fully_exhausted_shows_cancelled() {
    let te = MigrationTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    // Cap = exactly 1 charge worth
    let cap = AMOUNT;
    let deposit = 10_000_000i128;
    let id = te.make_subscription_with_cap(&subscriber, &merchant, cap, deposit);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&id);

    let summary = te.client.export_subscription_summary(&te.admin, &id);
    assert_eq!(summary.lifetime_cap, Some(cap));
    assert_eq!(summary.lifetime_charged, AMOUNT);
    assert_eq!(
        summary.status,
        SubscriptionStatus::Cancelled,
        "sub auto-cancelled when cap exhausted"
    );
}

#[test]
fn test_migration_lifetime_cap_partially_charged_preserved() {
    let te = MigrationTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    let cap = 3 * AMOUNT;
    let deposit = 10_000_000i128;
    let id = te.make_subscription_with_cap(&subscriber, &merchant, cap, deposit);

    te.jump(INTERVAL + 1);
    te.client.charge_subscription(&id);

    let summary = te.client.export_subscription_summary(&te.admin, &id);
    assert_eq!(summary.lifetime_cap, Some(cap));
    assert_eq!(summary.lifetime_charged, AMOUNT, "one charge tracked");
    assert_eq!(summary.status, SubscriptionStatus::Active, "still active");
}

// ─── Expiration fields ────────────────────────────────────────────────────────

#[test]
fn test_migration_active_expiring_subscription_preserves_expires_at() {
    let te = MigrationTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let expires_at = T0 + 10 * INTERVAL;

    let id = te.make_expiring_subscription(&subscriber, &merchant, expires_at, 10_000_000);

    let summary = te.client.export_subscription_summary(&te.admin, &id);
    assert_eq!(summary.expires_at, Some(expires_at));
    assert_eq!(summary.status, SubscriptionStatus::Active, "not yet expired");
}

// ─── Partial migration simulation ────────────────────────────────────────────

#[test]
fn test_migration_full_walk_covers_all_subscriptions() {
    let te = MigrationTestEnv::new();
    let merchant = Address::generate(&te.env);
    let total = 7usize;
    for _ in 0..total {
        te.make_subscription(&Address::generate(&te.env), &merchant, 5_000_000);
    }

    let snapshot = te.client.export_contract_snapshot(&te.admin);
    let next_id = snapshot.next_id;

    // Walk in pages of 3, collecting all IDs seen
    let page_size = 3u32;
    let mut cursor = 0u32;
    let mut seen = std::vec::Vec::new();

    loop {
        if cursor >= next_id {
            break;
        }
        let remaining = next_id - cursor;
        let batch_limit = remaining.min(page_size);
        let page = te
            .client
            .export_subscription_summaries(&te.admin, &cursor, &batch_limit);
        for i in 0..page.len() {
            seen.push(page.get(i).unwrap().subscription_id);
        }
        cursor += batch_limit;
    }

    assert_eq!(seen.len(), total, "full walk covers all {} subscriptions", total);
    // IDs should be 0..total
    for expected_id in 0u32..(total as u32) {
        assert!(
            seen.contains(&expected_id),
            "subscription {} present in full walk",
            expected_id
        );
    }
}

#[test]
fn test_migration_full_walk_balances_sum_matches_individual_queries() {
    let te = MigrationTestEnv::new();
    let merchant = Address::generate(&te.env);
    let deposits = [5_000_000i128, 8_000_000, 3_000_000, 6_000_000];

    for deposit in deposits {
        let sub = Address::generate(&te.env);
        te.make_subscription(&sub, &merchant, deposit);
    }

    // Sum balances via paginated export
    let summaries = te.client.export_subscription_summaries(&te.admin, &0, &10);
    let exported_total: i128 = (0..summaries.len())
        .map(|i| summaries.get(i as u32).unwrap().prepaid_balance)
        .sum();

    // Sum balances via individual subscription queries
    let direct_total: i128 = (0u32..4)
        .map(|id| te.client.get_subscription(&id).prepaid_balance)
        .sum();

    assert_eq!(
        exported_total, direct_total,
        "exported balance sum equals sum of direct queries"
    );
}

// ─── Emergency stop does not block exports ───────────────────────────────────

#[test]
fn test_migration_exports_work_during_emergency_stop() {
    let te = MigrationTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let id = te.make_subscription(&subscriber, &merchant, 5_000_000);

    te.client.enable_emergency_stop(&te.admin);

    // Export snapshot should still succeed (read-only, not blocked by emergency stop)
    let snapshot = te.client.export_contract_snapshot(&te.admin);
    assert_eq!(snapshot.next_id, 1);

    // Export single summary should succeed
    let summary = te.client.export_subscription_summary(&te.admin, &id);
    assert_eq!(summary.status, SubscriptionStatus::Active);

    // Paginated export should succeed
    let summaries = te.client.export_subscription_summaries(&te.admin, &0, &10);
    assert_eq!(summaries.len(), 1);
}
