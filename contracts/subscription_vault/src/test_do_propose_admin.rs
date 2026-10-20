//! Focused adversarial coverage for `admin::do_propose_admin`.

use crate::{admin, Error, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env};

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000_000);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, client, admin)
}

#[test]
fn do_propose_admin_stores_the_new_admin_and_expiry() {
    let (env, client, current_admin) = setup();
    let new_admin = Address::generate(&env);

    // Storage writes require a contract context.
    env.as_contract(&client.address, || {
        assert_eq!(
            admin::do_propose_admin(&env, current_admin.clone(), new_admin.clone()),
            Ok(())
        );
    });

    let proposal = client
        .get_admin_proposal()
        .expect("proposal must be stored");
    assert_eq!(proposal.new_admin, new_admin);
    assert_eq!(proposal.proposed_at, 1_000_000);
    assert_eq!(proposal.expires_at, 1_000_000 + 7 * 24 * 60 * 60);
    assert_eq!(client.get_admin(), current_admin);
}

#[test]
fn do_propose_admin_rejects_a_non_admin_without_mutating_state() {
    let (env, client, current_admin) = setup();
    let stranger = Address::generate(&env);
    let new_admin = Address::generate(&env);

    let result =
        env.as_contract(&client.address, || admin::do_propose_admin(&env, stranger, new_admin));

    assert_eq!(result, Err(Error::Unauthorized));
    assert!(client.get_admin_proposal().is_none());
    assert_eq!(client.get_admin(), current_admin);
}

#[test]
fn do_propose_admin_rejects_the_contract_address_without_mutating_state() {
    let (env, client, current_admin) = setup();

    let result = env.as_contract(&client.address, || {
        admin::do_propose_admin(&env, current_admin.clone(), client.address.clone())
    });

    assert_eq!(result, Err(Error::InvalidNewAdmin));
    assert!(client.get_admin_proposal().is_none());
    assert_eq!(client.get_admin(), current_admin);
}

#[test]
fn do_propose_admin_rejects_a_second_proposal_and_preserves_the_first() {
    let (env, client, current_admin) = setup();
    let first_admin = Address::generate(&env);
    let second_admin = Address::generate(&env);

    env.as_contract(&client.address, || {
        admin::do_propose_admin(&env, current_admin.clone(), first_admin.clone()).unwrap();
    });
    let before = client.get_admin_proposal();

    let result = env.as_contract(&client.address, || {
        admin::do_propose_admin(&env, current_admin, second_admin)
    });

    assert_eq!(result, Err(Error::ProposalAlreadyExists));
    assert_eq!(client.get_admin_proposal(), before);
}

#[test]
fn do_propose_admin_saturates_expiry_at_the_timestamp_boundary() {
    let (env, client, current_admin) = setup();
    env.ledger().set_timestamp(u64::MAX - 1);
    let new_admin = Address::generate(&env);

    env.as_contract(&client.address, || {
        assert_eq!(
            admin::do_propose_admin(&env, current_admin, new_admin),
            Ok(())
        );
    });

    let proposal = client
        .get_admin_proposal()
        .expect("proposal must be stored");
    assert_eq!(proposal.proposed_at, u64::MAX - 1);
    assert_eq!(proposal.expires_at, u64::MAX);
}
