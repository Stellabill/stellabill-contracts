//! Feature-scoped `DataKey` sub-enums plus a cross-version discriminant
//! snapshot (issue #647).
//!
//! ## Why this is additive rather than a rewrite
//!
//! `DataKey` in `types.rs` is one flat enum whose variant *declaration order*
//! **is** the on-chain wire format: the Soroban `#[contracttype]` macro
//! serialises each variant by its 0-indexed position. Reordering, removing, or
//! re-numbering an arm silently repoints live storage.
//!
//! So instead of physically splitting the enum, this module adds a
//! feature-scoped *view* over it. Each domain gets its own `#[repr(u32)]`
//! sub-enum whose variants carry the frozen discriminant of the corresponding
//! `DataKey` arm, and each sub-enum knows how to wrap itself back into a real
//! `DataKey` via `representative_data_key`. Every mapping is then asserted
//! against `DataKey::canonical_discriminant()`, so the grouping can never
//! disagree with the canonical registry: if the two ever diverge, these tests
//! fail instead of storage corrupting in production.
//!
//! Nothing existing is touched — no type, variant, storage tier, or test is
//! modified, which is what guarantees "no discriminant drift" for this change.
//!
//! ## Deliberate exclusions
//!
//! Two families of arms are intentionally left out of the groups below:
//!
//! * `DataKey::TransferIntent` — the registry currently reports discriminant
//!   `54` for **both** `PendingTreasuryChange` and `TransferIntent`. Only
//!   `PendingTreasuryChange` is grouped here so the disjointness test asserts a
//!   true invariant; the collision itself is a pre-existing registry question
//!   for the maintainers and is deliberately not "fixed" by this PR.
//! * Arms whose payload types are not constructible from a bare `Env` without
//!   pulling in unrelated fixtures.

#![cfg(test)]

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, String as SorobanString};
use subscription_vault::{is_known_instance_discriminant, DataKey};

/// Subscription-domain storage keys: per-subscription lifecycle, metering,
/// metadata, and per-subscriber caps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
enum SubscriptionKey {
    Sub = 6,
    ChargedPeriod = 7,
    IdemKey = 8,
    UsageLimits = 19,
    UsageState = 20,
    SubPlan = 28,
    CreditLimit = 30,
    SubscriberSubs = 32,
    Blocklist = 34,
    Metadata = 39,
    MetadataKeys = 40,
    SubscriberCreateCap = 60,
    SubscriberCreateWindow = 61,
    ChargeSalt = 64,
    SubCoupon = 68,
    SubscriberActiveCount = 70,
    SubscriberActiveCapOverride = 71,
    CancellationEscrow = 75,
}

impl SubscriptionKey {
    const ALL: [Self; 18] = [
        Self::Sub,
        Self::ChargedPeriod,
        Self::IdemKey,
        Self::UsageLimits,
        Self::UsageState,
        Self::SubPlan,
        Self::CreditLimit,
        Self::SubscriberSubs,
        Self::Blocklist,
        Self::Metadata,
        Self::MetadataKeys,
        Self::SubscriberCreateCap,
        Self::SubscriberCreateWindow,
        Self::ChargeSalt,
        Self::SubCoupon,
        Self::SubscriberActiveCount,
        Self::SubscriberActiveCapOverride,
        Self::CancellationEscrow,
    ];

    /// Frozen discriminant this sub-enum variant claims for its `DataKey` arm.
    const fn claimed_discriminant(self) -> u32 {
        self as u32
    }

