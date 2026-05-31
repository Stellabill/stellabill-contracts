#[test]
fn test_create_subscription_emits_event() {
    let env = Env::default();
    // ... setup contract ...
    
    let sub_id = client.create_subscription(&subscriber, &merchant, &1000, &3600, &None, &false);
    
    let last_event = env.events().all().last().unwrap();
    assert_eq!(
        last_event,
        (
            client.address.clone(),
            (Symbol::new(&env, "created"), sub_id).into_val(&env),
            SubscriptionCreatedEvent {
                subscription_id: sub_id,
                subscriber,
                merchant,
                amount: 1000,
                interval_seconds: 3600,
                lifetime_cap: None,
                expires_at: None,
            }.into_val(&env)
        )
    );
}
#[test]
fn test_create_subscription_emits_event() {
    let env = Env::default();
    // ... setup contract ...
    
    let sub_id = client.create_subscription(&subscriber, &merchant, &1000, &3600, &None, &false);
    
    let last_event = env.events().all().last().unwrap();
    assert_eq!(
        last_event,
        (
            client.address.clone(),
            (Symbol::new(&env, "created"), sub_id).into_val(&env),
            SubscriptionCreatedEvent {
                subscription_id: sub_id,
                subscriber,
                merchant,
                amount: 1000,
                interval_seconds: 3600,
                lifetime_cap: None,
                expires_at: None,
            }.into_val(&env)
        )
    );
}
// After storage update
env.events().publish(
    (Symbol::new(&env, "created"), subscription_id),
    SubscriptionCreatedEvent {
        subscription_id,
        subscriber: subscriber.clone(),
        merchant: merchant.clone(),
        amount,
        interval_seconds,
        lifetime_cap: None, // Update if your logic supports caps
        expires_at: expiration,
    },
);
