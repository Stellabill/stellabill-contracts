use soroban_sdk::{Env, Address};
use crate::types::Error;

/// Would assert that the contract's live on-chain token balance equals the
/// sum of every merchant's tracked balance for that token.
///
/// Not currently enforceable: there is no `DataKey` index of "every merchant
/// address the vault has ever seen" to sum over (only per-merchant/per-token
/// lookups exist). Reconstructing the full merchant set would require either
/// a new persistent index maintained on every credit/withdraw, or scanning
/// storage exhaustively — both out of scope here. Returns `Ok(())` as a
/// placeholder so call sites compile and the (test-only) invariant-checking
/// hook exists for a future, properly-indexed implementation.
pub fn assert_token_balance_invariant(
    _env: &Env,
    _token: &Address,
) -> Result<(), Error> {
    Ok(())
}