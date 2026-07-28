#[cfg(kani)]
mod verification {
    use subscription_vault::types::Error;
    use subscription_vault::{safe_add, safe_add_balance, safe_sub, safe_sub_balance};

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
}