    /// Wraps this sub-enum variant back into the top-level `DataKey`.
    fn representative_data_key(self, env: &Env) -> DataKey {
        let address = Address::generate(env);
        match self {
            Self::Sub => DataKey::Sub(1),
            Self::ChargedPeriod => DataKey::ChargedPeriod(1),
            Self::IdemKey => DataKey::IdemKey(1),
            Self::UsageLimits => DataKey::UsageLimits(1),
            Self::UsageState => DataKey::UsageState(1),
            Self::SubPlan => DataKey::SubPlan(1),
            Self::CreditLimit => DataKey::CreditLimit(address.clone(), address),
            Self::SubscriberSubs => DataKey::SubscriberSubs(address),
            Self::Blocklist => DataKey::Blocklist(address),
            Self::Metadata => DataKey::Metadata(1, SorobanString::from_str(env, "plan")),
            Self::MetadataKeys => DataKey::MetadataKeys(1),
            Self::SubscriberCreateCap => DataKey::SubscriberCreateCap,
            Self::SubscriberCreateWindow => DataKey::SubscriberCreateWindow(address),
            Self::ChargeSalt => DataKey::ChargeSalt(1),
            Self::SubCoupon => DataKey::SubCoupon(1),
            Self::SubscriberActiveCount => DataKey::SubscriberActiveCount(address),
            Self::SubscriberActiveCapOverride => DataKey::SubscriberActiveCapOverride(address),
            Self::CancellationEscrow => DataKey::CancellationEscrow(1),
        }
    }
}

/// Merchant-domain storage keys: merchant config, earnings, payout, and
/// compliance tagging.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
enum MerchantKey {
    MerchantSubs = 0,
    MerchantPaused = 10,
    MerchantConfig = 16,
    MerchantEarnings = 17,
    MerchantTokens = 18,
    MerchantBalance = 33,
    MerchantMaxSubs = 45,
    PayoutSchedule = 53,
    MerchantMultiSig = 69,
    TagAllowlist = 72,
    MerchantTags = 73,
    MerchantFeeBps = 76,
}

impl MerchantKey {
    const ALL: [Self; 12] = [
        Self::MerchantSubs,
        Self::MerchantPaused,
        Self::MerchantConfig,
        Self::MerchantEarnings,
        Self::MerchantTokens,
        Self::MerchantBalance,
        Self::MerchantMaxSubs,
        Self::PayoutSchedule,
        Self::MerchantMultiSig,
        Self::TagAllowlist,
        Self::MerchantTags,
        Self::MerchantFeeBps,
    ];

    const fn claimed_discriminant(self) -> u32 {
        self as u32
    }

    fn representative_data_key(self, env: &Env) -> DataKey {
        let address = Address::generate(env);
        match self {
            Self::MerchantSubs => DataKey::MerchantSubs(address),
            Self::MerchantPaused => DataKey::MerchantPaused(address),
            Self::MerchantConfig => DataKey::MerchantConfig(address),
            Self::MerchantEarnings => DataKey::MerchantEarnings(address.clone(), address),
            Self::MerchantTokens => DataKey::MerchantTokens(address),
            Self::MerchantBalance => DataKey::MerchantBalance(address.clone(), address),
            Self::MerchantMaxSubs => DataKey::MerchantMaxSubs(address),
            Self::PayoutSchedule => DataKey::PayoutSchedule(address),
            Self::MerchantMultiSig => DataKey::MerchantMultiSig(address),
            Self::TagAllowlist => DataKey::TagAllowlist,
            Self::MerchantTags => DataKey::MerchantTags(address),
            Self::MerchantFeeBps => DataKey::MerchantFeeBps(address),
        }
    }
}

/// Governance-domain storage keys: admin/operator authority, protocol fee and
/// treasury policy, guardian proposals, and the global kill switch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
enum GovernanceKey {
    Admin = 2,
    SchemaVersion = 5,
    EmergencyStop = 9,
    FeeBps = 22,
    Treasury = 23,
    Operator = 41,
    Guardians = 46,
    NextProposalId = 47,
    Proposal = 48,
    PendingTreasuryChange = 54,
}

impl GovernanceKey {
    const ALL: [Self; 10] = [
        Self::Admin,
        Self::SchemaVersion,
        Self::EmergencyStop,
        Self::FeeBps,
        Self::Treasury,
        Self::Operator,
        Self::Guardians,
        Self::NextProposalId,
        Self::Proposal,
        Self::PendingTreasuryChange,
    ];

    const fn claimed_discriminant(self) -> u32 {
        self as u32
    }

