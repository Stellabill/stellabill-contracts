#![cfg(test)]

extern crate alloc;

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    contract, contractimpl,
    testutils::{Address as _, Ledger},
    vec, Address, Env, IntoVal, String, Symbol, Val, Vec,
};
use subscription_vault::{SubscriptionVault, SubscriptionVaultClient, SubscriptionStatus};
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient as TokenAdminClient};

#[contract]
pub struct ChannelAccountContract;

#[contractimpl]
impl ChannelAccountContract {
    pub fn exec(
        env: Env,
        target: Address,
        func: Symbol,
        args: Vec<Val>,
        auth_entries: Vec<InvokerContractAuthEntry>,
    ) -> Val {
        env.authorize_as_current_contract(&auth_entries);
        env.invoke_contract(&target, &func, args)
    }

    pub fn exec_twice(
        env: Env,
        target: Address,
        func: Symbol,
        args: Vec<Val>,
        auth_entries: Vec<InvokerContractAuthEntry>,
    ) -> Val {
        env.authorize_as_current_contract(&auth_entries);
        env.invoke_contract::<Val>(&target, &func, args.clone());
        env.invoke_contract::<Val>(&target, &func, args)
    }
}

pub struct ChannelClient<'a> {
    pub env: &'a Env,
    pub address: Address,
}

impl<'a> ChannelClient<'a> {
    pub fn new(env: &'a Env, address: Address) -> Self {
        Self { env, address }
    }

    pub fn exec(
        &self,
        target: &Address,
        func: &Symbol,
        args: &Vec<Val>,
        auth_entries: &Vec<InvokerContractAuthEntry>,
    ) -> Val {
        let exec_args = (
            target.clone(),
            func.clone(),
            args.clone(),
            auth_entries.clone(),
        ).into_val(self.env);
        self.env.invoke_contract(&self.address, &Symbol::new(self.env, "exec"), exec_args)
    }

    pub fn exec_twice(
        &self,
        target: &Address,
        func: &Symbol,
        args: &Vec<Val>,
        auth_entries: &Vec<InvokerContractAuthEntry>,
    ) -> Val {
        let exec_args = (
            target.clone(),
            func.clone(),
            args.clone(),
            auth_entries.clone(),
        ).into_val(self.env);
        self.env.invoke_contract(&self.address, &Symbol::new(self.env, "exec_twice"), exec_args)
    }
}

fn create_token_contract<'a>(
    env: &Env,
    admin: &Address,
) -> (TokenClient<'a>, TokenAdminClient<'a>) {
    let contract_address = env.register_stellar_asset_contract_v2(admin.clone()).address();
    (
        TokenClient::new(env, &contract_address),
        TokenAdminClient::new(env, &contract_address),
    )
}

macro_rules! setup {
    () => {
        let env = Env::default();
        let token_admin_addr = Address::generate(&env);
        let (token, token_admin) = create_token_contract(&env, &token_admin_addr);

        let admin = env.register(ChannelAccountContract, ());
        let subscriber = env.register(ChannelAccountContract, ());
        let merchant = env.register(ChannelAccountContract, ());

        let vault_id = env.register(SubscriptionVault, ());
        let vault = SubscriptionVaultClient::new(&env, &vault_id);

        let min_topup = 1_000_000;
        let grace_period = 3 * 24 * 60 * 60;
        
        vault.init(
            &token.address,
            &7,
            &admin,
            &min_topup,
            &grace_period,
        );

        let redirect_url = String::from_str(&env, "https://example.com");
        
        let merchant_client = ChannelClient::new(&env, merchant.clone());
        let init_args = vec![
            &env,
            merchant.clone().into_val(&env),
            merchant.clone().into_val(&env),
            0i128.into_val(&env),
            0x1Fu32.into_val(&env),
            Option::<Address>::None.into_val(&env),
            redirect_url.into_val(&env),
        ];
        let init_auth = vec![&env, InvokerContractAuthEntry::Contract(SubContractInvocation {
            context: ContractContext {
                contract: vault_id.clone(),
                fn_name: Symbol::new(&env, "initialize_merchant_config"),
                args: init_args.clone(),
            },
            sub_invocations: vec![&env],
        })];
        merchant_client.exec(&vault_id, &Symbol::new(&env, "initialize_merchant_config"), &init_args, &init_auth);

        token_admin.mint(&subscriber, &10_000_000_000);
        let sub_client = ChannelClient::new(&env, subscriber.clone());
        let admin_client = ChannelClient::new(&env, admin.clone());
    };
}

