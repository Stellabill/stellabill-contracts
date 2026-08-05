#![cfg(test)]

//! Snapshot test: catches accidental DataKey layout drift.
//!
//! Any change to discriminant values or the KNOWN_INSTANCE_KEY_DISCRIMINANTS
//! array will break these tests, requiring a deliberate update and review
//! of on-chain migration impact.

use crate::types::{DataKey, KycKey, KNOWN_INSTANCE_KEY_DISCRIMINANTS};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, Env, String, Symbol};

fn env() -> Env {
    Env::default()
}

// ── canonical_discriminant snapshot ──────────────────────────────────────────

#[test]
fn test_datakey_discriminants_snapshot() {
    let env = env();
    let addr = Address::generate(&env);
    let addr2 = Address::generate(&env);

    let cases: &[(u32, DataKey)] = &[
        (0,  DataKey::MerchantSubs(addr.clone())),
        (1,  DataKey::Token),
        (2,  DataKey::Admin),
        (3,  DataKey::MinTopup),
        (4,  DataKey::NextId),
        (5,  DataKey::SchemaVersion),
        (6,  DataKey::Sub(0)),
        (7,  DataKey::ChargedPeriod(0)),
        (8,  DataKey::IdemKey(0)),
        (9,  DataKey::EmergencyStop),
        (10, DataKey::MerchantPaused(addr.clone())),
        (11, DataKey::BillingStatement(0, 0)),
        (12, DataKey::BillingStatementsBySubscription(0)),
        (13, DataKey::BillingStatementsByMerchant(addr.clone())),
        (14, DataKey::TotalAccounted(addr.clone())),
        (15, DataKey::Recovery(String::from_str(&env, "r"))),
        (16, DataKey::MerchantConfig(addr.clone())),
        (17, DataKey::MerchantEarnings(addr.clone(), addr2.clone())),
        (18, DataKey::MerchantTokens(addr.clone())),
        (19, DataKey::UsageLimits(0)),
        (20, DataKey::UsageState(0)),
        (21, DataKey::GracePeriod),
        (22, DataKey::FeeBps),
        (23, DataKey::Treasury),
        (24, DataKey::AcceptedTokens),
        (25, DataKey::TokenDecimals(addr.clone())),
        (26, DataKey::NextPlanId),
        (27, DataKey::Plan(0)),
        (28, DataKey::SubPlan(0)),
        (29, DataKey::PlanMaxActive(0)),
        (30, DataKey::CreditLimit(addr.clone(), addr2.clone())),
        (31, DataKey::TokenSubs(addr.clone())),
        (32, DataKey::SubscriberSubs(addr.clone())),
        (33, DataKey::MerchantBalance(addr.clone(), addr2.clone())),
        (34, DataKey::Blocklist(addr.clone())),
        (35, DataKey::Oracle),
        (36, DataKey::BillingPeriodSnapshot(0, 0)),
        (37, DataKey::BillingPeriodSnapshotIndex(0)),
        (38, DataKey::AdminNonce(addr.clone(), 0)),
        (39, DataKey::Metadata(0, String::from_str(&env, "k"))),
        (40, DataKey::MetadataKeys(0)),
        (41, DataKey::Operator),
        (42, DataKey::BillingRetentionConfig),
        (43, DataKey::BillingStatementSequence(0)),
        (44, DataKey::BillingStatementAggregate(0)),
        (45, DataKey::MerchantMaxSubs(addr.clone())),
        (46, DataKey::Guardians),
        (47, DataKey::NextProposalId),
        (48, DataKey::Proposal(0)),
        (49, DataKey::DisputeEscrow(0)),
        (50, DataKey::Dispute(0)),
        (51, DataKey::NextDisputeId),
        (52, DataKey::SubscriptionDispute(0)),
        (53, DataKey::PayoutSchedule(addr.clone())),
        (54, DataKey::PendingTreasuryChange),
        (55, DataKey::TransferIntent(0)),
        (56, DataKey::Kyc(KycKey::MerchantStatus(addr.clone()))),
        (57, DataKey::Coupon(Symbol::new(&env, "c"))),
        (58, DataKey::CouponRedemptions(Symbol::new(&env, "c"))),
        (59, DataKey::Credential(0)),
        (60, DataKey::AdminConfigLastChangedAt(BytesN::from_array(&env, &[0u8; 32]))),
        (61, DataKey::SubscriberCreateCap),
        (62, DataKey::SubscriberCreateWindow(addr.clone())),
        (63, DataKey::MerchantWhitelistMode),
        (64, DataKey::MerchantApproved(addr.clone())),
        (65, DataKey::ChargeSalt(0)),
        (66, DataKey::ChargeFailureCounter(0)),
        (67, DataKey::AutoPauseThreshold),
        (68, DataKey::BuyoutPremiumBps),
        (69, DataKey::SubCoupon(0)),
        (70, DataKey::MerchantMultiSig(addr.clone())),
        (71, DataKey::SubscriberActiveCount(addr.clone())),
        (72, DataKey::SubscriberActiveCapOverride(addr.clone())),
        (73, DataKey::TagAllowlist),
        (74, DataKey::MerchantTags(addr.clone())),
        (75, DataKey::FeeToken),
        (76, DataKey::CancellationEscrow(0)),
        (77, DataKey::MerchantFeeBps(addr.clone())),
        (78, DataKey::OraclePriceHistoryMeta(addr.clone())),
        (79, DataKey::OraclePriceHistoryEntry(addr.clone(), 0)),
        (80, DataKey::DelegatedPayerGrant(addr.clone(), addr2.clone())),
        (81, DataKey::SplitPayees(0)),
        (82, DataKey::DefaultMerchantWithdrawCap),
        (83, DataKey::MerchantWithdrawCap(addr.clone())),
        (84, DataKey::MerchantWithdrawalWindow(addr.clone())),
        (85, DataKey::EmergencyWithdrawIntent(0)),
        (86, DataKey::MerchantSubAccount(addr.clone(), soroban_sdk::Symbol::new(&env, "a"))),
        (87, DataKey::MerchantSubAccountList(addr.clone())),
    ];

    for (expected, key) in cases {
        assert_eq!(
            key.canonical_discriminant(),
            *expected,
            "DataKey::{} discriminant changed — on-chain storage break",
            expected
        );
    }
}

