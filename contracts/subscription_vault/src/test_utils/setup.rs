use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, Address, Env};

pub struct TestEnv {
    pub env: Env,
    pub client: SubscriptionVaultClient<'static>,
    pub admin: Address,
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn stellar_token_client(&self) -> soroban_sdk::token::StellarAssetClient<'static> {
        soroban_sdk::token::StellarAssetClient::new(&self.env, &self.token)
    }

    #[allow(dead_code)]
    pub fn jump(&self, seconds: u64) {
        self.env.ledger().set_timestamp(self.env.ledger().timestamp() + seconds);
    }
}

pub fn setup_env() -> Env {
    let env = Env::default();
    env.mock_all_auths();
    env
}

pub fn create_test_client<'a>(
    env: &Env,
    admin: &Address,
    token: &Address,
) -> SubscriptionVaultClient<'a> {
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);
    client.init(token, &6, admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    client
}

pub fn advance_ledger_by(env: &Env, seconds: u64) {
    env.ledger().set_timestamp(env.ledger().timestamp() + seconds);
}
