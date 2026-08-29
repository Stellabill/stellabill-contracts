#[cfg(kani)]
mod verification {
    use subscription_vault::compute_cancel_refund;

    /// Prove that the cancellation refund never exceeds the incoming prepaid
    /// balance for any reachable i128 value, including 0 and i128::MAX.
    ///
    /// Security property: the escrow amount == prepaid_balance, so no
    /// underflow-driven drain is possible.
    #[kani::proof]
    fn cancel_refund_bounded() {
        let balance: i128 = kani::any();

        let refund = compute_cancel_refund(balance);

        // Refund must not exceed what was held.
        assert!(refund <= balance);
        // Refund must equal the full balance (no hidden deduction).
        assert_eq!(refund, balance);
    }

    /// Edge case: zero balance produces zero refund.
    #[kani::proof]
    fn cancel_refund_zero_balance() {
        assert_eq!(compute_cancel_refund(0), 0);
    }

    /// Edge case: maximum balance does not overflow.
    #[kani::proof]
    fn cancel_refund_max_balance() {
        let refund = compute_cancel_refund(i128::MAX);
        assert_eq!(refund, i128::MAX);
    }
}
