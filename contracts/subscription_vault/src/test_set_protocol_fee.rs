//! Focused adversarial coverage for `admin::set_protocol_fee`.

use crate::test_utils::setup::TestEnv;
use crate::{DataKey, Error};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::Address;

fn stored_treasury(test_env: &TestEnv) -> Option<Address> {
    test_env.env.as_contract(&test_env.client.address, || {
        test_env
            .env
            .storage()
            .persistent()
            .get(&DataKey::Treasury)
            .or_else(|| test_env.env.storage().instance().get(&DataKey::Treasury))
    })
}

#[test]
fn set_protocol_fee_accepts_zero_and_maximum_valid_fees() {
    let zero = TestEnv::default();
    let zero_treasury = Address::generate(&zero.env);
    assert!(zero
        .client
        .try_set_protocol_fee(&zero.admin, &zero_treasury, &0u32)
        .unwrap()
        .is_ok());
    assert_eq!(zero.client.get_protocol_fee_bps(), 0);
    assert_eq!(stored_treasury(&zero), Some(zero_treasury));

    let maximum = TestEnv::default();
    let maximum_treasury = Address::generate(&maximum.env);
    assert!(maximum
        .client
        .try_set_protocol_fee(&maximum.admin, &maximum_treasury, &10_000u32)
        .unwrap()
        .is_ok());
    assert_eq!(maximum.client.get_protocol_fee_bps(), 10_000);
    assert_eq!(stored_treasury(&maximum), Some(maximum_treasury));
}

#[test]
fn set_protocol_fee_rejects_an_out_of_range_fee_without_mutating_state() {
    let test_env = TestEnv::default();
    let treasury = Address::generate(&test_env.env);

    assert_eq!(
        test_env
            .client
            .try_set_protocol_fee(&test_env.admin, &treasury, &10_001u32),
        Err(Ok(Error::InvalidInput))
    );
    assert_eq!(test_env.client.get_protocol_fee_bps(), 0);
    assert_eq!(stored_treasury(&test_env), None);
    assert!(!test_env.env.as_contract(&test_env.client.address, || {
        test_env
            .env
            .storage()
            .persistent()
            .has(&DataKey::PendingTreasuryChange)
    }));
}

#[test]
fn set_protocol_fee_rejects_the_contract_as_treasury_without_mutating_state() {
    let test_env = TestEnv::default();
    let contract_address = test_env.client.address.clone();

    assert_eq!(
        test_env
            .client
            .try_set_protocol_fee(&test_env.admin, &contract_address, &500u32,),
        Err(Ok(Error::InvalidInput))
    );
    assert_eq!(test_env.client.get_protocol_fee_bps(), 0);
    assert_eq!(stored_treasury(&test_env), None);
}

#[test]
fn set_protocol_fee_rejects_an_unauthorized_admin_without_mutating_state() {
    let test_env = TestEnv::default();
    let stranger = Address::generate(&test_env.env);
    let treasury = Address::generate(&test_env.env);

    assert_eq!(
        test_env
            .client
            .try_set_protocol_fee(&stranger, &treasury, &500u32),
        Err(Ok(Error::Forbidden))
    );
    assert_eq!(test_env.client.get_protocol_fee_bps(), 0);
    assert_eq!(stored_treasury(&test_env), None);
}

#[test]
fn set_protocol_fee_rejects_a_second_change_and_preserves_the_first_state() {
    let test_env = TestEnv::default();
    let first_treasury = Address::generate(&test_env.env);
    let second_treasury = Address::generate(&test_env.env);

    test_env
        .client
        .set_protocol_fee(&test_env.admin, &first_treasury, &250u32);

    let result = test_env
        .client
        .try_set_protocol_fee(&test_env.admin, &second_treasury, &750u32);

    assert_eq!(result, Err(Ok(Error::InvalidInput)));
    assert_eq!(test_env.client.get_protocol_fee_bps(), 250);
    assert_eq!(stored_treasury(&test_env), Some(first_treasury));
    let pending: crate::types::PendingTreasuryChange =
        test_env.env.as_contract(&test_env.client.address, || {
            test_env
                .env
                .storage()
                .persistent()
                .get(&DataKey::PendingTreasuryChange)
                .expect("first pending change must remain")
        });
    assert_eq!(pending.new_fee_bps, 250);
}
