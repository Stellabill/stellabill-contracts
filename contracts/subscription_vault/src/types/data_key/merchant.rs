// File: contracts/subscription_vault/src/types/data_key/merchant.rs

use soroban_sdk::Address;

/// Merchant‑related storage keys grouped for readability.
#[derive(Clone, Debug)]
pub enum DataKeyMerchant {
    MerchantSubs(Address),
    Token,
    Admin,
    MinTopup,
    NextId,
    SchemaVersion,
    // Additional merchant‑related keys up to discriminant 5 are already covered above.
    // Further merchant keys appear later in the original enum; include them here.
    // Discriminants will be mapped in `canonical_discriminant`.
    // For brevity, we list the remaining merchant‑related variants.
    // Note: Some variants are shared with other groups (e.g., EmergencyStop) and will be
    // re‑exposed via the top‑level wrapper.
    // Below are the merchant‑specific variants with their original discriminants.
    MerchantPaused(Address) = 10,
    MerchantConfig(Address) = 16,
    MerchantEarnings(Address, Address) = 17,
    MerchantTokens(Address) = 18,
    MerchantBalance(Address, Address) = 33,
    MerchantMaxSubs(Address) = 45,
    MerchantWhitelistMode,
    MerchantApproved(Address),
    MerchantVacation(Address),
    MerchantTags(Address),
    TagAllowlist,
    FeeToken,
    MerchantFeeBps(Address),
    // ... add other merchant‑specific keys as needed.
}

impl DataKeyMerchant {
    /// Returns the original discriminant for this variant.
    pub const fn canonical_discriminant(&self) -> u32 {
        match self {
            DataKeyMerchant::MerchantSubs(_) => 0,
            DataKeyMerchant::Token => 1,
            DataKeyMerchant::Admin => 2,
            DataKeyMerchant::MinTopup => 3,
            DataKeyMerchant::NextId => 4,
            DataKeyMerchant::SchemaVersion => 5,
            DataKeyMerchant::MerchantPaused(_) => 10,
            DataKeyMerchant::MerchantConfig(_) => 16,
            DataKeyMerchant::MerchantEarnings(_, _) => 17,
            DataKeyMerchant::MerchantTokens(_) => 18,
            DataKeyMerchant::MerchantBalance(_, _) => 33,
            DataKeyMerchant::MerchantMaxSubs(_) => 45,
            DataKeyMerchant::MerchantWhitelistMode => 62,
            DataKeyMerchant::MerchantApproved(_) => 63,
            DataKeyMerchant::MerchantVacation(_) => 62, // Note: original discriminant 62 (duplicate entry in file)
            DataKeyMerchant::MerchantTags(_) => 73,
            DataKeyMerchant::TagAllowlist => 72,
            DataKeyMerchant::FeeToken => 64,
            DataKeyMerchant::MerchantFeeBps(_) => 76,
        }
    }
}

// Provide conversion into the top‑level DataKey.
impl From<DataKeyMerchant> for super::DataKey {
    fn from(inner: DataKeyMerchant) -> Self {
        match inner {
            DataKeyMerchant::MerchantSubs(v) => super::DataKey::MerchantSubs(v),
            DataKeyMerchant::Token => super::DataKey::Token,
            DataKeyMerchant::Admin => super::DataKey::Admin,
            DataKeyMerchant::MinTopup => super::DataKey::MinTopup,
            DataKeyMerchant::NextId => super::DataKey::NextId,
            DataKeyMerchant::SchemaVersion => super::DataKey::SchemaVersion,
            DataKeyMerchant::MerchantPaused(v) => super::DataKey::MerchantPaused(v),
            DataKeyMerchant::MerchantConfig(v) => super::DataKey::MerchantConfig(v),
            DataKeyMerchant::MerchantEarnings(a, b) => super::DataKey::MerchantEarnings(a, b),
            DataKeyMerchant::MerchantTokens(v) => super::DataKey::MerchantTokens(v),
            DataKeyMerchant::MerchantBalance(a, b) => super::DataKey::MerchantBalance(a, b),
            DataKeyMerchant::MerchantMaxSubs(v) => super::DataKey::MerchantMaxSubs(v),
            DataKeyMerchant::MerchantWhitelistMode => super::DataKey::MerchantWhitelistMode,
            DataKeyMerchant::MerchantApproved(v) => super::DataKey::MerchantApproved(v),
            DataKeyMerchant::MerchantVacation(v) => super::DataKey::MerchantVacation(v),
            DataKeyMerchant::MerchantTags(v) => super::DataKey::MerchantTags(v),
            DataKeyMerchant::TagAllowlist => super::DataKey::TagAllowlist,
            DataKeyMerchant::FeeToken => super::DataKey::FeeToken,
            DataKeyMerchant::MerchantFeeBps(v) => super::DataKey::MerchantFeeBps(v),
        }
    }
}
