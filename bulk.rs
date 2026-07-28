#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BulkSubscriptionResult {
    pub subscription_id: u32,
    pub success: bool,
    pub changed: bool,
    pub error_code: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct BulkPauseEvent {
    pub caller: Address,
    pub requested: u32,
    pub paused: u32,
    pub skipped: u32,
    pub failed: u32,
    pub nonce: u64,
    pub timestamp: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct BulkCancelEvent {
    pub caller: Address,
    pub requested: u32,
    pub cancelled: u32,
    pub skipped: u32,
    pub failed: u32,
    pub nonce: u64,
    pub timestamp: u64,
    pub schema_version: u32,
}