// ── No duplicate discriminants ────────────────────────────────────────────────

#[test]
fn test_datakey_no_duplicate_discriminants() {
    let env = env();
    let addr = Address::generate(&env);
    let addr2 = Address::generate(&env);

    let all_keys: &[DataKey] = &[
        DataKey::MerchantSubs(addr.clone()),
        DataKey::Token,
        DataKey::Admin,
        DataKey::MinTopup,
        DataKey::NextId,
        DataKey::SchemaVersion,
        DataKey::Sub(0),
        DataKey::ChargedPeriod(0),
        DataKey::IdemKey(0),
        DataKey::EmergencyStop,
        DataKey::MerchantPaused(addr.clone()),
        DataKey::BillingStatement(0, 0),
        DataKey::BillingStatementsBySubscription(0),
        DataKey::BillingStatementsByMerchant(addr.clone()),
        DataKey::TotalAccounted(addr.clone()),
        DataKey::Recovery(String::from_str(&env, "r")),
        DataKey::MerchantConfig(addr.clone()),
        DataKey::MerchantEarnings(addr.clone(), addr2.clone()),
        DataKey::MerchantTokens(addr.clone()),
        DataKey::UsageLimits(0),
        DataKey::UsageState(0),
        DataKey::GracePeriod,
        DataKey::FeeBps,
        DataKey::Treasury,
        DataKey::AcceptedTokens,
        DataKey::TokenDecimals(addr.clone()),
        DataKey::NextPlanId,
        DataKey::Plan(0),
        DataKey::SubPlan(0),
        DataKey::PlanMaxActive(0),
        DataKey::CreditLimit(addr.clone(), addr2.clone()),
        DataKey::TokenSubs(addr.clone()),
        DataKey::SubscriberSubs(addr.clone()),
        DataKey::MerchantBalance(addr.clone(), addr2.clone()),
        DataKey::Blocklist(addr.clone()),
        DataKey::Oracle,
        DataKey::BillingPeriodSnapshot(0, 0),
        DataKey::BillingPeriodSnapshotIndex(0),
        DataKey::AdminNonce(addr.clone(), 0),
        DataKey::Metadata(0, String::from_str(&env, "k")),
        DataKey::MetadataKeys(0),
        DataKey::Operator,
        DataKey::BillingRetentionConfig,
        DataKey::BillingStatementSequence(0),
        DataKey::BillingStatementAggregate(0),
        DataKey::MerchantMaxSubs(addr.clone()),
        DataKey::Guardians,
        DataKey::NextProposalId,
        DataKey::Proposal(0),
        DataKey::DisputeEscrow(0),
        DataKey::Dispute(0),
        DataKey::NextDisputeId,
        DataKey::SubscriptionDispute(0),
        DataKey::PayoutSchedule(addr.clone()),
        DataKey::PendingTreasuryChange,
        DataKey::TransferIntent(0),
        DataKey::Kyc(KycKey::MerchantStatus(addr.clone())),
        DataKey::Coupon(Symbol::new(&env, "c")),
        DataKey::CouponRedemptions(Symbol::new(&env, "c")),
        DataKey::Credential(0),
        DataKey::AdminConfigLastChangedAt(BytesN::from_array(&env, &[0u8; 32])),
        DataKey::SubscriberCreateCap,
        DataKey::SubscriberCreateWindow(addr.clone()),
        DataKey::MerchantWhitelistMode,
        DataKey::MerchantApproved(addr.clone()),
        DataKey::ChargeSalt(0),
        DataKey::ChargeFailureCounter(0),
        DataKey::AutoPauseThreshold,
        DataKey::BuyoutPremiumBps,
        DataKey::SubCoupon(0),
        DataKey::MerchantMultiSig(addr.clone()),
        DataKey::SubscriberActiveCount(addr.clone()),
        DataKey::SubscriberActiveCapOverride(addr.clone()),
        DataKey::TagAllowlist,
        DataKey::MerchantTags(addr.clone()),
        DataKey::FeeToken,
        DataKey::CancellationEscrow(0),
        DataKey::MerchantFeeBps(addr.clone()),
        DataKey::OraclePriceHistoryMeta(addr.clone()),
        DataKey::OraclePriceHistoryEntry(addr.clone(), 0),
        DataKey::DelegatedPayerGrant(addr.clone(), addr2.clone()),
        DataKey::SplitPayees(0),
        DataKey::DefaultMerchantWithdrawCap,
        DataKey::MerchantWithdrawCap(addr.clone()),
        DataKey::MerchantWithdrawalWindow(addr.clone()),
        DataKey::EmergencyWithdrawIntent(0),
        DataKey::MerchantSubAccount(addr.clone(), soroban_sdk::Symbol::new(&env, "a")),
        DataKey::MerchantSubAccountList(addr.clone()),
    ];

    let mut seen = std::collections::HashSet::new();
    for key in all_keys {
        let d = key.canonical_discriminant();
        assert!(seen.insert(d), "duplicate canonical discriminant: {d}");
    }
}
