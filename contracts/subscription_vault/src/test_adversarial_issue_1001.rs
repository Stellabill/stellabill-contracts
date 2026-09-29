//! Adversarial coverage for `queue_treasury_change` (#1001).
//!
//! The tests exercise the accepted fee boundaries, authorization and treasury
//! validation, duplicate-queue rejection, and state preservation on failures.

#![cfg(test)]

use crate::{types::DataKey, Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{testutils::Address as _, Address, Env};

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    (env, client, admin)
}

fn pending(
    env: &Env,
    client: &SubscriptionVaultClient,
) -> Option<crate::types::PendingTreasuryChange> {
    env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .get(&DataKey::PendingTreasuryChange)
    })
}

#[test]
fn queue_treasury_change_accepts_zero_and_maximum_fee() {
    let (env, client, admin) = setup();
    let treasury = Address::generate(&env);

    assert_eq!(
        client.try_queue_treasury_change(&admin, &treasury, &0),
        Ok(Ok(()))
    );
    assert_eq!(client.get_protocol_fee_bps(), 0);
    assert_eq!(client.get_treasury(), Some(treasury));

    let (env, client, admin) = setup();
    let treasury = Address::generate(&env);
    assert_eq!(
        client.try_queue_treasury_change(&admin, &treasury, &10_000),
        Ok(Ok(()))
    );
    assert_eq!(client.get_protocol_fee_bps(), 10_000);
}

#[test]
fn queue_treasury_change_rejects_invalid_fee_without_mutating_state() {
    let (env, client, admin) = setup();
    let treasury = Address::generate(&env);
    let before_treasury = client.get_treasury();
    let before_fee = client.get_protocol_fee_bps();

    assert_eq!(
        client.try_queue_treasury_change(&admin, &treasury, &10_001),
        Err(Ok(Error::InvalidInput))
    );
    assert_eq!(client.get_treasury(), before_treasury);
    assert_eq!(client.get_protocol_fee_bps(), before_fee);
    assert!(pending(&env, &client).is_none());
}

#[test]
fn queue_treasury_change_rejects_unauthorized_and_self_treasury_without_mutation() {
    let (env, client, admin) = setup();
    let stranger = Address::generate(&env);
    let treasury = Address::generate(&env);

    assert_eq!(
        client.try_queue_treasury_change(&stranger, &treasury, &250),
        Err(Ok(Error::Forbidden))
    );
    assert!(pending(&env, &client).is_none());

    assert_eq!(
        client.try_queue_treasury_change(&admin, &client.address, &250),
        Err(Ok(Error::InvalidInput))
    );
    assert!(pending(&env, &client).is_none());
}

#[test]
fn queue_treasury_change_rejects_second_pending_change_without_overwriting_first() {
    let (env, client, admin) = setup();
    let first_treasury = Address::generate(&env);
    let second_treasury = Address::generate(&env);

    client.queue_treasury_change(&admin, &first_treasury, &250);
    let before = pending(&env, &client).expect("first queue should be stored");

    assert_eq!(
        client.try_queue_treasury_change(&admin, &second_treasury, &500),
        Err(Ok(Error::InvalidInput))
    );
    let after = pending(&env, &client).expect("rejected queue must preserve the first queue");
    assert_eq!(after.new_treasury, before.new_treasury);
    assert_eq!(after.new_fee_bps, before.new_fee_bps);
    assert_eq!(after.effective_at, before.effective_at);
    assert_eq!(client.get_treasury(), Some(first_treasury));
    assert_eq!(client.get_protocol_fee_bps(), 250);
}
