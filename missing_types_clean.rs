#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OracleKind {
    Spot,
    Twap,
    FixedRate,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProposalKind {
    RotateAdmin = 0,
    SetProtocolFee = 1,
    UpgradeContract = 2,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Proposal {
    pub id: u64,
    pub kind: ProposalKind,
    pub target: Address,
    pub target2: Option<Address>,
    pub target3: u32,
    pub quorum_bps: u32,
    pub votes: soroban_sdk::Map<Address, bool>,
    pub eta: u64,
    pub submitted_at: u64,
    pub executed: bool,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct ProposalSubmittedEvent {
    pub proposal_id: u64,
    pub kind: ProposalKind,
    pub target: Address,
    pub quorum_bps: u32,
    pub eta: u64,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct ProposalVotedEvent {
    pub proposal_id: u64,
    pub guardian: Address,
    pub voted_yes: bool,
    pub guardian_weight: u32,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct ProposalExecutedEvent {
    pub proposal_id: u64,
    pub kind: ProposalKind,
    pub votes_for: u32,
    pub votes_against: u32,
    pub total_weight: u32,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct ProposalCancelledEvent {
    pub proposal_id: u64,
    pub reason: String,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SignedMetadataPayload {
    pub subscription_id: u32,
    pub key: String,
    pub value: String,
    pub nonce: u64,
    pub expires_at: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MetadataSetSignedEvent {
    pub subscription_id: u32,
    pub key: String,
    pub signer: Address,
    pub nonce: u64,
    pub timestamp: u64,
    pub schema_version: u32,
}

pub const BATCH_MAX_SIZE: u32 = 100;
