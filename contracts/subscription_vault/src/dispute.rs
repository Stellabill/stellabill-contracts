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
