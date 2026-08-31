//! Multi-token withdrawal isolation test (#590).
//!
//! `merchant.rs` stores per-merchant balances keyed by `(merchant, token)`
//! (see `merchant_balance_key` / `get_merchant_balance_by_token` /
//! `set_merchant_balance`), and several shared helpers
//! (`credit_merchant_balance_for_token`, `withdraw_merchant_funds_for_token`,
//! `dispute::do_open_dispute`) mutate that keyed storage on behalf of *all*
//! tokens a merchant is accepted for. A regression in any of these shared
//! helpers (e.g. an accidental fallback to the contract's default token via
//! `get_merchant_balance` instead of `get_merchant_balance_by_token`) could
//! silently debit or credit the wrong token's balance.
//!
//! This suite funds a single merchant across three independent tokens (A, B,
//! C), withdraws token A, and asserts:
//!
//! 1. Token A's merchant balance drops by exactly the withdrawn amount.
//! 2. Token B's and token C's merchant balances are byte-for-byte unchanged.
//! 3. The vault contract's on-chain token balance for B and C is unchanged,
//!    while A's decreases by exactly the withdrawn amount (conservation).
//! 4. The behavior holds under three edge cases required by the issue:
//!    - withdrawing an amount equal to the full token A balance,
//!    - withdrawing token A while token B has funds locked in an open
//!      dispute escrow, and
//!    - repeated withdrawals of token A.
//!
//! # Security notes
//!
//! - `withdraw_merchant_funds_for_token` reads/writes storage exclusively
//!   through `merchant_balance_key(merchant, token)`, so in the current
//!   implementation there is no code path by which withdrawing token A can
//!   touch token B/C's balance key. This suite exists to pin that guarantee
//!   as a regression test, since the balance/earnings/accounting helpers are
//!   shared across tokens and a future edit could easily reintroduce a
//!   token-agnostic shortcut (e.g. calling `get_merchant_balance`, which
//!   hard-codes the contract's *default* token, from a code path that should
//!   be token-parameterized).
//! - Dispute escrow (`dispute::do_open_dispute`) debits the merchant's
//!   balance for the *disputed subscription's token only*, before any
//!   interaction; this suite confirms that debit is not visible to, or
//!   perturbed by, withdrawals of a different token.
//! - Separately, while reviewing `merchant.rs` for this test we observed that
//!   `withdraw_merchant_funds_for_token` increments `earnings.refunds` (in
//!   addition to `earnings.withdrawals`) on every withdrawal, and calls
//!   `set_merchant_balance` twice in a row. Neither affects cross-token
//!   isolation (both operate on the single token being withdrawn), so they
//!   are out of scope for this test, but are worth a follow-up ticket.

#![cfg(test)]

use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env, String,
};

const INTERVAL: u64 = 30 * 24 * 60 * 60;
const COOLDOWN: u64 = 6 * 60 * 60 + 1; // CONFIG_COOLDOWN_SECS + 1, from admin.rs

/// Amounts are distinct per token so any cross-token bleed is immediately
/// visible in the assertions (equal amounts could mask a swapped-token bug).
const AMOUNT_A: i128 = 10_000_000;
const AMOUNT_B: i128 = 20_000_000;
const AMOUNT_C: i128 = 30_000_000;

struct Fixture {
    env: Env,
    client: SubscriptionVaultClient<'static>,
    merchant: Address,
    token_a: Address,
    token_b: Address,
    token_c: Address,
}

/// Sets up a contract with three accepted tokens and one merchant, with the
/// merchant's balance funded to `AMOUNT_A` / `AMOUNT_B` / `AMOUNT_C` in
/// tokens A / B / C respectively via the standard create → deposit → charge
/// flow (so the funding path exercises real public entrypoints, not
/// test-only shortcuts).
fn setup() -> Fixture {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000_000);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_c = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token_a, &7, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    // Adding accepted tokens shares a single admin-config cooldown key
    // ("AcceptedTokens"), so back-to-back calls must be spaced out or the
    // second call fails with Error::CooldownActive.
    client.add_accepted_token(&admin, &token_b, &7);
    env.ledger().set_timestamp(env.ledger().timestamp() + COOLDOWN);
    client.add_accepted_token(&admin, &token_c, &7);

    let merchant = Address::generate(&env);
    let payout_address = merchant.clone();
    let redirect_url = String::from_str(&env, "https://stellabill.io/success");
    client.initialize_merchant_config(
        &merchant,
        &payout_address,
        &0i32,   // no platform fee, so charged amount == merchant credit
        &0x1F,   // all operations enabled
        &None::<Address>,
        &redirect_url,
    );

    let fixture = Fixture {
        env,
        client,
        merchant,
        token_a,
        token_b,
        token_c,
    };

    fund_merchant_balance(&fixture, &fixture.token_a, AMOUNT_A);
    fund_merchant_balance(&fixture, &fixture.token_b, AMOUNT_B);
    fund_merchant_balance(&fixture, &fixture.token_c, AMOUNT_C);

    fixture
}

