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

        let admin = Address::generate(&env);
        let subscriber = Address::generate(&env);
        let merchant = Address::generate(&env);
        let stranger = Address::generate(&env);
        let new_admin = Address::generate(&env);
        
        let token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
            
        client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
        
        // Use true for usage_tracking_enabled
        let plan_id = client.create_plan_template(&merchant, &10_000_000, &2592000, &true, &None);
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
            Operation::ExportContractSnapshot => {
                caller == Role::Admin
            }
            Operation::DepositFunds | Operation::WithdrawSubscriberFunds => {
                caller == Role::Subscriber
            }
            Operation::CancelSubscription | Operation::PauseSubscription | Operation::ResumeSubscription => {
                caller == Role::Subscriber || caller == Role::Merchant
            }
            Operation::ChargeOneOff | Operation::WithdrawMerchantFunds | Operation::PauseMerchant | 
            Operation::UnpauseMerchant | Operation::ConfigureUsageLimits => {
                caller == Role::Merchant
            }
            Operation::MerchantRefund => {
                caller == Role::Merchant
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

    let admin = Address::generate(&env);
    let person = Address::generate(&env); 
    
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
        
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));
    
    let plan_id = client.create_plan_template(&person, &10_000_000, &2592000, &false, &None);
    let sub_id = client.create_subscription_from_plan(&person, &plan_id);
    
    client.pause_subscription(&sub_id, &person);
    
    let sub = client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Paused);
}
