#[cfg(kani)]
mod verification {
    use subscription_vault::compute_next_nonce;
    use subscription_vault::types::Error;

    @kani::proof]
    pub fn check_nonce_monotonicity() {
        let stored: u64 = kani::any();
        let expected: u64 = kani::any();

        match compute_next_nonce(stored, expected) {
            Ok(next) => {
                // Monotonicity: next must be stored + 1
                assert_eq!(next, stored + 1);
                // Also next must be > stored
                assert!(next > stored);
            }
            Err(Error::NonceAlreadyUsed) => {
                assert!(expected != stored);
            }
            Err(Error::Overflow) => {
                assert_eq!(stored, u64::MAX);
                assert_eq(expected, u64::MAX);
            }
            Err(_) => unreachable(),
        }
    }
}