/// Credits the merchant's balance for `token` by `amount` via a real
/// subscription lifecycle: mint funds to a fresh subscriber, create a
/// subscription in `token`, deposit the prepaid balance, and charge it once
/// the interval elapses.
fn fund_merchant_balance(f: &Fixture, token: &Address, amount: i128) {
    let subscriber = Address::generate(&f.env);
    token::StellarAssetClient::new(&f.env, token).mint(&subscriber, &amount);

    let id = f.client.create_subscription_with_token(
        &subscriber,
        &f.merchant,
        token,
        &amount,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    f.client
        .deposit_funds(&id, &subscriber, &amount, &None::<soroban_sdk::BytesN<32>>);

    let now = f.env.ledger().timestamp();
    f.env.ledger().set_timestamp(now + INTERVAL + 1);
    f.client.charge_subscription(&id, &None::<soroban_sdk::BytesN<32>>);
}

/// Snapshot of everything that must stay conserved / isolated across a
/// withdrawal of a *different* token.
struct Balances {
    merchant_a: i128,
    merchant_b: i128,
    merchant_c: i128,
    vault_a: i128,
    vault_b: i128,
    vault_c: i128,
}

fn snapshot(f: &Fixture) -> Balances {
    Balances {
        merchant_a: f.client.get_merchant_balance_by_token(&f.merchant, &f.token_a),
        merchant_b: f.client.get_merchant_balance_by_token(&f.merchant, &f.token_b),
        merchant_c: f.client.get_merchant_balance_by_token(&f.merchant, &f.token_c),
        vault_a: token::Client::new(&f.env, &f.token_a).balance(&f.client.address),
        vault_b: token::Client::new(&f.env, &f.token_b).balance(&f.client.address),
        vault_c: token::Client::new(&f.env, &f.token_c).balance(&f.client.address),
    }
}

/// Core case: withdrawing the *entire* token A balance must not move token
/// B or token C's merchant balance, nor the vault's B/C token holdings.
#[test]
fn withdraw_full_token_a_balance_isolates_b_and_c() {
    let f = setup();
    let before = snapshot(&f);
    assert_eq!(before.merchant_a, AMOUNT_A);
    assert_eq!(before.merchant_b, AMOUNT_B);
    assert_eq!(before.merchant_c, AMOUNT_C);

    f.client
        .withdraw_merchant_token_funds(&f.merchant, &f.token_a, &AMOUNT_A);

    let after = snapshot(&f);

    // Token A: fully drained, both in merchant accounting and vault custody.
    assert_eq!(after.merchant_a, 0, "token A merchant balance must be zero");
    assert_eq!(
        after.vault_a,
        before.vault_a - AMOUNT_A,
        "vault token A balance must decrease by exactly the withdrawn amount"
    );

    // Token B / C: completely untouched (isolation).
    assert_eq!(
        after.merchant_b, before.merchant_b,
        "token B merchant balance must be unaffected by a token A withdrawal"
    );
    assert_eq!(
        after.merchant_c, before.merchant_c,
        "token C merchant balance must be unaffected by a token A withdrawal"
    );
    assert_eq!(
        after.vault_b, before.vault_b,
        "vault token B balance must be unaffected by a token A withdrawal"
    );
    assert_eq!(
        after.vault_c, before.vault_c,
        "vault token C balance must be unaffected by a token A withdrawal"
    );

    // Merchant's own wallet only received token A funds.
    let token_a_client = token::Client::new(&f.env, &f.token_a);
    let token_b_client = token::Client::new(&f.env, &f.token_b);
    let token_c_client = token::Client::new(&f.env, &f.token_c);
    assert_eq!(token_a_client.balance(&f.merchant), AMOUNT_A);
    assert_eq!(token_b_client.balance(&f.merchant), 0);
    assert_eq!(token_c_client.balance(&f.merchant), 0);
}

/// Edge case: token B has funds locked in an open dispute escrow (moved out
/// of the merchant's B balance) *before* the token A withdrawal happens. The
/// A withdrawal must not disturb the already-escrowed B funds, the
/// remaining B merchant balance, or C at all.
#[test]
fn withdraw_token_a_with_pending_token_b_dispute_isolates_balances() {
    let f = setup();

    // Fund a dedicated subscription in token B whose charge we can dispute,
    // in addition to the balance already funded by `setup`.
    let subscriber_b = Address::generate(&f.env);
    token::StellarAssetClient::new(&f.env, &f.token_b).mint(&subscriber_b, &AMOUNT_B);
    let sub_b_id = f.client.create_subscription_with_token(
        &subscriber_b,
        &f.merchant,
        &f.token_b,
        &AMOUNT_B,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
        &None::<u32>,
        &None::<soroban_sdk::Symbol>,
    );
    f.client.deposit_funds(
        &sub_b_id,
        &subscriber_b,
        &AMOUNT_B,
        &None::<soroban_sdk::BytesN<32>>,
    );
    let now = f.env.ledger().timestamp();
    f.env.ledger().set_timestamp(now + INTERVAL + 1);
    f.client
        .charge_subscription(&sub_b_id, &None::<soroban_sdk::BytesN<32>>);

    let total_b_before_dispute = f.client.get_merchant_balance_by_token(&f.merchant, &f.token_b);
    assert_eq!(total_b_before_dispute, AMOUNT_B + AMOUNT_B);

    let dispute_amount = AMOUNT_B / 4;
    let dispute_id = f
        .client
        .open_dispute(&subscriber_b, &sub_b_id, &dispute_amount, &None::<soroban_sdk::BytesN<32>>);

    let expected_b_after_dispute = total_b_before_dispute - dispute_amount;
    assert_eq!(
        f.client.get_merchant_balance_by_token(&f.merchant, &f.token_b),
        expected_b_after_dispute,
        "opening the dispute must debit exactly the disputed amount from token B"
    );

    let before = snapshot(&f);
    assert_eq!(before.merchant_b, expected_b_after_dispute);
    assert_eq!(before.merchant_c, AMOUNT_C);

    // Now withdraw the entire token A balance.
    f.client
        .withdraw_merchant_token_funds(&f.merchant, &f.token_a, &AMOUNT_A);

    let after = snapshot(&f);
    assert_eq!(after.merchant_a, 0);
    assert_eq!(
        after.merchant_b, before.merchant_b,
        "token B balance (net of the pending dispute) must be unaffected by the token A withdrawal"
    );
    assert_eq!(
        after.merchant_c, before.merchant_c,
        "token C balance must be unaffected by the token A withdrawal"
    );
    assert_eq!(after.vault_b, before.vault_b);
    assert_eq!(after.vault_c, before.vault_c);

    // The dispute itself must still be open and untouched by the unrelated
    // token A withdrawal.
    let dispute = f.client.get_dispute(&dispute_id);
    assert_eq!(dispute.amount, dispute_amount);
    assert_eq!(dispute.subscription_id, sub_b_id);
}

/// Edge case: repeated partial withdrawals of token A must never leak into
/// B or C, and the vault's A balance must track the cumulative withdrawn
/// amount exactly (no double-spend, no drift).
#[test]
fn repeated_token_a_withdrawals_do_not_leak_into_b_or_c() {
    let f = setup();
    let initial = snapshot(&f);

    let first = AMOUNT_A / 3;
    let second = AMOUNT_A / 3;
    let third = AMOUNT_A - first - second; // remainder, drains A to zero

    let mut withdrawn_so_far: i128 = 0;
    for amount in [first, second, third] {
        f.client
            .withdraw_merchant_token_funds(&f.merchant, &f.token_a, &amount);
        withdrawn_so_far += amount;

        let snap = snapshot(&f);
        assert_eq!(
            snap.merchant_a,
            AMOUNT_A - withdrawn_so_far,
            "token A merchant balance must reflect cumulative withdrawals"
        );
        assert_eq!(
            snap.vault_a,
            initial.vault_a - withdrawn_so_far,
            "vault token A balance must reflect cumulative withdrawals (conservation)"
        );
        assert_eq!(
            snap.merchant_b, initial.merchant_b,
            "token B merchant balance must never change across repeated token A withdrawals"
        );
        assert_eq!(
            snap.merchant_c, initial.merchant_c,
            "token C merchant balance must never change across repeated token A withdrawals"
        );
        assert_eq!(snap.vault_b, initial.vault_b);
        assert_eq!(snap.vault_c, initial.vault_c);
    }

    assert_eq!(f.client.get_merchant_balance_by_token(&f.merchant, &f.token_a), 0);

    // A 4th withdrawal against the now-empty token A balance must fail
    // cleanly, and must still leave B/C untouched.
    let err = f
        .client
        .try_withdraw_merchant_token_funds(&f.merchant, &f.token_a, &1i128);
    assert!(err.is_err(), "withdrawing from a zero token A balance must fail");

    let final_snap = snapshot(&f);
    assert_eq!(final_snap.merchant_b, initial.merchant_b);
    assert_eq!(final_snap.merchant_c, initial.merchant_c);
    assert_eq!(final_snap.vault_b, initial.vault_b);
    assert_eq!(final_snap.vault_c, initial.vault_c);
}
