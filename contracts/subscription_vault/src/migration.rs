use crate::types::{DataKey, Error, MigrationExportEvent, Subscription, SubscriptionSummary};
use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env, Vec};

const MAX_EXPORT_LIMIT: u32 = 100;

#[contract]
pub struct MigrationContract;

#[contractimpl]
impl MigrationContract {
    /// Exports a paginated batch of subscriptions as summaries for migration.
    /// Returns the data batch and the `next_start_id` cursor.
    pub fn export_snapshots(
        env: Env,
        start_id: u32,
        limit: u32,
    ) -> Result<(Vec<SubscriptionSummary>, u32), Error> {
        // 1. Strict Access Control
        // If Admin is not set, it fails gracefully instead of panicking
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        // 2. Limit Boundary Validation (Uses their Error::InvalidExportLimit)
        if limit == 0 || limit > MAX_EXPORT_LIMIT {
            return Err(Error::InvalidExportLimit);
        }

        let mut results = Vec::new(&env);
        let mut current_id = start_id;
        let mut collected_count = 0;

        // Fetch the global maximum ID to prevent infinite loops
        let max_id: u32 = env.storage().instance().get(&DataKey::NextId).unwrap_or(0);

        // 3. Deterministic Pagination (Skipping Sparse IDs)
        while collected_count < limit && current_id < max_id {
            // Using their `Sub` DataKey
            let key = DataKey::Sub(current_id);

            // If the subscription exists, map it to the Migration Summary struct
            if let Some(sub) = env.storage().persistent().get::<_, Subscription>(&key) {
                let summary = SubscriptionSummary {
                    subscription_id: current_id,
                    subscriber: sub.subscriber,
                    merchant: sub.merchant,
                    token: sub.token,
                    amount: sub.amount,
                    interval_seconds: sub.interval_seconds,
                    last_payment_timestamp: sub.last_payment_timestamp,
                    status: sub.status,
                    prepaid_balance: sub.prepaid_balance,
                    usage_enabled: sub.usage_enabled,
                    lifetime_cap: sub.lifetime_cap,
                    lifetime_charged: sub.lifetime_charged,
                    start_time: sub.start_time,
                    expires_at: sub.expires_at,
                };
                results.push_back(summary);
                collected_count += 1;
            }

            current_id += 1;
        }

        // 4. Verify and Emit their specific MigrationExportEvent Payload
        let event_payload = MigrationExportEvent {
            admin,
            start_id,
            limit,
            exported: collected_count,
            timestamp: env.ledger().timestamp(),
        };

        env.events()
            .publish((symbol_short!("export"), start_id), event_payload);

        // Return the batch and the cursor for the next page
        Ok((results, current_id))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::types::{Subscription, SubscriptionStatus};
    use soroban_sdk::{testutils::Address as _, Env};

    // Helper to generate a dummy subscription for testing
    fn create_mock_subscription(env: &Env) -> Subscription {
        Subscription {
            subscriber: Address::generate(env),
            merchant: Address::generate(env),
            token: Address::generate(env),
            amount: 1000,
            interval_seconds: 2592000,
            last_payment_timestamp: 0,
            status: SubscriptionStatus::Active,
            prepaid_balance: 5000,
            usage_enabled: false,
            lifetime_cap: None,
            lifetime_charged: 0,
            start_time: 0,
            expires_at: None,
            grace_start_timestamp: None,
        }
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #6004)")]
    fn test_export_limit_zero_fails() {
        let env = Env::default();
        let contract_id = env.register(MigrationContract, ());
        let client = MigrationContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);

        env.mock_all_auths();
        env.as_contract(&contract_id, || {
            env.storage().instance().set(&DataKey::Admin, &admin);
        });

        client.export_snapshots(&0, &0);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #6004)")]
    fn test_export_limit_exceeds_max_fails() {
        let env = Env::default();
        let contract_id = env.register(MigrationContract, ());
        let client = MigrationContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);

        env.mock_all_auths();
        env.as_contract(&contract_id, || {
            env.storage().instance().set(&DataKey::Admin, &admin);
        });

        client.export_snapshots(&0, &(MAX_EXPORT_LIMIT + 1));
    }

    #[test]
    #[should_panic(expected = "Error(Auth, InvalidAction)")]
    fn test_unauthorized_access_fails() {
        let env = Env::default();
        let contract_id = env.register(MigrationContract, ());
        let client = MigrationContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env); // Actual admin

        // We purposefully do NOT mock auth here to simulate an unauthorized call
        env.as_contract(&contract_id, || {
            env.storage().instance().set(&DataKey::Admin, &admin);
        });

        client.export_snapshots(&0, &10);
    }

    #[test]
    fn test_sparse_id_pagination() {
        let env = Env::default();
        let contract_id = env.register(MigrationContract, ());
        let client = MigrationContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);

        env.mock_all_auths();

        // Setup the mocked database state
        env.as_contract(&contract_id, || {
            env.storage().instance().set(&DataKey::Admin, &admin);
            env.storage().instance().set(&DataKey::NextId, &6u32);

            // Insert at IDs 1, 3, and 5 (leaving 0, 2, 4 completely empty/sparse)
            env.storage()
                .persistent()
                .set(&DataKey::Sub(1), &create_mock_subscription(&env));
            env.storage()
                .persistent()
                .set(&DataKey::Sub(3), &create_mock_subscription(&env));
            env.storage()
                .persistent()
                .set(&DataKey::Sub(5), &create_mock_subscription(&env));
        });

        // Request a limit of 2, starting from ID 0
        let (results, next_cursor) = client.export_snapshots(&0, &2);

        // Assertions
        assert_eq!(results.len(), 2, "Should have collected exactly 2 records");
        assert_eq!(
            results.get(0).unwrap().subscription_id,
            1,
            "First record should be ID 1"
        );
        assert_eq!(
            results.get(1).unwrap().subscription_id,
            3,
            "Second record should be ID 3"
        );

        // The loop should have checked 0, 1, 2, 3 and stopped, meaning the next ID to check is 4
        assert_eq!(
            next_cursor, 4,
            "Cursor should point to the next ID to evaluate"
        );
    }
}
