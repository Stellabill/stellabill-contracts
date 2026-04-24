#![cfg(test)]

use crate::{
    test_utils::{setup::TestEnv, fixtures},
    types::{MetadataSetEvent, MetadataDeletedEvent},
};
use soroban_sdk::{testutils::{Address as _, Events}, Address, String, Symbol, vec, IntoVal, FromVal};

#[test]
fn fixture_metadata_lifecycle_events() {
    let test_env = TestEnv::default();
    let merchant = Address::generate(&test_env.env);
    let subscriber = Address::generate(&test_env.env);

    // Create subscription
    let id = test_env.client.create_subscription(
        &subscriber,
        &merchant,
        &1000,
        &(30 * 24 * 60 * 60),
        &false,
        &None,
        &None::<u64>,
    );

    let key = String::from_str(&test_env.env, "plan_name");
    let val = String::from_str(&test_env.env, "Premium");

    // Action: Set Metadata
    test_env.client.set_metadata(&id, &subscriber, &key, &val);

    // Verify Event: MetadataSetEvent
    let last_event = test_env.env.events().all().last().unwrap();
    assert_eq!(last_event.0, test_env.client.address);
    
    let expected_topics = (Symbol::new(&test_env.env, "metadata_set"), id).into_val(&test_env.env);
    assert_eq!(last_event.1, expected_topics);
    
    let event_data = MetadataSetEvent::from_val(&test_env.env, &last_event.2);
    assert_eq!(event_data.subscription_id, id);
    assert_eq!(event_data.key, key);
    assert_eq!(event_data.authorizer, subscriber);

    // Action: Delete Metadata
    test_env.client.delete_metadata(&id, &merchant, &key);

    // Verify Event: MetadataDeletedEvent
    let last_event = test_env.env.events().all().last().unwrap();
    assert_eq!(last_event.0, test_env.client.address);
    
    let expected_topics = (Symbol::new(&test_env.env, "metadata_deleted"), id).into_val(&test_env.env);
    assert_eq!(last_event.1, expected_topics);
    
    let event_data = MetadataDeletedEvent::from_val(&test_env.env, &last_event.2);
    assert_eq!(event_data.subscription_id, id);
    assert_eq!(event_data.key, key);
    assert_eq!(event_data.authorizer, merchant);
}
