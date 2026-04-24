use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{testutils::Address as _, testutils::Ledger, Address, Env};

pub struct TestEnv {
    pub env: Env,
    pub client: SubscriptionVaultClient<'static>,
    pub token: Address,
    pub admin: Address,
}

impl TestEnv {
    /// Initialize a standard test environment with zero minimum top-up.
    pub fn default() -> Self {
        Self::with_min_topup(0)
    }

    /// Initialize a test environment with a custom minimum top-up threshold.
    ///
    /// Use this when the contract under test must enforce a non-zero minimum deposit
    /// (e.g. `test_insufficient_balance`, `test_recovery`).
    pub fn with_min_topup(min_topup: i128) -> Self {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        let grace_period = 7 * 24 * 60 * 60u64; // 7 days

        client.init(&token, &6, &admin, &min_topup, &grace_period);

        Self {
            env,
            client,
            token,
            admin,
        }
    }

    /// Create a standard token client.
    pub fn token_client(&self) -> soroban_sdk::token::Client<'static> {
        soroban_sdk::token::Client::new(&self.env, &self.token)
    }

    /// Create a stellar asset token client (for minting).
    pub fn stellar_token_client(&self) -> soroban_sdk::token::StellarAssetClient<'static> {
        soroban_sdk::token::StellarAssetClient::new(&self.env, &self.token)
    }

    /// Set the ledger timestamp.
    pub fn set_timestamp(&self, timestamp: u64) {
        self.env.ledger().with_mut(|li| li.timestamp = timestamp);
    }

    /// Fast-forward time by a given duration.
    pub fn jump(&self, duration: u64) {
        let current = self.env.ledger().timestamp();
        self.set_timestamp(current + duration);
    }
}