/// Tests a full positive lifecycle using channel accounts.
#[test]
fn test_positive_lifecycle() {
    setup!();

    // 1. Create plan
    let plan_args = vec![
        &env,
        merchant.clone().into_val(&env),
        5_000_000i128.into_val(&env),
        (30 * 24 * 60 * 60u64).into_val(&env),
        false.into_val(&env),
        Option::<i128>::None.into_val(&env),
    ];
    let plan_auth = vec![&env, InvokerContractAuthEntry::Contract(SubContractInvocation {
        context: ContractContext {
            contract: vault_id.clone(),
            fn_name: Symbol::new(&env, "create_plan_template"),
            args: plan_args.clone(),
        },
        sub_invocations: vec![&env],
    })];
    let plan_id: u32 = merchant_client.exec(&vault_id, &Symbol::new(&env, "create_plan_template"), &plan_args, &plan_auth).into_val(&env);

    // 2. Create subscription
    let sub_args = vec![
        &env,
        subscriber.clone().into_val(&env),
        plan_id.into_val(&env),
    ];
    let sub_auth = vec![&env, InvokerContractAuthEntry::Contract(SubContractInvocation {
        context: ContractContext {
            contract: vault_id.clone(),
            fn_name: Symbol::new(&env, "create_subscription_from_plan"),
            args: sub_args.clone(),
        },
        sub_invocations: vec![&env],
    })];
    let sub_id: u32 = sub_client.exec(&vault_id, &Symbol::new(&env, "create_subscription_from_plan"), &sub_args, &sub_auth).into_val(&env);

    // 3. Deposit funds (needs token transfer sub-invocation)
    let deposit_amount: i128 = 15_000_000;
    let transfer_args = vec![
        &env,
        subscriber.clone().into_val(&env),
        vault_id.clone().into_val(&env),
        deposit_amount.into_val(&env),
    ];
    let token_sub = SubContractInvocation {
        context: ContractContext {
            contract: token.address.clone(),
            fn_name: Symbol::new(&env, "transfer"),
            args: transfer_args,
        },
        sub_invocations: vec![&env],
    };
    let deposit_args = vec![
        &env,
        sub_id.into_val(&env),
        subscriber.clone().into_val(&env),
        deposit_amount.into_val(&env),
    ];
    let deposit_auth = vec![&env, InvokerContractAuthEntry::Contract(SubContractInvocation {
        context: ContractContext {
            contract: vault_id.clone(),
            fn_name: Symbol::new(&env, "deposit_funds"),
            args: deposit_args.clone(),
        },
        sub_invocations: vec![&env, token_sub],
    })];
    sub_client.exec(&vault_id, &Symbol::new(&env, "deposit_funds"), &deposit_args, &deposit_auth);

    // Assert balances after deposit
    assert_eq!(token.balance(&subscriber), 10_000_000_000 - deposit_amount);
    assert_eq!(token.balance(&vault_id), deposit_amount);

    // 4. Charge subscription
    env.ledger().set_timestamp(env.ledger().timestamp() + (30 * 24 * 60 * 60) + 1);
    let charge_args = vec![&env, sub_id.into_val(&env)];
    let charge_auth = vec![&env, InvokerContractAuthEntry::Contract(SubContractInvocation {
        context: ContractContext {
            contract: vault_id.clone(),
            fn_name: Symbol::new(&env, "charge_subscription"),
            args: charge_args.clone(),
        },
        sub_invocations: vec![&env],
    })];
    admin_client.exec(&vault_id, &Symbol::new(&env, "charge_subscription"), &charge_args, &charge_auth);

    // Assert balances after charge
    assert_eq!(vault.get_merchant_balance(&merchant), 5_000_000);
    assert_eq!(token.balance(&vault_id), deposit_amount); // Merchant hasn't withdrawn yet

    // 5. Withdraw merchant funds
    let withdraw_args = vec![&env, merchant.clone().into_val(&env), 5_000_000i128.into_val(&env)];
    let withdraw_auth = vec![&env, InvokerContractAuthEntry::Contract(SubContractInvocation {
        context: ContractContext {
            contract: vault_id.clone(),
            fn_name: Symbol::new(&env, "withdraw_merchant_funds"),
            args: withdraw_args.clone(),
        },
        sub_invocations: vec![&env],
    })];
    merchant_client.exec(&vault_id, &Symbol::new(&env, "withdraw_merchant_funds"), &withdraw_args, &withdraw_auth);

    // Assert balances after withdraw
    assert_eq!(token.balance(&merchant), 5_000_000);
    assert_eq!(token.balance(&vault_id), deposit_amount - 5_000_000);
}

