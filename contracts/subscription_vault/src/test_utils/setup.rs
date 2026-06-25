use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, Address, Env};

pub struct TestEnv {
    pub env: Env,
    pub client: SubscriptionVaultClient<'static>,
    pub admin: Address,
    pub token: Address,
}

impl Default for TestEnv {
    fn default() -> Self {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        let admin = Address::generate(&env);

        let token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
        TestEnv { env, client, admin, token }
    }
}

impl TestEnv {
    pub fn stellar_token_client(&self) -> soroban_sdk::token::StellarAssetClient<'static> {
        soroban_sdk::token::StellarAssetClient::new(&self.env, &self.token)
    }

    pub fn jump(&self, seconds: u64) {
        self.env.ledger().set_timestamp(self.env.ledger().timestamp() + seconds);
    }
}
