#[cfg(kani)]
mod verification {
    use subscription_vault::types::Error;
    use subscription_vault::{
        safe_add, safe_add_balance, safe_i128_to_u32, safe_i128_to_u64, safe_sub, safe_sub_balance,
    };

    #[kani::proof]
    pub fn check_safe_add() {
        let a: i128 = kani::any();
        let b: i128 = kani::any();

        match safe_add(a, b) {
            Ok(result) => {
                let expected = a.checked_add(b);
                assert_eq!(Some(result), expected);
            }
            Err(Error::Overflow) => {
                let expected = a.checked_add(b);
                assert!(expected.is_none());
            }
            Err(_) => {
                kani::assert(false, "Unexpected error type from safe_add");
            }
        }
    }

    #[kani::proof]
    pub fn check_safe_sub() {
        let a: i128 = kani::any();
        let b: i128 = kani::any();

        match safe_sub(a, b) {
            Ok(result) => {
                let expected = a.checked_sub(b);
                assert_eq!(Some(result), expected);
            }
            Err(Error::Underflow) => {
                let expected = a.checked_sub(b);
                assert!(expected.is_none());
            }
            Err(_) => {
                kani::assert(false, "Unexpected error type from safe_sub");
            }
        }
    }

    #[kani::proof]
    pub fn check_safe_add_balance() {
        let balance: i128 = kani::any();
        let amount: i128 = kani::any();

        match safe_add_balance(balance, amount) {
            Ok(result) => {
                assert!(amount >= 0);
                let expected = balance.checked_add(amount);
                assert_eq!(Some(result), expected);
            }
            Err(Error::Underflow) => {
                assert!(amount < 0);
            }
            Err(Error::Overflow) => {
                assert!(amount >= 0);
                assert!(balance.checked_add(amount).is_none());
            }
            Err(_) => {
                kani::assert(false, "Unexpected error type from safe_add_balance");
            }
        }
    }

    #[kani::proof]
    pub fn check_safe_sub_balance() {
        let balance: i128 = kani::any();
        let amount: i128 = kani::any();

        match safe_sub_balance(balance, amount) {
            Ok(result) => {
                assert!(amount >= 0);
                assert!(balance >= amount);
                let expected = balance.checked_sub(amount);
                assert_eq!(Some(result), expected);
                assert!(result >= 0);
            }
            Err(Error::Underflow) => {
                // Underflow happens if amount < 0 OR balance < amount
                assert!(amount < 0 || balance < amount);
            }
            Err(_) => {
                kani::assert(false, "Unexpected error type from safe_sub_balance");
            }
        }
    }

    /// SECURITY closure: `safe_i128_to_u32` must never silently truncate.
    /// For every i128 input it must either return the exact u32 value or
    /// an error; there is no third outcome.
    #[kani::proof]
    pub fn check_safe_i128_to_u32() {
        let value: i128 = kani::any();

        match safe_i128_to_u32(value) {
            Ok(cast) => {
                // Range check: Ok iff value within [0, u32::MAX].
                assert!(value >= 0);
                assert!(value <= i128::from(u32::MAX));
                // Exactness: the cast value must equal the low bits of value
                // AND value must fit (since cast is non-truncating on the
                // allowed range, equality is total).
                assert_eq!(u32::try_from(value).expect("precondition"), cast);
            }
            Err(Error::Underflow) => {
                assert!(value < 0);
            }
            Err(Error::Overflow) => {
                assert!(value >= 0);
                assert!(value > i128::from(u32::MAX));
            }
            Err(_) => {
                kani::assert(false, "Unexpected error type from safe_i128_to_u32");
            }
        }
    }

    /// SECURITY closure: `safe_i128_to_u64` must never silently truncate
    /// or flip sign on adversarial inputs.
    #[kani::proof]
    pub fn check_safe_i128_to_u64() {
        let value: i128 = kani::any();

        match safe_i128_to_u64(value) {
            Ok(cast) => {
                assert!(value >= 0);
                assert!(value <= i128::from(u64::MAX));
                assert_eq!(u64::try_from(value).expect("precondition"), cast);
            }
            Err(Error::Underflow) => {
                assert!(value < 0);
            }
            Err(Error::Overflow) => {
                assert!(value >= 0);
                assert!(value > i128::from(u64::MAX));
            }
            Err(_) => {
                kani::assert(false, "Unexpected error type from safe_i128_to_u64");
            }
        }
    }
}
