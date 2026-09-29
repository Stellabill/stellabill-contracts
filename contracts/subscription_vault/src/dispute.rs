use soroban_sdk::{Address, Env};
use crate::Error;

pub fn do_lodge_escrow_dispute(
    env: &Env,
    merchant: Address,
    subscription_id: u32,
) -> Result<(), Error> {
    merchant.require_auth();

    if !env.storage().instance().has(&subscription_id) {
        return Err(Error::NotFound);
    }
    
    let sub = crate::SubscriptionVault::get_subscription(env.clone(), subscription_id)?;
    if sub.merchant != merchant {
        return Err(Error::Unauthorized);
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};
    use crate::{SubscriptionVault, SubscriptionVaultClient};

    fn setup_env_and_contract() -> (Env, soroban_sdk::Address, SubscriptionVaultClient<'static>) {
        let env = Env::default();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);
        (env, contract_id, client)
    }

    #[test]
    #[should_panic(expected = "HostError: Error(Auth, InvalidAction)")]
    fn test_do_lodge_escrow_dispute_unauthorized_host() {
        let (env, contract_id, client) = setup_env_and_contract();
        let merchant = Address::generate(&env);
        
        let admin = Address::generate(&env);
        let usdc = Address::generate(&env);
        env.mock_all_auths();
        client.init(&usdc, &admin, &100);
        
        env.set_auths(&[]);
        
        let result = env.as_contract(&contract_id, || {
            do_lodge_escrow_dispute(&env, merchant.clone(), 1)
        });
        
        assert!(result.is_err());
    }

    #[test]
    fn test_do_lodge_escrow_dispute_not_found() {
        let (env, contract_id, client) = setup_env_and_contract();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let usdc = Address::generate(&env);
        client.init(&usdc, &admin, &100);
        
        let merchant = Address::generate(&env);
        
        let result = env.as_contract(&contract_id, || {
            do_lodge_escrow_dispute(&env, merchant.clone(), 999)
        });
        assert!(matches!(result, Err(Error::NotFound)));
    }
    
    #[test]
    fn test_do_lodge_escrow_dispute_unauthorized_merchant() {
        let (env, contract_id, client) = setup_env_and_contract();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let usdc = Address::generate(&env);
        client.init(&usdc, &admin, &100);
        
        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);
        let other_merchant = Address::generate(&env);
        
        let sub_id = client.create_subscription(
            &subscriber,
            &merchant,
            &100,
            &86400,
            &false,
            &None
        );
        
        let result = env.as_contract(&contract_id, || {
            do_lodge_escrow_dispute(&env, other_merchant.clone(), sub_id)
        });
        assert!(matches!(result, Err(Error::Unauthorized)));
    }

    #[test]
    fn test_do_lodge_escrow_dispute_happy_path() {
        let (env, contract_id, client) = setup_env_and_contract();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let usdc = Address::generate(&env);
        client.init(&usdc, &admin, &100);
        
        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);
        
        let sub_id = client.create_subscription(
            &subscriber,
            &merchant,
            &100,
            &86400,
            &false,
            &None
        );
        
        let result = env.as_contract(&contract_id, || {
            do_lodge_escrow_dispute(&env, merchant.clone(), sub_id)
        });
        assert!(result.is_ok());
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::setup::TestEnv;
    use crate::types::{DisputeStatus, DISPUTE_WINDOW_SECS, DataKey};
    use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, Address, BytesN};

    fn setup_dispute(te: &TestEnv, amount: i128) -> (u32, u64, Address, Address) {
        let subscriber = Address::generate(&te.env);
        let merchant = Address::generate(&te.env);
        
        let sub_id = te.client.create_subscription(
            &subscriber,
            &merchant,
            &10_000,
            &86400,
            &false,
            &None,
            &None::<u64>,
        );
        
        te.env.as_contract(&te.client.address, || {
            crate::merchant::set_merchant_balance(&te.env, &merchant, &te.token, &100_000);
        });

        let evidence = Some(BytesN::from_array(&te.env, &[1; 32]));
        let dispute_id = te.client.open_dispute(&subscriber, &sub_id, &amount, &evidence);
        
        (sub_id, dispute_id, subscriber, merchant)
    }

    #[test]
    #[should_panic(expected = "HostError: Error(Auth, InvalidAction)")]
    fn test_resolve_dispute_unauthorized() {
        let te = TestEnv::default();
        let (_, dispute_id, _, _) = setup_dispute(&te, 1000);
        
        let fake_admin = Address::generate(&te.env);
        te.env.set_auths(&[]);
        
        te.env.as_contract(&te.client.address, || {
            let _ = do_resolve_dispute(&te.env, fake_admin, dispute_id, true);
        });
    }

    #[test]
    fn test_resolve_dispute_already_resolved() {
        let te = TestEnv::default();
        let (_, dispute_id, _, _) = setup_dispute(&te, 1000);
        
        te.env.ledger().set_timestamp(te.env.ledger().timestamp() + DISPUTE_WINDOW_SECS + 1);
        
        let result = te.env.as_contract(&te.client.address, || {
            do_resolve_dispute(&te.env, te.admin.clone(), dispute_id, true)
        });
        assert!(result.is_ok());

        let result2 = te.env.as_contract(&te.client.address, || {
            do_resolve_dispute(&te.env, te.admin.clone(), dispute_id, true)
        });
        assert_eq!(result2.err().unwrap(), Error::DisputeAlreadyResolved);
    }

    #[test]
    fn test_resolve_dispute_not_responded() {
        let te = TestEnv::default();
        let (_, dispute_id, _, _) = setup_dispute(&te, 1000);
        
        let result = te.env.as_contract(&te.client.address, || {
            do_resolve_dispute(&te.env, te.admin.clone(), dispute_id, true)
        });
        assert_eq!(result.err().unwrap(), Error::DisputeNotResponded);
    }

    #[test]
    fn test_resolve_dispute_auto_resolve_to_subscriber() {
        let te = TestEnv::default();
        let amount = 1000;
        let (sub_id, dispute_id, subscriber, merchant) = setup_dispute(&te, amount);
        
        te.env.ledger().set_timestamp(te.env.ledger().timestamp() + DISPUTE_WINDOW_SECS + 1);
        
        let result = te.env.as_contract(&te.client.address, || {
            do_resolve_dispute(&te.env, te.admin.clone(), dispute_id, false) // even if false, it auto-resolves to subscriber
        });
        assert!(result.is_ok());

        te.env.as_contract(&te.client.address, || {
            let dispute = do_get_dispute(&te.env, dispute_id).unwrap();
            assert_eq!(dispute.status, DisputeStatus::ResolvedToSubscriber);
            
            let has_escrow = te.env.storage().instance().has(&DataKey::DisputeEscrow(dispute_id));
            assert!(!has_escrow);
            
            let has_sub_dispute = te.env.storage().instance().has(&DataKey::SubscriptionDispute(sub_id));
            assert!(!has_sub_dispute);
        });
        
        let sub_balance = te.stellar_token_client().balance(&subscriber);
        assert_eq!(sub_balance, amount);
    }

    #[test]
    fn test_resolve_dispute_to_merchant_after_response() {
        let te = TestEnv::default();
        let amount = 1000;
        let (_, dispute_id, _, merchant) = setup_dispute(&te, amount);
        
        te.client.respond_dispute(&te.admin, &dispute_id, &None);
        
        te.env.as_contract(&te.client.address, || {
            let current = crate::merchant::get_merchant_balance_by_token(&te.env, &merchant, &te.token);
            let result = do_resolve_dispute(&te.env, te.admin.clone(), dispute_id, false);
            assert!(result.is_ok());
            
            let new_balance = crate::merchant::get_merchant_balance_by_token(&te.env, &merchant, &te.token);
            assert_eq!(new_balance, current + amount);
            
            let dispute = do_get_dispute(&te.env, dispute_id).unwrap();
            assert_eq!(dispute.status, DisputeStatus::ResolvedToMerchant);
        });
    }

    #[test]
    fn test_resolve_dispute_to_subscriber_after_response() {
        let te = TestEnv::default();
        let amount = 1000;
        let (_, dispute_id, subscriber, _) = setup_dispute(&te, amount);
        
        te.client.respond_dispute(&te.admin, &dispute_id, &None);
        
        let result = te.env.as_contract(&te.client.address, || {
            do_resolve_dispute(&te.env, te.admin.clone(), dispute_id, true)
        });
        assert!(result.is_ok());

        te.env.as_contract(&te.client.address, || {
            let dispute = do_get_dispute(&te.env, dispute_id).unwrap();
            assert_eq!(dispute.status, DisputeStatus::ResolvedToSubscriber);
        });
        
        let sub_balance = te.stellar_token_client().balance(&subscriber);
        assert_eq!(sub_balance, amount);
    }
}