/// Tests that the subscriber can cancel their own subscription.
#[test]
fn test_cancel_subscriber() {
    setup!();
    let amount = 5_000_000i128;
    let create_args = vec![
        &env,
        subscriber.clone().into_val(&env),
        merchant.clone().into_val(&env),
        amount.into_val(&env),
        (30 * 24 * 60 * 60u64).into_val(&env),
        false.into_val(&env),
        Option::<i128>::None.into_val(&env),
    ];
    let create_auth = vec![&env, InvokerContractAuthEntry::Contract(SubContractInvocation {
        context: ContractContext {
            contract: vault_id.clone(),
            fn_name: Symbol::new(&env, "create_subscription"),
            args: create_args.clone(),
        },
        sub_invocations: vec![&env],
    })];
    let sub_id: u32 = sub_client.exec(&vault_id, &Symbol::new(&env, "create_subscription"), &create_args, &create_auth).into_val(&env);

    let cancel_args = vec![
        &env,
        sub_id.into_val(&env),
        subscriber.clone().into_val(&env),
    ];
    let cancel_auth = vec![&env, InvokerContractAuthEntry::Contract(SubContractInvocation {
        context: ContractContext {
            contract: vault_id.clone(),
            fn_name: Symbol::new(&env, "cancel_subscription"),
            args: cancel_args.clone(),
        },
        sub_invocations: vec![&env],
    })];
    sub_client.exec(&vault_id, &Symbol::new(&env, "cancel_subscription"), &cancel_args, &cancel_auth);

    assert_eq!(vault.get_subscription(&sub_id).status, SubscriptionStatus::Cancelled);
}

/// Tests that the merchant can cancel the subscriber's subscription.
#[test]
fn test_cancel_merchant() {
    setup!();
    let amount = 5_000_000i128;
    let create_args = vec![
        &env,
        subscriber.clone().into_val(&env),
        merchant.clone().into_val(&env),
        amount.into_val(&env),
        (30 * 24 * 60 * 60u64).into_val(&env),
        false.into_val(&env),
        Option::<i128>::None.into_val(&env),
    ];
    let create_auth = vec![&env, InvokerContractAuthEntry::Contract(SubContractInvocation {
        context: ContractContext {
            contract: vault_id.clone(),
            fn_name: Symbol::new(&env, "create_subscription"),
            args: create_args.clone(),
        },
        sub_invocations: vec![&env],
    })];
    let sub_id: u32 = sub_client.exec(&vault_id, &Symbol::new(&env, "create_subscription"), &create_args, &create_auth).into_val(&env);

    let cancel_args = vec![
        &env,
        sub_id.into_val(&env),
        merchant.clone().into_val(&env),
    ];
    let cancel_auth = vec![&env, InvokerContractAuthEntry::Contract(SubContractInvocation {
        context: ContractContext {
            contract: vault_id.clone(),
            fn_name: Symbol::new(&env, "cancel_subscription"),
            args: cancel_args.clone(),
        },
        sub_invocations: vec![&env],
    })];
    merchant_client.exec(&vault_id, &Symbol::new(&env, "cancel_subscription"), &cancel_args, &cancel_auth);

    assert_eq!(vault.get_subscription(&sub_id).status, SubscriptionStatus::Cancelled);
}

