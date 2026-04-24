extern crate std;

use crate::{
    SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, Vec as SorobanVec, IntoVal, Val,
};

// ── Roles ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Admin,
    Subscriber,
    Merchant,
    Stranger,
}

impl Role {
    pub fn all() -> &'static [Role] {
        &[
            Role::Admin,
            Role::Subscriber,
            Role::Merchant,
            Role::Stranger,
        ]
    }
}

// ── Operations ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    SetMinTopup,
    RotateAdmin,
    EnableEmergencyStop,
    DisableEmergencyStop,
    DepositFunds,
    CancelSubscription,
    PauseSubscription,
    ResumeSubscription,
    ChargeOneOff,
    WithdrawSubscriberFunds,
    WithdrawMerchantFunds,
    PauseMerchant,
    UnpauseMerchant,
    MerchantRefund,
    ConfigureUsageLimits,
    PartialRefund,
    BatchCharge,
    AddAcceptedToken,
    RemoveAcceptedToken,
    SetSubscriberCreditLimit,
    ExportContractSnapshot,
    SetProtocolFee,
    RecoverStrandedFunds,
    SetBillingRetention,
    CompactBillingStatements,
    SetOracleConfig,
    SetMerchantConfig,
    CreatePlanTemplateWithToken,
    UpdatePlanTemplate,
    SetPlanMaxActiveSubs,
    MigrateSubscriptionToPlan,
    CleanupSubscription,
    ChargeUsage,
    WithdrawMerchantTokenFunds,
    SetMetadata,
    DeleteMetadata,
    AddToBlocklist,
    RemoveFromBlocklist,
}

impl Operation {
    pub fn all() -> &'static [Operation] {
        &[
            Operation::SetMinTopup,
            Operation::RotateAdmin,
            Operation::EnableEmergencyStop,
            Operation::DisableEmergencyStop,
            Operation::DepositFunds,
            Operation::CancelSubscription,
            Operation::PauseSubscription,
            Operation::ResumeSubscription,
            Operation::ChargeOneOff,
            Operation::WithdrawSubscriberFunds,
            Operation::WithdrawMerchantFunds,
            Operation::PauseMerchant,
            Operation::UnpauseMerchant,
            Operation::MerchantRefund,
            Operation::ConfigureUsageLimits,
            Operation::PartialRefund,
            Operation::BatchCharge,
            Operation::AddAcceptedToken,
            Operation::RemoveAcceptedToken,
            Operation::SetSubscriberCreditLimit,
            Operation::ExportContractSnapshot,
            Operation::SetProtocolFee,
            Operation::RecoverStrandedFunds,
            Operation::SetBillingRetention,
            Operation::CompactBillingStatements,
            Operation::SetOracleConfig,
            Operation::SetMerchantConfig,
            Operation::CreatePlanTemplateWithToken,
            Operation::UpdatePlanTemplate,
            Operation::SetPlanMaxActiveSubs,
            Operation::MigrateSubscriptionToPlan,
            Operation::CleanupSubscription,
            Operation::ChargeUsage,
            Operation::WithdrawMerchantTokenFunds,
            Operation::SetMetadata,
            Operation::DeleteMetadata,
            Operation::AddToBlocklist,
            Operation::RemoveFromBlocklist,
        ]
    }
}

// ── Harness ──────────────────────────────────────────────────────────────────

pub struct FuzzHarness {
    pub env: Env,
    pub client: SubscriptionVaultClient<'static>,
    pub admin: Address,
    pub subscriber: Address,
    pub merchant: Address,
    pub stranger: Address,
    pub new_admin: Address,
    pub token: Address,
    pub plan_id: u32,
    pub subscription_id: u32,
}

impl FuzzHarness {
    pub fn setup() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        
        env.ledger().with_mut(|li| {
            li.timestamp = 1000;
        });

        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let subscriber = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let merchant = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let stranger = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let new_admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        
        let token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
            
        client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
        
        // Use true for usage_tracking_enabled
        let plan_id = client.create_plan_template(&merchant, &10_000_000, &2592000, &true, &None::<i128>);
        let subscription_id = client.create_subscription_from_plan(&subscriber, &plan_id);

