use soroban_sdk::{Env, Address, Vec};
use crate::types::Error;
use crate::merchant::get_merchant_balance_by_token;
use crate::types::DataKey;

pub fn assert_token_balance_invariant(
    env: &Env,
    token: &Address,
) -> Result<(), Error> {
    let contract = env.current_contract_address();
    let token_client = soroban_sdk::token::Client::new(env, token);

    // 1. LIVE on-chain balance
    let live_balance = token_client.balance(&contract);

    // 2. SUM all merchant balances for this token
    let merchants_key = DataKey::AllMerchants; // if you have it
    let merchants: Vec<Address> = env
        .storage()
        .instance()
        .get(&merchants_key)
        .unwrap_or(Vec::new(env));

    let mut expected: i128 = 0;

    for merchant in merchants.iter() {
        expected = expected
            .checked_add(get_merchant_balance_by_token(env, &merchant, token))
            .ok_or(Error::Overflow)?;
    }

    if live_balance != expected {
        return Err(Error::InvariantViolation);
    }

    Ok(())
}