/// Negative test: Mismatched signed args.
#[test]
#[should_panic(expected = "HostError")]
fn test_mismatched_signed_args() {
    setup!();
    let amount_auth = 5_000_000i128;
    let amount_call = 10_000_000i128;

    let auth_args = vec![
        &env,
        subscriber.clone().into_val(&env),
        merchant.clone().into_val(&env),
        amount_auth.into_val(&env),
        (30 * 24 * 60 * 60u64).into_val(&env),
        false.into_val(&env),
        Option::<i128>::None.into_val(&env),
    ];
    let create_auth = vec![&env, InvokerContractAuthEntry::Contract(SubContractInvocation {
        context: ContractContext {
            contract: vault_id.clone(),
            fn_name: Symbol::new(&env, "create_subscription"),
            args: auth_args.clone(),
        },
        sub_invocations: vec![&env],
    })];

    let call_args = vec![
        &env,
        subscriber.clone().into_val(&env),
        merchant.clone().into_val(&env),
        amount_call.into_val(&env), // Mismatch here!
        (30 * 24 * 60 * 60u64).into_val(&env),
        false.into_val(&env),
        Option::<i128>::None.into_val(&env),
    ];

    // This should panic due to auth mismatch
    sub_client.exec(&vault_id, &Symbol::new(&env, "create_subscription"), &call_args, &create_auth);
}

/// Negative test: Missing nested token transfer sub-invocation.
#[test]
#[should_panic(expected = "HostError")]
fn test_missing_nested_token_transfer() {
    setup!();
    let create_args = vec![
        &env,
        subscriber.clone().into_val(&env),
        merchant.clone().into_val(&env),
        5_000_000i128.into_val(&env),
        (30 * 24 * 60 * 60u64).into_val(&env),
        false.into_val(&env),
        Option::<i128>::None.into_val(&env),
    ];
    let create_auth = vec![&env, InvokerContractAuthEntry::Contract(SubContractInvocation {
        context: ContractContext {
            contract: vault_id.clone(),
            fn_name: Symbol::new(&env, "create_subscription"),
            args: create_args.clone(),
        },
        sub_invocations: vec![&env],
    })];
    let sub_id: u32 = sub_client.exec(&vault_id, &Symbol::new(&env, "create_subscription"), &create_args, &create_auth).into_val(&env);

    let deposit_amount = 15_000_000i128;
    let deposit_args = vec![
        &env,
        sub_id.into_val(&env),
        subscriber.clone().into_val(&env),
        deposit_amount.into_val(&env),
    ];
    let deposit_auth = vec![&env, InvokerContractAuthEntry::Contract(SubContractInvocation {
        context: ContractContext {
            contract: vault_id.clone(),
            fn_name: Symbol::new(&env, "deposit_funds"),
            args: deposit_args.clone(),
        },
        // Mismatch here! Missing the token transfer sub-invocation.
        sub_invocations: vec![&env],
    })];
    
    // This should panic because the vault attempts to transfer tokens without auth
    sub_client.exec(&vault_id, &Symbol::new(&env, "deposit_funds"), &deposit_args, &deposit_auth);
}

/// Negative test: Replaying an already-used auth entry.
#[test]
#[should_panic(expected = "HostError")]
fn test_replay_used_auth_entry() {
    setup!();
    let create_args = vec![
        &env,
        subscriber.clone().into_val(&env),
        merchant.clone().into_val(&env),
        5_000_000i128.into_val(&env),
        (30 * 24 * 60 * 60u64).into_val(&env),
        false.into_val(&env),
        Option::<i128>::None.into_val(&env),
    ];
    let create_auth = vec![&env, InvokerContractAuthEntry::Contract(SubContractInvocation {
        context: ContractContext {
            contract: vault_id.clone(),
            fn_name: Symbol::new(&env, "create_subscription"),
            args: create_args.clone(),
        },
        sub_invocations: vec![&env],
    })];
    
    // Calling exec_twice will invoke the same function twice within the same authorization context.
    // The first invoke_contract will consume the auth entry, causing the second invoke_contract to fail.
    sub_client.exec_twice(&vault_id, &Symbol::new(&env, "create_subscription"), &create_args, &create_auth);
}
