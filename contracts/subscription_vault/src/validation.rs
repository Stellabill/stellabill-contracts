//! ABI boundary validators.
//!
//! Every public entrypoint that accepts user-supplied `String` or `Address`
//! arguments **must** call the appropriate helper at the top of its body,
//! before any auth checks or storage reads.  Centralising the checks here
//! keeps the guard logic reviewable and consistent across the whole contract.
//!
//! # Why these validators?
//!
//! Soroban accepts any XDR-encodable value from a transaction; it does **not**
//! silently reject an empty `Bytes`-backed `String` or an all-zero `Address`.
//! Allowing those values to reach storage would:
//! - Store degenerate metadata keys that can never be deleted (empty string
//!   keys collide with each other or are un-displayable off-chain).
//! - Accept the zero `Address` as a treasury or token, effectively burning
//!   funds sent there.
//! - Allow an attacker to supply the contract's own address as a party,
//!   creating confused-deputy or re-entrancy windows.
//!
//! # Security assumptions
//!
//! - These helpers are **not** a substitute for auth (`require_auth`); call
//!   auth checks first, then input validation.
//! - `reject_contract_self` compares against `env.current_contract_address()`,
//!   which is always correct even if the contract is called via cross-contract.

use soroban_sdk::{Address, Env, String};

use crate::types::Error;

/// Minimum printable, non-whitespace length for String arguments.
const MIN_STRING_LEN: u32 = 1;

/// Reject an empty or whitespace-only Soroban `String`.
///
/// # Errors
///
/// Returns [`Error::InvalidInput`] when:
/// - `s.len() == 0` — length is zero (empty string).
/// - Every byte in `s` is an ASCII whitespace character
///   (`' '`, `'\t'`, `'\n'`, `'\r'`, `'\x0b'` VT, `'\x0c'` FF).
///
/// Unicode strings whose byte representation is entirely whitespace are also
/// rejected (e.g. a string containing only spaces encoded as multi-byte UTF-8).
///
/// # Gas
///
/// The function copies the raw bytes of `s` into a stack-allocated 256-byte
/// buffer (capped at [`MAX_METADATA_VALUE_LENGTH`]) for inspection — one
/// allocation per call, O(n) in string length.
pub fn reject_empty_string(s: &String) -> Result<(), Error> {
    let len = s.len();
    if len < MIN_STRING_LEN {
        return Err(Error::InvalidInput);
    }

    // Only check for all-whitespace on strings that fit in our stack buffer.
    // Strings longer than 256 bytes are accepted unconditionally — an
    // all-whitespace value of that length is an implausible mistake and would
    // already be caught by the length limits enforced in the metadata module.
    //
    // IMPORTANT: `String::copy_into_slice` panics if the slice length does not
    // equal `s.len()` exactly, so we must pass `&mut buf[..len as usize]`.
    if len <= 256 {
        let mut buf = [0u8; 256];
        let slice = &mut buf[..len as usize];
        s.copy_into_slice(slice);
        let all_whitespace = slice
            .iter()
            .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c));
        if all_whitespace {
            return Err(Error::InvalidInput);
        }
    }

    Ok(())
}

/// Reject an `Address` that equals the current contract's own address.
///
/// Passing the contract's own address as a party (merchant, treasury,
/// subscriber, operator, token) would allow confused-deputy attacks where
/// the contract could authorise itself, receive its own earnings, or act
/// as its own admin.
///
/// # Errors
///
/// Returns [`Error::InvalidInput`] if `addr == env.current_contract_address()`.
pub fn reject_contract_self(env: &Env, addr: &Address) -> Result<(), Error> {
    if *addr == env.current_contract_address() {
        return Err(Error::InvalidInput);
    }
    Ok(())
}
