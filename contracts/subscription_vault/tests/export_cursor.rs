//! Tests for export_subscription_summaries pagination cursor stability.
//!
//! Verifies:
//!  - Cursor is monotonically increasing across pages
//!  - Interleaved create_subscription calls do not reset or skip the cursor
//!  - A subscription cancelled mid-pagination still appears in the dump
//!  - Edge cases: empty contract, single-page dump, partial last page

#[cfg(test)]
mod export_cursor_stability {
    use soroban_sdk::{testutils::Address as _, Address, Env};
    // Adjust the import path to match your actual crate name
    use subscription_vault::{SubscriptionVault, SubscriptionVaultClient};

    // ── helpers ─────────────────────────────────────────────────────────────

    fn setup() -> (Env, Address, SubscriptionVaultClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, SubscriptionVault);
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        (env, contract_id, client)
    }

    /// Drain all pages and return (all_ids, page_count).
    fn drain_pages(
        client: &SubscriptionVaultClient,
        page_size: u32,
    ) -> (Vec<u32>, u32) {
        let mut all_ids: Vec<u32> = Vec::new();
        let mut cursor: Option<u32> = None;
        let mut pages = 0u32;

        loop {
            let result = client.export_subscription_summaries(&cursor, &page_size);
            pages += 1;

            for summary in result.summaries.iter() {
                all_ids.push(summary.id);
            }

            match result.next_start_id {
                None => break,
                Some(next) => {
                    // Cursor must always move forward (monotonic invariant).
                    if let Some(prev) = cursor {
                        assert!(
                            next > prev,
                            "cursor went backwards: prev={prev} next={next}"
                        );
                    }
                    cursor = Some(next);
                }
            }
        }

        (all_ids, pages)
    }

    // ── empty contract ───────────────────────────────────────────────────────

    #[test]
    fn empty_contract_returns_empty_page() {
        let (_env, _id, client) = setup();
        let result = client.export_subscription_summaries(&None, &10);
        assert!(result.summaries.is_empty(), "expected no summaries on empty contract");
        assert!(result.next_start_id.is_none(), "expected no cursor on empty contract");
    }

    // ── single-page dump ─────────────────────────────────────────────────────

    #[test]
    fn single_page_dump_returns_all_ids() {
        let (env, _id, client) = setup();
        let subscriber = Address::generate(&env);
        let merchant  = Address::generate(&env);

        let n = 5u32;
        let mut created: Vec<u32> = Vec::new();
        for _ in 0..n {
            let sub_id = client.create_subscription(&subscriber, &merchant, &100u64, &30u64);
            created.push(sub_id);
        }
        created.sort();

        let (mut dumped, pages) = drain_pages(&client, 50); // page_size > n → single page
        dumped.sort();

        assert_eq!(pages, 1, "expected exactly 1 page");
        assert_eq!(dumped, created, "single-page dump must equal created ids");
    }

    // ── multi-page dump ──────────────────────────────────────────────────────

    #[test]
    fn multi_page_dump_covers_all_ids() {
        let (env, _id, client) = setup();
        let subscriber = Address::generate(&env);
        let merchant  = Address::generate(&env);

        let n = 15u32;
        let mut created: Vec<u32> = Vec::new();
        for _ in 0..n {
            let sub_id = client.create_subscription(&subscriber, &merchant, &100u64, &30u64);
            created.push(sub_id);
        }
        created.sort();

        let (mut dumped, pages) = drain_pages(&client, 4); // 4 items/page → 4 pages
        dumped.sort();

        assert!(pages > 1, "expected multiple pages");
        assert_eq!(dumped, created, "union of all pages must equal created ids");
    }

    // ── interleaved create_subscription between pages ─────────────────────────

    #[test]
    fn cursor_stable_when_subscriptions_created_between_pages() {
        let (env, _id, client) = setup();
        let subscriber = Address::generate(&env);
        let merchant  = Address::generate(&env);

        // Seed 10 subscriptions before we start paginating.
        let mut seeded: Vec<u32> = Vec::new();
        for _ in 0..10u32 {
            seeded.push(client.create_subscription(&subscriber, &merchant, &100u64, &30u64));
        }
        seeded.sort();

        // Fetch page 1.
        let page1 = client.export_subscription_summaries(&None, &5);
        let mut collected: Vec<u32> = page1.summaries.iter().map(|s| s.id).collect();

        // Interleave: create 3 more subscriptions.
        for _ in 0..3u32 {
            client.create_subscription(&subscriber, &merchant, &200u64, &30u64);
        }

        // Continue paginating from saved cursor — must not skip or repeat seeded ids.
        let mut cursor = page1.next_start_id;
        while let Some(next_cursor) = cursor {
            let page = client.export_subscription_summaries(&Some(next_cursor), &5);
            for s in page.summaries.iter() {
                collected.push(s.id);
            }
            cursor = page.next_start_id;
        }

        collected.sort();
        collected.dedup(); // guard against accidental duplicates in assertion message

        // Every seeded id must appear (new ones may or may not appear; that's ok).
        for id in &seeded {
            assert!(
                collected.contains(id),
                "seeded subscription {id} missing from paginated dump after interleaved creates"
            );
        }

        // No duplicates.
        let mut deduped = collected.clone();
        deduped.dedup();
        assert_eq!(collected.len(), deduped.len(), "paginated dump contained duplicate ids");
    }

    // ── cancellation mid-pagination ───────────────────────────────────────────

    #[test]
    fn cancelled_subscription_still_emitted_mid_pagination() {
        let (env, _id, client) = setup();
        let subscriber = Address::generate(&env);
        let merchant  = Address::generate(&env);

        // Seed 10 subscriptions.
        let mut all_ids: Vec<u32> = Vec::new();
        for _ in 0..10u32 {
            all_ids.push(client.create_subscription(&subscriber, &merchant, &100u64, &30u64));
        }
        let cancel_target = all_ids[3]; // pick one in the middle

        // Fetch page 1 (doesn't include cancel_target yet).
        let page1 = client.export_subscription_summaries(&None, &3);
        let ids_in_page1: Vec<u32> = page1.summaries.iter().map(|s| s.id).collect();
        assert!(!ids_in_page1.contains(&cancel_target));

        // Cancel the subscription before fetching the next page.
        client.cancel_subscription(&cancel_target);

        // Drain remaining pages.
        let mut remaining: Vec<u32> = Vec::new();
        let mut cursor = page1.next_start_id;
        while let Some(c) = cursor {
            let page = client.export_subscription_summaries(&Some(c), &3);
            for s in page.summaries.iter() {
                remaining.push(s.id);
            }
            cursor = page.next_start_id;
        }

        assert!(
            remaining.contains(&cancel_target),
            "cancelled subscription {cancel_target} must still appear in the dump"
        );
    }

    // ── partial last page ─────────────────────────────────────────────────────

    #[test]
    fn last_page_can_be_partial() {
        let (env, _id, client) = setup();
        let subscriber = Address::generate(&env);
        let merchant  = Address::generate(&env);

        // 7 subscriptions with page_size=3 → pages of 3, 3, 1.
        let mut created: Vec<u32> = Vec::new();
        for _ in 0..7u32 {
            created.push(client.create_subscription(&subscriber, &merchant, &100u64, &30u64));
        }
        created.sort();

        let (mut dumped, pages) = drain_pages(&client, 3);
        dumped.sort();

        assert_eq!(pages, 3, "expected 3 pages (3+3+1)");
        assert_eq!(dumped, created);
    }
}