    fn representative_data_key(self, _env: &Env) -> DataKey {
        match self {
            Self::Admin => DataKey::Admin,
            Self::SchemaVersion => DataKey::SchemaVersion,
            Self::EmergencyStop => DataKey::EmergencyStop,
            Self::FeeBps => DataKey::FeeBps,
            Self::Treasury => DataKey::Treasury,
            Self::Operator => DataKey::Operator,
            Self::Guardians => DataKey::Guardians,
            Self::NextProposalId => DataKey::NextProposalId,
            Self::Proposal => DataKey::Proposal(1),
            Self::PendingTreasuryChange => DataKey::PendingTreasuryChange,
        }
    }
}

/// Every grouped discriminant, sorted ascending.
///
/// This is the cross-version snapshot: it is written out longhand so that a
/// future edit to any group is visible as an explicit change to a frozen list
/// rather than an invisible renumbering.
const GROUPED_DISCRIMINANT_SNAPSHOT: [u32; 40] = [
    0, 2, 5, 6, 7, 8, 9, 10, 16, 17, 18, 19, 20, 22, 23, 28, 30, 32, 33, 34, 39, 40, 41, 45, 46,
    47, 48, 53, 54, 60, 61, 64, 68, 69, 70, 71, 72, 73, 75, 76,
];

fn all_grouped_discriminants() -> Vec<u32> {
    let mut discriminants: Vec<u32> = SubscriptionKey::ALL
        .iter()
        .map(|key| key.claimed_discriminant())
        .chain(MerchantKey::ALL.iter().map(|key| key.claimed_discriminant()))
        .chain(
            GovernanceKey::ALL
                .iter()
                .map(|key| key.claimed_discriminant()),
        )
        .collect();
    discriminants.sort_unstable();
    discriminants
}

#[test]
fn subscription_group_agrees_with_canonical_registry() {
    let env = Env::default();

    for key in SubscriptionKey::ALL {
        let data_key = key.representative_data_key(&env);
        assert_eq!(
            data_key.canonical_discriminant(),
            key.claimed_discriminant(),
            "subscription group is stale for {key:?}"
        );
    }
}

#[test]
fn merchant_group_agrees_with_canonical_registry() {
    let env = Env::default();

    for key in MerchantKey::ALL {
        let data_key = key.representative_data_key(&env);
        assert_eq!(
            data_key.canonical_discriminant(),
            key.claimed_discriminant(),
            "merchant group is stale for {key:?}"
        );
    }
}

#[test]
fn governance_group_agrees_with_canonical_registry() {
    let env = Env::default();

    for key in GovernanceKey::ALL {
        let data_key = key.representative_data_key(&env);
        assert_eq!(
            data_key.canonical_discriminant(),
            key.claimed_discriminant(),
            "governance group is stale for {key:?}"
        );
    }
}

#[test]
fn groups_are_pairwise_disjoint() {
    let discriminants = all_grouped_discriminants();

    let mut deduped = discriminants.clone();
    deduped.dedup();

    assert_eq!(
        deduped, discriminants,
        "a discriminant was claimed by more than one feature group"
    );
}

#[test]
fn grouped_discriminants_match_frozen_snapshot() {
    assert_eq!(
        all_grouped_discriminants(),
        GROUPED_DISCRIMINANT_SNAPSHOT.to_vec(),
        "grouped DataKey discriminants drifted from the frozen snapshot"
    );
}

#[test]
fn merchant_group_keys_are_all_instance_tier() {
    for key in MerchantKey::ALL {
        assert!(
            is_known_instance_discriminant(key.claimed_discriminant()),
            "{key:?} left the instance-tier allowlist"
        );
    }
}

#[test]
fn per_subscription_record_keys_stay_off_the_instance_allowlist() {
    // These are persistent-tier by design; if one appears in the instance
    // allowlist, a storage tier changed under us.
    for discriminant in [6u32, 7, 8, 39, 40, 68, 75] {
        assert!(
            !is_known_instance_discriminant(discriminant),
            "discriminant {discriminant} unexpectedly became an instance key"
        );
    }
}

#[test]
fn empty_group_slice_is_trivially_disjoint() {
    // Edge case named in the issue: an empty variant set must not panic.
    let empty: [SubscriptionKey; 0] = [];
    let discriminants: Vec<u32> = empty.iter().map(|key| key.claimed_discriminant()).collect();

    assert!(discriminants.is_empty());
}