        let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &token);
        token_admin.mint(&subscriber, &1_000_000_000);
        token_admin.mint(&merchant, &100_000_000);
        token_admin.mint(&contract_id, &100_000_000);

        Self {
            env,
            client,
            admin,
            subscriber,
            merchant,
            stranger,
            new_admin,
            token,
            plan_id,
            subscription_id,
        }
    }

    pub fn get_address(&self, role: Role) -> Address {
        match role {
            Role::Admin => self.admin.clone(),
            Role::Subscriber => self.subscriber.clone(),
            Role::Merchant => self.merchant.clone(),
            Role::Stranger => self.stranger.clone(),
        }
    }

    pub fn execute(&self, op: Operation, caller: Role) -> Result<(), std::string::String> {
        let address = self.get_address(caller);
        self.env.mock_auths(&[]); 
        
        let env = &self.env;
        
        let is_allowed = self.is_allowed(op, caller);
        if is_allowed {
            self.env.mock_all_auths();
        } else {
            self.env.mock_auths(&[]);
        }

        let res = match op {
            Operation::SetMinTopup => {
                std::format!("{:?}", self.client.try_set_min_topup(&address, &2_000_000))
            }
            Operation::RotateAdmin => {
                std::format!("{:?}", self.client.try_rotate_admin(&address, &self.new_admin))
            }
            Operation::EnableEmergencyStop => {
                std::format!("{:?}", self.client.try_enable_emergency_stop(&address))
            }
            Operation::DisableEmergencyStop => {
                self.env.mock_all_auths();
                self.client.enable_emergency_stop(&self.admin);
                if !is_allowed { self.env.mock_auths(&[]); }
                std::format!("{:?}", self.client.try_disable_emergency_stop(&address))
            }
            Operation::DepositFunds => {
                std::format!("{:?}", self.client.try_deposit_funds(&self.subscription_id, &address, &5_000_000))
            }
            Operation::CancelSubscription => {
                std::format!("{:?}", self.client.try_cancel_subscription(&self.subscription_id, &address))
            }
            Operation::PauseSubscription => {
                std::format!("{:?}", self.client.try_pause_subscription(&self.subscription_id, &address))
            }
            Operation::ResumeSubscription => {
                self.env.mock_all_auths();
                self.client.pause_subscription(&self.subscription_id, &self.subscriber);
                if !is_allowed { self.env.mock_auths(&[]); }
                std::format!("{:?}", self.client.try_resume_subscription(&self.subscription_id, &address))
            }
            Operation::ChargeOneOff => {
                self.env.mock_all_auths(); 
                let _ = self.client.try_deposit_funds(&self.subscription_id, &self.subscriber, &20_000_000);
                if !is_allowed { self.env.mock_auths(&[]); }
                std::format!("{:?}", self.client.try_charge_one_off(&self.subscription_id, &address, &1_000_000))
            }
            Operation::WithdrawSubscriberFunds => {
                self.env.mock_all_auths();
                let _ = self.client.try_deposit_funds(&self.subscription_id, &self.subscriber, &10_000_000);
                let _ = self.client.try_cancel_subscription(&self.subscription_id, &self.subscriber);
                if !is_allowed { self.env.mock_auths(&[]); }
                std::format!("{:?}", self.client.try_withdraw_subscriber_funds(&self.subscription_id, &address))
            }
            Operation::WithdrawMerchantFunds => {
                self.env.mock_all_auths();
                let _ = self.client.try_deposit_funds(&self.subscription_id, &self.subscriber, &50_000_000);
                let _ = self.client.try_charge_one_off(&self.subscription_id, &self.merchant, &10_000_000);
                if !is_allowed { self.env.mock_auths(&[]); }
                std::format!("{:?}", self.client.try_withdraw_merchant_funds(&address, &1_000_000))
            }
            Operation::PauseMerchant => {
                std::format!("{:?}", self.client.try_pause_merchant(&address))
            }
            Operation::UnpauseMerchant => {
                self.env.mock_all_auths();
                self.client.pause_merchant(&self.merchant);
                if !is_allowed { self.env.mock_auths(&[]); }
                std::format!("{:?}", self.client.try_unpause_merchant(&address))
            }
            Operation::MerchantRefund => {
                self.env.mock_all_auths();
                let _ = self.client.try_deposit_funds(&self.subscription_id, &self.subscriber, &50_000_000);
                let _ = self.client.try_charge_one_off(&self.subscription_id, &self.merchant, &10_000_000);
                if !is_allowed { self.env.mock_auths(&[]); }
                std::format!("{:?}", self.client.try_merchant_refund(&address, &self.subscriber, &self.token, &1_000_000))
            }
            Operation::ConfigureUsageLimits => {
                std::format!("{:?}", self.client.try_configure_usage_limits(&address, &self.subscription_id, &Some(100), &3600, &60, &None))
            }
            Operation::PartialRefund => {
                self.env.mock_all_auths();
                let _ = self.client.try_deposit_funds(&self.subscription_id, &self.subscriber, &50_000_000);
                let _ = self.client.try_charge_one_off(&self.subscription_id, &self.merchant, &10_000_000);
                if !is_allowed { self.env.mock_auths(&[]); }
                std::format!("{:?}", self.client.try_partial_refund(&address, &self.subscription_id, &self.subscriber, &1_000_000))
            }
            Operation::BatchCharge => {
                if is_allowed {
                    self.env.mock_all_auths();
                } else {
                    let batch: SorobanVec<u32> = SorobanVec::from_array(env, [self.subscription_id]);
                    let mut args_vec = SorobanVec::new(env);
                    args_vec.push_back(batch.into_val(env));
                    self.env.mock_auths(&[soroban_sdk::testutils::MockAuth {
                        address: &address,
                        invoke: &soroban_sdk::testutils::MockAuthInvoke {
                            contract: &self.client.address,
                            fn_name: "batch_charge",
                            args: args_vec,
                            sub_invokes: &[],
                        },
                    }]);
                }
                std::format!("{:?}", self.client.try_batch_charge(&SorobanVec::from_array(env, [self.subscription_id])))
            }
            Operation::AddAcceptedToken => {
                let other_token = Address::generate(env);
                std::format!("{:?}", self.client.try_add_accepted_token(&address, &other_token, &6))
            }
            Operation::RemoveAcceptedToken => {
                let other_token = Address::generate(env);
                self.env.mock_all_auths();
                self.client.add_accepted_token(&self.admin, &other_token, &6);
                if !is_allowed { self.env.mock_auths(&[]); }
                std::format!("{:?}", self.client.try_remove_accepted_token(&address, &other_token))
            }
            Operation::SetSubscriberCreditLimit => {
                std::format!("{:?}", self.client.try_set_subscriber_credit_limit(&address, &self.subscriber, &self.token, &100_000_000))
            }
            Operation::ExportContractSnapshot => {
                std::format!("{:?}", self.client.try_export_contract_snapshot(&address))
            }
            Operation::SetProtocolFee => {
                let treasury = Address::generate(env);
                std::format!("{:?}", self.client.try_set_protocol_fee(&address, &treasury, &500))
            }
            Operation::RecoverStrandedFunds => {
                let recipient = Address::generate(env);
                let recovery_id = soroban_sdk::String::from_str(env, "fuzz_recovery");
                std::format!("{:?}", self.client.try_recover_stranded_funds(&address, &self.token, &recipient, &1_000_000, &recovery_id, &crate::RecoveryReason::UserOverpayment))
            }
            Operation::SetBillingRetention => {
                std::format!("{:?}", self.client.try_set_billing_retention(&address, &100))
            }
            Operation::CompactBillingStatements => {
                std::format!("{:?}", self.client.try_compact_billing_statements(&address, &self.subscription_id, &None))
            }
            Operation::SetOracleConfig => {
                std::format!("{:?}", self.client.try_set_oracle_config(&address, &true, &Some(Address::generate(env)), &300))
            }
            Operation::SetMerchantConfig => {
                let redirect = soroban_sdk::String::from_str(env, "https://example.com");
                std::format!("{:?}", self.client.try_set_merchant_config(&address, &None, &redirect, &false))
            }
            Operation::CreatePlanTemplateWithToken => {
                std::format!("{:?}", self.client.try_create_plan_template_with_token(&address, &self.token, &10_000_000, &2592000, &true, &None))
            }
            Operation::UpdatePlanTemplate => {
                // For allowed callers (Merchant), create their own plan; for others use
                // the harness plan (owned by self.merchant) so non-merchants get Forbidden.
                let target_plan_id = if is_allowed {
                    self.env.mock_all_auths();
                    let pid = self.client.create_plan_template(&address, &10_000_000, &2592000, &true, &None);
                    if !is_allowed { self.env.mock_auths(&[]); }
                    pid
                } else {
                    self.env.mock_auths(&[]);
                    self.plan_id // owned by self.merchant, so non-merchants get Forbidden
                };
                std::format!("{:?}", self.client.try_update_plan_template(&address, &target_plan_id, &15_000_000, &2592000, &true, &None))
            }
            Operation::SetPlanMaxActiveSubs => {
                let target_plan_id = if is_allowed {
                    self.env.mock_all_auths();
                    let pid = self.client.create_plan_template(&address, &10_000_000, &2592000, &true, &None);
                    if !is_allowed { self.env.mock_auths(&[]); }
                    pid
                } else {
                    self.env.mock_auths(&[]);
                    self.plan_id // owned by self.merchant, so non-merchants get Forbidden
                };
                std::format!("{:?}", self.client.try_set_plan_max_active_subs(&address, &target_plan_id, &5))
            }
            Operation::MigrateSubscriptionToPlan => {
                // Build a fresh plan family and sub so every role iteration is independent.
                self.env.mock_all_auths();
                let fresh_plan_id = self.client.create_plan_template(&self.merchant, &10_000_000, &2592000, &true, &None);
                let fresh_sub_id = self.client.create_subscription_from_plan(&self.subscriber, &fresh_plan_id);
                let new_plan_id = self.client.update_plan_template(&self.merchant, &fresh_plan_id, &12_000_000, &2592000, &true, &None);
                if !is_allowed { self.env.mock_auths(&[]); }
                std::format!("{:?}", self.client.try_migrate_subscription_to_plan(&address, &fresh_sub_id, &new_plan_id))
            }
            Operation::CleanupSubscription => {
                // Create a fresh sub, cancel it (sets terminal state), then try cleanup.
                // Subs created from plan templates have no expires_at so we cancel to reach
                // a terminal state that cleanup_subscription can archive.
                self.env.mock_all_auths();
                let cleanup_sub_id = self.client.create_subscription_from_plan(&self.subscriber, &self.plan_id);
                self.client.cancel_subscription(&cleanup_sub_id, &self.subscriber);
                if !is_allowed { self.env.mock_auths(&[]); }
                let r = std::format!("{:?}", self.client.try_cleanup_subscription(&cleanup_sub_id, &address));
                r
            }
            Operation::ChargeUsage => {
                // Deposit funds first so the balance check passes (no auth gate on charge_usage).
                self.env.mock_all_auths();
                let _ = self.client.try_deposit_funds(&self.subscription_id, &self.subscriber, &10_000_000);
                // charge_usage has no caller auth check — it succeeds/fails on business logic only.
                // Restore mock state for the actual call.
                if !is_allowed {
                    self.env.mock_auths(&[]);
                } else {
                    self.env.mock_all_auths();
                }
                std::format!("{:?}", self.client.try_charge_usage(&self.subscription_id, &1_000_000))
            }
            Operation::WithdrawMerchantTokenFunds => {
                self.env.mock_all_auths();
                let _ = self.client.try_deposit_funds(&self.subscription_id, &self.subscriber, &50_000_000);
                let _ = self.client.try_charge_one_off(&self.subscription_id, &self.merchant, &10_000_000);
                if !is_allowed { self.env.mock_auths(&[]); }
                std::format!("{:?}", self.client.try_withdraw_merchant_token_funds(&address, &self.token, &1_000_000))
            }
            Operation::SetMetadata => {
                let k = soroban_sdk::String::from_str(env, "test_key");
                let v = soroban_sdk::String::from_str(env, "test_val");
                std::format!("{:?}", self.client.try_set_metadata(&self.subscription_id, &address, &k, &v))
            }
            Operation::DeleteMetadata => {
                let k = soroban_sdk::String::from_str(env, "test_key");
                let v = soroban_sdk::String::from_str(env, "test_val");
                self.env.mock_all_auths();
                self.client.set_metadata(&self.subscription_id, &self.subscriber, &k, &v);
                if !is_allowed { self.env.mock_auths(&[]); }
                std::format!("{:?}", self.client.try_delete_metadata(&self.subscription_id, &address, &k))
            }
            Operation::AddToBlocklist => {
                std::format!("{:?}", self.client.try_add_to_blocklist(&address, &self.subscriber, &None))
            }
            Operation::RemoveFromBlocklist => {
                self.env.mock_all_auths();
                self.client.add_to_blocklist(&self.admin, &self.subscriber, &None);
                if !is_allowed { self.env.mock_auths(&[]); }
                std::format!("{:?}", self.client.try_remove_from_blocklist(&address, &self.subscriber))
            }
        };

        if res.contains("Ok(Ok(") || res.contains("Ok(())") {
            Ok(())
        } else {
            Err(res)
        }
    }

    pub fn is_allowed(&self, op: Operation, caller: Role) -> bool {
        match op {
            Operation::SetMinTopup | Operation::RotateAdmin | Operation::EnableEmergencyStop | 
            Operation::DisableEmergencyStop | Operation::PartialRefund | Operation::BatchCharge | 
            Operation::AddAcceptedToken | Operation::RemoveAcceptedToken | Operation::SetSubscriberCreditLimit |
            Operation::ExportContractSnapshot | Operation::SetProtocolFee | Operation::RecoverStrandedFunds |
            Operation::SetBillingRetention | Operation::CompactBillingStatements | Operation::SetOracleConfig => {
                caller == Role::Admin
            }
            Operation::DepositFunds | Operation::WithdrawSubscriberFunds => {
                caller == Role::Subscriber
            }
            Operation::CancelSubscription | Operation::PauseSubscription | Operation::ResumeSubscription |
            Operation::CleanupSubscription | Operation::SetMetadata | Operation::DeleteMetadata => {
                caller == Role::Subscriber || caller == Role::Merchant
            }
            Operation::ChargeOneOff | Operation::WithdrawMerchantFunds | Operation::PauseMerchant | 
            Operation::UnpauseMerchant | Operation::ConfigureUsageLimits | Operation::SetMerchantConfig |
            Operation::CreatePlanTemplateWithToken | Operation::UpdatePlanTemplate | Operation::SetPlanMaxActiveSubs |
            Operation::WithdrawMerchantTokenFunds => {
                caller == Role::Merchant
            }
            Operation::MerchantRefund => {
                caller == Role::Merchant
            }
            Operation::MigrateSubscriptionToPlan => {
                caller == Role::Subscriber
            }
            Operation::ChargeUsage => {
                true // Current implementation has no auth checks
            }
            Operation::AddToBlocklist => {
                caller == Role::Admin || caller == Role::Merchant
            }
            Operation::RemoveFromBlocklist => {
                caller == Role::Admin
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn test_authorization_matrix_fuzz() {
    for &op in Operation::all() {
        for &role in Role::all() {
            let harness = FuzzHarness::setup();
            let result = harness.execute(op, role);
            let expected_allowed = harness.is_allowed(op, role);
            
            match (expected_allowed, result) {
                (true, Ok(())) => {}, 
                (true, Err(e)) => {
                    std::panic!("Operation {:?} should be allowed for role {:?}, but failed! Outcome: {}", op, role, e);
                }
                (false, Ok(())) => {
                    std::panic!("Operation {:?} should NOT be allowed for role {:?}, but succeeded!", op, role);
                }
                (false, Err(_)) => {}
            }
        }
    }
}

#[test]
fn test_admin_rotation_edge_case() {
    let harness = FuzzHarness::setup();
    let old_admin = harness.admin.clone();
    let new_admin = harness.new_admin.clone();
    
    harness.env.mock_all_auths();
    harness.client.rotate_admin(&old_admin, &new_admin);
    
    let res = harness.client.try_set_min_topup(&old_admin, &3_000_000);
    assert!(res.is_err(), "Old admin should no longer be authorized after rotation");
    
    let res_new = harness.client.try_set_min_topup(&new_admin, &4_000_000);
    assert!(res_new.is_ok(), "New admin should be authorized after rotation");
}

#[test]
fn test_identity_collision_subscriber_is_merchant() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
    let person = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env); 
    
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
        
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    
    let plan_id = client.create_plan_template(&person, &10_000_000, &2592000, &false, &None::<i128>);
    let sub_id = client.create_subscription_from_plan(&person, &plan_id);
    
    client.pause_subscription(&sub_id, &person);
    
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Paused);
}
