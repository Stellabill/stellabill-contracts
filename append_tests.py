test_content = '''

// ── Blocklist Tests ────────────────────────────────────────────────────────────

#[test]
fn test_merchant_can_block_and_unblock_subscriber() {
    let (env, client, _, _) = setup_test_env();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    assert_eq!(client.is_subscriber_blocked(&merchant, &subscriber), false);

    client.block_subscriber(&merchant, &subscriber);
    assert_eq!(client.is_subscriber_blocked(&merchant, &subscriber), true);

    client.unblock_subscriber(&merchant, &subscriber);
    assert_eq!(client.is_subscriber_blocked(&merchant, &subscriber), false);
}

#[test]
#[should_panic(expected = "Error(Contract, #1022)")]
fn test_blocked_subscriber_cannot_create_subscription() {
    let (env, client, _, _) = setup_test_env();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    client.block_subscriber(&merchant, &subscriber);

    client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #1022)")]
fn test_blocked_subscriber_cannot_deposit_funds() {
    let (env, client, _, _) = setup_test_env();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    let id = client.create_subscription(
        &subscriber,
        &merchant,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None,
    );

    client.block_subscriber(&merchant, &subscriber);

    client.deposit_funds(&id, &subscriber, &AMOUNT);
}

#[test]
fn test_blocklist_isolation_between_merchants() {
    let (env, client, _, _) = setup_test_env();
    let merchant_a = Address::generate(&env);
    let merchant_b = Address::generate(&env);
    let subscriber = Address::generate(&env);

    client.block_subscriber(&merchant_a, &subscriber);

    // Should be blocked for A but not B
    assert_eq!(client.is_subscriber_blocked(&merchant_a, &subscriber), true);
    assert_eq!(client.is_subscriber_blocked(&merchant_b, &subscriber), false);

    // Can still create subscription for B
    let id_b = client.create_subscription(
        &subscriber,
        &merchant_b,
        &AMOUNT,
        &INTERVAL,
        &false,
        &None,
        &None,
    );
    
    // Deposit funds for B
    client.deposit_funds(&id_b, &subscriber, &AMOUNT);
}

#[test]
#[should_panic(expected = "Error(Contract, #403)")]
fn test_block_subscriber_unauthorized() {
    let (env, client, _, _) = setup_test_env();
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let other = Address::generate(&env);

    client.block_subscriber(&other, &subscriber); // Should panic because other isn't authorized for other
}

'''

with open("contracts/subscription_vault/src/test.rs", "a") as f:
    f.write(test_content)
