#![cfg(test)]

use soroban_sdk::{testutils::Address as _, Env, Address};
use subscription_vault::{
    DataKey, Subscription, SubscriptionStatus,
    SUB_TTL_THRESHOLD, SUB_TTL_EXTEND_TO, extend_subscription_ttl,
};

#[test]
fn bench_ttl_extension_cost() {
    let env = Env::default();
    env.mock_all_auths();
    
    let key = DataKey::Sub(1);
    
    let sub = Subscription {
        subscriber: Address::generate(&env),
        merchant: Address::generate(&env),
        token: Address::generate(&env),
        amount: 100,
        interval_seconds: 3600,
        last_payment_timestamp: 0,
        status: SubscriptionStatus::Active,
        prepaid_balance: 0,
        usage_enabled: false,
        lifetime_cap: None,
        lifetime_charged: 0,
        start_time: 0,
        expires_at: None,
        grace_start_timestamp: None,
        cancel_at: None,
        expires_at_ledger: None,
        sub_account_label: None,
        auto_renew: true,
        auto_renew_disabled_at: None,
    };
    
    env.storage().persistent().set(&key, &sub);
    
    // 1. Below threshold
    env.storage().persistent().extend_ttl(&key, 10, 100);
    env.budget().reset_default();
    extend_subscription_ttl(&env, &key);
    let cpu_cost_below = env.budget().cpu_instruction_cost();
    
    // 2. Above threshold
    env.budget().reset_default();
    extend_subscription_ttl(&env, &key);
    let cpu_cost_above = env.budget().cpu_instruction_cost();
    
    std::println!("Cost Below Threshold: CPU: {}", cpu_cost_below);
    std::println!("Cost Above Threshold: CPU: {}", cpu_cost_above);
    
    assert!(cpu_cost_below > cpu_cost_above, "Extension should cost more than skipping");
    
    // Edge Case: Threshold exactly at boundary
    // If the threshold is SUB_TTL_THRESHOLD, we set live_until to SUB_TTL_THRESHOLD-1 (triggers)
    // and SUB_TTL_THRESHOLD (skips).
    // Note: extend_ttl takes (threshold, extend_to).
    // Let's set it to exactly SUB_TTL_THRESHOLD
    env.storage().persistent().extend_ttl(&key, SUB_TTL_THRESHOLD, SUB_TTL_THRESHOLD);
    env.budget().reset_default();
    extend_subscription_ttl(&env, &key);
    let cpu_boundary = env.budget().cpu_instruction_cost();
    std::println!("Cost Exactly At Boundary (should skip): CPU: {}", cpu_boundary);
    assert_eq!(cpu_boundary, cpu_cost_above, "Should match skip cost");

    // Edge Case: Absurdly large target TTL
    // The underlying SDK will handle it if it doesn't overflow, but max TTL is limited.
    // We'll just test extending it repeatedly.
    env.storage().persistent().extend_ttl(&key, 10, 100);
    env.budget().reset_default();
    env.storage().persistent().extend_ttl(&key, SUB_TTL_THRESHOLD, u32::MAX - 1);
    let cpu_large = env.budget().cpu_instruction_cost();
    std::println!("Cost Large Extension: CPU: {}", cpu_large);
    
    // Edge Case: Repeated triggers
    env.storage().persistent().extend_ttl(&key, 10, 100);
    env.budget().reset_default();
    for _ in 0..10 {
        extend_subscription_ttl(&env, &key);
    }
    let cpu_repeated = env.budget().cpu_instruction_cost();
    std::println!("Cost 10x repeated (1 trigger + 9 skips): CPU: {}", cpu_repeated);
}
