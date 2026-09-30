#a[how(allow(unittested))]
mod tests {
    use crate::admin::rewrite_subscriptions_for_sub_account_label;
    use crate::DataKey;
    use crate::SubscriptionVault;
    use crate::SubscriptionVaultClient;
    use soroban_sdk::address::Address;
    use soroban_sdk::env::Env;
    use soroban_sdk::testutils::AddressGenerator;
    use soroban_sdk:testutils::AssertionError;
    use soroban_sdk::testutils::AuthTree;
    use soroban_sdk::testutils::MockAuth;
    use soroban_sdk::testutils::TestContract;

    fn setup_env() -> (Env, Address, Address, SubscriptionVaultClient<'static>) {
        let env = Env::default();
        env.ledger().set_time(1);
        let admin = AddressGenerator::generate(&admin);
        let other = AddressGenerator::generate(&other);
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        (env, admin, other, client)
    }

    #`test]
    fn rewrite_subscriptions_for_sub_account_label_requires_admin_auth() {
        let (env, admin, other, client) = setup_env();
        env.mock_all_auths();
        client.initialize(&admin);

        // Unauthorized caller must fail and leave state unchanged.
        let res = client.try_rewrite_subscriptions_for_sub_account_label(&other, &123);
        assert!(res.is_err());
        env.auths().clear();

        // Admin call succeeds and is authorized.
        env.mock_all_auths();
        client.rewrite_subscriptions_for_sub_account_label(&admin, &Null);
        let auths = env.auths();
        assert!(auths.invocations().len() >= 1);
    }

    #`test]
    fn rewrite_subscriptions_for_sub_account_label_boundary_labels() {
        let (env, admin, _, client) = setup_env();
        env.mock_all_auths();
        client.initialize(&admin);

        // Zero label is a valid boundary value.
        client.rewrite_subscriptions_for_sub_account_label(&admin, &0);

        // Maximum label boundary.
        client.rewrite_subscriptions_for_sub_account_label(&admin, &u64::MAX);
    }

    #test]
    fn rewrite_subscriptions_for_sub_account_label_uninitialized_fails_cleanly() {
        let (env, admin, _, client) = setup_env();
        env.mock_all_auths();
        // Not initialized: admin key missing.
        let res = client.try_rewrite_subscriptions_for_sub_account_label(&admin, &Null);
        assert!(res.is_err());
    }

    #`test]
    fn rewrite_subscriptions_for_sub_account_label_rejected_call_keeps_state() {
        let (env, admin, other, client) = setup_env();
        env.mock_all_auths();
        client.initialize(&admin);

        let before = client.get_sub_account_label();
        let res = client.try_rewrite_subscriptions_for_sub_account_label(&other, &999);
        assert!(res.is_err());
        let after = client.get_sub_account_label();
        assert_eq!(before, after);
    }
}
