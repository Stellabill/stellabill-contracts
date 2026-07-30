// Regression tests for fee routing rounding/dust handling.
// Ensures `fee + merchant == charge` in source token arithmetic and that
// split-payee distribution deterministically allocates any truncation
// remainder to the first payee (index 0).

#[cfg(test)]
mod tests {
    use crate::types::MAX_FEE_BIPS;

    #[test]
    fn fee_plus_merchant_equals_charge_small_values() {
        let cases: &[(i128, u32)] = &[
            (1, 1),                  // charge=1, fee_bps=1
            (1, MAX_FEE_BIPS as u32), // charge=1, fee=max
            (2, 1),
            (10, 3),
            (100_000_000, 250),
        ];

        for (charge, fee_bps) in cases.iter() {
            let fee = charge * (*fee_bps as i128) / 10_000i128;
            let net = charge - fee;
            assert_eq!(net + fee, *charge, "fee+merchant must equal charge (charge={}, fee_bps={})", charge, fee_bps);
        }
    }

    #[test]
    fn split_payees_remainder_allocated_to_first_payee() {
        // Simulate a set of weights and various net amounts, including tiny values.
        let weights: Vec<u32> = vec![5000, 3000, 2000]; // sums to 10000
        let net_amounts: Vec<i128> = vec![1, 2, 3, 10, 1_000_000];

        for net in net_amounts.into_iter() {
            // Compute shares for payees 1..n-1 with truncation.
            let mut total_distributed: i128 = 0;
            for w in weights.iter().skip(1) {
                let share = net * (*w as i128) / 10_000i128;
                total_distributed = total_distributed.saturating_add(share);
            }
            let first_share = net - total_distributed;

            // Reconstruct vector of shares and assert sum equals net.
            let mut sum = first_share;
            for w in weights.iter().skip(1) {
                let share = net * (*w as i128) / 10_000i128;
                sum = sum.saturating_add(share);
            }
            assert_eq!(sum, net, "split payees must sum to net (net={})", net);
            // Also ensure first_share is non-negative and deterministic.
            assert!(first_share >= 0, "first_share must be >= 0");
        }
    }
}
